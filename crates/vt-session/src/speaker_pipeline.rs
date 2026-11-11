use super::*;
use std::collections::BTreeMap;
use vt_diarize::{
    Anchor, ConstraintKind, DiarizationResult, Diarizer, DiarizerConfig, Phase, SpeakerTurn,
    TemporalConstraint,
};

fn constraints(service: &AppService, id: &str) -> Result<Vec<TemporalConstraint>> {
    store::speaker_constraints(&service.root, id)?
        .into_iter()
        .map(|constraint| {
            Ok(TemporalConstraint {
                left: Anchor {
                    track: constraint.left.track,
                    start_ms: constraint.left.start_ms,
                    end_ms: constraint.left.end_ms,
                },
                right: Anchor {
                    track: constraint.right.track,
                    start_ms: constraint.right.start_ms,
                    end_ms: constraint.right.end_ms,
                },
                kind: if constraint.kind == "must_link" {
                    ConstraintKind::MustLink
                } else {
                    ConstraintKind::CannotLink
                },
                provenance: format!("user:{}", constraint.id),
            })
        })
        .collect()
}

fn configured(service: &AppService) -> bool {
    service
        .audio_preferences
        .lock()
        .map(|prefs| prefs.diarization_enabled)
        .unwrap_or(false)
        && vt_diarize::ModelManager::new(service.root.join("models")).installed()
}

/// Gap reason when live speaker turns were produced but could never be saved
/// (database lock contention outlasted every retry, then the pending update
/// overflowed or the recording ended). Refine saved audio to recover them.
pub(crate) const PERSISTENCE_FAILED_GAP: &str = "persistence_failed";
/// Live turns are incremental: each window emits new turns with fresh IDs and
/// never re-emits earlier ones, so unsaved turns can be carried into the next
/// save. This bounds memory if the database stays locked for a long time.
pub(crate) const MAX_PENDING_LIVE_TURNS: usize = 2048;

pub(crate) fn persistence_warning(cause: &str) -> String {
    format!(
        "Live speaker labels could not be saved yet; they are kept and retried with the next update. Raw audio is preserved. Cause: {cause}"
    )
}

/// Saves live speaker updates. Each save waits at most about one second for the
/// write lock (`LiveSaveBudget::PRODUCTION`, 1.05 s worst case) and then holds
/// it only for point lookups and upserts of the pending turns. Lock contention
/// never stops the worker: unsaved turns are retained (bounded) and written by
/// the next successful save; any that must be dropped are recorded as
/// `persistence_failed` inference gaps.
pub(crate) struct LivePublisher {
    service: Arc<AppService>,
    id: String,
    run: String,
    budget: store::LiveSaveBudget,
    pending: Vec<(u64, SpeakerTurn)>,
}
impl LivePublisher {
    pub(crate) fn new(
        service: Arc<AppService>,
        id: &str,
        budget: store::LiveSaveBudget,
    ) -> Result<Self> {
        let run = store::detail(&service.root, id)?
            .active_run_id
            .ok_or_else(|| anyhow::anyhow!("The recording transcript run is unavailable."))?;
        Ok(Self {
            service,
            id: id.into(),
            run,
            budget,
            pending: Vec::new(),
        })
    }
    pub(crate) fn publish(&mut self, generation: u64, turns: Vec<SpeakerTurn>) -> Result<()> {
        for turn in turns {
            match self.pending.iter_mut().find(|(_, old)| old.id == turn.id) {
                Some(slot) => *slot = (generation, turn),
                None => self.pending.push((generation, turn)),
            }
        }
        if !self.service.inference_allowed(backlog::Worker::Speakers) {
            // Suspended inference never publishes; do not let the carry-forward
            // grow while it is suspended.
            self.shed_excess();
            return Ok(());
        }
        if self.pending.is_empty() {
            return Ok(());
        }
        let result = DiarizationResult {
            turns: self.pending.iter().map(|(_, turn)| turn.clone()).collect(),
            speakers: Vec::new(),
            merges: Vec::new(),
            constraint_reviews: Vec::new(),
            segmentation_context: Some(Phase::Live.segmentation_context()),
            embedding_version: vt_diarize::EMBEDDING_VERSION.into(),
            calibrated: false,
            warnings: vec!["Speaker labels are experimental and not verified identities.".into()],
        };
        match store::save_live_diarization_with(
            &self.service.root,
            &self.id,
            &self.run,
            &result,
            self.budget,
        ) {
            Ok(()) => {
                self.pending.clear();
                Ok(())
            }
            Err(error) if store::sqlite_busy(&error) => {
                let warning = persistence_warning(&store::describe_error(&error));
                self.service
                    .backlog
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .warn(&warning);
                self.shed_excess();
                Ok(())
            }
            Err(error) => Err(error),
        }
    }
    /// Keeps the carry-forward bounded; the oldest unsaved turns become gaps.
    fn shed_excess(&mut self) {
        if self.pending.len() > MAX_PENDING_LIVE_TURNS {
            let excess = self.pending.len() - MAX_PENDING_LIVE_TURNS;
            let dropped = self.pending.drain(..excess).collect::<Vec<_>>();
            self.record_gaps(&dropped);
        }
    }
    /// The final reconciled result supersedes every live update.
    fn superseded(&mut self) {
        self.pending.clear();
    }
    fn record_gaps(&self, dropped: &[(u64, SpeakerTurn)]) {
        let mut spans = BTreeMap::<(u64, &str), (u64, u64)>::new();
        for (generation, turn) in dropped {
            let span = spans
                .entry((*generation, turn.track.as_str()))
                .or_insert((turn.start_ms, turn.end_ms));
            span.0 = span.0.min(turn.start_ms);
            span.1 = span.1.max(turn.end_ms);
        }
        let mut controller = self
            .service
            .backlog
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        for ((generation, track), (start, end)) in spans {
            let track = if track == "mic" {
                TrackKind::Mic
            } else {
                TrackKind::System
            };
            controller.gap(
                backlog::Worker::Speakers,
                track,
                generation,
                start,
                end,
                PERSISTENCE_FAILED_GAP,
            );
        }
    }
}
impl Drop for LivePublisher {
    fn drop(&mut self) {
        if !self.pending.is_empty() {
            let dropped = std::mem::take(&mut self.pending);
            self.record_gaps(&dropped);
        }
    }
}

pub(super) struct LiveSpeakers {
    service: Arc<AppService>,
    id: String,
    run: String,
    engine: Diarizer,
    publisher: LivePublisher,
}
impl LiveSpeakers {
    pub(super) fn new(service: Arc<AppService>, id: &str) -> Result<Option<Self>> {
        if !service.inference_allowed(backlog::Worker::Speakers) || !configured(&service) {
            return Ok(None);
        }
        let publisher = LivePublisher::new(service.clone(), id, store::LiveSaveBudget::PRODUCTION)?;
        let run = publisher.run.clone();
        let engine = Diarizer::new(
            &vt_diarize::ModelManager::new(service.root.join("models")).model_dir(),
            DiarizerConfig {
                mode: store::meeting_mode(&service.root, id)?,
                phase: Phase::Live,
                ..Default::default()
            },
        )?;
        if let Ok(mut runtime) = service.runtime.lock() {
            runtime.live_processing.diarization_active = true;
            runtime.live_processing.diarization_warning =
                Some("Speaker labels are provisional and refined after the meeting.".into());
        }
        Ok(Some(Self {
            service,
            id: id.into(),
            run,
            engine,
            publisher,
        }))
    }
    /// Engine errors are fatal (the Diarizer is poisoned); persistence lock
    /// contention is absorbed by the publisher and never reaches the worker.
    pub(super) fn accept(
        &mut self,
        track: TrackKind,
        samples: &[f32],
        rate: u32,
        start: u64,
    ) -> Result<()> {
        let generation = self
            .service
            .backlog
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .generation;
        let turns = self.engine.accept(track, samples, rate, start)?;
        if self.service.inference_current(generation) {
            self.publisher.publish(generation, turns)
        } else {
            Ok(())
        }
    }
    pub(super) fn reset(&mut self, track: TrackKind) -> Result<()> {
        let _ = self.engine.reset_track(track)?;
        Ok(())
    }
    pub(super) fn finish(&mut self) -> Result<()> {
        if !self.service.inference_allowed(backlog::Worker::Speakers) {
            return Ok(());
        }
        self.engine
            .set_temporal_constraints(constraints(&self.service, &self.id)?)?;
        let result = self.engine.finish()?;
        if self.service.inference_allowed(backlog::Worker::Speakers) {
            store::save_diarization_result(&self.service.root, &self.id, &self.run, &result, true)?;
            self.publisher.superseded();
        }
        Ok(())
    }
}

pub(super) fn refine(
    service: &AppService,
    id: &str,
    run: &str,
    echo_enabled: bool,
) -> Result<Option<Vec<SpeakerTurn>>> {
    if !configured(service) {
        return store::retain_speaker_annotations(&service.root, id, run);
    }
    let previous = store::detail(&service.root, id)?;
    let prior = previous
        .speakers
        .iter()
        .filter(|speaker| {
            !speaker.provisional
                && speaker.track
                    == if store::meeting_mode(&service.root, id).ok()
                        == Some(vt_diarize::MeetingMode::InPerson)
                    {
                        "mic"
                    } else {
                        "system"
                    }
        })
        .count();
    let mut engine = Diarizer::new(
        &vt_diarize::ModelManager::new(service.root.join("models")).model_dir(),
        DiarizerConfig {
            mode: store::meeting_mode(&service.root, id)?,
            phase: Phase::Refined,
            temporal_constraints: constraints(service, id)?,
            live_speaker_prior: (prior > 0).then_some(prior),
            ..Default::default()
        },
    )?;
    // Capacity raised by `Diarizer::new` (too many saved constraints) stays fatal.
    let separated = separate(service, id, echo_enabled, &mut engine);
    settle(service, id, run, separated)
}

fn separate(
    service: &AppService,
    id: &str,
    echo_enabled: bool,
    engine: &mut Diarizer,
) -> Result<DiarizationResult> {
    crate::echo_pipeline::saved(service, id, echo_enabled, |packet, fraction| {
        processing::progress(
            service,
            id,
            "Separating speaker turns",
            0.06 + fraction * 0.34,
        );
        match packet {
            processing::AudioPacket::Gap { track, .. } => {
                engine.reset_track(track)?;
            }
            processing::AudioPacket::Audio {
                track,
                samples,
                rate,
                start_ms,
            } => {
                engine.accept(track, &samples, rate, start_ms)?;
            }
        }
        Ok(())
    })?;
    processing::progress(
        service,
        id,
        "Reconciling speaker labels and corrections",
        0.42,
    );
    Ok(engine.finish()?)
}

/// Persists a completed separation, or applies the refinement policy for a
/// failed one: a capacity limit reached while processing audio or finishing
/// (turns, embeddings, clusters) must not cost the refined transcript, so the
/// previous run's speaker annotations are carried forward exactly as when the
/// speaker model is unavailable and a run warning names the limit. Nothing
/// from the failed attempt is saved. Every other error is returned unchanged.
pub(super) fn settle(
    service: &AppService,
    id: &str,
    run: &str,
    separated: Result<DiarizationResult>,
) -> Result<Option<Vec<SpeakerTurn>>> {
    let error = match separated {
        Ok(result) => {
            return Ok(Some(store::save_diarization_result(
                &service.root,
                id,
                run,
                &result,
                true,
            )?));
        }
        Err(error) => error,
    };
    let Some(limit) = recoverable_capacity(&error) else {
        return Err(error);
    };
    let in_person = store::meeting_mode(&service.root, id)? == vt_diarize::MeetingMode::InPerson;
    store::retain_speakers_after_failed_separation(&service.root, id, run, |retained| {
        capacity_warning(limit, in_person, retained)
    })
}

fn recoverable_capacity(error: &anyhow::Error) -> Option<vt_diarize::CapacityLimit> {
    error
        .downcast_ref::<vt_diarize::DiarizeError>()?
        .capacity_limit()
        .filter(|limit| *limit != vt_diarize::CapacityLimit::Constraints)
}

pub(super) fn capacity_warning(
    limit: vt_diarize::CapacityLimit,
    in_person: bool,
    retained: bool,
) -> String {
    let labels = if retained {
        "Speaker labels, names and corrections from the previous transcript were kept and applied by time."
    } else if in_person {
        "Speakers are shown as Unknown."
    } else {
        "The transcript is labelled by channel (Me / Them)."
    };
    format!(
        "Speaker separation could not finish for this recording (limit: {limit}). The refined transcript is complete. {labels}"
    )
}

fn label(turns: &[SpeakerTurn], track: &str, start: u64, end: u64) -> (String, Vec<String>) {
    let active = vt_diarize::turns_at_midpoint(turns, track, start, end);
    if active.is_empty() {
        return ("Unknown".into(), Vec::new());
    }
    if active.len() > 1 || active.iter().any(|turn| turn.overlap) {
        return ("Overlap".into(), Vec::new());
    }
    if active.iter().any(|turn| turn.uncertain || turn.provisional) {
        return ("Unknown".into(), Vec::new());
    }
    (active[0].label.clone(), active[0].speaker_ids.clone())
}

/// Preserve exact source text when splitting acoustic words at speaker changes.
/// If a decoder's word strings cannot be located in order, keep one honest
/// multi-speaker segment rather than changing its text or inventing alignment.
pub(super) fn split_segment(mut segment: Segment, turns: &[SpeakerTurn]) -> Vec<Segment> {
    if segment.words.is_empty() || segment.words.iter().any(|word| word.text.trim().is_empty()) {
        let mut labels = turns
            .iter()
            .filter(|turn| {
                turn.track == segment.track
                    && turn.start_ms < segment.end_ms
                    && turn.end_ms > segment.start_ms
            })
            .map(|turn| {
                if turn.overlap {
                    "Overlap".into()
                } else if turn.uncertain || turn.provisional {
                    "Unknown".into()
                } else {
                    turn.label.clone()
                }
            })
            .collect::<Vec<_>>();
        labels.sort();
        labels.dedup();
        segment.speaker = if labels.is_empty() {
            "Unknown".into()
        } else {
            labels.join(" / ")
        };
        return vec![segment];
    }
    let mut positions = Vec::with_capacity(segment.words.len());
    let mut cursor = 0;
    for word in &segment.words {
        let Some(found) = segment.text[cursor..]
            .find(word.text.trim())
            .map(|offset| cursor + offset)
        else {
            let mut labels = segment
                .words
                .iter()
                .map(|word| label(turns, &segment.track, word.start_ms, word.end_ms).0)
                .collect::<Vec<_>>();
            labels.sort();
            labels.dedup();
            segment.speaker = labels.join(" / ");
            return vec![segment];
        };
        positions.push(found);
        cursor = found + word.text.trim().len();
    }
    let mut groups = Vec::new();
    let mut first = 0;
    let mut previous = label(
        turns,
        &segment.track,
        segment.words[0].start_ms,
        segment.words[0].end_ms,
    );
    for i in 1..segment.words.len() {
        let current = label(
            turns,
            &segment.track,
            segment.words[i].start_ms,
            segment.words[i].end_ms,
        );
        if current != previous {
            groups.push((first, i, previous));
            first = i;
            previous = current;
        }
    }
    groups.push((first, segment.words.len(), previous));
    if groups.len() == 1 {
        segment.speaker = groups.remove(0).2.0;
        return vec![segment];
    }
    groups
        .into_iter()
        .map(|(first, last, (speaker, _))| Segment {
            id: if first == 0 {
                segment.id.clone()
            } else {
                uuid::Uuid::new_v4().to_string()
            },
            track: segment.track.clone(),
            start_ms: if first == 0 {
                segment.start_ms
            } else {
                segment.words[first].start_ms
            },
            end_ms: if last == segment.words.len() {
                segment.end_ms
            } else {
                segment.words[last - 1].end_ms
            },
            text: segment.text[if first == 0 { 0 } else { positions[first] }
                ..if last == segment.words.len() {
                    segment.text.len()
                } else {
                    positions[last]
                }]
                .into(),
            speaker,
            words: segment.words[first..last].to_vec(),
        })
        .collect()
}

#[cfg(test)]
#[path = "speaker_pipeline_tests.rs"]
mod tests;
