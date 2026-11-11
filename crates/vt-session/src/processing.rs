use super::*;
use anyhow::{Context, Result, bail};
use std::panic::{AssertUnwindSafe, catch_unwind};
use vt_core::TrackKind;
use vt_stt::{BatchRefiner, RefinedSegment, RefinementModelManager};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Start {
    Manual,
    AfterRecording,
    AfterRefinement,
    AfterReplay,
}

fn reserve(service: &AppService, id: &str, kind: &str, start: Start) -> Result<MeetingDetail> {
    if service.shutting_down.load(Ordering::Relaxed) {
        bail!("VanTranscribe is closing. Reopen it to process this meeting.");
    }
    service.ensure_processing_space()?;
    if kind == "refine" {
        service.ensure_saved_audio(id)?;
    }
    let detail = store::detail(&service.root, id)?;
    let mut runtime = service
        .runtime
        .lock()
        .map_err(|_| anyhow::anyhow!("session state unavailable"))?;
    if service.shutting_down.load(Ordering::Relaxed) {
        bail!("VanTranscribe is closing. Reopen it to process this meeting.");
    }
    let continuing = matches!(start, Start::AfterRefinement | Start::AfterReplay)
        && runtime.processing_view.as_ref().is_some_and(|job| {
            job.meeting_id == id && matches!(job.kind.as_str(), "refine" | "replay")
        });
    if runtime.processing && !continuing {
        bail!("Another processing job is already running.");
    }
    if runtime.current.as_ref().is_some_and(|current| {
        matches!(
            current.view.status.as_str(),
            "recording" | "paused" | "processing"
        ) && !(start == Start::AfterRecording
            && current.view.id == id
            && current.view.status == "processing")
    }) {
        bail!("Finish the active recording before processing saved audio.");
    }
    if kind == "refine" && !runtime.refinement_model.installed {
        bail!("Install the Parakeet refinement pack in Settings first.");
    }
    if kind == "summary" {
        let active = detail
            .active_run_id
            .as_deref()
            .context("This meeting has no transcript run.")?;
        if detail.segments.is_empty()
            || !detail
                .runs
                .iter()
                .any(|run| run.id == active && run.status == "complete")
        {
            bail!("Finish or transcribe this meeting before generating AI notes.");
        }
        if !detail.has_summary_input() {
            bail!(
                "No visible speech remains for AI notes. Restore a microphone line before generating notes."
            );
        }
    }
    store::set_status(
        &service.root,
        id,
        "processing",
        if kind == "summary" {
            detail.meeting.error.as_deref()
        } else {
            None
        },
    )?;
    runtime.processing = true;
    runtime.processing_view = Some(ProcessingView {
        meeting_id: id.into(),
        kind: kind.into(),
        stage: "Preparing".into(),
        progress: 0.0,
        error: None,
    });
    if start == Start::AfterRecording
        || runtime
            .current
            .as_ref()
            .is_some_and(|current| current.view.id == id && current.view.status == "failed")
    {
        runtime.current = None;
        runtime.partials.clear();
    }
    if !continuing {
        service.cancel_processing.store(false, Ordering::Relaxed);
    }
    Ok(detail)
}

pub(super) fn progress(service: &AppService, id: &str, stage: &str, fraction: f64) {
    if let Ok(mut runtime) = service.runtime.lock()
        && let Some(job) = runtime
            .processing_view
            .as_mut()
            .filter(|job| job.meeting_id == id)
    {
        job.stage = stage.into();
        job.progress = fraction.clamp(0.0, 1.0);
    }
}

pub(super) fn finish(service: &AppService, id: &str, result: Result<()>) {
    if let Err(error) = result {
        let interrupted = service.cancel_processing.load(Ordering::Relaxed);
        let _ = store::set_status(
            &service.root,
            id,
            if interrupted { "recovered" } else { "failed" },
            Some(&error.to_string()),
        );
    }
    if let Ok(mut runtime) = service.runtime.lock() {
        runtime.processing = false;
        runtime.processing_view = None;
        runtime.partials.clear();
    }
}

fn check_cancel(service: &AppService) -> Result<()> {
    if service.cancel_processing.load(Ordering::Relaxed) {
        bail!("Processing cancelled. The previous transcript and notes were preserved.");
    }
    Ok(())
}

#[cfg(test)]
#[path = "processing_tests.rs"]
mod tests;

pub(super) fn after_recording(service: Arc<AppService>, id: &str) {
    if service.shutting_down.load(Ordering::Relaxed) {
        if let Ok(mut runtime) = service.runtime.lock() {
            runtime.current = None;
            runtime.partials.clear();
        }
        return;
    }
    let prefs = service
        .preferences
        .lock()
        .map(|prefs| prefs.clone())
        .unwrap_or_default();
    let can_refine = service
        .runtime
        .lock()
        .map(|runtime| runtime.refinement_model.installed)
        .unwrap_or(false);
    let summary_only = !(prefs.auto_refine && can_refine);
    let result = if prefs.auto_refine && can_refine {
        start_refinement(service.clone(), id, Start::AfterRecording)
    } else if prefs.auto_summarize
        && prefs.summary.provider != vt_summarize::Provider::None
        && store::detail(&service.root, id).is_ok_and(|detail| detail.has_summary_input())
    {
        start_summary(service.clone(), id, Start::AfterRecording)
    } else {
        if let Ok(mut runtime) = service.runtime.lock() {
            runtime.current = None;
            runtime.partials.clear();
        }
        Ok(())
    };
    if let Err(error) = result {
        if summary_only {
            summary_start_failed(&service, id, error);
        } else if !service.shutting_down.load(Ordering::Relaxed) {
            let _ = store::set_status(
                &service.root,
                id,
                "ready",
                Some(&format!("Automatic processing could not start: {error}")),
            );
        }
        if let Ok(mut runtime) = service.runtime.lock() {
            runtime.current = None;
            runtime.processing = false;
            runtime.processing_view = None;
            runtime.partials.clear();
        }
    }
}

pub(super) fn refine(service: Arc<AppService>, id: &str) -> Result<()> {
    start_refinement(service, id, Start::Manual)
}

pub(super) fn after_replay(service: Arc<AppService>, id: &str) {
    let prefs = service
        .preferences
        .lock()
        .map(|prefs| prefs.clone())
        .unwrap_or_default();
    if !service.cancel_processing.load(Ordering::Relaxed)
        && prefs.auto_summarize
        && prefs.summary.provider != vt_summarize::Provider::None
        && store::detail(&service.root, id).is_ok_and(|detail| detail.has_summary_input())
    {
        if let Err(error) = start_summary(service.clone(), id, Start::AfterReplay) {
            summary_start_failed(&service, id, error);
        }
    } else {
        finish(&service, id, Ok(()));
    }
}

fn start_refinement(service: Arc<AppService>, id: &str, start: Start) -> Result<()> {
    reserve(&service, id, "refine", start)?;
    let id = id.to_owned();
    let fallback_id = id.clone();
    let fallback = service.clone();
    let spawn=std::thread::Builder::new().name("post-meeting-refinement".into()).spawn(move||{
        let result=catch_unwind(AssertUnwindSafe(||refinement_job(&service,&id)))
            .unwrap_or_else(|_|Err(anyhow::anyhow!("Refinement worker stopped unexpectedly. The previous transcript is preserved.")));
        if result.is_ok()&&!service.cancel_processing.load(Ordering::Relaxed) {
            let prefs=service.preferences.lock().map(|prefs|prefs.clone()).unwrap_or_default();
            if prefs.auto_summarize&&prefs.summary.provider!=vt_summarize::Provider::None
                &&store::detail(&service.root,&id).is_ok_and(|detail|!detail.segments.is_empty()) {
                if let Err(error)=start_summary(service.clone(),&id,Start::AfterRefinement){summary_start_failed(&service,&id,error);}
                return;
            }
        }
        finish(&service,&id,result);
    });
    if let Err(error) = spawn {
        finish(
            &fallback,
            &fallback_id,
            Err(anyhow::anyhow!(error.to_string())),
        );
        return Err(error.into());
    }
    Ok(())
}

fn refinement_job(service: &AppService, id: &str) -> Result<()> {
    check_cancel(service)?;
    progress(service, id, "Verifying recorded audio", 0.02);
    service.ensure_saved_audio(id)?;
    let _report = vt_record::Recording::recover(&service.root, id)?;
    let echo_enabled = service
        .audio_preferences
        .lock()
        .map(|p| p.echo_cancellation_enabled)
        .unwrap_or(false);
    let weights = RefinementModelManager::new(service.root.join("models"))
        .recorded_weights()
        .unwrap_or_else(|_| {
            vt_stt::REFINEMENT_FILES
                .iter()
                .map(|&(name, bytes, hash)| (name.to_owned(), bytes, hash.to_owned()))
                .collect::<Vec<_>>()
        });
    let engine = serde_json::json!({"engine":"sherpa-onnx","version":"2.0.1","model":vt_stt::REFINEMENT_MODEL_ID,"provider":"cpu","weights":weights,"echo_engine":vt_dsp::AEC_ENGINE,"echo_requested":echo_enabled});
    let run = store::create_run(&service.root, id, "refine", &engine.to_string())?;
    let outcome = (|| -> Result<()> {
        let turns = crate::speaker_pipeline::refine(service, id, &run.id, echo_enabled)?;
        let in_person =
            store::meeting_mode(&service.root, id)? == vt_diarize::MeetingMode::InPerson;
        progress(
            service,
            id,
            "Loading Parakeet and voice activity detection",
            if turns.is_some() { 0.46 } else { 0.06 },
        );
        let mut refiner = BatchRefiner::new(
            &RefinementModelManager::new(service.root.join("models")).model_dir(),
        )?;
        let echo = crate::echo_pipeline::saved(service, id, echo_enabled, |packet, fraction| {
            check_cancel(service)?;
            progress(
                service,
                id,
                "Refining speech",
                if turns.is_some() {
                    0.48 + fraction * 0.40
                } else {
                    0.12 + fraction * 0.76
                },
            );
            let segments = match packet {
                AudioPacket::Gap { track, .. } => refiner.reset_track(track)?,
                AudioPacket::Audio {
                    track,
                    samples,
                    rate,
                    start_ms,
                } => refiner.accept(track, &samples, rate, start_ms)?,
            };
            store::append_run_segments(
                &service.root,
                &run.id,
                &segments
                    .into_iter()
                    .flat_map(|segment| prepared_segment(segment, turns.as_deref(), in_person))
                    .collect::<Vec<_>>(),
            )?;
            Ok(())
        })?;
        progress(service, id, "Finalizing words", 0.92);
        let tail = refiner
            .finish()?
            .into_iter()
            .flat_map(|segment| prepared_segment(segment, turns.as_deref(), in_person))
            .collect::<Vec<_>>();
        store::append_run_segments(&service.root, &run.id, &tail)?;
        crate::echo_store::save_info(&service.root, id, &run.id, &echo)?;
        check_cancel(service)?;
        progress(service, id, "Publishing transcript and search index", 0.98);
        store::publish_run(&service.root, id, &run.id)?;
        store::set_status(&service.root, id, "ready", None)?;
        Ok(())
    })();
    if outcome.is_err() {
        let _ = store::abandon_run(&service.root, &run.id);
    }
    outcome
}

fn prepared_segment(
    segment: RefinedSegment,
    turns: Option<&[vt_diarize::SpeakerTurn]>,
    in_person: bool,
) -> Vec<Segment> {
    let mut segment = segment_from_refined(segment);
    if let Some(turns) = turns {
        crate::speaker_pipeline::split_segment(segment, turns)
    } else {
        if in_person {
            segment.speaker = "Unknown".into();
        }
        vec![segment]
    }
}

fn segment_from_refined(segment: RefinedSegment) -> Segment {
    Segment {
        id: uuid::Uuid::new_v4().to_string(),
        track: segment.track.as_str().into(),
        start_ms: segment.start_ms,
        end_ms: segment.end_ms,
        text: segment.text,
        speaker: if segment.track == TrackKind::Mic {
            "Me"
        } else {
            "Them"
        }
        .into(),
        words: segment
            .words
            .into_iter()
            .map(|word| TranscriptWord {
                text: word.text,
                start_ms: word.start_ms,
                end_ms: word.end_ms,
            })
            .collect(),
    }
}

pub(super) enum AudioPacket {
    Audio {
        track: TrackKind,
        samples: Vec<f32>,
        rate: u32,
        start_ms: u64,
    },
    Gap {
        track: TrackKind,
        start_ms: u64,
    },
}

pub(super) fn for_each_audio(
    service: &AppService,
    id: &str,
    mut consume: impl FnMut(AudioPacket, f64) -> Result<()>,
) -> Result<()> {
    let mut reader = vt_playback::SavedAudioReader::open(&service.root, id)?;
    loop {
        check_cancel(service)?;
        let Some(packet) = reader.next_packet()? else {
            break;
        };
        let packet = match packet {
            vt_playback::SavedAudioPacket::Audio {
                track,
                samples,
                rate,
                start_ms,
            } => AudioPacket::Audio {
                track,
                samples,
                rate,
                start_ms,
            },
            vt_playback::SavedAudioPacket::Gap {
                track, start_ms, ..
            } => AudioPacket::Gap { track, start_ms },
        };
        consume(packet, reader.progress())?;
    }
    Ok(())
}

pub(super) fn generate_summary(service: Arc<AppService>, id: &str) -> Result<()> {
    start_summary(service, id, Start::Manual)
}

fn start_summary(service: Arc<AppService>, id: &str, start: Start) -> Result<()> {
    let prefs = service
        .preferences
        .lock()
        .map_err(|_| anyhow::anyhow!("settings unavailable"))?
        .clone();
    vt_summarize::validate_config(&prefs.summary)?;
    let detail = reserve(&service, id, "summary", start)?;
    let previous = detail.meeting.clone();
    let fallback_previous = previous.clone();
    let id = id.to_owned();
    let fallback_id = id.clone();
    let fallback = service.clone();
    let spawn = std::thread::Builder::new()
        .name("meeting-ai-notes".into())
        .spawn(move || {
            let outcome = catch_unwind(AssertUnwindSafe(|| {
                summary_job(&service, &id, &prefs.summary, detail)
            }))
            .unwrap_or_else(|_| {
                Err(anyhow::anyhow!(
                    "The summary worker stopped unexpectedly. Previous notes were preserved."
                ))
            });
            finish_summary(&service, &id, &previous, outcome);
        });
    if let Err(error) = spawn {
        finish_summary(
            &fallback,
            &fallback_id,
            &fallback_previous,
            Err(anyhow::anyhow!(error.to_string())),
        );
        return Err(error.into());
    }
    Ok(())
}

fn finish_summary(service: &AppService, id: &str, previous: &Meeting, outcome: Result<()>) {
    // A provider operation cannot change whether the transcript needs repair.
    let error = outcome.err().map(|error| error.to_string());
    let _ = store::set_summary_error(&service.root, id, error.as_deref());
    let _ = store::set_status(
        &service.root,
        id,
        &previous.status,
        previous.error.as_deref(),
    );
    finish(service, id, Ok(()));
}

fn summary_start_failed(service: &AppService, id: &str, error: anyhow::Error) {
    let _ = store::set_summary_error(&service.root, id, Some(&error.to_string()));
    if let Ok(detail) = store::detail(&service.root, id) {
        let _ = store::set_status(
            &service.root,
            id,
            &detail.meeting.status,
            detail.meeting.error.as_deref(),
        );
    }
    finish(service, id, Ok(()));
}

fn summary_job(
    service: &AppService,
    id: &str,
    config: &vt_summarize::SummaryConfig,
    detail: MeetingDetail,
) -> Result<()> {
    check_cancel(service)?;
    let run = detail
        .active_run_id
        .as_deref()
        .context("No active transcript is available.")?;
    let hidden = detail
        .bleed_decisions
        .iter()
        .filter(|d| d.status == "applied" && d.decision == "bleed")
        .filter_map(|d| d.segment_id.as_deref())
        .collect::<std::collections::BTreeSet<_>>();
    let segments = detail
        .segments
        .iter()
        .filter(|segment| !hidden.contains(segment.id.as_str()))
        .map(|segment| vt_summarize::SummarySegment {
            id: segment.id.clone(),
            text: format!(
                "[{}ms · {}] {}",
                segment.start_ms, segment.speaker, segment.text
            ),
        })
        .collect::<Vec<_>>();
    let helper = service.helper.with_file_name("vt-summary-helper");
    let result = vt_summarize::summarize(
        config,
        &segments,
        Some(&helper),
        service.cancel_processing.clone(),
        |update| {
            let stage = match update.stage.as_str() {
                "map" => "Reading transcript",
                "reduce" => "Combining grounded notes",
                "complete" => "Saving notes",
                _ => "Generating notes",
            };
            progress(service, id, stage, update.fraction);
        },
    )?;
    check_cancel(service)?;
    let mut errors = result.errors.clone();
    if detail.meeting.error.is_some() {
        errors.push(
            "The transcript has a recorded processing warning; notes cover the available text."
                .into(),
        );
    }
    let gaps: i64 = store::read(&service.root, |db| {
        Ok(db.query_row("SELECT COUNT(*) FROM audio_chunks WHERE meeting_id=?1 AND state='gap' AND gap_reason!='pause'",[id],|row|row.get(0))?)
    })?;
    if gaps > 0 {
        errors.push(
            "The recording contains unavailable audio intervals; notes cover captured speech only."
                .into(),
        );
    }
    let partial = result.partial || !errors.is_empty();
    let markdown = vt_summarize::render_markdown(&result.content, partial);
    let content = serde_json::json!({"content":result.content,"partial":partial,"errors":errors});
    store::save_summary(
        &service.root,
        id,
        run,
        detail.correction_version,
        result.provider.as_str(),
        &result.model,
        &result.template,
        &content.to_string(),
        &markdown,
        &result.citations,
    )?;
    Ok(())
}
