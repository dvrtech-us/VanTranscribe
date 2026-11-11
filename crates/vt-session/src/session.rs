use super::*;
use anyhow::Context;
use std::{
    str::FromStr,
    sync::mpsc::{Receiver, RecvTimeoutError, TrySendError},
    time::Duration,
};
use vt_capture::{CaptureEvent, CaptureProcess};
use vt_core::{AudioBlock, FramePosition, GapReason, StreamFormat};
use vt_record::{Gap, Recording};
use vt_stt::TranscriptEvent;

struct SourcePosition {
    epoch: u32,
    frame: u64,
    host_ns: u64,
}
struct TrackBuffer {
    track: TrackKind,
    format: Option<StreamFormat>,
    source_epoch: u32,
    source_next: Option<u64>,
    writer_epoch: u32,
    persisted_epoch: Option<u32>,
    writer_frame: u64,
    writer_end: Option<u64>,
    next_seq: u64,
    samples: Vec<f32>,
    first_source: Option<SourcePosition>,
    resume_pending: bool,
    last_source_end: Option<u64>,
}

impl TrackBuffer {
    fn new(track: TrackKind) -> Self {
        Self {
            track,
            format: None,
            source_epoch: 0,
            source_next: None,
            writer_epoch: 0,
            persisted_epoch: None,
            writer_frame: 0,
            writer_end: None,
            next_seq: 0,
            samples: Vec::new(),
            first_source: None,
            resume_pending: false,
            last_source_end: None,
        }
    }

    fn flush(
        &mut self,
        recorder: &mut Recording,
        service: &AppService,
        id: &str,
        busy_warned: &mut bool,
    ) -> Result<()> {
        if self.samples.is_empty() {
            return Ok(());
        }
        service.recording_budget(self.samples.len() as u64 * 3 + 4096, busy_warned)?;
        let format = self.format.context("capture format is missing")?;
        let source = self
            .first_source
            .take()
            .context("capture position is missing")?;
        let start = self.writer_end.unwrap_or(source.host_ns);
        let samples = std::mem::take(&mut self.samples);
        let block = AudioBlock::new(
            format,
            FramePosition {
                epoch: self.writer_epoch,
                first_frame: self.writer_frame,
                host_time_ns: start,
            },
            samples,
        )?;
        // Persist the original hardware observation before publication. The
        // worker's exact adjacency must not erase measured source-clock drift.
        // `app_audio_clock` has no live reader, so a lock skips this row.
        tolerate_busy(
            service,
            busy_warned,
            store::save_clock(
                &service.root,
                id,
                self.track.as_str(),
                self.next_seq,
                source.epoch,
                source.frame,
                source.host_ns,
                store::LOOP_BUSY_TIMEOUT,
            ),
        )?;
        let chunk = recorder.write_chunk(self.track, &block);
        note_recorder_waits(service, recorder);
        let chunk = chunk?;
        self.writer_frame += block.frames();
        self.writer_end = Some(chunk.end_ns);
        self.next_seq = chunk.seq + 1;
        self.persisted_epoch = Some(chunk.epoch);
        Ok(())
    }

    fn gap_until(
        &mut self,
        target: u64,
        reason: GapReason,
        recorder: &mut Recording,
        service: &AppService,
        id: &str,
        busy_warned: &mut bool,
    ) -> Result<()> {
        self.flush(recorder, service, id, busy_warned)?;
        if let (Some(end), Some(format)) = (self.writer_end, self.format)
            && target > end
        {
            let gap = recorder.write_gap(
                self.track,
                format,
                Gap {
                    epoch: self.writer_epoch,
                    first_frame: self.writer_frame,
                    frame_count: 0,
                    start_ns: end,
                    end_ns: target,
                    reason,
                },
            );
            note_recorder_waits(service, recorder);
            let gap = gap?;
            self.writer_end = Some(target);
            self.next_seq = gap.seq + 1;
            self.persisted_epoch = Some(gap.epoch);
            if let Ok(mut runtime) = service.runtime.lock()
                && let Some(current) = &mut runtime.current
            {
                current.view.gaps += 1;
            }
        }
        self.source_next = None;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn set_format(
        &mut self,
        format: StreamFormat,
        epoch: u32,
        start: u64,
        recorder: &mut Recording,
        service: &AppService,
        id: &str,
        busy_warned: &mut bool,
    ) -> Result<()> {
        if self.format.is_some() && (self.format != Some(format) || self.source_epoch != epoch) {
            self.gap_until(
                start,
                GapReason::DeviceChange,
                recorder,
                service,
                id,
                busy_warned,
            )?;
            if let Some(last) = self.persisted_epoch {
                self.writer_epoch = last.checked_add(1).context("capture epoch overflow")?;
                self.writer_frame = 0;
            }
        }
        self.format = Some(format);
        self.source_epoch = epoch;
        self.source_next = None;
        Ok(())
    }

    fn accept(
        &mut self,
        position: SourcePosition,
        samples: Vec<f32>,
        recorder: &mut Recording,
        service: &AppService,
        id: &str,
        busy_warned: &mut bool,
    ) -> Result<(Vec<f32>, u32, u64)> {
        let SourcePosition {
            epoch,
            frame: first,
            host_ns: host,
        } = position;
        let format = self.format.context("Audio arrived before its format.")?;
        if samples.is_empty()
            || !samples.len().is_multiple_of(format.channels as usize)
            || samples.len() > format.sample_rate as usize * format.channels as usize * 2
            || samples.iter().any(|s| !s.is_finite())
        {
            bail!("Capture emitted invalid or oversized audio.");
        }
        if self.source_epoch != epoch {
            self.set_format(format, epoch, host, recorder, service, id, busy_warned)?;
        }
        if self.resume_pending || self.source_next.is_some_and(|next| next != first) {
            self.gap_until(
                host,
                if self.resume_pending {
                    GapReason::Pause
                } else {
                    GapReason::Overflow
                },
                recorder,
                service,
                id,
                busy_warned,
            )?;
            self.resume_pending = false;
        }
        let frames = samples.len() / format.channels as usize;
        let start = self
            .writer_end
            .or_else(|| self.first_source.as_ref().map(|source| source.host_ns))
            .unwrap_or(host)
            + self.samples.len() as u64 / format.channels as u64 * 1_000_000_000
                / format.sample_rate as u64;
        let mono = samples
            .chunks_exact(format.channels as usize)
            .map(|frame| frame.iter().sum::<f32>() / format.channels as f32)
            .collect();
        let mut offset = 0;
        while offset < samples.len() {
            if self.samples.is_empty() {
                let frame_offset = offset / format.channels as usize;
                self.first_source = Some(SourcePosition {
                    epoch,
                    frame: first + frame_offset as u64,
                    host_ns: host + frame_offset as u64 * 1_000_000_000 / format.sample_rate as u64,
                });
            }
            let capacity = format.sample_rate as usize * format.channels as usize * 2;
            let take = (capacity - self.samples.len()).min(samples.len() - offset);
            self.samples
                .extend_from_slice(&samples[offset..offset + take]);
            offset += take;
            if self.samples.len() == capacity {
                self.flush(recorder, service, id, busy_warned)?;
            }
        }
        self.source_next = Some(first + frames as u64);
        Ok((mono, format.sample_rate, start / 1_000_000))
    }
}

// Every queued capture/DSP audio message contains at most20ms of native
// frames. The128-slot capture queue therefore holds at most2.56s aggregate
// audio; the600-slot DSP queue at most12s. No valid2s helper packet can turn
// message-count limits into minutes of buffered audio. Hardware origin/frame
// positions are preserved, deriving sub-slice times only from sample offsets.
fn relay_capture_event(
    event: vt_capture::Result<CaptureEvent>,
    formats: &mut BTreeMap<String, StreamFormat>,
    sender: &mpsc::SyncSender<vt_capture::Result<CaptureEvent>>,
) -> bool {
    let event = match event {
        Ok(event) => event,
        Err(error) => {
            let _ = sender.send(Err(error));
            return false;
        }
    };
    match event {
        CaptureEvent::Format {
            track,
            sample_rate,
            channels,
            epoch,
            start_ns,
        } => {
            if let Ok(format) = StreamFormat::new(sample_rate, channels) {
                formats.insert(track.clone(), format);
            }
            sender
                .send(Ok(CaptureEvent::Format {
                    track,
                    sample_rate,
                    channels,
                    epoch,
                    start_ns,
                }))
                .is_ok()
        }
        CaptureEvent::Audio {
            track,
            epoch,
            first_frame,
            host_time_ns,
            samples,
        } => {
            let Some(format) = formats.get(&track) else {
                return sender
                    .send(Ok(CaptureEvent::Audio {
                        track,
                        epoch,
                        first_frame,
                        host_time_ns,
                        samples,
                    }))
                    .is_ok();
            };
            if samples.is_empty()
                || !samples.len().is_multiple_of(usize::from(format.channels))
                || samples.len() > format.sample_rate as usize * usize::from(format.channels) * 2
            {
                return sender
                    .send(Ok(CaptureEvent::Audio {
                        track,
                        epoch,
                        first_frame,
                        host_time_ns,
                        samples,
                    }))
                    .is_ok();
            }
            let frame_limit = (format.sample_rate as usize / 50).max(1);
            let scalar_limit = frame_limit * usize::from(format.channels);
            for (index, part) in samples.chunks(scalar_limit).enumerate() {
                let offset = (index * frame_limit) as u64;
                let Some(first) = first_frame.checked_add(offset) else {
                    let _ = sender.send(Err(vt_capture::CaptureError::Invalid(
                        "Capture frame position overflow".into(),
                    )));
                    return false;
                };
                let Some(host) = host_time_ns
                    .checked_add(offset * 1_000_000_000 / u64::from(format.sample_rate))
                else {
                    let _ = sender.send(Err(vt_capture::CaptureError::Invalid(
                        "Capture source time overflow".into(),
                    )));
                    return false;
                };
                if sender
                    .send(Ok(CaptureEvent::Audio {
                        track: track.clone(),
                        epoch,
                        first_frame: first,
                        host_time_ns: host,
                        samples: part.to_vec(),
                    }))
                    .is_err()
                {
                    return false;
                }
            }
            true
        }
        event => sender.send(Ok(event)).is_ok(),
    }
}

#[derive(Debug)]
pub(super) enum Inference {
    Audio {
        track: TrackKind,
        samples: Vec<f32>,
        rate: u32,
        start_ms: u64,
        generation: u64,
    },
    Reset(TrackKind, u64),
}

fn helper_gap_reason(reason: &str) -> GapReason {
    match reason {
        "pause" => GapReason::Pause,
        "overflow" => GapReason::Overflow,
        "device_change" | "process_exclusion_change" | "sleep" | "wake" => GapReason::DeviceChange,
        _ => GapReason::Unavailable,
    }
}

/// Per-worker context for live transcript publication, resolved once at
/// worker start (the meeting mode no longer costs a connection per event).
pub(super) struct LiveTranscript {
    pub(super) in_person: bool,
    pub(super) sink: crate::live_transcript::TranscriptSink,
}

/// Publishes one recognizer event. Partials update the runtime view only.
/// Finals are shown at once through `Runtime::live_finals` and handed to the
/// session's transcript writer; this never touches SQLite and cannot fail, so
/// persistence can neither block nor stop recognition.
pub(super) fn save_transcript(
    service: &AppService,
    live: &LiveTranscript,
    generation: u64,
    event: TranscriptEvent,
) {
    if !event.is_final
        && !service
            .backlog
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .partials()
    {
        return;
    }

    let speaker = if live.in_person {
        "Unknown"
    } else if event.track == TrackKind::Mic {
        "Me"
    } else {
        "Them"
    };
    let segment = Segment {
        id: if event.is_final {
            uuid::Uuid::new_v4().to_string()
        } else {
            format!("partial-{}", event.track.as_str())
        },
        track: event.track.as_str().into(),
        start_ms: event.start_ms,
        end_ms: event.end_ms.max(event.start_ms),
        text: event.text,
        speaker: speaker.into(),
        words: event
            .words
            .into_iter()
            .map(|word| TranscriptWord {
                text: word.text,
                start_ms: word.start_ms,
                end_ms: word.end_ms,
            })
            .collect(),
    };
    if event.is_final {
        let keep = !segment.text.trim().is_empty();
        if let Ok(mut runtime) = service.runtime.lock() {
            runtime.partials.remove(&event.track);
            if keep {
                runtime.live_finals.push(segment.clone());
            }
        }
        if keep {
            live.sink.push(generation, event.track, segment);
        }
    } else if let Ok(mut runtime) = service.runtime.lock()
        && !segment.text.trim().is_empty()
    {
        runtime.partials.insert(event.track, segment);
    }
}

// Completion measures accepted input, including silence and VAD-suppressed audio.
// No source watermark is advanced by model initialization or transcript output.
pub(super) fn consume_inference(
    service: &AppService,
    worker: backlog::Worker,
    rx: &Receiver<Inference>,
    mut accept: impl FnMut(Inference) -> Result<()>,
) -> Result<()> {
    let mut previous = None;
    while let Ok(message) = rx.recv() {
        if !service.inference_allowed(worker) {
            break;
        }
        let generation = match &message {
            Inference::Audio { generation, .. } | Inference::Reset(_, generation) => *generation,
        };
        if !service.inference_current(generation) {
            continue;
        }
        if previous != Some(generation) {
            for track in [TrackKind::Mic, TrackKind::System] {
                accept(Inference::Reset(track, generation))?;
            }
            previous = Some(generation);
        }
        let completion = match &message {
            Inference::Audio {
                track,
                samples,
                rate,
                start_ms,
                ..
            } => Some((
                *track,
                start_ms.saturating_add(samples.len() as u64 * 1000 / u64::from(*rate)),
            )),
            _ => None,
        };
        accept(message)?;
        if let Some((track, end)) = completion {
            service.inference_complete(worker, track, generation, end);
        }
    }
    Ok(())
}
pub(crate) fn spawn_inference_worker(
    service: Arc<AppService>,
    worker: backlog::Worker,
    name: &str,
    work: impl FnOnce() -> Result<()> + Send + 'static,
) -> Option<std::thread::JoinHandle<Result<()>>> {
    let finished = service.clone();
    let thread_name = name.to_owned();
    match std::thread::Builder::new()
        .name(name.into())
        .spawn(move || {
            let result = work();
            // A no-op when the worker already recorded its own exit.
            finished
                .backlog
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .worker_exit(worker, &thread_name, &result);
            result
        }) {
        Ok(worker) => Some(worker),
        Err(_) => {
            let mut controller = service.backlog.lock().unwrap_or_else(|e| e.into_inner());
            controller.stopped_because(worker, backlog::LiveStopReason::WorkerUnavailable);
            controller.warn("A live inference worker could not start. Raw audio is preserved.");
            None
        }
    }
}
/// The fatal cause, including any SQLite extended result code, bounded so a
/// long error chain cannot crowd the inference health report.
pub(crate) fn worker_failure_warning(name: &str, error: &anyhow::Error) -> String {
    let mut cause = store::describe_error(error);
    if cause.chars().count() > 300 {
        cause = cause.chars().take(300).collect::<String>() + "…";
    }
    format!("Live inference worker {name} stopped: {cause}")
}

/// Speech engine errors the live speech worker recovers from in one recording.
/// Each recovery loses at most the failing packet (at most 2 s) plus the
/// recognizer context of one track, so 20 bounds the live text lost to roughly
/// 40 s even when alternating causes defeat the repeat rule, while a transient
/// fault (one bad packet, a resampler that could not be created after a device
/// change) never ends live transcription. Raw audio and refine are unaffected.
pub(crate) const STT_RECOVERY_BUDGET: u32 = 20;
/// Inference gap reason for the packet whose recognition failed and recovered.
pub(crate) const RECOGNIZER_ERROR_GAP: &str = "recognizer_error";
pub(crate) const STT_RECOVERED_WARNING: &str = "Live transcription recovered from a speech engine error; a short stretch of live text may be missing. Raw audio is preserved; refine saved audio for complete coverage.";

/// The recognizer operations used by the live speech worker.
pub(crate) trait LiveRecognizer {
    fn accept(
        &mut self,
        track: TrackKind,
        samples: &[f32],
        rate: u32,
        start_ms: u64,
        generation: u64,
    ) -> Result<Vec<TranscriptEvent>>;
    /// Finalizes and clears one track; returns its pending hypothesis.
    fn reset_track(&mut self, track: TrackKind, generation: u64) -> Result<Vec<TranscriptEvent>>;
    fn finish(&mut self) -> Result<Vec<TranscriptEvent>>;
    /// Replaces the engine. Used only after the engine itself failed: it may
    /// reload model files.
    fn rebuild(&mut self) -> Result<()>;
}

struct SherpaRecognizer {
    engine: vt_stt::StreamingRecognizer,
    manager: vt_stt::ModelManager,
    vad: Option<std::path::PathBuf>,
}
impl LiveRecognizer for SherpaRecognizer {
    fn accept(
        &mut self,
        track: TrackKind,
        samples: &[f32],
        rate: u32,
        start_ms: u64,
        _: u64,
    ) -> Result<Vec<TranscriptEvent>> {
        Ok(self.engine.accept(track, samples, rate, start_ms)?)
    }
    fn reset_track(&mut self, track: TrackKind, _: u64) -> Result<Vec<TranscriptEvent>> {
        Ok(self.engine.reset_track(track)?)
    }
    fn finish(&mut self) -> Result<Vec<TranscriptEvent>> {
        Ok(self.engine.finish()?)
    }
    fn rebuild(&mut self) -> Result<()> {
        self.engine = match live_models::recognizer(&self.manager, self.vad.as_deref()) {
            Ok(engine) => engine,
            Err(_) if self.vad.is_some() => live_models::recognizer(&self.manager, None)?,
            Err(error) => return Err(error),
        };
        Ok(())
    }
}

#[cfg(test)]
struct FakeRecognizer(Arc<backlog::FakeAccept>);
#[cfg(test)]
impl LiveRecognizer for FakeRecognizer {
    fn accept(
        &mut self,
        track: TrackKind,
        samples: &[f32],
        rate: u32,
        start_ms: u64,
        generation: u64,
    ) -> Result<Vec<TranscriptEvent>> {
        (self.0)(
            backlog::Worker::Stt,
            &Inference::Audio {
                track,
                samples: samples.to_vec(),
                rate,
                start_ms,
                generation,
            },
        )
    }
    fn reset_track(&mut self, track: TrackKind, generation: u64) -> Result<Vec<TranscriptEvent>> {
        (self.0)(backlog::Worker::Stt, &Inference::Reset(track, generation))
    }
    fn finish(&mut self) -> Result<Vec<TranscriptEvent>> {
        Ok(Vec::new())
    }
    fn rebuild(&mut self) -> Result<()> {
        Ok(())
    }
}

fn track_index(track: TrackKind) -> usize {
    usize::from(track == TrackKind::System)
}

/// Per-message speech recognition with in-place recovery. Recovery must happen
/// on this thread: it owns the only receiver of the speech queue.
struct SpeechWorker<'a, R> {
    service: &'a AppService,
    live: LiveTranscript,
    engine: R,
    recoveries: u32,
    /// Cause of the previous error while no message has succeeded since.
    last_cause: Option<String>,
    /// End of the audio accepted per track since its last reset.
    fed_end: [Option<u64>; 2],
    /// Last final published per track: (start_ms, text).
    last_final: [Option<(u64, String)>; 2],
    last_generation: Option<u64>,
}
impl<R: LiveRecognizer> SpeechWorker<'_, R> {
    fn handle(&mut self, message: Inference) -> Result<()> {
        match message {
            Inference::Audio {
                track,
                samples,
                rate,
                start_ms,
                generation,
            } => {
                let end = start_ms.saturating_add(samples.len() as u64 * 1000 / u64::from(rate));
                // The hypothesis pending before a generation change belongs
                // to the previous generation; it is saved by the reset.
                match self
                    .engine
                    .accept(track, &samples, rate, start_ms, generation)
                {
                    Ok(events) => {
                        self.last_generation = Some(generation);
                        self.last_cause = None;
                        let fed = &mut self.fed_end[track_index(track)];
                        *fed = Some(fed.map_or(end, |old| old.max(end)));
                        if self.service.inference_allowed(backlog::Worker::Stt)
                            && self.service.inference_current(generation)
                        {
                            for event in events {
                                self.publish(generation, event);
                            }
                        }
                    }
                    Err(error) => self.recover(error, track, generation, start_ms, end)?,
                }
            }
            Inference::Reset(track, generation) => {
                match self.engine.reset_track(track, generation) {
                    Ok(events) => {
                        self.last_cause = None;
                        self.save_pending(track, events);
                    }
                    Err(error) => self.recover(error, track, generation, 0, 0)?,
                }
                if let Ok(mut runtime) = self.service.runtime.lock() {
                    runtime.partials.remove(&track);
                }
            }
        }
        Ok(())
    }
    fn publish(&mut self, generation: u64, event: TranscriptEvent) {
        if event.is_final {
            self.last_final[track_index(event.track)] = Some((event.start_ms, event.text.clone()));
        }
        save_transcript(self.service, &self.live, generation, event);
    }
    /// Saves the finals a reset or finish returned for `track`: bounded by the
    /// audio accepted before the reset (so never past a gap start) and skipped
    /// when identical to the last final already published for the track.
    fn save_pending(&mut self, track: TrackKind, events: Vec<TranscriptEvent>) {
        let index = track_index(track);
        let bound = self.fed_end[index].take();
        if !self.service.inference_allowed(backlog::Worker::Stt) {
            return;
        }
        let generation = self.last_generation.unwrap_or_default();
        for mut event in events {
            if !event.is_final || event.track != track || event.text.trim().is_empty() {
                continue;
            }
            if let Some(bound) = bound {
                event.end_ms = event.end_ms.min(bound);
                event.start_ms = event.start_ms.min(event.end_ms);
                for word in &mut event.words {
                    word.end_ms = word.end_ms.min(event.end_ms);
                    word.start_ms = word.start_ms.clamp(event.start_ms, word.end_ms);
                }
            }
            if self.last_final[index]
                .as_ref()
                .is_some_and(|(start, text)| *start == event.start_ms && *text == event.text)
            {
                continue;
            }
            self.publish(generation, event);
        }
    }
    fn save_all_pending(&mut self, events: Vec<TranscriptEvent>) {
        for track in [TrackKind::Mic, TrackKind::System] {
            let mine = events
                .iter()
                .filter(|e| e.track == track)
                .cloned()
                .collect();
            self.save_pending(track, mine);
        }
    }
    /// Records a recognition error as an STT gap and failure, resets the
    /// affected recognizer state, and continues. The same cause twice with no
    /// successful message in between, or an exhausted budget, is terminal.
    fn recover(
        &mut self,
        error: anyhow::Error,
        track: TrackKind,
        generation: u64,
        start_ms: u64,
        end_ms: u64,
    ) -> Result<()> {
        let cause = store::describe_error(&error);
        if self.last_cause.as_deref() == Some(cause.as_str()) {
            return Err(error);
        }
        if self.recoveries >= STT_RECOVERY_BUDGET {
            return Err(error.context(backlog::RecoveryExhausted));
        }
        self.recoveries += 1;
        {
            let mut controller = self
                .service
                .backlog
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            controller.gap(
                backlog::Worker::Stt,
                track,
                generation,
                start_ms,
                end_ms,
                RECOGNIZER_ERROR_GAP,
            );
            controller.failure(
                backlog::Worker::Stt,
                (end_ms > start_ms).then_some(start_ms),
                &cause,
                false,
            );
            controller.warn(STT_RECOVERED_WARNING);
        }
        self.last_cause = Some(cause);
        let engine_failed = error
            .downcast_ref::<vt_stt::SttError>()
            .is_some_and(|error| matches!(error, vt_stt::SttError::Engine));
        if !engine_failed && let Ok(events) = self.engine.reset_track(track, generation) {
            self.save_pending(track, events);
            return Ok(());
        }
        // The engine itself failed: save what both tracks had, then replace it.
        if let Ok(events) = self.engine.finish() {
            self.save_all_pending(events);
        }
        self.fed_end = [None; 2];
        self.engine.rebuild()
    }
    fn finish(mut self) -> Result<()> {
        // Saved even when the last generation is no longer current (a pause or
        // gap just before stop): the audio was real and is bounded above.
        if self.service.inference_allowed(backlog::Worker::Stt) {
            let events = self.engine.finish()?;
            self.save_all_pending(events);
        }
        Ok(())
    }
}

pub(crate) fn infer_worker(
    service: Arc<AppService>,
    id: String,
    rx: Receiver<Inference>,
    sink: crate::live_transcript::TranscriptSink,
) -> Result<()> {
    let result = speech_worker(&service, &id, &rx, sink);
    // Record how the worker ended while its queue is still open, so the audio
    // stage sees the real reason (not a closed queue) when its sends fail.
    service
        .backlog
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .worker_exit(backlog::Worker::Stt, "speech-inference", &result);
    drop(rx);
    result
}
fn speech_worker(
    service: &Arc<AppService>,
    id: &str,
    rx: &Receiver<Inference>,
    sink: crate::live_transcript::TranscriptSink,
) -> Result<()> {
    if !service.inference_allowed(backlog::Worker::Stt) {
        return Ok(());
    }
    let live = LiveTranscript {
        in_person: store::meeting_mode(&service.root, id)? == vt_diarize::MeetingMode::InPerson,
        sink,
    };
    #[cfg(test)]
    let fake = service.test_workers.lock().unwrap().clone();
    #[cfg(test)]
    if let Some(fake) = fake {
        return run_speech(service, id, rx, live, FakeRecognizer(fake.accept.clone()));
    }

    let manager = service.live_model_manager()?;
    let vad = service.vad_path();
    let (engine, vad) = match live_models::recognizer(&manager, vad.as_deref()) {
        Ok(engine) => {
            if let Ok(mut runtime) = service.runtime.lock() {
                runtime.live_processing.vad_active = vad.is_some();
            }
            (engine, vad)
        }
        Err(error) if vad.is_some() => {
            if let Ok(mut runtime) = service.runtime.lock() {
                runtime.live_processing.vad_warning = Some(format!(
                    "Voice activity detection is unavailable; live recognition continues without it: {error}"
                ));
            }
            (live_models::recognizer(&manager, None)?, None)
        }
        Err(error) => return Err(error),
    };
    run_speech(
        service,
        id,
        rx,
        live,
        SherpaRecognizer {
            engine,
            manager,
            vad,
        },
    )
}
fn run_speech<R: LiveRecognizer>(
    service: &AppService,
    id: &str,
    rx: &Receiver<Inference>,
    live: LiveTranscript,
    engine: R,
) -> Result<()> {
    if let Ok(mut runtime) = service.runtime.lock()
        && let Some(current) = runtime.current.as_mut().filter(|c| c.view.id == id)
    {
        current.view.transcription_configured = true;
        current.view.transcription_active = service.inference_allowed(backlog::Worker::Stt);
    }
    let mut worker = SpeechWorker {
        service,
        live,
        engine,
        recoveries: 0,
        last_cause: None,
        fed_end: [None; 2],
        last_final: [None, None],
        last_generation: None,
    };
    consume_inference(service, backlog::Worker::Stt, rx, |message| {
        worker.handle(message)
    })?;
    worker.finish()
}
fn speaker_worker(service: Arc<AppService>, id: String, rx: Receiver<Inference>) -> Result<()> {
    let outcome = (|| -> Result<()> {
        if !service.inference_allowed(backlog::Worker::Speakers) {
            return Ok(());
        }
        #[cfg(test)]
        let fake = service.test_workers.lock().unwrap().clone();
        #[cfg(test)]
        if let Some(fake) = fake {
            // Optional synthetic turns exercise the real live publisher without models.
            let mut publisher = fake
                .speaker_turns
                .as_ref()
                .map(|_| {
                    crate::speaker_pipeline::LivePublisher::new(
                        service.clone(),
                        &id,
                        fake.live_save,
                    )
                })
                .transpose()?;
            return consume_inference(&service, backlog::Worker::Speakers, &rx, |message| {
                (fake.accept)(backlog::Worker::Speakers, &message)?;
                if let (Some(publisher), Some(turns), Inference::Audio { generation, .. }) =
                    (publisher.as_mut(), fake.speaker_turns.as_ref(), &message)
                    && service.inference_current(*generation)
                {
                    publisher.publish(*generation, turns(&message)?)?;
                }
                Ok(())
            });
        }

        let Some(mut engine) = crate::speaker_pipeline::LiveSpeakers::new(service.clone(), &id)?
        else {
            return Ok(());
        };
        let mut last_generation = None;
        consume_inference(&service, backlog::Worker::Speakers, &rx, |message| {
            if service.shutting_down.load(Ordering::Relaxed) {
                bail!(
                    "Live speaker work was interrupted while closing. Refine saved audio to finish labels."
                );
            }
            match message {
                Inference::Audio {
                    track,
                    samples,
                    rate,
                    start_ms,
                    generation,
                } => {
                    last_generation = Some(generation);
                    engine.accept(track, &samples, rate, start_ms)?;
                }
                Inference::Reset(track, _) => engine.reset(track)?,
            }
            Ok(())
        })?;
        if service.inference_allowed(backlog::Worker::Speakers)
            && last_generation.is_some_and(|g| service.inference_current(g))
        {
            engine.finish()?;
        }
        Ok(())
    })();
    if let Err(error) = &outcome {
        let _ = store::speaker_warning(&service.root, &id, &store::describe_error(error));
    }
    if let Ok(mut runtime) = service.runtime.lock() {
        runtime.live_processing.diarization_active = false;
        if let Err(error) = &outcome {
            runtime.live_processing.diarization_warning = Some(store::describe_error(error));
        }
    }
    outcome
}

pub(crate) const HOUSEKEEPING_BUSY_WARNING: &str = "Recording details (duration, status, health, chunk clock or output route) could not be saved while the database was busy; recording continues and periodic details are saved again on the next update.";
/// Reported once (the backlog deduplicates identical warnings) when a chunk or
/// gap write waited more than 1 s for the database lock.
pub(crate) const RECORDER_LOCK_WAIT_WARNING: &str = "Saving recorded audio waited more than 1 s for a busy database; recording continued. A long wait can show up as a gap in the saved audio.";
/// Writes after capture has stopped (route seal, live-run completion, final
/// status and duration) retry lock errors instead of failing the meeting:
/// 4 x 5 s + 200 + 400 + 800 ms = 21.4 s worst case each. No audio is at risk.
const AFTER_STOP_BUDGET: store::LiveSaveBudget = store::LiveSaveBudget {
    busy_timeout: Duration::from_secs(5),
    attempts: 4,
    backoff: Duration::from_millis(200),
};

/// Route changes are rare; this many unsaved ones means the database has been
/// busy for a long time. Past it the history is treated as incomplete.
const MAX_PENDING_ROUTES: usize = 64;
/// While a route is waiting, the loop retries at most this often so a long lock
/// costs the capture loop one 250 ms attempt per interval, not per packet.
const ROUTE_RETRY_INTERVAL: Duration = Duration::from_millis(500);
pub(crate) const ROUTE_OVERFLOW_WARNING: &str = "Output route history could not be saved: too many route changes waited for a busy database. Raw recording continues; saved audio is transcribed without echo cancellation.";

/// Capture-time route changes not yet saved, oldest first. They are saved
/// strictly in order (a later one never ahead of an earlier one) with their
/// original timestamps; a lock only delays them. The history is incomplete
/// (never sealed) only after a non-lock error, overflow, or a lock that outlasts
/// the stop-time retry.
struct PendingRoutes {
    queue: std::collections::VecDeque<RouteObservation>,
    retry_at: Option<Instant>,
    complete: bool,
}

impl PendingRoutes {
    fn new() -> Self {
        Self {
            queue: Default::default(),
            retry_at: None,
            complete: true,
        }
    }

    fn push(&mut self, service: &AppService, observation: RouteObservation) {
        if !self.complete {
            return;
        }
        if self.queue.len() >= MAX_PENDING_ROUTES {
            self.abandon(service, ROUTE_OVERFLOW_WARNING.into());
            return;
        }
        self.queue.push_back(observation);
    }

    fn abandon(&mut self, service: &AppService, message: String) {
        self.complete = false;
        self.queue.clear();
        warning(service, message);
    }

    /// Recording-loop save: short timeout, a lock waits for the next retry.
    fn drain(&mut self, service: &AppService, id: &str, warned: &mut bool) -> Result<()> {
        if self.queue.is_empty() || self.retry_at.is_some_and(|at| Instant::now() < at) {
            return Ok(());
        }
        while let Some(front) = self.queue.front() {
            match crate::echo_store::record_route_with(
                &service.root,
                id,
                front,
                store::LOOP_BUSY_TIMEOUT,
            ) {
                Ok(()) => {
                    self.queue.pop_front();
                }
                Err(error) if store::sqlite_busy(&error) => {
                    self.retry_at = Some(Instant::now() + ROUTE_RETRY_INTERVAL);
                    return tolerate_busy(service, warned, Err(error));
                }
                Err(error) => {
                    self.abandon(
                        service,
                        format!(
                            "Output route history could not be saved; raw recording continues without echo cancellation: {error}"
                        ),
                    );
                    return Ok(());
                }
            }
        }
        self.retry_at = None;
        Ok(())
    }

    /// At stop: saves what is still pending, in order, then seals the history,
    /// retrying lock errors under `budget`. A failure leaves it unsealed.
    fn seal(&mut self, service: &AppService, id: &str, budget: store::LiveSaveBudget) {
        if !self.complete {
            return;
        }
        let queue = &mut self.queue;
        if let Err(error) = store::retry_busy(budget, |busy_timeout| {
            while let Some(front) = queue.front() {
                crate::echo_store::record_route_with(&service.root, id, front, busy_timeout)?;
                queue.pop_front();
            }
            crate::echo_store::seal_routes_with(&service.root, id, busy_timeout)
        }) {
            self.complete = false;
            self.queue.clear();
            warning(
                service,
                format!(
                    "Output route history could not be verified for saved echo processing: {error}"
                ),
            );
        }
    }
}

/// Publishes the chunk writer's lock waits and warns once past 1 s.
fn note_recorder_waits(service: &AppService, recorder: &Recording) {
    let waits = recorder.lock_waits();
    if waits.max > Duration::from_secs(1) {
        service.backlog_warn(RECORDER_LOCK_WAIT_WARNING);
    }
    if let Ok(mut runtime) = service.runtime.lock() {
        runtime.recorder_lock_waits = waits;
    }
}

/// Housekeeping writes on the capture consumer loop must not end a recording
/// because the database is momentarily locked: a lock error is skipped (the
/// next periodic write or the final write at stop supersedes it) and reported
/// once per recording. Any other error keeps today's fatal behaviour.
pub(super) fn tolerate_busy(
    service: &AppService,
    warned: &mut bool,
    result: Result<()>,
) -> Result<()> {
    match result {
        Err(error) if store::sqlite_busy(&error) => {
            if !*warned {
                *warned = true;
                service.backlog_warn(&format!(
                    "{HOUSEKEEPING_BUSY_WARNING} Cause: {}",
                    store::describe_error(&error)
                ));
            }
            Ok(())
        }
        other => other,
    }
}

fn publish_capture(service: &AppService, monitor: &crate::capture_health::CaptureMonitor) {
    if let Ok(mut runtime) = service.runtime.lock()
        && let Some(current) = &mut runtime.current
    {
        current.view.capture = monitor.view().clone();
    }
}

fn warning(service: &AppService, message: String) {
    if let Ok(mut runtime) = service.runtime.lock()
        && let Some(current) = &mut runtime.current
    {
        current.view.error = Some(message);
    }
}

pub(super) fn run(
    service: Arc<AppService>,
    id: String,
    mode: CaptureMode,
    input_device: Option<String>,
    mut recorder: Recording,
    controls: Receiver<Control>,
) {
    let result = record_session(
        &service,
        &id,
        mode,
        input_device.as_deref(),
        &mut recorder,
        controls,
    );
    let finished = if let Ok(mut runtime) = service.runtime.lock() {
        runtime.source_timeline.finish();
        let timing = runtime
            .source_timeline
            .snapshot(service.resources.uptime_ns());
        Some((timing, runtime.source_timeline.origin_ns))
    } else {
        None
    };
    // The timing is copied under `runtime` and written without it.
    let elapsed = if let Some((timing, origin)) = finished {
        let _ = health::timing(&service.root, &id, &timing, origin);
        timing.elapsed_ms
    } else {
        0
    };
    // The recorder retries its final status update on lock errors itself.
    let finish = recorder.finish();
    let mut error = result
        .err()
        .map(|e| e.to_string())
        .or_else(|| finish.err().map(|e| e.to_string()));
    if error.is_none()
        && let Err(problem) = finish_live_run(&service.root, &id)
    {
        error = Some(problem.to_string());
    }
    let _ = store::retry_busy(AFTER_STOP_BUDGET, |busy_timeout| {
        store::set_duration_with(&service.root, &id, elapsed, busy_timeout)
    });
    if let Some(error) = error {
        if let Ok(detail) = store::detail(&service.root, &id)
            && let Some(run) = detail.active_run_id
        {
            let _ = store::abandon_run(&service.root, &run);
        }
        if service.ensure_saved_audio(&id).is_ok() {
            let _ = Recording::recover(&service.root, &id);
        }
        // The live run was not sealed, so its catch-up search rebuild did not
        // run; do it here for edits deferred during the recording.
        let _ = store::retry_busy(AFTER_STOP_BUDGET, |busy_timeout| {
            store::rebuild_search_with(&service.root, &id, busy_timeout)
        });
        let _ = store::retry_busy(AFTER_STOP_BUDGET, |busy_timeout| {
            store::set_status_with(&service.root, &id, "failed", Some(&error), busy_timeout)
        });
        if let Ok(mut runtime) = service.runtime.lock() {
            if let Some(current) = &mut runtime.current {
                current.view.status = "failed".into();
                current.view.elapsed_ms = elapsed;
                current.view.error = Some(error);
                current.view.mic_level = 0.0;
                current.view.system_level = 0.0;
            }
            runtime.partials.clear();
        }
    } else {
        let warning = service.runtime.lock().ok().and_then(|runtime| {
            runtime
                .current
                .as_ref()
                .and_then(|current| current.view.error.clone())
        });
        let _ = store::retry_busy(AFTER_STOP_BUDGET, |busy_timeout| {
            store::set_status_with(
                &service.root,
                &id,
                "ready",
                warning.as_deref(),
                busy_timeout,
            )
        });
        if service.audit_warning_unpersisted() {
            // Keep the warning available even when its metadata write failed.
            // A ready Current neither holds capture open nor blocks a new run.
            if let Ok(mut runtime) = service.runtime.lock() {
                if let Some(current) = runtime.current.as_mut() {
                    current.view.status = "ready".into();
                    current.view.elapsed_ms = elapsed;
                    current.view.mic_level = 0.;
                    current.view.system_level = 0.;
                }
                runtime.partials.clear();
            }
            return;
        }
        if service.ensure_processing_space().is_ok() {
            processing::after_recording(service.clone(), &id);
        } else if let Ok(mut runtime) = service.runtime.lock() {
            runtime.current = None;
            runtime.partials.clear();
        }
    }
}

/// Seals the live run after stop. A lock error is retried under
/// `AFTER_STOP_BUDGET` before it can mark the meeting failed.
pub(super) fn finish_live_run(root: &Path, id: &str) -> Result<()> {
    store::retry_busy(AFTER_STOP_BUDGET, |busy_timeout| {
        store::complete_live_run_with(root, id, busy_timeout)
    })
}

fn record_session(
    service: &Arc<AppService>,
    id: &str,
    mode: CaptureMode,
    input_device: Option<&str>,
    recorder: &mut Recording,
    controls: Receiver<Control>,
) -> Result<()> {
    let mut process = CaptureProcess::spawn_with_device(
        &service.helper,
        mode.as_str(),
        std::process::id(),
        input_device,
    )?;
    let events = process.take_events()?;
    let (capture_tx, capture_rx) = mpsc::sync_channel(128);
    let reader = std::thread::Builder::new()
        .name("capture-reader".into())
        .spawn(move || {
            let mut formats = BTreeMap::new();
            for event in events {
                if !relay_capture_event(event, &mut formats, &capture_tx) {
                    break;
                }
            }
        })?;
    let installed = service
        .runtime
        .lock()
        .map(|runtime| {
            runtime.current.as_ref().is_some_and(|current| {
                current.view.id == id && current.view.transcription_configured
            }) && !service.inference_suspended.load(Ordering::Relaxed)
        })
        .unwrap_or(false);
    let speakers_enabled = !service.inference_suspended.load(Ordering::Relaxed)
        && service
            .audio_preferences
            .lock()
            .map(|prefs| prefs.diarization_enabled)
            .unwrap_or(false)
        && vt_diarize::ModelManager::new(service.root.join("models")).installed();
    #[cfg(test)]
    let (installed, speakers_enabled) = service
        .test_workers
        .lock()
        .unwrap()
        .as_ref()
        .map(|fake| (fake.stt, fake.speakers))
        .unwrap_or((installed, speakers_enabled));
    service
        .backlog
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .configure(installed, speakers_enabled);
    service.refresh_backlog();
    let live_run = if installed || speakers_enabled {
        Some(
            store::detail(&service.root, id)?
                .active_run_id
                .context("Missing live run")?,
        )
    } else {
        None
    };
    // The health report also carries capture diagnostics, so it is kept for
    // every recording, not only for those with live inference.
    let audit_run = live_run.clone().or_else(|| {
        store::detail(&service.root, id)
            .ok()
            .and_then(|detail| detail.active_run_id)
    });
    let audit = if let Some(run) = audit_run {
        match backlog::AuditWriter::start(service.clone(), id.into(), run) {
            Ok(writer) => Some(writer),
            Err(error) => {
                warning(
                    service,
                    format!(
                        "Inference status writer unavailable: {error}. Raw recording continues."
                    ),
                );
                None
            }
        }
    } else {
        None
    };
    // Live transcript lines are saved by this session-owned writer, joined
    // after the speech worker and before the live run is sealed.
    let transcript_writer = match live_run.clone().filter(|_| installed) {
        Some(run) => {
            #[cfg(not(test))]
            let budget = store::LiveSaveBudget::PRODUCTION;
            #[cfg(test)]
            let budget = service
                .test_workers
                .lock()
                .unwrap()
                .as_ref()
                .map(|fake| fake.live_save)
                .unwrap_or(store::LiveSaveBudget::PRODUCTION);
            match crate::live_transcript::TranscriptWriter::start(
                service.clone(),
                id.into(),
                run,
                budget,
            ) {
                Ok(writer) => Some(writer),
                Err(_) => {
                    let mut controller = service.backlog.lock().unwrap_or_else(|e| e.into_inner());
                    controller.stopped(backlog::Worker::Stt);
                    controller
                        .warn("A live inference worker could not start. Raw audio is preserved.");
                    None
                }
            }
        }
        None => None,
    };
    let (inference, inference_worker) = if let Some(writer) = &transcript_writer {
        let (tx, rx) = mpsc::sync_channel(600);
        let worker_service = service.clone();
        let worker_id = id.to_owned();
        let sink = writer.sink();
        (
            Some(tx),
            spawn_inference_worker(
                service.clone(),
                backlog::Worker::Stt,
                "speech-inference",
                move || infer_worker(worker_service, worker_id, rx, sink),
            ),
        )
    } else {
        (None, None)
    };
    let (speaker_inference, speaker_worker_handle) = if speakers_enabled {
        let (tx, rx) = mpsc::sync_channel(600);
        let worker_service = service.clone();
        let worker_id = id.to_owned();
        (
            Some(tx),
            spawn_inference_worker(
                service.clone(),
                backlog::Worker::Speakers,
                "speaker-inference",
                move || speaker_worker(worker_service, worker_id, rx),
            ),
        )
    } else {
        (None, None)
    };
    let (mut dsp_input, dsp_worker) = if inference.is_some() || speaker_inference.is_some() {
        let (tx, rx) = mpsc::sync_channel(600);
        let worker_service = service.clone();
        let worker_id = id.to_owned();
        (
            Some(tx),
            spawn_inference_worker(
                service.clone(),
                backlog::Worker::Dsp,
                "echo-inference",
                move || {
                    crate::echo_pipeline::worker(
                        worker_service,
                        worker_id,
                        rx,
                        inference,
                        speaker_inference,
                    )
                },
            ),
        )
    } else {
        (None, None)
    };
    let mut output_route: Option<RouteObservation> = None;
    let mut routes = PendingRoutes::new();
    let mut tracks = BTreeMap::from([
        (TrackKind::Mic, TrackBuffer::new(TrackKind::Mic)),
        (TrackKind::System, TrackBuffer::new(TrackKind::System)),
    ]);
    let mut paused = false;
    let mut stopping: Option<Instant> = None;
    let mut forced_stop = false;
    let mut resource_stop = false;
    let mut last_duration = Instant::now();
    let mut audio_received = false;
    let mut housekeeping_warned = false;
    let mut capture = crate::capture_health::CaptureMonitor::default();
    let outcome = (|| -> Result<()> {
        // Housekeeping writes on this loop give up the in-process writer
        // queue after a few ms (they are skipped and retried later).
        let _capture_loop = crate::db::CaptureLoopScope::enter();
        loop {
            {
                // Measured at the top of each iteration: a late loop is the
                // consumer's delay, never evidence against the helper.
                let now = service.resources.tick_ns();
                let source_ms = source_audio_clock_ms(&tracks);
                let active = !paused && stopping.is_none();
                let mut changed = false;
                service
                    .backlog
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .capture_health(|health| {
                        changed = capture.iteration(now, source_ms, health)
                            | capture.evaluate(now, active, health);
                        changed
                    });
                if changed {
                    publish_capture(service, &capture);
                }
            }
            tolerate_busy(
                service,
                &mut housekeeping_warned,
                service.refresh_health(false).map(drop),
            )?;
            routes.drain(service, id, &mut housekeeping_warned)?;
            service.refresh_backlog();
            if service.inference_suspended.load(Ordering::Relaxed) {
                dsp_input = None;
            }
            if service.resource_stop.load(Ordering::Relaxed) && stopping.is_none() {
                process.request_stop()?;
                stopping = Some(Instant::now());
                resource_stop = true;
                if let Ok(mut runtime) = service.runtime.lock()
                    && let Some(current) = runtime.current.as_mut()
                {
                    current.view.status = "processing".into();
                    current.view.error = Some(
                        "Recording stopped because available disk space fell below 100 MB.".into(),
                    );
                }
            }
            while let Ok(action) = controls.try_recv() {
                match action {
                    Control::Stop => {
                        if stopping.is_none() {
                            process.request_stop()?;
                            stopping = Some(Instant::now());
                        }
                    }
                    Control::Pause => {
                        for track in tracks.values_mut() {
                            track.flush(recorder, service, id, &mut housekeeping_warned)?;
                            track.resume_pending = true;
                        }
                        paused = true;
                        service
                            .backlog
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .reset("pause", true);
                        if let Some(tx) = &dsp_input {
                            let _ = tx.try_send(crate::echo_pipeline::Input::Reset {
                                // AEC frames use the helper's observed audio
                                // clock, which excludes app/helper startup delay.
                                start_ms: source_audio_clock_ms(&tracks),
                                generation: service
                                    .backlog
                                    .lock()
                                    .unwrap_or_else(|e| e.into_inner())
                                    .generation,
                            });
                        }
                        tolerate_busy(
                            service,
                            &mut housekeeping_warned,
                            store::set_status(&service.root, id, "paused", None),
                        )?;
                    }
                    Control::Resume => {
                        paused = false;
                        service
                            .backlog
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .resume();
                        tolerate_busy(
                            service,
                            &mut housekeeping_warned,
                            store::set_status(&service.root, id, "recording", None),
                        )?;
                    }
                }
            }
            if stopping.is_some_and(|started| started.elapsed() > Duration::from_secs(5))
                && !forced_stop
            {
                process.stop()?;
                forced_stop = true;
            }
            if last_duration.elapsed() > Duration::from_secs(2) {
                let elapsed = service
                    .runtime
                    .lock()
                    .ok()
                    .map(|runtime| {
                        runtime
                            .source_timeline
                            .snapshot(service.resources.uptime_ns())
                            .elapsed_ms
                    })
                    .unwrap_or(0);
                // A busy database skips this update; the next one (2 s later)
                // or the final duration write at stop supersedes it.
                last_duration = Instant::now();
                tolerate_busy(
                    service,
                    &mut housekeeping_warned,
                    store::set_duration_with(&service.root, id, elapsed, store::LOOP_BUSY_TIMEOUT),
                )?;
            }
            let event = match capture_rx.recv_timeout(Duration::from_millis(50)) {
                Ok(event) => event?,
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => {
                    if stopping.is_some() && (audio_received || resource_stop) {
                        return Ok(());
                    }
                    if !audio_received {
                        bail!(
                            "Audio capture ended before receiving audio. Check microphone and system audio permissions in System Settings."
                        );
                    }
                    bail!("The audio capture helper stopped unexpectedly.");
                }
            };
            match event {
                CaptureEvent::SourceClock {
                    clock,
                    origin_ns,
                    observed_ns,
                } => {
                    let (timing, origin) = {
                        let mut runtime = service
                            .runtime
                            .lock()
                            .map_err(|_| anyhow::anyhow!("session state unavailable"))?;
                        runtime.source_timeline.anchor(
                            clock,
                            origin_ns,
                            observed_ns,
                            service.resources.uptime_ns(),
                        );
                        (
                            runtime
                                .source_timeline
                                .snapshot(service.resources.uptime_ns()),
                            runtime.source_timeline.origin_ns,
                        )
                    };
                    // The final timing is written again after capture ends.
                    tolerate_busy(
                        service,
                        &mut housekeeping_warned,
                        health::timing_with(
                            &service.root,
                            id,
                            &timing,
                            origin,
                            store::LOOP_BUSY_TIMEOUT,
                        ),
                    )?;
                }
                CaptureEvent::OutputRoute {
                    epoch,
                    start_ns,
                    route,
                } => {
                    let mut observation = RouteObservation {
                        epoch,
                        start_ns,
                        route,
                    };
                    routes.push(service, observation.clone());
                    routes.drain(service, id, &mut housekeeping_warned)?;
                    if !routes.complete {
                        observation.route = Default::default();
                    }
                    if let Ok(mut runtime) = service.runtime.lock() {
                        runtime.live_processing.output_route = Some(observation.route.clone());
                    }
                    if let Some(tx) = &dsp_input {
                        let _ =
                            tx.try_send(crate::echo_pipeline::Input::Route(observation.clone()));
                    }
                    output_route = Some(observation);
                }
                CaptureEvent::Format {
                    track,
                    sample_rate,
                    channels,
                    epoch,
                    start_ns,
                } => {
                    let kind = TrackKind::from_str(&track)?;
                    let format = StreamFormat::new(sample_rate, channels)?;
                    let state = tracks.get(&kind).context("unknown track")?;
                    if state.format.is_some()
                        && (state.format != Some(format) || state.source_epoch != epoch)
                    {
                        service
                            .backlog
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .reset("source_format", false);
                    }
                    tracks.get_mut(&kind).context("unknown track")?.set_format(
                        format,
                        epoch,
                        start_ns,
                        recorder,
                        service,
                        id,
                        &mut housekeeping_warned,
                    )?;
                }
                CaptureEvent::Audio {
                    track,
                    epoch,
                    first_frame,
                    host_time_ns,
                    samples,
                } => {
                    audio_received = true;
                    let kind = TrackKind::from_str(&track)?;
                    let state = tracks.get_mut(&kind).context("unknown track")?;
                    if let Some(format) = state.format {
                        let end = host_time_ns
                            .checked_add(
                                samples.len() as u64 / u64::from(format.channels) * 1_000_000_000
                                    / u64::from(format.sample_rate),
                            )
                            .context("Capture source time overflow")?;
                        state.last_source_end = Some(end);
                        if let Ok(mut runtime) = service.runtime.lock() {
                            runtime.source_timeline.observe(end);
                        }
                    }
                    if paused {
                        continue;
                    }
                    let (mono, rate, start_ms) =
                        tracks.get_mut(&kind).context("unknown track")?.accept(
                            SourcePosition {
                                epoch,
                                frame: first_frame,
                                host_ns: host_time_ns,
                            },
                            samples,
                            recorder,
                            service,
                            id,
                            &mut housekeeping_warned,
                        )?;
                    let end_ms =
                        start_ms.saturating_add(mono.len() as u64 * 1000 / u64::from(rate));
                    let generation = service
                        .backlog
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .raw(kind, start_ms, end_ms, epoch, service.resources.tick_ns());
                    if service.inference_allowed(backlog::Worker::Dsp)
                        && let Some(tx) = &dsp_input
                    {
                        match tx.try_send(crate::echo_pipeline::Input::Audio {
                            track: kind,
                            samples: mono,
                            rate,
                            start_ms,
                            route: output_route.clone(),
                            generation,
                        }) {
                            Ok(()) => (),
                            Err(TrySendError::Full(_)) => {
                                service.inference_gap(
                                    backlog::Worker::Dsp,
                                    kind,
                                    generation,
                                    start_ms,
                                    end_ms,
                                    "queue_full",
                                );
                                warning(service,"Live audio processing fell behind. Raw audio recording continues; refine after stopping.".into());
                            }
                            Err(TrySendError::Disconnected(_)) => {
                                warning(service,"Live audio processing stopped. Raw audio recording continues; refine after stopping.".into());
                                service.inference_gap(
                                    backlog::Worker::Dsp,
                                    kind,
                                    generation,
                                    start_ms,
                                    end_ms,
                                    "worker_unavailable",
                                );
                                service
                                    .backlog
                                    .lock()
                                    .unwrap_or_else(|e| e.into_inner())
                                    .stopped(backlog::Worker::Dsp);
                                dsp_input = None;
                            }
                        }
                    }
                }
                CaptureEvent::Gap {
                    track,
                    end_ns,
                    reason,
                    ..
                } => {
                    let kind = TrackKind::from_str(&track)?;
                    if let Ok(mut runtime) = service.runtime.lock() {
                        runtime.source_timeline.observe(end_ns);
                    }
                    tracks.get_mut(&kind).context("unknown track")?.gap_until(
                        end_ns,
                        helper_gap_reason(&reason),
                        recorder,
                        service,
                        id,
                        &mut housekeeping_warned,
                    )?;
                    service
                        .backlog
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .reset("source_gap", false);
                    if let Some(tx) = &dsp_input {
                        let _ = tx.try_send(crate::echo_pipeline::Input::Reset {
                            generation: service
                                .backlog
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .generation,
                            start_ms: end_ns / 1_000_000,
                        });
                    }
                }
                CaptureEvent::Level { track, value } => {
                    if let Ok(mut runtime) = service.runtime.lock()
                        && let Some(current) = &mut runtime.current
                    {
                        let value = if value.is_finite() {
                            value.clamp(0.0, 1.0)
                        } else {
                            0.0
                        };
                        if track == "mic" {
                            current.view.mic_level = value;
                        } else {
                            current.view.system_level = value;
                        }
                    }
                }
                CaptureEvent::Status {
                    diagnostic: Some(diagnostic),
                    ..
                } => {
                    let now = service.resources.tick_ns();
                    let source_ms = source_audio_clock_ms(&tracks);
                    let mut changed = false;
                    service
                        .backlog
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .capture_health(|health| {
                            // Heartbeat counters ride along with the next
                            // persisted change instead of forcing a write.
                            changed = match &diagnostic {
                                vt_capture::CaptureDiagnostic::Heartbeat { .. } => {
                                    capture.heartbeat(now, source_ms, &diagnostic, health)
                                }
                                other => health.record(source_ms, other),
                            };
                            changed
                        });
                    if changed {
                        publish_capture(service, &capture);
                    }
                }
                CaptureEvent::Status { .. } => (),
                CaptureEvent::Error { message, fatal } => {
                    if fatal {
                        if resource_stop {
                            return Ok(());
                        }
                        bail!(message);
                    }
                    warning(service, message);
                }
            }
        }
    })();
    // Worker exits from here on are shutdown, not a live failure to report.
    service
        .backlog
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .closing();
    capture.finish();
    publish_capture(service, &capture);
    let _ = process.stop();
    drop(capture_rx);
    let _ = reader.join();
    let mut flush_error = None;
    for track in tracks.values_mut() {
        if track.resume_pending
            && let Some(end) = track.last_source_end
            && let Err(error) = track.gap_until(
                end,
                GapReason::Pause,
                recorder,
                service,
                id,
                &mut housekeeping_warned,
            )
        {
            flush_error = Some(error);
        }
        if let Err(error) = track.flush(recorder, service, id, &mut housekeeping_warned) {
            flush_error = Some(error);
        }
    }
    if outcome.is_ok() {
        routes.seal(service, id, AFTER_STOP_BUDGET);
    }
    drop(dsp_input);
    if let Some(worker) = dsp_worker {
        match worker.join() {
            Ok(Ok(())) => (),
            Ok(Err(error)) => warning(
                service,
                format!(
                    "Live echo processing stopped: {error}. Raw audio is available for refinement."
                ),
            ),
            Err(_) => warning(
                service,
                "Live echo processing stopped unexpectedly. Raw audio is available for refinement."
                    .into(),
            ),
        }
    }
    if let Some(worker) = speaker_worker_handle {
        let error=match worker.join(){Ok(Ok(()))=>None,Ok(Err(error))=>Some(error.to_string()),Err(_)=>Some("The live speaker worker stopped unexpectedly. Refine saved audio to regenerate labels.".into())};
        let persisted_warning = if let Ok(mut runtime) = service.runtime.lock() {
            runtime.live_processing.diarization_active = false;
            if let Some(error) = error {
                runtime.live_processing.diarization_warning = Some(error);
            }
            runtime.live_processing.diarization_warning.clone()
        } else {
            None
        };
        if let Some(warning) = persisted_warning {
            let _ = store::speaker_warning(&service.root, id, &warning);
        }
    }
    if let Some(worker) = inference_worker {
        match worker.join() {
            Ok(Ok(())) => (),
            Ok(Err(error)) => warning(
                service,
                format!("Transcription stopped: {error}. The recorded audio is available."),
            ),
            Err(_) => warning(
                service,
                "The speech engine stopped unexpectedly. The recorded audio is available.".into(),
            ),
        }
    }
    // Drain every queued final into the still-running live run before the
    // inference audit is finalized and before `run` seals the live run.
    if let Some(writer) = transcript_writer {
        writer.finish();
    }
    if let Some(audit) = audit {
        audit.finish();
    }
    service.refresh_backlog();
    outcome?;
    if let Some(error) = flush_error {
        return Err(error);
    }
    Ok(())
}

pub(super) fn transcribe_saved(service: Arc<AppService>, id: &str) -> Result<()> {
    service.ensure_processing_space()?;
    service.ensure_saved_audio(id)?;
    let detail = store::detail(&service.root, id)?;
    let mut runtime = service
        .runtime
        .lock()
        .map_err(|_| anyhow::anyhow!("session state unavailable"))?;
    if runtime.processing
        || runtime.current.as_ref().is_some_and(|c| {
            matches!(
                c.view.status.as_str(),
                "recording" | "paused" | "processing"
            )
        })
    {
        bail!("Stop the active recording before transcribing saved audio.");
    }
    if !runtime.model.installed {
        bail!("Install the English speech model first.");
    }
    if detail.meeting.segment_count > 0
        && !matches!(detail.meeting.status.as_str(), "failed" | "recovered")
        && detail.meeting.error.is_none()
    {
        bail!(
            "This meeting already has a complete transcript. Use Refine to create a new version."
        );
    }
    store::set_status(&service.root, id, "processing", None)?;
    runtime.processing = true;
    runtime.processing_view = Some(ProcessingView {
        meeting_id: id.into(),
        kind: "replay".into(),
        stage: "Preparing saved audio".into(),
        progress: 0.0,
        error: None,
    });
    if runtime.current.as_ref().is_some_and(|c| c.view.id == id) {
        runtime.current = None;
    }
    service.cancel_processing.store(false, Ordering::Relaxed);
    drop(runtime);
    let id = id.to_owned();
    let cleanup = service.clone();
    let cleanup_id = id.clone();
    let spawn = std::thread::Builder::new()
        .name("saved-transcription".into())
        .spawn(move || {
            let job = uuid::Uuid::new_v4().to_string();
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                || -> Result<()> {
                    service.ensure_saved_audio(&id)?;
                    let _report = Recording::recover(&service.root, &id)?;
                    let manager = service.live_model_manager()?;
                    let engines = live_models::provenance(&service.root, true)?;
                    let mut engine =
                        live_models::recognizer(&manager, service.vad_path().as_deref())?;
                    let echo_enabled = service
                        .audio_preferences
                        .lock()
                        .map(|p| p.echo_cancellation_enabled)
                        .unwrap_or(false);
                    let echo = crate::echo_pipeline::saved(
                        &service,
                        &id,
                        echo_enabled,
                        |packet, fraction| {
                            processing::progress(
                                &service,
                                &id,
                                "Transcribing saved audio",
                                fraction * 0.95,
                            );
                            let events = match packet {
                                processing::AudioPacket::Gap { track, .. } => {
                                    engine.reset_track(track)?
                                }
                                processing::AudioPacket::Audio {
                                    track,
                                    samples,
                                    rate,
                                    start_ms,
                                } => engine.accept(track, &samples, rate, start_ms)?,
                            };
                            for event in events {
                                stage_final(&service, &job, &id, event)?;
                            }
                            Ok(())
                        },
                    )?;
                    for event in engine.finish()? {
                        stage_final(&service, &job, &id, event)?;
                    }
                    if service.cancel_processing.load(Ordering::Relaxed) {
                        bail!(
                            "Transcription was interrupted. The previous transcript was preserved."
                        );
                    }
                    store::publish_staged_with_provenance(
                        &service.root,
                        &job,
                        &id,
                        Some(&echo),
                        &engines,
                    )?;
                    Ok(())
                },
            ))
            .unwrap_or_else(|_| {
                Err(anyhow::anyhow!(
                    "Saved transcription stopped unexpectedly. Previous transcripts were preserved."
                ))
            });
            if let Err(error) = result {
                let _ = store::discard_staged(&service.root, &job);
                processing::finish(&service, &id, Err(error));
            } else {
                processing::after_replay(service, &id);
            }
        });
    if let Err(error) = spawn {
        if let Ok(mut runtime) = cleanup.runtime.lock() {
            runtime.processing = false;
            runtime.processing_view = None;
        }
        store::set_status(
            &cleanup.root,
            &cleanup_id,
            "failed",
            Some(&error.to_string()),
        )?;
        return Err(error.into());
    }
    Ok(())
}

fn source_audio_clock_ms(tracks: &BTreeMap<TrackKind, TrackBuffer>) -> u64 {
    tracks
        .values()
        .filter_map(|track| track.last_source_end)
        .max()
        .unwrap_or(0)
        / 1_000_000
}

fn stage_final(
    service: &AppService,
    job: &str,
    meeting: &str,
    event: TranscriptEvent,
) -> Result<()> {
    if event.is_final && !event.text.trim().is_empty() {
        let in_person =
            store::meeting_mode(&service.root, meeting)? == vt_diarize::MeetingMode::InPerson;
        store::stage_segment(
            &service.root,
            job,
            meeting,
            &Segment {
                id: uuid::Uuid::new_v4().to_string(),
                track: event.track.as_str().into(),
                start_ms: event.start_ms,
                end_ms: event.end_ms.max(event.start_ms),
                text: event.text,
                speaker: if in_person {
                    "Unknown"
                } else if event.track == TrackKind::Mic {
                    "Me"
                } else {
                    "Them"
                }
                .into(),
                words: event
                    .words
                    .into_iter()
                    .map(|word| TranscriptWord {
                        text: word.text,
                        start_ms: word.start_ms,
                        end_ms: word.end_ms,
                    })
                    .collect(),
            },
        )?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "live_speaker_persistence_tests.rs"]
mod live_speaker_persistence_tests;

#[cfg(test)]
#[path = "live_recovery_tests.rs"]
mod live_recovery_tests;

#[cfg(test)]
mod regression_tests {
    use super::*;

    #[test]
    fn large_native_capture_packets_have_time_bounded_queues_without_changing_pcm_or_hardware_origins()
     {
        for (rate, channels) in [(48000, 2), (11025, 8)] {
            let (tx, rx) = mpsc::sync_channel(128);
            let mut formats = BTreeMap::new();
            assert!(relay_capture_event(
                Ok(CaptureEvent::Format {
                    track: "mic".into(),
                    sample_rate: rate,
                    channels,
                    epoch: 7,
                    start_ns: 1234000000
                }),
                &mut formats,
                &tx
            ));
            let original: Vec<f32> = (0..rate as usize * 2 * usize::from(channels))
                .map(|n| (n % 31) as f32 / 64.0)
                .collect();
            assert!(relay_capture_event(
                Ok(CaptureEvent::Audio {
                    track: "mic".into(),
                    epoch: 7,
                    first_frame: 9000,
                    host_time_ns: 1234000000,
                    samples: original.clone()
                }),
                &mut formats,
                &tx
            ));
            drop(tx);
            let mut restored = Vec::new();
            let mut offset = 0u64;
            for event in rx {
                if let CaptureEvent::Audio {
                    epoch,
                    first_frame,
                    host_time_ns,
                    samples,
                    ..
                } = event.unwrap()
                {
                    assert_eq!(epoch, 7);
                    assert_eq!(first_frame, 9000 + offset);
                    assert_eq!(
                        host_time_ns,
                        1234000000 + offset * 1_000_000_000 / u64::from(rate)
                    );
                    let frames = samples.len() / usize::from(channels);
                    assert!(
                        frames * 1000 <= rate as usize * 20,
                        "queued audio exceeded20ms"
                    );
                    offset += frames as u64;
                    restored.extend(samples);
                }
            }
            assert_eq!(restored, original);
            assert_eq!(offset, u64::from(rate) * 2);
        }
    }

    #[test]
    fn pause_uses_source_clock_so_startup_delay_cannot_disable_resumed_echo_processing() {
        use vt_dsp::{DspConfig, EchoProcessor, RouteEligibility};

        let mut tracks = BTreeMap::from([
            (TrackKind::Mic, TrackBuffer::new(TrackKind::Mic)),
            (TrackKind::System, TrackBuffer::new(TrackKind::System)),
        ]);
        assert_eq!(source_audio_clock_ms(&tracks), 0);
        tracks.get_mut(&TrackKind::Mic).unwrap().last_source_end = Some(110_000_000);
        tracks.get_mut(&TrackKind::System).unwrap().last_source_end = Some(108_000_000);
        let source_pause_ms = source_audio_clock_ms(&tracks);
        let app_wall_pause_ms = source_pause_ms + 5_000;
        let reference: Vec<f32> = (0..160).map(|n| (n as f32 * 0.071).sin() * 0.2).collect();
        let microphone: Vec<f32> = reference.iter().map(|s| s * 0.5).collect();
        let ready = || {
            let mut processor = EchoProcessor::new(DspConfig::default()).unwrap();
            processor
                .configure_at_time(
                    RouteEligibility {
                        eligible: true,
                        epoch: 1,
                        reason: None,
                    },
                    100,
                )
                .unwrap();
            processor
                .accept(TrackKind::System, &reference, 16_000, 100)
                .unwrap();
            let initial = processor
                .accept(TrackKind::Mic, &microphone, 16_000, 100)
                .unwrap();
            assert!(
                initial
                    .iter()
                    .any(|packet| packet.track == TrackKind::Mic && packet.aec_applied)
            );
            processor
        };
        let resumed = |processor: &mut EchoProcessor| {
            let mut packets = processor
                .accept(TrackKind::System, &reference, 16_000, 110)
                .unwrap();
            packets.extend(
                processor
                    .accept(TrackKind::Mic, &microphone, 16_000, 110)
                    .unwrap(),
            );
            packets.extend(processor.flush().unwrap());
            packets
                .into_iter()
                .filter(|packet| packet.track == TrackKind::Mic)
                .collect::<Vec<_>>()
        };

        let mut source_clock = ready();
        assert!(
            source_clock
                .reset_at_time("pause", source_pause_ms)
                .unwrap()
                .is_empty()
        );
        let actual = resumed(&mut source_clock);
        assert_eq!(actual.len(), 1);
        assert_eq!(actual[0].start_ms, 110);
        assert_eq!(actual[0].samples.len(), 160);
        assert!(
            actual[0].aec_applied,
            "resumed AEC must not wait for the startup offset"
        );
        assert!(actual[0].reason.is_none());

        // The former wall-clock reset demonstrates the observable failure: the
        // same resumed frame predates the artificial future route epoch.
        let mut wall_clock = ready();
        wall_clock
            .reset_at_time("pause", app_wall_pause_ms)
            .unwrap();
        let old = resumed(&mut wall_clock);
        assert!(!old[0].aec_applied);
        assert_eq!(old[0].samples, microphone);
        assert!(old[0].reason.as_deref().unwrap().contains("precedes"));
    }

    #[test]
    fn inference_packet_times_follow_source_audio_before_and_across_the_first_chunk_flush() {
        let temp = tempfile::TempDir::new().unwrap();
        let service =
            AppService::new(temp.path().to_path_buf(), temp.path().join("unused-helper")).unwrap();
        let id = "inference-clock";
        let mut recording = Recording::create(temp.path(), id).unwrap();
        store::create_meeting(
            temp.path(),
            id,
            "Synthetic inference clock fixture",
            "remote",
        )
        .unwrap();
        let mut track = TrackBuffer::new(TrackKind::Mic);
        track
            .set_format(
                StreamFormat::new(16_000, 1).unwrap(),
                0,
                0,
                &mut recording,
                &service,
                id,
                &mut false,
            )
            .unwrap();
        for packet in 0..105 {
            let samples = vec![0.25; 320];
            let (mono, rate, start_ms) = track
                .accept(
                    SourcePosition {
                        epoch: 0,
                        frame: packet * 320,
                        host_ns: packet * 20_000_000,
                    },
                    samples.clone(),
                    &mut recording,
                    &service,
                    id,
                    &mut false,
                )
                .unwrap();
            assert_eq!(mono, samples);
            assert_eq!(rate, 16_000);
            assert_eq!(
                start_ms,
                packet * 20,
                "inference timestamp for packet {packet}"
            );
        }
        track
            .flush(&mut recording, &service, id, &mut false)
            .unwrap();
        recording.finish().unwrap();
        let chunks = Recording::recover(temp.path(), id).unwrap();
        assert_eq!(chunks.verified_chunks, 2);
    }

    #[test]
    fn replay_preserves_unknown_in_person_fallback_and_remote_channel_labels() {
        let temp = tempfile::TempDir::new().unwrap();
        let service =
            AppService::new(temp.path().to_path_buf(), temp.path().join("unused-helper")).unwrap();
        for (meeting, mode, expected) in [
            ("in-person", "in_person", "Unknown"),
            ("remote", "remote", "Me"),
        ] {
            Recording::create(temp.path(), meeting)
                .unwrap()
                .finish()
                .unwrap();
            store::create_meeting(temp.path(), meeting, "Replay label fixture", mode).unwrap();
            stage_final(
                &service,
                meeting,
                meeting,
                TranscriptEvent {
                    track: TrackKind::Mic,
                    start_ms: 0,
                    end_ms: 1_000,
                    text: "A public fixture sentence.".into(),
                    is_final: true,
                    words: Vec::new(),
                },
            )
            .unwrap();
            assert!(service.detail(meeting).unwrap().segments.is_empty());
            store::publish_staged(temp.path(), meeting, meeting).unwrap();
            let detail = service.detail(meeting).unwrap();
            assert_eq!(detail.segments[0].speaker, expected);
            assert_eq!(detail.segments[0].text, "A public fixture sentence.");
        }
    }

    /// A lock outlasting the stop-time retry leaves the route history
    /// unsealed (incomplete) with a warning instead of failing the stop.
    #[test]
    fn pending_routes_locked_past_the_stop_budget_stay_unsealed() {
        let temp = tempfile::TempDir::new().unwrap();
        let service =
            AppService::new(temp.path().to_path_buf(), temp.path().join("unused-helper")).unwrap();
        let id = "route-budget";
        Recording::create(temp.path(), id)
            .unwrap()
            .finish()
            .unwrap();
        store::create_meeting(temp.path(), id, "Route budget fixture", "remote").unwrap();
        let mut routes = PendingRoutes::new();
        for start_ns in [1_000, 2_000] {
            routes.push(
                &service,
                RouteObservation {
                    epoch: 0,
                    start_ns,
                    route: Default::default(),
                },
            );
        }
        let blocker = store::open(temp.path()).unwrap();
        blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
        routes.seal(
            &service,
            id,
            store::LiveSaveBudget {
                busy_timeout: Duration::from_millis(20),
                attempts: 2,
                backoff: Duration::from_millis(5),
            },
        );
        blocker.execute_batch("ROLLBACK").unwrap();
        assert!(!routes.complete);
        let db = store::open(temp.path()).unwrap();
        assert!(!crate::echo_store::routes_complete(&db, id).unwrap());
        let saved: i64 = db
            .query_row(
                "SELECT COUNT(*) FROM app_output_routes WHERE meeting_id=?1",
                [id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(saved, 0);
        // Once incomplete, later changes are not queued and nothing is sealed.
        routes.push(
            &service,
            RouteObservation {
                epoch: 0,
                start_ns: 3_000,
                route: Default::default(),
            },
        );
        assert!(routes.queue.is_empty());
        service.wait_for_shutdown();
    }

    #[test]
    fn consecutive_unwritten_source_epochs_do_not_skip_manifest_epochs() {
        let temp = tempfile::TempDir::new().unwrap();
        let service =
            AppService::new(temp.path().to_path_buf(), temp.path().join("unused-helper")).unwrap();
        let mut recording = Recording::create(temp.path(), "epoch-regression").unwrap();
        store::create_meeting(temp.path(), "epoch-regression", "Epoch fixture", "remote").unwrap();
        let mut track = TrackBuffer::new(TrackKind::Mic);
        track
            .set_format(
                StreamFormat::new(8_000, 1).unwrap(),
                0,
                0,
                &mut recording,
                &service,
                "epoch-regression",
                &mut false,
            )
            .unwrap();
        track
            .accept(
                SourcePosition {
                    epoch: 0,
                    frame: 0,
                    host_ns: 0,
                },
                vec![0.0; 80],
                &mut recording,
                &service,
                "epoch-regression",
                &mut false,
            )
            .unwrap();
        track
            .flush(&mut recording, &service, "epoch-regression", &mut false)
            .unwrap();
        for (source_epoch, rate) in [(1, 16_000), (2, 48_000)] {
            track
                .set_format(
                    StreamFormat::new(rate, 1).unwrap(),
                    source_epoch,
                    10_000_000,
                    &mut recording,
                    &service,
                    "epoch-regression",
                    &mut false,
                )
                .unwrap();
        }
        track
            .accept(
                SourcePosition {
                    epoch: 2,
                    frame: 0,
                    host_ns: 10_000_000,
                },
                vec![0.0; 80],
                &mut recording,
                &service,
                "epoch-regression",
                &mut false,
            )
            .unwrap();
        track
            .flush(&mut recording, &service, "epoch-regression", &mut false)
            .unwrap();
        assert_eq!(
            recording
                .chunks()
                .unwrap()
                .iter()
                .map(|chunk| chunk.epoch)
                .collect::<Vec<_>>(),
            vec![0, 1]
        );
    }
}
