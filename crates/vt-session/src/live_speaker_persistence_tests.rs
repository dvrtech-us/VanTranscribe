//! Synthetic live speaker worker persistence coverage. A fake speaker engine
//! emits turns through the real `LivePublisher`; no models, devices or capture.
use super::*;
use std::time::Instant;
use tempfile::TempDir;
use vt_diarize::SpeakerTurn;

/// Short enough that a held write lock outlasts every retry in milliseconds.
const TEST_BUDGET: store::LiveSaveBudget = store::LiveSaveBudget {
    busy_timeout: Duration::from_millis(20),
    attempts: 2,
    backoff: Duration::from_millis(5),
};

fn wait(mut condition: impl FnMut() -> bool) {
    let until = Instant::now() + Duration::from_secs(10);
    while !condition() {
        assert!(Instant::now() < until, "synthetic speaker worker timed out");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn synthetic_turn(message: &Inference) -> Result<Vec<SpeakerTurn>> {
    let Inference::Audio {
        track,
        samples,
        rate,
        start_ms,
        ..
    } = message
    else {
        return Ok(vec![]);
    };
    Ok(vec![SpeakerTurn {
        id: format!("{}:turn-{start_ms}", track.as_str()),
        track: track.as_str().into(),
        start_ms: *start_ms,
        end_ms: start_ms + samples.len() as u64 * 1000 / u64::from(*rate),
        speaker_ids: vec![format!("{}:a", track.as_str())],
        label: "Speaker 1".into(),
        provenance: "live-cluster".into(),
        score: Some(0.9),
        overlap: false,
        uncertain: false,
        provisional: false,
    }])
}

struct Fixture {
    _temp: TempDir,
    app: Arc<AppService>,
    run: String,
    tx: Option<mpsc::SyncSender<Inference>>,
    worker: Option<std::thread::JoinHandle<Result<()>>>,
}
impl Fixture {
    fn start(turns: Arc<backlog::FakeTurns>) -> Self {
        let temp = TempDir::new().unwrap();
        let app = AppService::new(temp.path().join("data"), "missing-helper".into()).unwrap();
        let recording = vt_record::Recording::create(&app.root, "live").unwrap();
        store::create_meeting(&app.root, "live", "Synthetic live speakers", "remote").unwrap();
        recording.finish().unwrap();
        let run = store::detail(&app.root, "live")
            .unwrap()
            .active_run_id
            .unwrap();
        *app.test_workers.lock().unwrap() = Some(Arc::new(backlog::TestWorkers {
            stt: false,
            speakers: true,
            accept: Arc::new(|_, _| Ok(vec![])),
            speaker_turns: Some(turns),
            live_save: TEST_BUDGET,
        }));
        app.backlog.lock().unwrap().configure(false, true);
        let (tx, rx) = mpsc::sync_channel(16);
        let worker_app = app.clone();
        let worker = spawn_inference_worker(
            app.clone(),
            backlog::Worker::Speakers,
            "speaker-inference",
            move || speaker_worker(worker_app, "live".into(), rx),
        );
        Self {
            _temp: temp,
            app,
            run,
            tx: Some(tx),
            worker,
        }
    }
    /// One second of system audio starting at `second`.
    fn send(&self, second: u64) {
        let start = second * 1000;
        let generation =
            self.app
                .backlog
                .lock()
                .unwrap()
                .raw(TrackKind::System, start, start + 1000, 0, 0);
        self.tx
            .as_ref()
            .unwrap()
            .send(Inference::Audio {
                track: TrackKind::System,
                samples: vec![0.; 16_000],
                rate: 16_000,
                start_ms: start,
                generation,
            })
            .unwrap();
    }
    fn saved(&self) -> Vec<String> {
        let db = store::open(&self.app.root).unwrap();
        let mut query = db
            .prepare("SELECT id FROM app_speaker_turns WHERE run_id=?1 ORDER BY start_ms")
            .unwrap();
        query
            .query_map([&self.run], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<String>>>()
            .unwrap()
    }
    fn report(&self) -> backlog::BacklogReport {
        self.app.backlog.lock().unwrap().report().1
    }
    fn finish(&mut self) -> Result<()> {
        self.tx.take();
        self.worker.take().unwrap().join().unwrap()
    }
}

#[test]
fn speaker_worker_survives_a_write_lock_longer_than_its_retry_budget_and_saves_later() {
    let mut fixture = Fixture::start(Arc::new(synthetic_turn));
    fixture.send(0);
    wait(|| fixture.saved().len() == 1);

    // Another connection holds the write lock well past the live retry budget.
    let blocker = store::open(&fixture.app.root).unwrap();
    blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
    fixture.send(1);
    wait(|| {
        fixture
            .report()
            .warnings
            .iter()
            .any(|warning| warning.starts_with("Live speaker labels could not be saved yet"))
    });
    let report = fixture.report();
    let warning = report
        .warnings
        .iter()
        .find(|warning| warning.starts_with("Live speaker labels could not be saved yet"))
        .unwrap();
    assert!(
        warning.contains("database is locked (SQLite extended code 5 SQLITE_BUSY)"),
        "{warning}"
    );
    assert!(fixture.app.inference_allowed(backlog::Worker::Speakers));
    assert!(report.gaps.iter().all(|gap| gap.reason != "worker_failed"));
    assert_eq!(fixture.saved().len(), 1);

    blocker.execute_batch("ROLLBACK").unwrap();
    // The carried-forward turn is written by the next successful save.
    fixture.send(2);
    wait(|| fixture.saved().len() == 3);
    assert_eq!(
        fixture.saved(),
        ["system:turn-0", "system:turn-1000", "system:turn-2000"]
    );
    assert!(fixture.app.inference_allowed(backlog::Worker::Speakers));
    fixture.finish().unwrap();
    let report = fixture.report();
    assert!(report.gaps.is_empty(), "{:?}", report.gaps);
    // Diagnosis survives even though warnings_json is overwritten by later saves.
    assert!(
        report
            .warnings
            .iter()
            .filter(|warning| warning.starts_with("Live speaker labels could not be saved yet"))
            .count()
            == 1
    );
}

#[test]
fn turns_still_unsaved_when_the_worker_ends_become_persistence_failed_gaps() {
    let mut fixture = Fixture::start(Arc::new(synthetic_turn));
    let blocker = store::open(&fixture.app.root).unwrap();
    blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
    fixture.send(3);
    wait(|| {
        fixture
            .report()
            .warnings
            .iter()
            .any(|warning| warning.starts_with("Live speaker labels could not be saved yet"))
    });
    fixture.finish().unwrap();
    blocker.execute_batch("ROLLBACK").unwrap();
    let report = fixture.report();
    assert!(
        report.gaps.iter().any(|gap| gap.worker == "speakers"
            && gap.track == "system"
            && gap.reason == crate::speaker_pipeline::PERSISTENCE_FAILED_GAP
            && gap.start_ms == 3000
            && gap.end_ms == 4000),
        "{:?}",
        report.gaps
    );
    assert!(report.gaps.iter().all(|gap| gap.reason != "worker_failed"));
}

#[test]
fn engine_errors_still_stop_the_speaker_worker_as_worker_failed() {
    let mut fixture = Fixture::start(Arc::new(|_: &Inference| -> Result<Vec<SpeakerTurn>> {
        anyhow::bail!("synthetic engine failure")
    }));
    fixture.send(0);
    let outcome = fixture.finish();
    assert_eq!(outcome.unwrap_err().to_string(), "synthetic engine failure");
    assert!(!fixture.app.inference_allowed(backlog::Worker::Speakers));
    let report = fixture.report();
    assert!(
        report
            .gaps
            .iter()
            .any(|gap| gap.worker == "speakers" && gap.reason == "worker_failed"),
        "{:?}",
        report.gaps
    );
    assert!(report.warnings.iter().any(|warning| warning
        == "Live inference worker speaker-inference stopped: synthetic engine failure"));
}

#[test]
fn fatal_worker_warning_carries_the_sqlite_extended_code() {
    let error = anyhow::Error::from(rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY_SNAPSHOT),
        Some("database is locked".into()),
    ));
    assert_eq!(
        worker_failure_warning("speaker-inference", &error),
        "Live inference worker speaker-inference stopped: database is locked (SQLite extended code 517 SQLITE_BUSY_SNAPSHOT)"
    );
}
