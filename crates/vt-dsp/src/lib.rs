//! Inference-only, explicitly gated echo control. Caller buffers and archives
//! remain immutable. All processing belongs on a worker, never an audio callback.
use serde::{Deserialize, Serialize};
use sherpa_onnx::LinearResampler;
use sonora::{AudioProcessing, Config, StreamConfig, config::EchoCanceller};
use std::collections::VecDeque;
use thiserror::Error;
use vt_core::TrackKind;
#[cfg(test)]
mod tests;
pub const AEC_ENGINE: &str = "sonora-aec3@0.2.0";
pub const INFERENCE_RATE: u32 = 16000;
pub const FRAME_SAMPLES: usize = 160;
const TICKS_PER_MS: u64 = 16;
const UNVERIFIED: &str = "AEC bypassed: the output route is not explicitly verified.";
const MISSING: &str =
    "AEC bypassed: matching system reference did not arrive within the bounded jitter window.";
const PARTIAL: &str = "AEC bypassed: the final partial frame was preserved without padding.";
const ENGINE_FAILURE: &str =
    "AEC bypassed: echo processing failed; microphone audio was preserved.";
#[derive(Debug, Error)]
pub enum DspError {
    #[error("Invalid echo processing input: {0}")]
    Invalid(&'static str),
    #[error("Inference resampling failed.")]
    Resampler,
}
pub type Result<T> = std::result::Result<T, DspError>;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct DspConfig {
    pub max_jitter_ms: u64,
}
impl Default for DspConfig {
    fn default() -> Self {
        Self { max_jitter_ms: 200 }
    }
}
/// Eligibility is an explicit caller decision about the meeting application's
/// actual output route and the global render mix. Unknown/headphone/mismatched
/// routes must remain ineligible. Epoch changes invalidate the acoustic path.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RouteEligibility {
    pub eligible: bool,
    pub epoch: u64,
    pub reason: Option<String>,
}
#[derive(Clone, Debug)]
pub struct ProcessedAudio {
    pub track: TrackKind,
    pub samples: Vec<f32>,
    pub rate: u32,
    pub start_ms: u64,
    pub aec_applied: bool,
    pub reason: Option<String>,
}
#[derive(Clone)]
struct Frame {
    start: u64,
    samples: Vec<f32>,
}
impl Frame {
    fn end(&self) -> u64 {
        self.start + self.samples.len() as u64
    }
}
#[derive(Default)]
struct Converter {
    rate: Option<u32>,
    origin: u64,
    input_frames: u64,
    output_frames: u64,
    pending: VecDeque<f32>,
    resampler: Option<LinearResampler>,
}
impl Converter {
    fn begin(&mut self, rate: u32, start_ms: u64) -> Result<()> {
        self.rate = Some(rate);
        self.origin = ticks(start_ms)?;
        if rate != INFERENCE_RATE {
            self.resampler = Some(
                LinearResampler::create(rate as i32, INFERENCE_RATE as i32)
                    .ok_or(DspError::Resampler)?,
            );
        }
        Ok(())
    }
    fn expected_ms(&self) -> u64 {
        self.origin / TICKS_PER_MS
            + self.input_frames * 1000 / u64::from(self.rate.unwrap_or(INFERENCE_RATE))
    }
    fn push(&mut self, samples: &[f32]) -> Result<Vec<Frame>> {
        self.input_frames += samples.len() as u64;
        let converted = if let Some(resampler) = &self.resampler {
            resampler.resample(samples, false)
        } else {
            samples.to_vec()
        };
        if converted.iter().any(|s| !s.is_finite()) {
            return Err(DspError::Resampler);
        }
        self.pending.extend(converted);
        self.take_frames(false)
    }
    fn take_frames(&mut self, tail: bool) -> Result<Vec<Frame>> {
        let mut frames = Vec::new();
        while self.pending.len() >= FRAME_SAMPLES || tail && !self.pending.is_empty() {
            let n = self.pending.len().min(FRAME_SAMPLES);
            let start = self
                .origin
                .checked_add(self.output_frames)
                .ok_or(DspError::Invalid("timestamp overflow"))?;
            let samples = self.pending.drain(..n).collect();
            self.output_frames += n as u64;
            frames.push(Frame { start, samples });
        }
        Ok(frames)
    }
    fn flush(&mut self) -> Result<Vec<Frame>> {
        if let Some(resampler) = &self.resampler {
            let samples = resampler.resample(&[], true);
            if samples.iter().any(|s| !s.is_finite()) {
                return Err(DspError::Resampler);
            }
            self.pending.extend(samples);
        }
        self.take_frames(true)
    }
}
/// Two independently clocked tracks, with at most200ms of pending Mic/reference
/// frames. Each track must be internally chronological; cross-track arrival is
/// arbitrary. Offline callers should interleave≤20ms chronological source slices.
pub struct EchoProcessor {
    config: DspConfig,
    route: RouteEligibility,
    epoch_start: u64,
    converters: [Converter; 2],
    last_native_end: [Option<u64>; 2],
    near: VecDeque<Frame>,
    far: VecDeque<Frame>,
    watermark: u64,
    engine: Option<AudioProcessing>,
    finished: bool,
}
impl EchoProcessor {
    pub fn new(config: DspConfig) -> Result<Self> {
        if config.max_jitter_ms > 200 {
            return Err(DspError::Invalid("max_jitter_ms must be0–200"));
        }
        Ok(Self {
            config,
            route: RouteEligibility::default(),
            epoch_start: 0,
            converters: [Converter::default(), Converter::default()],
            last_native_end: [None, None],
            near: VecDeque::new(),
            far: VecDeque::new(),
            watermark: 0,
            engine: None,
            finished: false,
        })
    }
    /// Drains old audio conservatively before adopting the new explicit route.
    /// No already-received frame is silently reinterpreted under a new epoch.
    pub fn configure_at_time(
        &mut self,
        route: RouteEligibility,
        start_ms: u64,
    ) -> Result<Vec<ProcessedAudio>> {
        self.check_active()?;
        ticks(start_ms)?;
        if route.reason.as_ref().is_some_and(|s| s.len() > 512) {
            return Err(DspError::Invalid("route reason exceeds512bytes"));
        }
        if self.route == route {
            return Ok(Vec::new());
        }
        let output = self.reset_at_time(
            "AEC bypassed: output route changed; echo state reset.",
            start_ms,
        )?;
        self.route = route;
        Ok(output)
    }
    /// A gap/format change resets both resamplers and AEC conservatively. Every
    /// old tail/queued microphone sample is returned as bypass audio, never lost.
    pub fn reset_at_time(&mut self, reason: &str, start_ms: u64) -> Result<Vec<ProcessedAudio>> {
        self.check_active()?;
        let at = ticks(start_ms)?;
        if reason.len() > 512 {
            return Err(DspError::Invalid("reset reason exceeds512bytes"));
        }
        let output = self.drain_all(Some(reason))?;
        self.converters = [Converter::default(), Converter::default()];
        self.near.clear();
        self.far.clear();
        self.engine = None;
        self.epoch_start = self.epoch_start.max(at);
        Ok(output)
    }
    pub fn accept(
        &mut self,
        track: TrackKind,
        samples: &[f32],
        rate: u32,
        start_ms: u64,
    ) -> Result<Vec<ProcessedAudio>> {
        self.check_active()?;
        if !(8000..=192000).contains(&rate)
            || samples.len() > rate as usize * 2
            || samples.iter().any(|s| !s.is_finite() || s.abs() > 16.)
        {
            return Err(DspError::Invalid(
                "finite normalized mono audio at8–192kHz, at most2seconds, is required",
            ));
        }
        let start = ticks(start_ms)?;
        let end = start
            .checked_add((samples.len() as u64 * 16000).div_ceil(u64::from(rate)))
            .ok_or(DspError::Invalid("timestamp overflow"))?;
        if samples.is_empty() {
            return Ok(Vec::new());
        }
        let i = index(track);
        if self.last_native_end[i]
            .is_some_and(|previous| start.saturating_add(5 * TICKS_PER_MS) < previous)
        {
            return Err(DspError::Invalid("track audio overlaps or moves backwards"));
        }
        let mut output = Vec::new();
        if self.converters[i].rate.is_some_and(|r| r != rate)
            || self.converters[i].rate.is_some()
                && self.converters[i].expected_ms().abs_diff(start_ms) > 5
        {
            output.extend(self.reset_at_time(
                "AEC bypassed: audio format or timing changed; echo state reset.",
                start_ms,
            )?);
        }
        if self.converters[i].rate.is_none() {
            self.converters[i].begin(rate, start_ms)?;
        }
        // Small native chunks bound the resampler/framer transient storage even
        // when callers hand us a permitted two-second decoded archive packet.
        let chunk = (rate as usize).div_ceil(100);
        for packet in samples.chunks(chunk) {
            let frames = self.converters[i].push(packet)?;
            let native_end = self.converters[i]
                .origin
                .checked_add(
                    self.converters[i]
                        .input_frames
                        .checked_mul(u64::from(INFERENCE_RATE))
                        .ok_or(DspError::Invalid("timestamp overflow"))?
                        / u64::from(rate),
                )
                .ok_or(DspError::Invalid("timestamp overflow"))?;
            self.watermark = self.watermark.max(native_end);
            for frame in frames {
                self.ingest(track, frame, &mut output);
            }
            self.drain_ready(&mut output, false);
            self.trim_reference();
        }
        self.last_native_end[i] = Some(end);
        Ok(output)
    }
    /// Terminal flush. Full aligned frames may still use AEC; incomplete or
    /// unmatched tails bypass without zero-padding or destroying short replies.
    pub fn flush(&mut self) -> Result<Vec<ProcessedAudio>> {
        self.check_active()?;
        let output = self.drain_all(None)?;
        self.finished = true;
        self.near.clear();
        self.far.clear();
        self.engine = None;
        Ok(output)
    }
    fn check_active(&self) -> Result<()> {
        if self.finished {
            Err(DspError::Invalid("processor is finished"))
        } else {
            Ok(())
        }
    }
    fn ingest(&mut self, track: TrackKind, frame: Frame, output: &mut Vec<ProcessedAudio>) {
        if track == TrackKind::System {
            output.push(processed(track, frame.clone(), false, None));
            if self.route.eligible {
                self.far.push_back(frame);
                self.drain_ready(output, false);
                self.trim_reference();
            }
        } else if !self.route.eligible {
            output.push(processed(
                track,
                frame,
                false,
                Some(
                    self.route
                        .reason
                        .clone()
                        .unwrap_or_else(|| UNVERIFIED.into()),
                ),
            ));
        } else {
            self.near.push_back(frame);
            self.drain_ready(output, false);
        }
    }
    fn reference(&self, start: u64, len: usize) -> Option<Vec<f32>> {
        let end = start.checked_add(len as u64)?;
        let mut cursor = start;
        let mut samples = Vec::with_capacity(len);
        for frame in &self.far {
            if frame.end() <= cursor {
                continue;
            }
            if frame.start > cursor {
                return None;
            }
            let offset = (cursor - frame.start) as usize;
            let count = (end - cursor).min((frame.samples.len() - offset) as u64) as usize;
            samples.extend_from_slice(&frame.samples[offset..offset + count]);
            cursor += count as u64;
            if cursor == end {
                return Some(samples);
            }
        }
        None
    }
    fn drain_ready(&mut self, output: &mut Vec<ProcessedAudio>, force: bool) {
        while let Some(frame) = self.near.front() {
            let partial = frame.samples.len() != FRAME_SAMPLES;
            let before_epoch = frame.start < self.epoch_start;
            let reference = if !partial && !before_epoch {
                self.reference(frame.start, frame.samples.len())
            } else {
                None
            };
            let deadline = frame
                .start
                .saturating_add(self.config.max_jitter_ms * TICKS_PER_MS);
            let over_capacity =
                self.near.iter().map(|f| f.samples.len()).sum::<usize>() > self.pending_cap();
            if reference.is_none()
                && !partial
                && !before_epoch
                && !force
                && !over_capacity
                && self.watermark < deadline
            {
                break;
            }
            let frame = self.near.pop_front().unwrap();
            if let Some(reference) = reference {
                let original = frame.samples.clone();
                let mut cleaned = vec![0.; FRAME_SAMPLES];
                let mut render_out = vec![0.; FRAME_SAMPLES];
                let engine = self.engine.get_or_insert_with(new_engine);
                let success = engine
                    .process_render_f32(&[&reference], &mut [&mut render_out])
                    .and_then(|()| engine.set_stream_delay_ms(0))
                    .and_then(|()| engine.process_capture_f32(&[&original], &mut [&mut cleaned]))
                    .is_ok()
                    && cleaned.iter().all(|s| s.is_finite());
                if success {
                    output.push(processed(
                        TrackKind::Mic,
                        Frame {
                            start: frame.start,
                            samples: cleaned,
                        },
                        true,
                        None,
                    ));
                } else {
                    self.engine = None;
                    output.push(processed(
                        TrackKind::Mic,
                        frame,
                        false,
                        Some(ENGINE_FAILURE.into()),
                    ));
                }
            } else {
                self.engine = None;
                let reason = if partial {
                    PARTIAL
                } else if before_epoch {
                    "AEC bypassed: audio precedes the current route epoch."
                } else {
                    MISSING
                };
                output.push(processed(TrackKind::Mic, frame, false, Some(reason.into())));
            }
        }
    }
    fn pending_cap(&self) -> usize {
        (self.config.max_jitter_ms as usize * 16).max(FRAME_SAMPLES)
    }
    fn trim_reference(&mut self) {
        let mut total = self.far.iter().map(|f| f.samples.len()).sum::<usize>();
        while total > self.pending_cap() {
            if let Some(frame) = self.far.pop_front() {
                total -= frame.samples.len();
            } else {
                break;
            }
        }
    }
    fn drain_all(&mut self, reason: Option<&str>) -> Result<Vec<ProcessedAudio>> {
        let mut output = Vec::new();
        // Render tails arrive first to make already-held near frames usable.
        for track in [TrackKind::System, TrackKind::Mic] {
            for frame in self.converters[index(track)].flush()? {
                if let Some(reason) = reason {
                    output.push(processed(
                        track,
                        frame,
                        false,
                        if track == TrackKind::Mic {
                            Some(reason.into())
                        } else {
                            None
                        },
                    ));
                } else {
                    self.ingest(track, frame, &mut output);
                }
            }
        }
        if let Some(reason) = reason {
            while let Some(frame) = self.near.pop_front() {
                output.push(processed(TrackKind::Mic, frame, false, Some(reason.into())));
            }
        } else {
            self.drain_ready(&mut output, true);
        }
        // Drain return order does not imply rewriting absolute track clocks.
        output.sort_by_key(|audio| (audio.start_ms, index(audio.track)));
        Ok(output)
    }
}
fn processed(
    track: TrackKind,
    frame: Frame,
    aec_applied: bool,
    reason: Option<String>,
) -> ProcessedAudio {
    ProcessedAudio {
        track,
        samples: frame.samples,
        rate: INFERENCE_RATE,
        start_ms: frame.start / TICKS_PER_MS,
        aec_applied,
        reason,
    }
}
fn new_engine() -> AudioProcessing {
    AudioProcessing::builder()
        .config(Config {
            echo_canceller: Some(EchoCanceller::default()),
            ..Default::default()
        })
        .capture_config(StreamConfig::new(INFERENCE_RATE, 1))
        .render_config(StreamConfig::new(INFERENCE_RATE, 1))
        .build()
}
fn ticks(ms: u64) -> Result<u64> {
    ms.checked_mul(TICKS_PER_MS)
        .ok_or(DspError::Invalid("timestamp overflow"))
}
fn index(track: TrackKind) -> usize {
    if track == TrackKind::Mic { 0 } else { 1 }
}
