//! Ignored measurement harness: replays a COPY of a recorded meeting through the
//! speaker diarizer (Refined and Live phases) and prints only counters and
//! timing. It never prints transcript text, audio or embeddings.
//!
//! VT_REPLAY_ROOT=<copy of app data dir> VT_REPLAY_MEETING=<id> \
//!   cargo test -p vt-session real_call_replay -- --ignored --nocapture
use crate::{AppService, echo_pipeline, processing, store};
use std::{path::PathBuf, time::Instant};
use vt_diarize::{Diarizer, DiarizerConfig, MeetingMode, Phase};

fn replay(service: &AppService, id: &str, phase: Phase, echo: bool) {
    let started = Instant::now();
    let mode: MeetingMode = match store::meeting_mode(&service.root, id) {
        Ok(mode) => mode,
        Err(error) => return println!("{{\"phase\":\"{phase:?}\",\"setup_error\":\"{error}\"}}"),
    };
    let mut engine = match Diarizer::new(
        &vt_diarize::ModelManager::new(service.root.join("models")).model_dir(),
        DiarizerConfig {
            mode,
            phase,
            ..Default::default()
        },
    ) {
        Ok(engine) => engine,
        Err(error) => return println!("{{\"phase\":\"{phase:?}\",\"setup_error\":\"{error}\"}}"),
    };
    let mut audio_ms = [0u64; 2];
    let streamed = echo_pipeline::saved(service, id, echo, |packet, _| {
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
                let slot = usize::from(track != vt_core::TrackKind::Mic);
                audio_ms[slot] = start_ms + samples.len() as u64 * 1000 / u64::from(rate);
                engine.accept(track, &samples, rate, start_ms)?;
            }
        }
        Ok(())
    });
    let stream_error = streamed.err().map(|e| e.to_string());
    let stream_secs = started.elapsed().as_secs_f64();
    let (finish_error, outcome) = if stream_error.is_none() {
        match engine.finish() {
            Ok(result) => (None, summarize(&result)),
            Err(e) => (Some(e.to_string()), serde_json::Value::Null),
        }
    } else {
        (None, serde_json::Value::Null)
    };
    let report = serde_json::json!({
        "phase": format!("{phase:?}"),
        "echo_cancellation": echo,
        "mode": format!("{mode:?}"),
        "audio_end_ms_reached": {"mic": audio_ms[0], "system": audio_ms[1]},
        "stream_error": stream_error,
        "finish_error": finish_error,
        "stream_secs": stream_secs,
        "total_secs": started.elapsed().as_secs_f64(),
        "outcome": outcome,
        "diagnostics": engine.diagnostics(),
    });
    println!("REPLAY {}", serde_json::to_string(&report).unwrap());
}

/// Counters only: speaker counts, merges, turn counts and attributed vs
/// Unknown/uncertain milliseconds per track. No labels, text or audio.
fn summarize(result: &vt_diarize::DiarizationResult) -> serde_json::Value {
    let mut tracks = serde_json::Map::new();
    for track in ["mic", "system"] {
        let speakers = result.speakers.iter().filter(|s| s.track == track);
        let turns = result.turns.iter().filter(|t| t.track == track);
        let ms = |t: &vt_diarize::SpeakerTurn| t.end_ms - t.start_ms;
        let attributed = turns
            .clone()
            .filter(|t| !t.overlap && !t.uncertain && !t.speaker_ids.is_empty())
            .map(ms)
            .sum::<u64>();
        let unknown = turns
            .clone()
            .filter(|t| !t.overlap && (t.uncertain || t.speaker_ids.is_empty()))
            .map(ms)
            .sum::<u64>();
        let overlap = turns.clone().filter(|t| t.overlap).map(ms).sum::<u64>();
        // Split of Unknown: no speaker id at all vs a provisional (<3 s) speaker.
        let no_speaker = turns
            .clone()
            .filter(|t| !t.overlap && t.speaker_ids.is_empty())
            .map(ms)
            .sum::<u64>();
        let no_speaker_long = turns
            .clone()
            .filter(|t| !t.overlap && t.speaker_ids.is_empty() && ms(t) >= 1000)
            .map(ms)
            .sum::<u64>();
        tracks.insert(
            track.into(),
            serde_json::json!({
                "final_speakers": speakers.clone().count(),
                "final_confident_speakers": speakers.filter(|s| !s.provisional).count(),
                "turns": turns.clone().count(),
                "unattributed_turns": turns.filter(|t| !t.overlap && t.speaker_ids.is_empty()).count(),
                "attributed_ms": attributed,
                "unknown_uncertain_ms": unknown,
                "overlap_ms": overlap,
                "unknown_no_speaker_ms": no_speaker,
                "unknown_no_speaker_ge_1s_ms": no_speaker_long,
                "unknown_provisional_ms": unknown - no_speaker,
            }),
        );
    }
    serde_json::json!({"merges": result.merges.len(), "tracks": tracks})
}

#[test]
#[ignore = "needs VT_REPLAY_ROOT (copy of app data) and VT_REPLAY_MEETING"]
fn real_call_replay() {
    let (Some(root), Ok(id)) = (
        std::env::var_os("VT_REPLAY_ROOT"),
        std::env::var("VT_REPLAY_MEETING"),
    ) else {
        eprintln!("VT_REPLAY_ROOT / VT_REPLAY_MEETING unset; skipping");
        return;
    };
    let service = AppService::new(PathBuf::from(root), "missing-helper".into()).unwrap();
    let echo = service
        .audio_preferences
        .lock()
        .map(|p| p.echo_cancellation_enabled)
        .unwrap_or(false);
    replay(&service, &id, Phase::Refined, echo);
    replay(&service, &id, Phase::Live, echo);
}
