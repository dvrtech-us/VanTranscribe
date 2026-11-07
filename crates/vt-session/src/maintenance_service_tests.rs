//! Runtime exclusion checks using temporary synthetic audio and a silent sink.
use super::*;
use std::{fs, sync::atomic::AtomicUsize};
use tempfile::TempDir;
use vt_core::{AudioBlock, FramePosition, StreamFormat};
use vt_playback::{AudioRenderer, AudioSink, AudioSinkFactory, OpenedSink, OutputFormat, Playback};

fn fixture() -> (TempDir, Arc<AppService>) {
    let temp = TempDir::new().unwrap();
    let service =
        AppService::new(temp.path().join("data"), temp.path().join("unused-helper")).unwrap();
    let mut recording = vt_record::Recording::create(service.root(), "saved").unwrap();
    store::create_meeting(service.root(), "saved", "Synthetic maintenance", "remote").unwrap();
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
    store::complete_live_run(service.root(), "saved").unwrap();
    store::set_status(service.root(), "saved", "ready", None).unwrap();
    (temp, service)
}
fn current(service: &AppService, status: &str) {
    let (control, _) = mpsc::channel();
    service.runtime.lock().unwrap().current = Some(Current {
        view: RecordingView {
            id: "other".into(),
            title: "Other".into(),
            status: status.into(),
            elapsed_ms: 0,
            mic_level: 0.0,
            system_level: 0.0,
            gaps: 0,
            transcription_configured: false,
            transcription_active: false,
            error: None,
        },
        control,
    });
}
#[derive(Default)]
struct SinkState {
    dropped: AtomicUsize,
    renderer: Mutex<Option<AudioRenderer>>,
}
struct Sink(Arc<SinkState>);
impl AudioSink for Sink {
    fn play(&self) -> vt_playback::Result<()> {
        Ok(())
    }
}
impl Drop for Sink {
    fn drop(&mut self) {
        self.0.renderer.lock().unwrap().take();
        self.0.dropped.fetch_add(1, Ordering::SeqCst);
    }
}
struct Factory(Arc<SinkState>);
impl AudioSinkFactory for Factory {
    fn open(&mut self, renderer: AudioRenderer) -> vt_playback::Result<OpenedSink> {
        *self.0.renderer.lock().unwrap() = Some(renderer);
        Ok(OpenedSink {
            format: OutputFormat {
                sample_rate: 8000,
                channels: 1,
            },
            sink: Box::new(Sink(self.0.clone())),
        })
    }
}
fn listening(service: &AppService) -> Arc<SinkState> {
    let sink = Arc::new(SinkState::default());
    let player = Playback::with_sink(service.root(), "saved", Factory(sink.clone())).unwrap();
    player.play().unwrap();
    *service.playback.lock().unwrap() = Some(player);
    sink
}
#[test]
fn active_recording_and_processing_block_all_storage_mutation() {
    for status in ["recording", "paused", "processing"] {
        let (_temp, service) = fixture();
        let preview = service
            .preview_deletion("saved", DeletionKind::Audio)
            .unwrap();
        current(&service, status);
        assert!(
            service
                .preview_deletion("saved", DeletionKind::Meeting)
                .is_err()
        );
        assert!(service.preview_cleanup().is_err());
        assert!(service.commit_deletion(&preview.token).is_err());
        assert!(
            service
                .save_maintenance_policy(MaintenancePolicy::default())
                .is_err()
        );
        assert!(retention::audio_available(service.root(), "saved").unwrap());
    }
    let (_temp, service) = fixture();
    service.runtime.lock().unwrap().processing = true;
    assert!(service.preview_cleanup().is_err());
    assert!(
        service
            .save_maintenance_policy(MaintenancePolicy::default())
            .is_err()
    );
}
#[test]
fn reservation_blocks_playback_and_capture_and_releases_after_panic() {
    let (_temp, service) = fixture();
    let sink = listening(&service);
    service
        .with_maintenance("saved", "Testing reservation", false, || {
            assert!(service.play_audio("saved", None).is_err());
            assert!(service.audio_waveform("saved", 8).is_err());
            assert!(
                service
                    .start("Blocked".into(), CaptureMode::Remote, true, None)
                    .is_err()
            );
            assert!(service.preview_cleanup().is_err());
            Ok(())
        })
        .unwrap();
    assert_eq!(sink.dropped.load(Ordering::SeqCst), 0);
    let result: Result<()> =
        service.with_maintenance("", "Panic fixture", false, || panic!("synthetic"));
    assert!(result.is_err());
    assert!(!service.runtime.lock().unwrap().processing);
    assert!(service.runtime.lock().unwrap().processing_view.is_none());
    service.stop_audio().unwrap();
}
#[test]
fn confirmed_deletion_drops_output_and_preserves_exports_and_receipt() {
    let (_temp, service) = fixture();
    let sink = listening(&service);
    fs::create_dir(service.root().join("exports")).unwrap();
    let exported = service.root().join("exports/synthetic.txt");
    fs::write(&exported, b"previous export").unwrap();
    let preview = service
        .preview_deletion("saved", DeletionKind::Audio)
        .unwrap();
    assert_eq!(
        sink.dropped.load(Ordering::SeqCst),
        0,
        "Preview must not interrupt listening"
    );
    let report = service.commit_deletion(&preview.token).unwrap();
    assert_eq!(report.audio_purged, 1);
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert!(sink.dropped.load(Ordering::SeqCst) > 0);
    assert!(service.playback.lock().unwrap().is_none());
    assert_eq!(fs::read(exported).unwrap(), b"previous export");
    assert_eq!(service.last_maintenance_report().unwrap().audio_purged, 1);
    assert!(!retention::audio_available(service.root(), "saved").unwrap());
}
#[test]
fn changed_policy_invalidates_preview_and_automatic_cleanup_respects_listening() {
    let (_temp, service) = fixture();
    let policy = MaintenancePolicy {
        audio_keep_days: Some(1),
        ..Default::default()
    };
    service.save_maintenance_policy(policy).unwrap();
    store::open(service.root())
        .unwrap()
        .execute(
            "UPDATE app_meetings SET started_at='2020-01-01T00:00:00Z' WHERE id='saved'",
            [],
        )
        .unwrap();
    let preview = service.preview_cleanup().unwrap();
    assert_eq!(preview.actions.len(), 1);
    service
        .save_maintenance_policy(MaintenancePolicy {
            cleanup_obsolete_runs: false,
            ..Default::default()
        })
        .unwrap();
    assert!(service.apply_cleanup(preview).is_err());
    assert!(retention::audio_available(service.root(), "saved").unwrap());
    service
        .save_maintenance_policy(MaintenancePolicy {
            audio_keep_days: Some(1),
            ..Default::default()
        })
        .unwrap();
    let sink = listening(&service);
    service.automatic_maintenance().unwrap();
    assert_eq!(sink.dropped.load(Ordering::SeqCst), 0);
    assert!(retention::audio_available(service.root(), "saved").unwrap());
    service.stop_audio().unwrap();
    service.automatic_maintenance().unwrap();
    assert!(!retention::audio_available(service.root(), "saved").unwrap());
    assert_eq!(service.last_maintenance_report().unwrap().audio_purged, 1);
}

#[test]
fn failed_automatic_sweeps_back_off_until_an_hour_has_passed() {
    let (_temp, service) = fixture();
    service
        .save_maintenance_policy(MaintenancePolicy {
            audio_keep_days: Some(1),
            ..Default::default()
        })
        .unwrap();
    let db = store::open(service.root()).unwrap();
    db.execute(
        "UPDATE app_meetings SET started_at='2020-01-01T00:00:00Z' WHERE id='saved'",
        [],
    )
    .unwrap();
    db.execute("CREATE TABLE unhandled_child(id TEXT PRIMARY KEY, meeting_id TEXT REFERENCES app_meetings(id))", []).unwrap();
    service.automatic_maintenance().unwrap();
    assert!(!service.last_maintenance_report().unwrap().errors.is_empty());
    assert!(retention::audio_available(service.root(), "saved").unwrap());
    db.execute("DROP TABLE unhandled_child", []).unwrap();
    // A repaired underlying error does not trigger another heavy inventory on
    // the next minute tick. Manual cleanup remains independently available.
    service.automatic_maintenance().unwrap();
    assert!(retention::audio_available(service.root(), "saved").unwrap());
    save(
        service.root(),
        LAST_ATTEMPT_KEY,
        &(Utc::now() - chrono::Duration::seconds(3601)),
    )
    .unwrap();
    service.automatic_maintenance().unwrap();
    assert!(!retention::audio_available(service.root(), "saved").unwrap());
    assert!(service.last_maintenance_report().unwrap().errors.is_empty());
}
#[test]
fn a_new_audio_policy_can_run_at_the_next_idle_check() {
    let (_temp, service) = fixture();
    store::open(service.root())
        .unwrap()
        .execute(
            "UPDATE app_meetings SET started_at='2020-01-01T00:00:00Z' WHERE id='saved'",
            [],
        )
        .unwrap();
    service.automatic_maintenance().unwrap();
    assert!(retention::audio_available(service.root(), "saved").unwrap());
    service
        .save_maintenance_policy(MaintenancePolicy {
            audio_keep_days: Some(1),
            ..Default::default()
        })
        .unwrap();
    service.automatic_maintenance().unwrap();
    assert!(!retention::audio_available(service.root(), "saved").unwrap());
}

#[test]
fn rechecking_an_already_confirmed_operation_retains_visible_pending_status() {
    let (_temp, service) = fixture();
    let preview = service
        .preview_deletion("saved", DeletionKind::Audio)
        .unwrap();
    // This is the durable state after a crash at the confirmed-before-move
    // checkpoint. The core's parent/child tests also exercise that actual crash.
    store::open(service.root())
        .unwrap()
        .execute(
            "UPDATE app_retention_intents SET state='confirmed' WHERE token=?1",
            [&preview.token],
        )
        .unwrap();
    assert!(service.commit_deletion(&preview.token).is_err());
    let report = service.last_maintenance_report().unwrap();
    assert_eq!(report.pending_cleanup, vec![preview.token]);
    assert!(!report.errors.is_empty());
    let (ready, total): (i64, i64) = store::open(service.root())
        .unwrap()
        .query_row(
            "SELECT sum(state='ready'),count(*) FROM audio_chunks WHERE meeting_id='saved'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(ready, total);
    assert!(total > 0);
    assert!(service.play_audio("saved", None).is_err());
}

#[test]
fn biometric_expiry_runs_at_startup_and_idle_even_when_memory_is_off_and_no_key_is_readable() {
    let (temp, service) = fixture();
    let root = service.root().to_owned();
    let helper = temp.path().join("unused-helper");
    let db = store::open(&root).unwrap();
    // Deliberately unreadable fixture payload/key-check: expiry must delete by
    // schedule without needing a secret or attempting biometric inference.
    db.execute_batch("INSERT INTO app_people(id,display_name,is_self,created_at) VALUES ('expiry-person','Expiry fixture',0,'2020-01-01T00:00:00Z');
        INSERT INTO app_biometric_consents(id,person_id,attested_at,method,attested_by,is_self) VALUES ('expiry-consent','expiry-person','2020-01-01T00:00:00Z','Fixture only','Fixture operator',0);
        UPDATE app_voice_memory_settings SET key_check=x'00' WHERE singleton=1;").unwrap();
    let insert = |id: &str, expires: &str| {
        db.execute("INSERT INTO app_voiceprints(id,person_id,channel_type,channel_provenance,embedding_version,exemplars,n_exemplars,last_matched_at,expires_at,updated_at) VALUES (?1,'expiry-person','remote_wideband','user_declared',?2,x'00',1,'2020-01-01T00:00:00Z',?3,'2020-01-01T00:00:00Z')",rusqlite::params![id,id.repeat(64),expires]).unwrap();
    };
    insert("a", "2020-02-01T00:00:00.000Z");
    insert("b", "2999-02-01T00:00:00.000Z");
    drop(service);
    let service = AppService::new(root.clone(), helper).unwrap();
    assert!(!vt_speakers::settings(&db).unwrap().enabled);
    assert_eq!(
        db.query_row("SELECT COUNT(*) FROM app_voiceprints", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    insert("c", "2020-02-01T00:00:00.000Z");
    service.automatic_maintenance().unwrap();
    assert_eq!(
        db.query_row("SELECT COUNT(*) FROM app_voiceprints", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        db.query_row("SELECT COUNT(*) FROM app_people", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        db.query_row("SELECT COUNT(*) FROM app_biometric_consents", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    let status = service.voice_memory_status().unwrap();
    assert!(!status.enabled);
    assert_eq!(status.people[0].voiceprints.len(), 1);
}
