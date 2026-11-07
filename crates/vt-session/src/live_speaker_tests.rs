//! Explicit opt-in coverage of the real AppService live inference workers.
//! The helper streams only an existing public WAV over fake NDJSON capture.
//! It never opens an input/output device or performs a network request.
#![cfg(unix)]
use super::*;
use std::{
    collections::BTreeSet, fs, os::unix::fs::PermissionsExt, path::Path, thread, time::Duration,
};

fn hardlink_public_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let destination = destination.join(entry.file_name());
        let kind = entry.file_type().unwrap();
        if kind.is_dir() {
            hardlink_public_tree(&entry.path(), &destination);
        } else {
            assert!(
                kind.is_file(),
                "public model cache may contain only regular files/directories"
            );
            fs::hard_link(entry.path(), destination).unwrap();
        }
    }
}
struct ShutdownOnDrop(Arc<AppService>);
impl Drop for ShutdownOnDrop {
    fn drop(&mut self) {
        self.0.wait_for_shutdown();
    }
}
fn collect_warnings(snapshot: &Snapshot, warnings: &mut BTreeSet<String>) {
    if let Some(recording) = &snapshot.recording
        && let Some(error) = &recording.error
    {
        warnings.insert(error.clone());
    }
    if let Some(live) = &snapshot.live_processing {
        if let Some(warning) = &live.vad_warning {
            warnings.insert(warning.clone());
        }
        if let Some(warning) = &live.diarization_warning {
            warnings.insert(warning.clone());
        }
    }
}
fn vector_payload(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Object(fields) => fields.iter().any(|(key, value)| {
            matches!(
                key.as_str(),
                "embedding" | "embeddings" | "vector" | "vectors"
            ) || vector_payload(value)
        }),
        serde_json::Value::Array(values) => values.iter().any(vector_payload),
        _ => false,
    }
}

#[test]
#[ignore = "explicit public WAV + verified cached English/Parakeet-VAD/diarization models; fake paced capture only, no devices/network/private data"]
fn real_public_live_workers_vad_speakers_and_words_seal_without_capture_or_output_devices() {
    public_live_workers(vt_stt::LiveModel::EnglishSmall);
}

#[test]
#[ignore = "explicit public WAV + verified cached Standard/Parakeet-VAD/diarization models; fake paced capture only, no devices/network/private data"]
fn real_public_standard_live_workers_vad_speakers_and_words_seal_without_capture_or_output_devices()
{
    public_live_workers(vt_stt::LiveModel::EnglishStandard);
}

fn public_live_workers(model: vt_stt::LiveModel) {
    let fixture = Path::new("/private/tmp/vantranscribe-stt-fixture.wav");
    let streaming_root = match model {
        vt_stt::LiveModel::EnglishSmall => Path::new("/private/tmp/vantranscribe-stt-models"),
        vt_stt::LiveModel::EnglishStandard => {
            Path::new("/private/tmp/vantranscribe-ami-eval/verified-live-models")
        }
    };
    let streaming = streaming_root.join(model.id());
    let diarization =
        Path::new("/private/tmp/vt-diarization-models/verified").join(vt_diarize::MODEL_ID);
    let refinement =
        Path::new("/private/tmp/vantranscribe-refinement-models").join(vt_stt::REFINEMENT_MODEL_ID);
    assert!(
        fixture.is_file() && streaming.is_dir() && diarization.is_dir() && refinement.is_dir(),
        "the explicitly selected public fixture and verified caches must exist; this test never downloads them"
    );
    let public_before = fs::read(fixture).unwrap();
    let wave = sherpa_onnx::Wave::read(fixture.to_str().unwrap()).unwrap();
    assert_eq!(
        wave.sample_rate(),
        16000,
        "the fake capture contract requires the known16k public fixture"
    );
    let expected = vt_core::pcm24(wave.samples()).unwrap();
    let temp = tempfile::TempDir::new_in("/private/tmp").unwrap();
    let root = temp.path().join("data");
    let models = root.join("models");
    hardlink_public_tree(&streaming, &models.join(model.id()));
    hardlink_public_tree(&diarization, &models.join(vt_diarize::MODEL_ID));
    // The verified refinement manifest makes its pinned Silero available for
    // reuse. Parakeet inference/automatic refinement remain disabled below.
    hardlink_public_tree(&refinement, &models.join(vt_stt::REFINEMENT_MODEL_ID));
    fs::hard_link(fixture, temp.path().join("public-fixture.wav")).unwrap();
    let helper = temp.path().join("capture-public-live-fixture");
    fs::write(&helper,r#"#!/usr/bin/env python3
import json, pathlib, select, struct, sys, time, wave
folder = pathlib.Path(__file__).parent
with wave.open(str(folder / 'public-fixture.wav'), 'rb') as wav:
    assert wav.getnchannels() == 1 and wav.getframerate() == 16000
    assert wav.getsampwidth() == 2 and wav.getcomptype() == 'NONE'
    raw = wav.readframes(wav.getnframes())
values = struct.unpack('<' + 'h' * (len(raw) // 2), raw)
samples = [value / 32768.0 for value in values]
def stopped():
    if select.select([sys.stdin], [], [], 0)[0]:
        return sys.stdin.readline().strip() == 'stop'
    return False
def emit(value):
    print(json.dumps(value, separators=(',', ':')), flush=True)
for track in ['mic', 'system']:
    emit({'type':'format','track':track,'sample_rate':16000,'channels':1,'epoch':0,'start_ns':0})
# Readiness gating excludes native model setup from the paced-input interval.
while not (folder / 'start-feed').exists():
    if stopped(): sys.exit(0)
    time.sleep(0.01)
started = time.monotonic()
for first in range(0, len(samples), 320):
    if stopped(): sys.exit(0)
    packet = samples[first:first + 320]
    for track in ['mic', 'system']:
        emit({'type':'audio','track':track,'epoch':0,'first_frame':first,'host_time_ns':first * 1000000000 // 16000,'samples':packet})
    target = started + (first + len(packet)) / 16000.0
    time.sleep(max(0, target - time.monotonic()))
(folder / 'fixture-fed.tmp').write_text(json.dumps({'frames':len(samples),'tracks':2,'rate':16000}))
(folder / 'fixture-fed.tmp').replace(folder / 'fixture-fed')
while not stopped():
    time.sleep(0.01)
"#).unwrap();
    fs::set_permissions(&helper, fs::Permissions::from_mode(0o700)).unwrap();
    let service = AppService::new(root, helper).unwrap();
    let _shutdown = ShutdownOnDrop(service.clone());
    {
        let mut preferences = service.preferences.lock().unwrap();
        preferences.auto_refine = false;
        preferences.auto_summarize = false;
        let mut audio = service.audio_preferences.lock().unwrap();
        audio.live_vad_enabled = true;
        audio.diarization_enabled = true;
        audio_preferences::save(service.root(), &audio).unwrap();
    }
    // Exercise legacy Small-directory retention and the new-install Standard
    // choice without writing a model preference that could hide either rule.
    assert_eq!(service.live_model_catalog().unwrap().selected, model);
    assert_eq!(service.live_model_manager().unwrap().model(), model);
    let setup = service.snapshot().unwrap();
    assert!(
        setup.models.installed && setup.vad_models.installed && setup.diarization_models.installed
    );
    assert!(setup.playback.is_none());
    let started = service
        .start(
            "Public paced live speaker fixture".into(),
            CaptureMode::Remote,
            true,
            None,
        )
        .unwrap();
    let meeting = started.recording.unwrap().id;
    let mut warnings = BTreeSet::new();
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let snapshot = service.snapshot().unwrap();
        collect_warnings(&snapshot, &mut warnings);
        if snapshot
            .live_processing
            .as_ref()
            .is_some_and(|live| live.vad_active && live.diarization_active)
            && snapshot
                .recording
                .as_ref()
                .is_some_and(|recording| recording.transcription_active)
        {
            break;
        }
        assert!(
            !snapshot
                .recording
                .as_ref()
                .is_some_and(|recording| recording.status == "failed"),
            "live model initialization failed: {warnings:?}"
        );
        assert!(
            Instant::now() < deadline,
            "real VAD and speaker workers did not become ready: {warnings:?}"
        );
        thread::sleep(Duration::from_millis(20));
    }
    fs::write(temp.path().join("start-feed"), b"ready").unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    while !temp.path().join("fixture-fed").is_file() {
        let snapshot = service.snapshot().unwrap();
        collect_warnings(&snapshot, &mut warnings);
        assert!(
            snapshot
                .recording
                .as_ref()
                .is_some_and(|recording| recording.status == "recording"),
            "paced public fixture ended unexpectedly: {warnings:?}"
        );
        assert!(
            Instant::now() < deadline,
            "fake helper did not finish the public fixture"
        );
        thread::sleep(Duration::from_millis(20));
    }
    let fed: serde_json::Value =
        serde_json::from_slice(&fs::read(temp.path().join("fixture-fed")).unwrap()).unwrap();
    assert_eq!(fed["frames"].as_u64(), Some(expected.len() as u64));
    service.stop().unwrap();
    let deadline = Instant::now() + Duration::from_secs(180);
    loop {
        let snapshot = service.snapshot().unwrap();
        collect_warnings(&snapshot, &mut warnings);
        if snapshot.recording.is_none() && snapshot.processing.is_none() {
            break;
        }
        assert!(
            !snapshot
                .recording
                .as_ref()
                .is_some_and(|recording| recording.status == "failed"),
            "live stop failed: {warnings:?}"
        );
        assert!(
            Instant::now() < deadline,
            "live inference workers did not drain and seal"
        );
        thread::sleep(Duration::from_millis(20));
    }
    let detail = service.detail(&meeting).unwrap();
    assert_eq!(detail.meeting.status, "ready", "{:?}", detail.meeting.error);
    if let Some(error) = &detail.meeting.error {
        warnings.insert(error.clone());
    }
    let info = detail
        .diarization
        .as_ref()
        .expect("independent live speaker metadata must be persisted");
    warnings.extend(info.warnings.iter().cloned());
    for warning in &warnings {
        let lower = warning.to_lowercase();
        assert!(
            ![
                "fell behind",
                "queue overflow",
                "queue full",
                "stopped unexpectedly",
                "processing stopped",
                "failed",
                "interrupted"
            ]
            .iter()
            .any(|needle| lower.contains(needle)),
            "paced input must not lose a live worker or overflow its queue: {warning}; all warnings {warnings:?}; final error {:?}; echo {:?}",
            detail.meeting.error,
            detail.echo
        );
    }
    assert_eq!(info.embedding_version, vt_diarize::EMBEDDING_VERSION);
    assert_eq!(
        info.segmentation_context,
        Some(vt_diarize::Phase::Live.segmentation_context()),
        "the actual live pass must retain its five-second source context rather than inherit offline settings"
    );
    assert!(!info.calibrated);
    assert_eq!(
        detail.runs.len(),
        1,
        "automatic refinement was disabled; this must be the actual live pass"
    );
    let active = detail.active_run_id.as_ref().unwrap();
    let run = detail.runs.iter().find(|run| &run.id == active).unwrap();
    assert_eq!(run.kind, "live");
    assert_eq!(run.status, "complete");
    // Read provenance from the sealed run, rather than reconstructing it from
    // today's preference or inferring the selected model from emitted text.
    let engines: serde_json::Value = serde_json::from_str(&run.engines_json).unwrap();
    let stt = &engines["live_stt"];
    assert_eq!(stt["requested_model"], model.id());
    assert_eq!(stt["enabled"], true);
    assert_eq!(stt["vad_requested"], true);
    let expected_threshold: f32 = match model {
        vt_stt::LiveModel::EnglishSmall => 0.5,
        vt_stt::LiveModel::EnglishStandard => 0.2,
    };
    assert_eq!(
        stt["requested_vad_options"]["threshold"].as_f64(),
        Some(f64::from(expected_threshold))
    );
    assert_eq!(stt["archive_sha256"], model.archive_sha256());
    assert_eq!(stt["provider"], "cpu");
    assert_eq!(stt["threads"], 2);
    assert_eq!(stt["runtime"], "sherpa-onnx-2.0.1");
    assert_eq!(
        stt["files"],
        serde_json::to_value(
            model
                .files()
                .iter()
                .map(|file| serde_json::json!({"name":file.name,"bytes":file.bytes,"sha256":file.sha256}))
                .collect::<Vec<_>>()
        )
        .unwrap()
    );
    let health = detail
        .inference_health
        .as_ref()
        .expect("the sealed live run must retain its worker backlog audit");
    assert!(health.complete, "the live audit must be finalized");
    assert!(health.stages.is_empty(), "unexpected shedding: {health:?}");
    assert!(
        health.gaps.is_empty(),
        "unexpected inference gap: {health:?}"
    );
    assert!(
        !health.truncated,
        "the clean fixture audit must be complete"
    );
    assert!(
        detail
            .segments
            .iter()
            .any(|segment| segment.track == "mic" && !segment.words.is_empty()),
        "Mic words missing; segment geometry {:?}; echo {:?}",
        detail
            .segments
            .iter()
            .map(|s| (
                &s.track,
                s.start_ms,
                s.end_ms,
                s.text.chars().count(),
                s.words.len()
            ))
            .collect::<Vec<_>>(),
        detail.echo,
    );
    assert!(
        detail
            .segments
            .iter()
            .any(|segment| segment.track == "system" && !segment.words.is_empty())
    );
    for segment in &detail.segments {
        assert!(!segment.id.starts_with("partial-"));
        assert!(segment.end_ms <= wave.samples().len() as u64 * 1000 / 16000);
        assert!(
            segment
                .words
                .iter()
                .all(|word| segment.start_ms <= word.start_ms
                    && word.start_ms <= word.end_ms
                    && word.end_ms <= segment.end_ms)
        );
    }
    assert!(
        detail.speaker_turns.iter().any(|turn| turn.track == "mic"
            && turn.label == "Me"
            && turn.provenance == "channel_prior"),
        "remote microphone Me is a channel prior, not an embedding identity claim"
    );
    assert!(
        detail
            .speaker_turns
            .iter()
            .any(|turn| turn.track == "system")
    );
    assert!(
        detail
            .speaker_turns
            .iter()
            .all(|turn| turn.start_ms < turn.end_ms && !turn.provenance.is_empty())
    );
    assert!(
        detail
            .speaker_assignments
            .iter()
            .any(|assignment| !assignment.words.is_empty())
    );
    for assignment in &detail.speaker_assignments {
        let segment = detail
            .segments
            .iter()
            .find(|segment| segment.id == assignment.segment_id)
            .unwrap();
        assert_eq!(assignment.words.len(), segment.words.len());
        assert!(["audio-derived", "user"].contains(&assignment.provenance.as_str()));
    }
    assert!(
        !vector_payload(&serde_json::to_value(&detail).unwrap()),
        "live results must not expose embedding vectors"
    );
    let db = store::open(service.root()).unwrap();
    let mut query=db.prepare("SELECT track,path,sha256,sample_rate,channels,frame_count FROM audio_chunks WHERE meeting_id=?1 AND state='ready' ORDER BY track,seq").unwrap();
    let rows = query
        .query_map([&meeting], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, u32>(3)?,
                row.get::<_, u16>(4)?,
                row.get::<_, i64>(5)?,
            ))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    for track in ["mic", "system"] {
        let mut decoded = Vec::new();
        let mut total = 0;
        for (row_track, path, hash, rate, channels, frames) in &rows {
            if row_track != track {
                continue;
            }
            assert_eq!((*rate, *channels), (16000, 1));
            total += frames;
            let path = service.root().join(path);
            let bytes = fs::read(&path).unwrap();
            use sha2::{Digest, Sha256};
            assert_eq!(format!("{:x}", Sha256::digest(bytes)), *hash);
            let mut reader = claxon::FlacReader::open(path).unwrap();
            decoded.extend(
                reader
                    .samples()
                    .collect::<std::result::Result<Vec<_>, _>>()
                    .unwrap(),
            );
        }
        assert_eq!(total, expected.len() as i64);
        assert!(
            decoded == expected,
            "live inference/VAD must not gate, alter or lose the original public-source PCM24 on either archived track"
        );
    }
    let missing: i64 = db
        .query_row(
            "SELECT COUNT(*) FROM audio_chunks WHERE meeting_id=?1 AND state!='ready'",
            [&meeting],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        missing, 0,
        "this paced no-gap fixture should archive every emitted frame"
    );
    assert!(
        fs::read(fixture).unwrap() == public_before,
        "the original public fixture must remain unchanged"
    );
    assert!(service.snapshot().unwrap().playback.is_none());
    eprintln!(
        "public live integration model={} vad_threshold={} frames_per_track={} tracks=2 segments={} words={} speaker_turns={} backlog_max_lag_ms={} shedding={} inference_gaps={} raw_pcm24_exact=true",
        model.id(),
        expected_threshold,
        expected.len(),
        detail.segments.len(),
        detail
            .segments
            .iter()
            .map(|segment| segment.words.len())
            .sum::<usize>(),
        detail.speaker_turns.len(),
        health.max_lag_ms,
        health.stages.len(),
        health.gaps.len(),
    );
    drop(query);
    drop(db);
}
