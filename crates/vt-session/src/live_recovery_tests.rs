//! Live speech worker recovery, truthful live state and capture diagnostics,
//! with a fake recognizer and a synthetic capture helper (no models, devices
//! or real capture).
use super::*;
use crate::{
    CaptureMode, Current, LiveState, LiveStopReason, RecordingView,
    backlog::{self, FakeAccept, TestWorkers},
    live_transcript::TranscriptWriter,
};
use std::{
    fs,
    path::Path,
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Instant,
};
use tempfile::TempDir;

const TEST_BUDGET: store::LiveSaveBudget = store::LiveSaveBudget {
    busy_timeout: Duration::from_millis(20),
    attempts: 2,
    backoff: Duration::from_millis(5),
};

fn wait(mut condition: impl FnMut() -> bool) {
    let until = Instant::now() + Duration::from_secs(15);
    while !condition() {
        assert!(Instant::now() < until, "live recovery fixture timed out");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn line(track: TrackKind, start_ms: u64, end_ms: u64, text: &str) -> TranscriptEvent {
    TranscriptEvent {
        track,
        start_ms,
        end_ms,
        text: text.into(),
        is_final: true,
        words: vec![vt_stt::WordTiming {
            text: text.split(' ').next().unwrap().into(),
            start_ms,
            end_ms,
        }],
    }
}

/// One final line per audio packet, except where `fail` returns an error.
fn lines_except(fail: impl Fn(u64) -> Option<String> + Send + Sync + 'static) -> Arc<FakeAccept> {
    Arc::new(move |_, message| match message {
        Inference::Audio {
            track, start_ms, ..
        } => match fail(*start_ms) {
            Some(cause) => Err(anyhow::anyhow!(cause)),
            None => Ok(vec![line(
                *track,
                *start_ms,
                start_ms + 900,
                &format!("Line {start_ms}"),
            )]),
        },
        Inference::Reset(..) => Ok(vec![]),
    })
}

struct Fixture {
    _temp: TempDir,
    app: Arc<AppService>,
    run: String,
    tx: Option<mpsc::SyncSender<Inference>>,
    worker: Option<std::thread::JoinHandle<Result<()>>>,
    writer: Option<TranscriptWriter>,
}
impl Fixture {
    /// The real speech worker (thread wrapper, recovery, writer) with a fake
    /// recognizer, and a current recording view to observe.
    fn start(accept: Arc<FakeAccept>) -> Self {
        let temp = TempDir::new().unwrap();
        let app = AppService::new(temp.path().join("data"), "missing-helper".into()).unwrap();
        vt_record::Recording::create(&app.root, "live")
            .unwrap()
            .finish()
            .unwrap();
        store::create_meeting(&app.root, "live", "Synthetic live recovery", "remote").unwrap();
        let run = store::detail(&app.root, "live")
            .unwrap()
            .active_run_id
            .unwrap();
        *app.test_workers.lock().unwrap() = Some(Arc::new(TestWorkers {
            stt: true,
            speakers: false,
            accept,
            speaker_turns: None,
            live_save: TEST_BUDGET,
        }));
        app.backlog.lock().unwrap().configure(true, false);
        let (control, _) = mpsc::channel();
        app.runtime.lock().unwrap().current = Some(Current {
            view: RecordingView {
                id: "live".into(),
                title: "Synthetic".into(),
                status: "recording".into(),
                elapsed_ms: 0,
                mic_level: 0.,
                system_level: 0.,
                gaps: 0,
                transcription_configured: true,
                transcription_active: false,
                live_transcription: None,
                capture: Default::default(),
                error: None,
            },
            control,
        });
        let writer =
            TranscriptWriter::start(app.clone(), "live".into(), run.clone(), TEST_BUDGET).unwrap();
        let sink = writer.sink();
        let (tx, rx) = mpsc::sync_channel(600);
        let worker_app = app.clone();
        let worker = spawn_inference_worker(
            app.clone(),
            backlog::Worker::Stt,
            "speech-inference",
            move || infer_worker(worker_app, "live".into(), rx, sink),
        );
        Self {
            _temp: temp,
            app,
            run,
            tx: Some(tx),
            worker,
            writer: Some(writer),
        }
    }
    /// One second of system audio at `start` ms.
    fn send(&self, start: u64) {
        let generation =
            self.app
                .backlog
                .lock()
                .unwrap()
                .raw(TrackKind::System, start, start + 1000, 0, 0);
        let _ = self.tx.as_ref().unwrap().try_send(Inference::Audio {
            track: TrackKind::System,
            samples: vec![0.; 16_000],
            rate: 16_000,
            start_ms: start,
            generation,
        });
    }
    fn finish(&mut self) -> Result<()> {
        self.tx.take();
        let result = self.worker.take().unwrap().join().unwrap();
        self.writer.take().unwrap().finish();
        result
    }
    fn saved(&self) -> Vec<(u64, u64, String)> {
        let db = store::open(&self.app.root).unwrap();
        let mut query = db
            .prepare(
                "SELECT start_ms,end_ms,text FROM app_run_segments WHERE run_id=?1 ORDER BY start_ms",
            )
            .unwrap();
        query
            .query_map([&self.run], |row| {
                Ok((
                    row.get::<_, i64>(0)? as u64,
                    row.get::<_, i64>(1)? as u64,
                    row.get(2)?,
                ))
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    }
    fn report(&self) -> backlog::BacklogReport {
        self.app.backlog.lock().unwrap().report().1
    }
    fn view(&self) -> RecordingView {
        self.app.refresh_backlog();
        self.app.snapshot().unwrap().recording.unwrap()
    }
}

#[test]
fn a_recognizer_error_is_recorded_as_a_gap_and_cause_and_later_audio_is_still_transcribed() {
    let mut fixture = Fixture::start(lines_except(|start| {
        (start == 2_000).then(|| "synthetic decode fault".into())
    }));
    for second in 0..6 {
        fixture.send(second * 1000);
    }
    fixture.finish().unwrap();
    let starts = fixture.saved().iter().map(|s| s.0).collect::<Vec<_>>();
    assert_eq!(starts, vec![0, 1_000, 3_000, 4_000, 5_000]);
    let report = fixture.report();
    assert!(
        report.gaps.iter().any(|g| g.worker == "stt"
            && g.reason == RECOGNIZER_ERROR_GAP
            && (g.start_ms, g.end_ms) == (2_000, 3_000)),
        "{:?}",
        report.gaps
    );
    assert!(report.gaps.iter().all(|g| g.reason != "worker_failed"));
    assert_eq!(report.failures.len(), 1);
    let failure = &report.failures[0];
    assert_eq!(
        (
            failure.worker.as_str(),
            failure.source_ms,
            failure.terminal,
            failure.count
        ),
        ("stt", 2_000, false, 1)
    );
    assert!(failure.cause.contains("synthetic decode fault"));
    assert!(report.warnings.iter().any(|w| w == STT_RECOVERED_WARNING));
}

#[test]
fn the_same_error_twice_in_a_row_is_terminal_with_a_failure_entry_and_a_stopped_live_state() {
    let mut fixture = Fixture::start(lines_except(|start| {
        (start >= 2_000).then(|| "synthetic persistent fault".into())
    }));
    for second in 0..2 {
        fixture.send(second * 1000);
    }
    wait(|| fixture.saved().len() == 2);
    assert!(fixture.view().transcription_active);
    assert_eq!(
        fixture.view().live_transcription.unwrap().state,
        LiveState::Active
    );
    for second in 2..6 {
        fixture.send(second * 1000);
    }
    let error = fixture.finish().unwrap_err();
    assert!(error.to_string().contains("synthetic persistent fault"));
    let report = fixture.report();
    let failures = report
        .failures
        .iter()
        .map(|f| (f.source_ms, f.terminal, f.count))
        .collect::<Vec<_>>();
    assert_eq!(failures[0], (2_000, false, 1));
    assert_eq!((failures[1].1, failures[1].2), (true, 2));
    assert!(
        report.failures[1]
            .cause
            .contains("synthetic persistent fault")
    );
    assert!(
        report
            .gaps
            .iter()
            .any(|g| g.worker == "stt" && g.reason == "worker_failed")
    );
    let view = fixture.view();
    assert!(!view.transcription_active);
    let live = view.live_transcription.unwrap();
    assert_eq!(live.state, LiveState::Stopped);
    assert_eq!(live.reason, Some(LiveStopReason::WorkerFailed));
    assert!(live.since_source_ms.is_some());
    assert!(
        view.error
            .unwrap()
            .contains(backlog::live_stopped_warning(LiveStopReason::WorkerFailed))
    );
    assert_eq!(fixture.saved().len(), 2);
}

#[test]
fn distinct_errors_stop_live_transcription_once_the_recovery_budget_is_spent() {
    let mut fixture = Fixture::start(lines_except(|start| {
        (start >= 1_000).then(|| format!("synthetic fault at {start}"))
    }));
    for second in 0..u64::from(STT_RECOVERY_BUDGET) + 4 {
        fixture.send(second * 1000);
    }
    let error = fixture.finish().unwrap_err();
    assert!(error.downcast_ref::<backlog::RecoveryExhausted>().is_some());
    let report = fixture.report();
    let latest = report.failures.last().unwrap();
    assert!(latest.terminal);
    assert_eq!(latest.count, u64::from(STT_RECOVERY_BUDGET) + 1);
    assert_eq!(report.failures.len(), 2);
    // Adjacent per-packet gaps merge into one entry.
    let recovered = report
        .gaps
        .iter()
        .filter(|g| g.reason == RECOGNIZER_ERROR_GAP)
        .collect::<Vec<_>>();
    assert_eq!(recovered.len(), 1);
    assert_eq!(
        (recovered[0].start_ms, recovered[0].end_ms),
        (1_000, 1_000 + 1_000 * u64::from(STT_RECOVERY_BUDGET))
    );
    assert!(
        report
            .gaps
            .iter()
            .any(|g| g.reason == "recovery_budget_exhausted")
    );
    assert_eq!(
        fixture.view().live_transcription.unwrap().reason,
        Some(LiveStopReason::RecoveryBudgetExhausted)
    );
}

#[test]
fn an_engine_failure_rebuilds_the_engine_and_recognition_continues() {
    let mut fixture = Fixture::start(Arc::new(|_, message| match message {
        Inference::Audio {
            start_ms: 1_000, ..
        } => Err(vt_stt::SttError::Engine.into()),
        Inference::Audio {
            track, start_ms, ..
        } => Ok(vec![line(*track, *start_ms, start_ms + 900, "Line")]),
        Inference::Reset(..) => Ok(vec![]),
    }));
    for second in 0..4 {
        fixture.send(second * 1000);
    }
    fixture.finish().unwrap();
    assert_eq!(
        fixture.saved().iter().map(|s| s.0).collect::<Vec<_>>(),
        vec![0, 2_000, 3_000]
    );
    assert!(!fixture.report().failures[0].terminal);
}

#[test]
fn a_source_reset_saves_the_pending_hypothesis_once_and_never_past_the_gap() {
    // The fake recognizer finalizes "Already final" at 1 s; at the reset it
    // returns that line again plus the unfinished hypothesis, whose raw end
    // runs past the gap start (2 s).
    let pending = Arc::new(Mutex::new(false));
    let state = pending.clone();
    let mut fixture = Fixture::start(Arc::new(move |_, message| {
        let mut fed = state.lock().unwrap();
        Ok(match message {
            Inference::Audio {
                track: TrackKind::System,
                start_ms: 1_000,
                ..
            } => {
                *fed = true;
                vec![line(TrackKind::System, 500, 1_500, "Already final")]
            }
            Inference::Audio { start_ms, .. } => {
                // Only the audio before the gap leaves a hypothesis.
                *fed |= *start_ms < 2_000;
                vec![]
            }
            Inference::Reset(TrackKind::System, _) if std::mem::take(&mut *fed) => vec![
                line(TrackKind::System, 500, 1_500, "Already final"),
                line(TrackKind::System, 1_500, 99_000, "Pending hypothesis"),
            ],
            Inference::Reset(..) => vec![],
        })
    }));
    fixture.send(0);
    fixture.send(1_000);
    wait(|| fixture.saved().len() == 1);
    // Capture gap: audio resumes at 10 s in a new generation.
    fixture
        .app
        .backlog
        .lock()
        .unwrap()
        .reset("source_gap", false);
    fixture.send(10_000);
    wait(|| fixture.saved().len() == 2);
    // A later reset with nothing pending saves nothing more.
    fixture
        .app
        .backlog
        .lock()
        .unwrap()
        .reset("source_gap", false);
    fixture.send(20_000);
    fixture.finish().unwrap();
    let saved = fixture.saved();
    assert_eq!(
        saved,
        vec![
            (500, 1_500, "Already final".into()),
            (1_500, 2_000, "Pending hypothesis".into()),
        ],
        "{saved:?}"
    );
    let words: String = store::open(&fixture.app.root)
        .unwrap()
        .query_row(
            "SELECT words_json FROM app_run_segments WHERE run_id=?1 AND text='Pending hypothesis'",
            [&fixture.run],
            |r| r.get(0),
        )
        .unwrap();
    assert!(words.contains("\"end_ms\":2000"), "{words}");
}

#[test]
fn the_final_echo_save_survives_a_briefly_held_lock_and_is_not_fatal_under_a_long_one() {
    for hold in [Duration::from_millis(150), Duration::from_secs(3)] {
        let temp = TempDir::new().unwrap();
        let app = AppService::new(temp.path().join("data"), "missing-helper".into()).unwrap();
        vt_record::Recording::create(&app.root, "echo")
            .unwrap()
            .finish()
            .unwrap();
        store::create_meeting(&app.root, "echo", "Synthetic echo save", "remote").unwrap();
        let run = store::detail(&app.root, "echo")
            .unwrap()
            .active_run_id
            .unwrap();
        // 4 x 100 ms + backoff: outlasts 150 ms, not 3 s.
        *app.test_workers.lock().unwrap() = Some(Arc::new(TestWorkers {
            stt: false,
            speakers: false,
            accept: Arc::new(|_, _| Ok(vec![])),
            speaker_turns: None,
            live_save: store::LiveSaveBudget {
                busy_timeout: Duration::from_millis(100),
                attempts: 4,
                backoff: Duration::from_millis(10),
            },
        }));
        app.backlog.lock().unwrap().configure(true, false);
        let (tx, rx) = mpsc::sync_channel(4);
        drop(tx);
        let blocker = store::open(&app.root).unwrap();
        blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
        let release = std::thread::spawn(move || {
            std::thread::sleep(hold);
            blocker.execute_batch("ROLLBACK").unwrap();
        });
        crate::echo_pipeline::worker(app.clone(), "echo".into(), rx, None, None).unwrap();
        release.join().unwrap();
        let saved = crate::echo_store::info(&store::open(&app.root).unwrap(), &run).unwrap();
        let warned = app
            .backlog
            .lock()
            .unwrap()
            .report()
            .1
            .warnings
            .iter()
            .any(|w| w.starts_with(crate::echo_pipeline::ECHO_SAVE_BUSY_WARNING));
        if hold < Duration::from_secs(1) {
            assert!(saved.is_some() && !warned);
        } else {
            assert!(saved.is_none() && warned);
        }
    }
}

#[test]
fn refine_publication_keeps_the_live_failure_cause_in_the_live_run_report() {
    let temp = TempDir::new().unwrap();
    let app = AppService::new(temp.path().join("data"), "missing-helper".into()).unwrap();
    vt_record::Recording::create(&app.root, "kept")
        .unwrap()
        .finish()
        .unwrap();
    store::create_meeting(&app.root, "kept", "Synthetic failure history", "remote").unwrap();
    let live = store::detail(&app.root, "kept")
        .unwrap()
        .active_run_id
        .unwrap();
    {
        let mut c = app.backlog.lock().unwrap();
        c.configure(true, false);
        c.worker_exit(
            backlog::Worker::Stt,
            "speech-inference",
            &Err(anyhow::anyhow!("synthetic fatal speech fault")),
        );
    }
    backlog::AuditWriter::start(app.clone(), "kept".into(), live.clone())
        .unwrap()
        .finish();
    store::complete_live_run(&app.root, "kept").unwrap();
    store::set_status(&app.root, "kept", "ready", Some("Transcription stopped")).unwrap();
    // Refine clears the meeting error and publishes a new run.
    store::set_status(&app.root, "kept", "processing", None).unwrap();
    let refined = store::create_run(&app.root, "kept", "refine", "{}").unwrap();
    store::publish_run(&app.root, "kept", &refined.id).unwrap();
    let detail = store::detail_run(&app.root, "kept", &live).unwrap();
    assert!(detail.meeting.error.is_none());
    let failure = &detail.inference_health.unwrap().failures[0];
    assert!(failure.terminal && failure.cause.contains("synthetic fatal speech fault"));
}

struct Resources(AtomicU64);
impl crate::health::ResourceProbe for Resources {
    fn free_bytes(&self, _: &Path) -> Result<u64> {
        Ok(4_000_000_000)
    }
    fn uptime_ns(&self) -> Option<u64> {
        None
    }
    fn tick_ns(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
}

/// A synthetic helper. It emits `diagnostics.json` lines (if present) after
/// the formats, then 20 ms packets up to the count in `target`.
#[cfg(unix)]
fn helper(temp: &TempDir) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = temp.path().join("fake-helper");
    fs::write(&path, r#"#!/usr/bin/env python3
import json,pathlib,select,sys,time
folder=pathlib.Path(__file__).parent
def emit(value): print(json.dumps(value),flush=True)
for track in ['mic','system']:
    emit({'type':'format','track':track,'sample_rate':16000,'channels':1,'epoch':0,'start_ns':0})
try:
    for diagnostic in json.loads((folder/'diagnostics.json').read_text()):
        emit({'type':'status','message':'','diagnostic':diagnostic})
except FileNotFoundError: pass
packets=0
while True:
    if select.select([sys.stdin],[],[],0)[0]:
        if sys.stdin.readline().strip()=='stop': break
    try: target=int((folder/'target').read_text())
    except (FileNotFoundError,ValueError): target=0
    if packets<target:
        for track in ['system','mic']:
            emit({'type':'audio','track':track,'epoch':0,'first_frame':packets*320,'host_time_ns':packets*20000000,'samples':[0.25 if track=='mic' else -0.125]*320})
        packets+=1
    else: time.sleep(0.002)
"#).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    path
}
#[cfg(unix)]
struct Session {
    temp: TempDir,
    app: Arc<AppService>,
    id: String,
}
#[cfg(unix)]
impl Session {
    fn start(workers: TestWorkers, diagnostics: Option<serde_json::Value>) -> Self {
        let temp = TempDir::new().unwrap();
        if let Some(diagnostics) = diagnostics {
            fs::write(
                temp.path().join("diagnostics.json"),
                diagnostics.to_string(),
            )
            .unwrap();
        }
        let resources = Arc::new(Resources(AtomicU64::new(0)));
        let app =
            AppService::with_resources(temp.path().join("data"), helper(&temp), resources).unwrap();
        *app.test_workers.lock().unwrap() = Some(Arc::new(workers));
        app.runtime.lock().unwrap().model.installed = true;
        let id = app
            .start("Synthetic session".into(), CaptureMode::Remote, true, None)
            .unwrap()
            .recording
            .unwrap()
            .id;
        Self { temp, app, id }
    }
    fn feed(&self, packets: u64) {
        fs::write(self.temp.path().join("target"), packets.to_string()).unwrap();
    }
    fn stop_and_wait(&self) {
        let _ = self.app.stop();
        wait(|| {
            store::detail(&self.app.root, &self.id)
                .unwrap()
                .runs
                .iter()
                .all(|r| r.status != "running")
        });
        wait(|| {
            store::detail(&self.app.root, &self.id)
                .unwrap()
                .meeting
                .status
                != "recording"
        });
    }
    fn live_run(&self) -> String {
        store::open(&self.app.root)
            .unwrap()
            .query_row(
                "SELECT id FROM app_processing_runs WHERE meeting_id=?1 AND kind='live'",
                [&self.id],
                |r| r.get(0),
            )
            .unwrap()
    }
}
#[cfg(unix)]
impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.app.stop();
        self.app.wait_for_shutdown();
    }
}

/// The real failure sequence: the speaker worker dies early, the speech worker
/// much later. The audio stage then stops, and it must name the speech
/// worker's failure, not "resource protection".
#[cfg(unix)]
#[test]
fn speaker_death_then_a_later_speech_stop_reports_the_true_reason() {
    let session = Session::start(
        TestWorkers {
            stt: true,
            speakers: true,
            accept: Arc::new(|worker, message| match (worker, message) {
                (backlog::Worker::Speakers, Inference::Audio { .. }) => {
                    Err(anyhow::anyhow!("synthetic speaker fault"))
                }
                (backlog::Worker::Stt, Inference::Audio { start_ms, .. }) if *start_ms >= 1_000 => {
                    Err(anyhow::anyhow!("synthetic speech fault"))
                }
                _ => Ok(vec![]),
            }),
            speaker_turns: None,
            live_save: TEST_BUDGET,
        },
        None,
    );
    session.feed(150);
    wait(|| {
        session.app.snapshot().unwrap().recording.is_some_and(|r| {
            r.live_transcription
                .is_some_and(|live| live.state == LiveState::Stopped)
        })
    });
    let wanted = backlog::audio_stage_stopped_warning(LiveStopReason::WorkerFailed);
    let recording = session.app.snapshot().unwrap().recording.unwrap();
    assert_eq!(recording.status, "recording");
    assert!(!recording.transcription_active);
    assert_eq!(
        recording.live_transcription.unwrap().reason,
        Some(LiveStopReason::WorkerFailed)
    );
    session.stop_and_wait();
    let detail = store::detail_run(&session.app.root, &session.id, &session.live_run()).unwrap();
    let echo = detail.echo.unwrap();
    assert!(
        echo.warnings.iter().any(|w| w == wanted),
        "{:?}",
        echo.warnings
    );
    assert!(
        echo.warnings
            .iter()
            .all(|w| !w.contains("resource protection")),
        "{:?}",
        echo.warnings
    );
    let report = detail.inference_health.unwrap();
    assert!(
        report.warnings.iter().any(|w| w == wanted),
        "{:?}",
        report.warnings
    );
    // Audio arriving after the stop is labelled with the real cause.
    assert!(
        report
            .gaps
            .iter()
            .all(|g| g.worker != "stt" || g.reason != "worker_unavailable"),
        "{:?}",
        report.gaps
    );
    for worker in ["speakers", "stt"] {
        assert!(
            report
                .failures
                .iter()
                .any(|f| f.worker == worker && f.terminal),
            "{:?}",
            report.failures
        );
    }
}

#[cfg(unix)]
#[test]
fn capture_diagnostics_from_the_helper_are_persisted_with_the_recording_and_bounded() {
    let mut diagnostics = vec![
        serde_json::json!({"kind":"epoch","epoch":1,"sample_rate":48000,"mic_channels":1,"system_channels":2,"quantum":512,"input_transport":"bluetooth"}),
        serde_json::json!({"kind":"settled","epoch":1,"settle_ns":900000000u64,"absorbed":0,"stable":true}),
        serde_json::json!({"kind":"rebuild","reason":"device_change","trigger":"notification","previous_epoch":0,"epoch":1,"trigger_age_ns":2000000000u64,"fingerprint_ns":1000000,"stop_ns":47000000000u64,"start_ns":300000000,"result":"ok"}),
        serde_json::json!({"kind":"heartbeat","epoch":1,"callbacks":10,"control_ticks":5,"queued_packets":2,"max_emit_ns":3000000,"at_ns":1}),
        serde_json::json!({"kind":"something_newer","value":1}),
    ];
    for _ in 0..40 {
        diagnostics.push(serde_json::json!({"kind":"slow_call","call":"AudioDeviceStop","duration_ns":250000000,"phase":"stop"}));
    }
    // No live inference: the health report is still kept for diagnostics.
    let session = Session::start(
        TestWorkers {
            stt: false,
            speakers: false,
            accept: Arc::new(|_, _| Ok(vec![])),
            speaker_turns: None,
            live_save: TEST_BUDGET,
        },
        Some(serde_json::Value::Array(diagnostics)),
    );
    session.feed(50);
    wait(|| {
        session
            .app
            .backlog
            .lock()
            .unwrap()
            .report()
            .1
            .capture
            .slow_calls_total
            == 40
    });
    session.stop_and_wait();
    let report = store::detail(&session.app.root, &session.id)
        .unwrap()
        .inference_health
        .unwrap();
    assert!(report.complete);
    let capture = report.capture;
    assert_eq!(capture.epochs.len(), 1);
    assert_eq!(
        capture.epochs[0].input_transport,
        vt_capture::InputTransport::Bluetooth
    );
    assert_eq!(capture.epochs[0].settle_ms, Some(900));
    assert_eq!(capture.rebuilds.len(), 1);
    assert_eq!(
        (
            capture.rebuilds[0].stop_ms,
            capture.rebuilds[0].trigger_age_ms
        ),
        (47_000, 2_000)
    );
    assert_eq!(capture.slow_calls.len(), crate::capture_health::MAX_RECORDS);
    assert_eq!(capture.slow_calls_total, 40);
    assert!(capture.truncated);
    assert_eq!(capture.heartbeats, 1);
    assert_eq!(capture.max_queued_packets, 2);
    // The frozen test clock never shows a slow loop or a stall.
    assert_eq!(capture.slow_iterations, 0);
    assert!(capture.stalls.is_empty());
    let meeting = store::detail(&session.app.root, &session.id)
        .unwrap()
        .meeting;
    assert_ne!(meeting.status, "failed", "{:?}", meeting.error);
}
