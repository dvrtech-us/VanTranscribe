use super::*;
use std::sync::mpsc::{Receiver, SyncSender, TrySendError};
use vt_dsp::{DspConfig, EchoProcessor, ProcessedAudio, RouteEligibility};
#[cfg(test)]
#[path = "echo_pipeline_tests.rs"]
mod tests;

/// Final echo-info save at stop: 3 x 1 s + 100 ms + 200 ms worst case.
const FINAL_SAVE_BUDGET: store::LiveSaveBudget = store::LiveSaveBudget {
    busy_timeout: std::time::Duration::from_secs(1),
    attempts: 3,
    backoff: std::time::Duration::from_millis(100),
};
pub(crate) const ECHO_SAVE_BUSY_WARNING: &str = "Live echo processing details could not be saved because the database stayed busy. Raw audio is preserved.";
const INCOMPLETE_HISTORY: &str = "Capture-time output route history is incomplete or unavailable. Saved audio is transcribed without echo cancellation.";

pub(super) enum Input {
    Audio {
        track: TrackKind,
        samples: Vec<f32>,
        rate: u32,
        start_ms: u64,
        generation: u64,
        route: Option<RouteObservation>,
    },
    Route(RouteObservation),
    Reset {
        start_ms: u64,
        generation: u64,
    },
}

pub(super) struct EchoStream {
    processor: EchoProcessor,
    pub info: EchoInfo,
    route: Option<RouteObservation>,
    observed: bool,
    generation: u64,
    eligibility_block: Option<&'static str>,
}
impl EchoStream {
    pub fn new(enabled: bool) -> Result<Self> {
        Ok(Self {
            processor: EchoProcessor::new(DspConfig::default())?,
            info: EchoInfo {
                enabled,
                engine: vt_dsp::AEC_ENGINE.into(),
                ..Default::default()
            },
            route: None,
            observed: false,
            generation: 0,
            eligibility_block: None,
        })
    }
    fn count(&mut self, audio: &[ProcessedAudio]) {
        for packet in audio.iter().filter(|p| p.track == TrackKind::Mic) {
            let duration = packet.samples.len() as u64 * 1000 / u64::from(packet.rate);
            if packet.aec_applied {
                self.info.applied_mic_ms += duration;
            } else {
                self.info.bypassed_mic_ms += duration;
            }
            if let Some(reason) = &packet.reason {
                self.warn(reason);
            }
        }
    }
    fn warn(&mut self, reason: &str) {
        if self.info.warnings.len() < 12 && !self.info.warnings.iter().any(|old| old == reason) {
            self.info.warnings.push(reason.chars().take(1000).collect());
        }
    }
    pub fn observe(
        &mut self,
        route: Option<&RouteObservation>,
        start_ms: u64,
    ) -> Result<Vec<ProcessedAudio>> {
        if self.observed && self.route.as_ref() == route {
            return Ok(Vec::new());
        }
        self.observed = true;
        self.route = route.cloned();
        self.generation += 1;
        let eligible = self.info.enabled
            && self.eligibility_block.is_none()
            && route.is_some_and(|r| r.route.aec_eligible());
        let reason = if !self.info.enabled {
            Some("Echo cancellation was disabled in audio processing preferences.".into())
        } else if let Some(reason) = self.eligibility_block {
            Some(reason.into())
        } else if eligible {
            None
        } else {
            Some(route.map(|r|r.route.reason.explanation()).unwrap_or("No capture-time output route evidence is available. Echo cancellation is skipped.").into())
        };
        let packets = self.processor.configure_at_time(
            RouteEligibility {
                eligible,
                epoch: self.generation,
                reason,
            },
            start_ms,
        )?;
        self.count(&packets);
        Ok(packets)
    }
    pub fn accept(
        &mut self,
        track: TrackKind,
        samples: &[f32],
        rate: u32,
        start_ms: u64,
    ) -> Result<Vec<ProcessedAudio>> {
        let packets = self.processor.accept(track, samples, rate, start_ms)?;
        self.count(&packets);
        Ok(packets)
    }
    pub fn reset(&mut self, start_ms: u64) -> Result<Vec<ProcessedAudio>> {
        let packets = self
            .processor
            .reset_at_time("capture_discontinuity", start_ms)?;
        self.count(&packets);
        Ok(packets)
    }
    pub fn finish(&mut self) -> Result<Vec<ProcessedAudio>> {
        let packets = self.processor.flush()?;
        self.count(&packets);
        Ok(packets)
    }
}

struct Pipe {
    sender: Option<SyncSender<session::Inference>>,
    resets: [bool; 2],
    speakers: bool,
}
impl Pipe {
    fn reset(&mut self) {
        self.resets = [true, true];
    }
    fn send(&mut self, service: &AppService, packet: &ProcessedAudio, generation: u64) {
        let worker = if self.speakers {
            backlog::Worker::Speakers
        } else {
            backlog::Worker::Stt
        };
        if !service.inference_current(generation) || !service.inference_allowed(worker) {
            return;
        }
        let end = packet
            .start_ms
            .saturating_add(packet.samples.len() as u64 * 1000 / u64::from(packet.rate));
        let Some(sender) = &self.sender else {
            return;
        };
        for (i, track) in [TrackKind::Mic, TrackKind::System].into_iter().enumerate() {
            if self.resets[i] {
                match sender.try_send(session::Inference::Reset(track, generation)) {
                    Ok(()) => self.resets[i] = false,
                    Err(TrySendError::Disconnected(_)) => {
                        self.sender = None;
                        return;
                    }
                    Err(TrySendError::Full(_)) => {
                        service.inference_gap(
                            worker,
                            packet.track,
                            generation,
                            packet.start_ms,
                            end,
                            "queue_full",
                        );
                        self.overload(service);
                        return;
                    }
                }
            }
        }
        match sender.try_send(session::Inference::Audio {
            track: packet.track,
            samples: packet.samples.clone(),
            rate: packet.rate,
            start_ms: packet.start_ms,
            generation,
        }) {
            Ok(()) => (),
            Err(TrySendError::Full(_)) => {
                self.reset();
                service.inference_gap(
                    worker,
                    packet.track,
                    generation,
                    packet.start_ms,
                    end,
                    "queue_full",
                );
                self.overload(service);
            }
            Err(TrySendError::Disconnected(_)) => {
                self.sender = None;
                service.inference_gap(
                    worker,
                    packet.track,
                    generation,
                    packet.start_ms,
                    end,
                    "worker_unavailable",
                );
                service
                    .backlog
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .stopped(worker);
                self.overload(service);
            }
        }
    }
    fn overload(&self, service: &AppService) {
        if let Ok(mut runtime) = service.runtime.lock() {
            if self.speakers {
                runtime.live_processing.diarization_warning = Some(
                    "Live speaker labels fell behind. Refine the saved audio after stopping."
                        .into(),
                );
            } else if let Some(current) = runtime.current.as_mut() {
                current.view.error=Some("Live transcription fell behind. Audio recording continues; refine the saved audio after stopping.".into());
            }
        }
    }
}

pub(super) fn worker(
    service: Arc<AppService>,
    id: String,
    receiver: Receiver<Input>,
    speech: Option<SyncSender<session::Inference>>,
    speakers: Option<SyncSender<session::Inference>>,
) -> Result<()> {
    let enabled = service
        .audio_preferences
        .lock()
        .map(|p| p.echo_cancellation_enabled)
        .unwrap_or(false);
    let mut stream = EchoStream::new(enabled)?;
    let mut speech = Pipe {
        sender: speech,
        resets: [false; 2],
        speakers: false,
    };
    let mut speakers = Pipe {
        sender: speakers,
        resets: [false; 2],
        speakers: true,
    };
    let run = store::detail(&service.root, &id)?
        .active_run_id
        .ok_or_else(|| anyhow::anyhow!("Live transcript run is unavailable"))?;
    let mut generation = service
        .backlog
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .generation;
    let mut source_ends = [None; 2];
    while let Ok(input) = receiver.recv() {
        if !service.inference_allowed(backlog::Worker::Dsp) {
            break;
        }
        match input {
            Input::Route(route) => {
                // Metadata is also attached to each audio packet, so a queued
                // route event never needs to flush audio across a source reset.
                if service.inference_current(generation) {
                    deliver(
                        &service,
                        &mut speech,
                        &mut speakers,
                        stream.observe(Some(&route), route.start_ns / 1_000_000)?,
                        generation,
                    );
                }
            }
            Input::Reset {
                start_ms,
                generation: incoming,
            } => {
                if !service.inference_current(incoming) {
                    continue;
                }
                let _ = stream.reset(start_ms)?;
                generation = incoming;
                source_ends = [None; 2];
                speech.reset();
                speakers.reset();
            }
            Input::Audio {
                track,
                samples,
                rate,
                start_ms,
                route,
                generation: incoming,
            } => {
                if !service.inference_current(incoming) {
                    continue;
                }
                let index = usize::from(track == TrackKind::System);
                if generation != incoming
                    || source_ends[index].is_some_and(|end: u64| start_ms > end.saturating_add(1))
                {
                    // Inference queue loss also resets adaptive/context state,
                    // but must not reset the raw watermark/overload evidence.
                    let _ = stream.reset(start_ms)?;
                    generation = incoming;
                    source_ends = [None; 2];
                    speech.reset();
                    speakers.reset();
                }
                source_ends[index] =
                    Some(start_ms.saturating_add(samples.len() as u64 * 1000 / u64::from(rate)));
                deliver(
                    &service,
                    &mut speech,
                    &mut speakers,
                    stream.observe(route.as_ref(), start_ms)?,
                    generation,
                );
                deliver(
                    &service,
                    &mut speech,
                    &mut speakers,
                    stream.accept(track, &samples, rate, start_ms)?,
                    generation,
                );
                service.inference_complete(
                    backlog::Worker::Dsp,
                    track,
                    generation,
                    start_ms.saturating_add(samples.len() as u64 * 1000 / u64::from(rate)),
                );
            }
        }
        if let Ok(mut runtime) = service.runtime.lock() {
            runtime.live_processing.echo = Some(stream.info.clone());
        }
    }
    if !service.inference_allowed(backlog::Worker::Dsp) {
        // Name the real reason: a stopped speech worker, load shedding or disk
        // protection (a dead speaker worker alone never stops this stage).
        let warning = backlog::audio_stage_stopped_warning(service.audio_stage_stop_reason());
        stream.warn(warning);
        service.backlog_warn(warning);
    } else if service.inference_current(generation) {
        deliver(
            &service,
            &mut speech,
            &mut speakers,
            stream.finish()?,
            generation,
        );
    }

    // The final save may meet a busy database at stop: retry briefly, and do
    // not fail the worker for lock contention (the info is diagnostic only).
    #[cfg(not(test))]
    let budget = FINAL_SAVE_BUDGET;
    #[cfg(test)]
    let budget = service
        .test_workers
        .lock()
        .unwrap()
        .as_ref()
        .map(|fake| fake.live_save)
        .unwrap_or(FINAL_SAVE_BUDGET);
    match store::retry_busy(budget, |busy_timeout| {
        echo_store::save_info_with(&service.root, &id, &run, &stream.info, busy_timeout)
    }) {
        Ok(()) => (),
        Err(error) if store::sqlite_busy(&error) => service.backlog_warn(&format!(
            "{ECHO_SAVE_BUSY_WARNING} Cause: {}",
            store::describe_error(&error)
        )),
        Err(error) => return Err(error),
    }
    if let Ok(mut runtime) = service.runtime.lock() {
        runtime.live_processing.echo = Some(stream.info);
    }
    Ok(())
}

/// Reads route evidence recorded with the source, never the machine's route now.
struct RouteLookup {
    current: Option<RouteObservation>,
    next: Option<u64>,
    initialized: bool,
}
impl RouteLookup {
    fn at(
        &mut self,
        root: &std::path::Path,
        id: &str,
        ms: u64,
    ) -> Result<Option<&RouteObservation>> {
        let ns = ms
            .checked_mul(1_000_000)
            .ok_or_else(|| anyhow::anyhow!("Audio time overflow"))?;
        if !self.initialized || self.next.is_some_and(|next| ns >= next) {
            // Looked up only at route boundaries, each on a fresh read
            // connection, so audio decoding never runs inside a database read.
            let (current, next) = store::read(root, |db| {
                let current = echo_store::route_at(db, id, ms)?;
                let next=db.query_row("SELECT MIN(start_ns) FROM app_output_routes WHERE meeting_id=?1 AND start_ns>?2",rusqlite::params![id,i64::try_from(ns)?],|r|r.get::<_,Option<i64>>(0))?.map(u64::try_from).transpose()?;
                Ok((current, next))
            })?;
            self.current = current;
            self.next = next;
            self.initialized = true;
        }
        Ok(self.current.as_ref())
    }
}

pub(super) fn saved(
    service: &AppService,
    id: &str,
    enabled: bool,
    mut consume: impl FnMut(processing::AudioPacket, f64) -> Result<()>,
) -> Result<EchoInfo> {
    let complete = store::read(&service.root, |db| echo_store::routes_complete(db, id))?;
    let mut stream = EchoStream::new(enabled)?;
    if enabled && !complete {
        stream.eligibility_block = Some(INCOMPLETE_HISTORY);
        stream.warn(INCOMPLETE_HISTORY);
    }
    let mut lookup = RouteLookup {
        current: None,
        next: None,
        initialized: false,
    };
    processing::for_each_audio(service, id, |packet, fraction| {
        match packet {
            processing::AudioPacket::Audio {
                track,
                samples,
                rate,
                start_ms,
            } => {
                deliver_saved(
                    &mut consume,
                    stream.observe(lookup.at(&service.root, id, start_ms)?, start_ms)?,
                    fraction,
                )?;
                deliver_saved(
                    &mut consume,
                    stream.accept(track, &samples, rate, start_ms)?,
                    fraction,
                )?;
            }
            processing::AudioPacket::Gap { track, start_ms } => {
                deliver_saved(&mut consume, stream.reset(start_ms)?, fraction)?;
                consume(processing::AudioPacket::Gap { track, start_ms }, fraction)?;
            }
        }
        Ok(())
    })?;
    deliver_saved(&mut consume, stream.finish()?, 1.0)?;
    Ok(stream.info)
}

fn deliver(
    service: &AppService,
    speech: &mut Pipe,
    speakers: &mut Pipe,
    packets: Vec<ProcessedAudio>,
    generation: u64,
) {
    for packet in packets {
        speech.send(service, &packet, generation);
        speakers.send(service, &packet, generation);
    }
}
fn deliver_saved(
    consume: &mut impl FnMut(processing::AudioPacket, f64) -> Result<()>,
    packets: Vec<ProcessedAudio>,
    fraction: f64,
) -> Result<()> {
    for packet in packets {
        consume(
            processing::AudioPacket::Audio {
                track: packet.track,
                samples: packet.samples,
                rate: packet.rate,
                start_ms: packet.start_ms,
            },
            fraction,
        )?;
    }
    Ok(())
}
