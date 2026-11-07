//! Authoritative media guards and startup recovery, using generated temporary files only.
use super::*;
use rusqlite::params;
use std::{
    fs,
    process::{Command, Stdio},
    sync::atomic::AtomicUsize,
    thread,
    time::Duration,
};
use tempfile::TempDir;
use vt_core::{AudioBlock, FramePosition, StreamFormat};
use vt_playback::{AudioRenderer, AudioSinkFactory, OpenedSink, Playback, PlaybackError};

const ID: &str = "maintenance-media";
fn source() -> Segment {
    Segment {
        id: "synthetic-source".into(),
        track: "mic".into(),
        start_ms: 0,
        end_ms: 100,
        text: "Synthetic retained speech".into(),
        speaker: "Me".into(),
        words: vec![],
    }
}
fn fixture() -> (TempDir, Arc<AppService>) {
    let temp = TempDir::new().unwrap();
    let app = AppService::new(
        temp.path().join("data"),
        temp.path().join("never-launched-helper"),
    )
    .unwrap();
    let mut recording = vt_record::Recording::create(app.root(), ID).unwrap();
    store::create_meeting(app.root(), ID, "Synthetic media authority", "remote").unwrap();
    recording
        .write_chunk(
            TrackKind::Mic,
            &AudioBlock::new(
                StreamFormat::new(8000, 1).unwrap(),
                FramePosition {
                    epoch: 0,
                    first_frame: 0,
                    host_time_ns: 0,
                },
                vec![0.125; 800],
            )
            .unwrap(),
        )
        .unwrap();
    recording.finish().unwrap();
    store::save_segment(app.root(), ID, &source()).unwrap();
    store::complete_live_run(app.root(), ID).unwrap();
    store::set_status(app.root(), ID, "ready", None).unwrap();
    (temp, app)
}
struct NoDevice(Arc<AtomicUsize>);
impl AudioSinkFactory for NoDevice {
    fn open(&mut self, _: AudioRenderer) -> vt_playback::Result<OpenedSink> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(PlaybackError::Output("This test prohibits output".into()))
    }
}
fn prepare_player(app: &AppService) -> Arc<AtomicUsize> {
    let opened = Arc::new(AtomicUsize::new(0));
    *app.playback.lock().unwrap() =
        Some(Playback::with_sink(app.root(), ID, NoDevice(opened.clone())).unwrap());
    opened
}
fn raw_rows(app: &AppService) -> Vec<String> {
    store::open(app.root()).unwrap().prepare("SELECT json_object('track',track,'seq',seq,'state',state,'kind',kind,'path',path,'sha256',sha256) FROM audio_chunks WHERE meeting_id=?1 ORDER BY track,seq").unwrap().query_map([ID],|r|r.get(0)).unwrap().collect::<rusqlite::Result<_>>().unwrap()
}
fn assert_media_blocked(app: &Arc<AppService>, opened: &AtomicUsize) {
    // If a future regression reaches playback, the injected factory still
    // prohibits every real output-device open.
    assert!(
        app.play_audio(ID, None)
            .unwrap_err()
            .to_string()
            .contains("audio")
    );
    assert!(app.seek_audio(ID, 10).is_err());
    assert!(app.set_playback_volumes(ID, 0.0, 0.0).is_err());
    assert!(app.audio_waveform(ID, 8).is_err());
    {
        let mut runtime = app.runtime.lock().unwrap();
        runtime.model.installed = true;
        runtime.refinement_model.installed = true;
    }
    assert!(
        app.transcribe(ID)
            .unwrap_err()
            .to_string()
            .contains("audio")
    );
    assert!(app.refine(ID).unwrap_err().to_string().contains("audio"));
    assert_eq!(opened.load(Ordering::SeqCst), 0);
    let runtime = app.runtime.lock().unwrap();
    assert!(!runtime.processing && runtime.processing_view.is_none());
}

#[test]
fn pending_retention_blocks_cached_output_and_all_audio_jobs_before_recovery() {
    let (_temp, app) = fixture();
    let opened = prepare_player(&app);
    let before = raw_rows(&app);
    store::open(app.root()).unwrap().execute("INSERT INTO app_retention_intents(token,meeting_id,kind,state,payload_json,created_at) VALUES ('synthetic-pending',?1,'audio','confirmed','{}',?2)",params![ID,chrono::Utc::now().to_rfc3339()]).unwrap();
    assert!(!app.detail(ID).unwrap().audio_available);
    assert_media_blocked(&app, &opened);
    assert_eq!(
        raw_rows(&app),
        before,
        "blocked jobs must not turn moved ready audio into gaps"
    );
    assert_eq!(app.detail(ID).unwrap().meeting.status, "ready");
    assert_eq!(app.detail(ID).unwrap().segments[0].text, source().text);
}

#[test]
fn actual_audio_purge_keeps_transcript_but_disables_output_waveform_and_inference() {
    let (_temp, app) = fixture();
    let opened = prepare_player(&app);
    let preview = app.preview_deletion(ID, DeletionKind::Audio).unwrap();
    let report = app.commit_deletion(&preview.token).unwrap();
    assert_eq!(report.audio_purged, 1);
    assert!(!app.detail(ID).unwrap().audio_available);
    assert!(!app.root().join(format!("meetings/{ID}/audio")).exists());
    assert_media_blocked(&app, &opened);
    assert!(raw_rows(&app).is_empty());
    assert_eq!(app.detail(ID).unwrap().segments[0].text, source().text);
    assert_eq!(app.search("retained").unwrap().len(), 1);
}

#[test]
fn maintenance_runtime_reservation_blocks_text_edits_without_partial_note_metadata() {
    let (_temp, app) = fixture();
    let before = app.detail(ID).unwrap();
    let run = before.run_id.as_deref().unwrap();
    {
        let mut runtime = app.runtime.lock().unwrap();
        runtime.processing = true;
        runtime.processing_view = Some(ProcessingView {
            meeting_id: ID.into(),
            kind: "maintenance".into(),
            stage: "Synthetic reservation".into(),
            progress: 0.0,
            error: None,
        });
    }
    assert!(
        app.save_note(ID, "Must not publish")
            .unwrap_err()
            .to_string()
            .contains("maintenance")
    );
    assert!(app.rename(ID, "Must not rename").is_err());
    assert!(app.export(ID, "json").is_err());
    assert!(
        app.correct_segment(ID, run, "synthetic-source", "Must not correct")
            .is_err()
    );
    assert!(
        app.resolve_correction(ID, "missing", None)
            .unwrap_err()
            .to_string()
            .contains("maintenance")
    );
    let after = app.detail(ID).unwrap();
    assert_eq!(after.meeting.title, before.meeting.title);
    assert!(after.notes.is_empty() && after.corrections.is_empty());
    assert!(app.search("Must not publish").unwrap().is_empty());
    assert_eq!(
        store::open(app.root())
            .unwrap()
            .query_row("SELECT count(*) FROM app_note_timing", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn note_readback_handles_store_only_legacy_and_atomic_timing_metadata() {
    let temp = TempDir::new().unwrap();
    store::initialize(temp.path()).unwrap();
    vt_record::Recording::create(temp.path(), ID)
        .unwrap()
        .finish()
        .unwrap();
    store::create_meeting(temp.path(), ID, "Synthetic legacy notes", "remote").unwrap();
    store::save_note(temp.path(), ID, 20, "Legacy note").unwrap();
    let legacy = store::detail(temp.path(), ID).unwrap();
    assert_eq!(legacy.notes[0].timing_provenance, None);
    assert_eq!(legacy.notes[0].timing_synchronized, None);
    health::initialize(temp.path()).unwrap();
    store::save_note_with_metadata(
        temp.path(),
        ID,
        40,
        "Explicit approximate note",
        |db, note| {
            health::note_timing(
                db,
                note,
                &SourceTiming {
                    provenance: TimingProvenance::LatestAudioObservation,
                    ..Default::default()
                },
            )
        },
    )
    .unwrap();
    let detail = store::detail(temp.path(), ID).unwrap();
    assert_eq!(detail.notes[0].timing_provenance, None);
    assert_eq!(
        detail.notes[1].timing_provenance.as_deref(),
        Some("latest_audio_observation")
    );
    assert_eq!(detail.notes[1].timing_synchronized, Some(false));
    let exported: serde_json::Value =
        serde_json::from_slice(&fs::read(store::export(temp.path(), ID, "json").unwrap()).unwrap())
            .unwrap();
    assert_eq!(
        exported["notes"][1]["timing_provenance"],
        "latest_audio_observation"
    );
    for (at, text, provenance, synchronized) in [
        (
            0,
            "Explicit untimed text",
            TimingProvenance::UntimedNote,
            false,
        ),
        (
            60,
            "Exact saved bookmark",
            TimingProvenance::SavedPlaybackPosition,
            true,
        ),
    ] {
        store::save_note_with_metadata(temp.path(), ID, at, text, |db, note| {
            health::note_timing(
                db,
                note,
                &SourceTiming {
                    provenance,
                    synchronized,
                    ..Default::default()
                },
            )
        })
        .unwrap();
    }
    for format in ["markdown", "txt"] {
        let content = fs::read_to_string(store::export(temp.path(), ID, format).unwrap()).unwrap();
        assert!(content.contains("Untimed") && content.contains("Explicit untimed text"));
        assert!(
            content.contains("?00:00:00.020"),
            "legacy timing must remain unverified"
        );
        assert!(
            content.contains("~00:00:00.040"),
            "observed fallback must be approximate"
        );
        let exact = content
            .lines()
            .find(|line| line.contains("Exact saved bookmark"))
            .unwrap();
        assert!(exact.contains("00:00:00.060") && !exact.contains('~') && !exact.contains('?'));
    }
}

#[test]
fn startup_completes_confirmed_retention_before_interrupted_staging_cleanup() {
    let (temp, app) = fixture();
    let root = app.root().to_path_buf();
    // Staging belongs to the confirmed fingerprint. Clearing it first would
    // invalidate that snapshot and restore audio instead of completing purge.
    store::stage_segment(
        &root,
        "synthetic-interrupted-stage",
        ID,
        &Segment {
            id: "staged".into(),
            ..source()
        },
    )
    .unwrap();
    let preview = retention::plan(&root, ID, DeletionKind::Audio).unwrap();
    let marker = temp.path().join("moved-checkpoint");
    struct ChildGuard(std::process::Child);
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let mut child = ChildGuard(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "retention::tests::retention_crash_child",
                "--ignored",
                "--nocapture",
            ])
            .env("VT_RETENTION_CHILD_ROOT", &root)
            .env("VT_RETENTION_CHILD_TOKEN", &preview.token)
            .env("VT_RETENTION_TEST_PHASE", "moved")
            .env("VT_RETENTION_TEST_MARKER", &marker)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(15);
    while !marker.exists() {
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "synthetic child exited before confirmed move"
        );
        assert!(
            Instant::now() < deadline,
            "synthetic retention checkpoint timed out"
        );
        thread::sleep(Duration::from_millis(10));
    }
    child.0.kill().unwrap();
    child.0.wait().unwrap();
    assert!(retention::has_pending(&root, ID).unwrap());
    assert!(!root.join(format!("meetings/{ID}/audio")).exists());
    drop(app);
    let reopened =
        AppService::new(root.clone(), temp.path().join("never-launched-helper")).unwrap();
    assert!(!retention::has_pending(&root, ID).unwrap());
    assert!(!reopened.detail(ID).unwrap().audio_available);
    assert!(raw_rows(&reopened).is_empty());
    assert_eq!(reopened.detail(ID).unwrap().segments[0].text, source().text);
    let db = store::open(&root).unwrap();
    assert_eq!(
        db.query_row(
            "SELECT state FROM app_retention_intents WHERE token=?1",
            [&preview.token],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "done"
    );
    assert_eq!(
        db.query_row("SELECT count(*) FROM app_transcription_staging", [], |r| {
            r.get::<_, i64>(0)
        })
        .unwrap(),
        0
    );
    assert_eq!(reopened.last_maintenance_report().unwrap().audio_purged, 1);
}

#[test]
fn saved_reader_lease_blocks_maintenance_without_blocking_snapshots_and_releases_on_error_or_panic()
{
    let (_temp, app) = fixture();
    let (entered_tx, entered_rx) = mpsc::sync_channel(1);
    let (release_tx, release_rx) = mpsc::sync_channel(1);
    let reader_app = app.clone();
    let reader = thread::spawn(move || {
        audio_service::with_saved_audio_reader(&reader_app, ID, || {
            entered_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            Ok(())
        })
    });
    entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let (snapshot_tx, snapshot_rx) = mpsc::sync_channel(1);
    let snapshot_app = app.clone();
    let snapshot = thread::spawn(move || {
        let _ = snapshot_tx.send(snapshot_app.snapshot());
    });
    let responsive = snapshot_rx.recv_timeout(Duration::from_secs(2));
    if responsive.is_err() {
        release_tx.send(()).unwrap();
        reader.join().unwrap().unwrap();
        snapshot.join().unwrap();
        panic!("saved reader held the runtime mutex across its read");
    }
    responsive.unwrap().unwrap();
    assert_eq!(app.runtime.lock().unwrap().saved_audio_readers, 1);
    assert!(app.preview_deletion(ID, DeletionKind::Audio).is_err());
    release_tx.send(()).unwrap();
    reader.join().unwrap().unwrap();
    snapshot.join().unwrap();
    assert_eq!(app.runtime.lock().unwrap().saved_audio_readers, 0);
    assert!(app.preview_deletion(ID, DeletionKind::Audio).is_ok());
    assert!(
        audio_service::with_saved_audio_reader(&app, ID, || -> Result<()> {
            bail!("synthetic decode error")
        })
        .is_err()
    );
    assert_eq!(app.runtime.lock().unwrap().saved_audio_readers, 0);
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = audio_service::with_saved_audio_reader(&app, ID, || -> Result<()> {
            panic!("synthetic reader panic")
        });
    }));
    assert!(panic.is_err());
    assert_eq!(app.runtime.lock().unwrap().saved_audio_readers, 0);
    assert!(app.snapshot().is_ok());
}
