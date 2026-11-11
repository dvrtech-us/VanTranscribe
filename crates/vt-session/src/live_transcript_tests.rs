//! Live transcript writer coverage with a fake recognizer (no models, devices
//! or capture), plus full-session checks driven by a synthetic capture helper.
use super::*;
use crate::{
    CaptureMode,
    session::{Inference, infer_worker, spawn_inference_worker},
};
use std::{
    fs,
    path::Path,
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        mpsc,
    },
};
use tempfile::TempDir;

/// Short enough that a held write lock outlasts every retry in milliseconds.
const TEST_BUDGET: LiveSaveBudget = LiveSaveBudget {
    busy_timeout: Duration::from_millis(20),
    attempts: 2,
    backoff: Duration::from_millis(5),
};

fn wait(mut condition: impl FnMut() -> bool) {
    let until = Instant::now() + Duration::from_secs(15);
    while !condition() {
        assert!(Instant::now() < until, "live transcript fixture timed out");
        thread::sleep(Duration::from_millis(5));
    }
}

fn final_event(message: &Inference, bad_words: bool) -> Vec<vt_stt::TranscriptEvent> {
    let Inference::Audio {
        track, start_ms, ..
    } = message
    else {
        return vec![];
    };
    vec![vt_stt::TranscriptEvent {
        track: *track,
        start_ms: *start_ms,
        end_ms: start_ms + 900,
        text: format!("Synthetic line {start_ms}"),
        is_final: true,
        words: vec![vt_stt::WordTiming {
            text: "Synthetic".into(),
            start_ms: *start_ms,
            end_ms: if bad_words {
                start_ms + 5_000
            } else {
                start_ms + 900
            },
        }],
    }]
}

struct Fixture {
    _temp: TempDir,
    app: Arc<AppService>,
    run: String,
    accepted: Arc<AtomicUsize>,
    tx: Option<mpsc::SyncSender<Inference>>,
    worker: Option<thread::JoinHandle<Result<()>>>,
    writer: Option<TranscriptWriter>,
}
impl Fixture {
    /// A real speech worker loop whose fake recognizer emits one final line
    /// per second of audio; `reject_ms` lines carry an invalid word timestamp.
    fn start(budget: LiveSaveBudget, reject_ms: Option<u64>) -> Self {
        let temp = TempDir::new().unwrap();
        let app = AppService::new(temp.path().join("data"), "missing-helper".into()).unwrap();
        let recording = vt_record::Recording::create(&app.root, "live").unwrap();
        store::create_meeting(&app.root, "live", "Synthetic live transcript", "remote").unwrap();
        recording.finish().unwrap();
        let run = store::detail(&app.root, "live")
            .unwrap()
            .active_run_id
            .unwrap();
        let accepted = Arc::new(AtomicUsize::new(0));
        let counter = accepted.clone();
        *app.test_workers.lock().unwrap() = Some(Arc::new(backlog::TestWorkers {
            stt: true,
            speakers: false,
            accept: Arc::new(move |_, message| {
                if matches!(message, Inference::Audio { .. }) {
                    counter.fetch_add(1, Ordering::SeqCst);
                }
                let bad = matches!(message, Inference::Audio { start_ms, .. } if Some(*start_ms) == reject_ms);
                Ok(final_event(message, bad))
            }),
            speaker_turns: None,
            live_save: budget,
        }));
        app.backlog.lock().unwrap().configure(true, false);
        let writer =
            TranscriptWriter::start(app.clone(), "live".into(), run.clone(), budget).unwrap();
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
            accepted,
            tx: Some(tx),
            worker,
            writer: Some(writer),
        }
    }
    fn send(&self, second: u64) {
        let start = second * 1000;
        let generation =
            self.app
                .backlog
                .lock()
                .unwrap()
                .raw(TrackKind::System, start, start + 1000, 0, 0);
        // try_send: the recognizer must never make its producer wait.
        self.tx
            .as_ref()
            .unwrap()
            .try_send(Inference::Audio {
                track: TrackKind::System,
                samples: vec![0.; 16_000],
                rate: 16_000,
                start_ms: start,
                generation,
            })
            .unwrap();
    }
    fn saved(&self) -> Vec<(String, u64)> {
        let db = store::open(&self.app.root).unwrap();
        let mut query = db
            .prepare("SELECT id,start_ms FROM app_run_segments WHERE run_id=?1 ORDER BY start_ms")
            .unwrap();
        query
            .query_map([&self.run], |row| {
                Ok((row.get(0)?, row.get::<_, i64>(1)? as u64))
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    }
    fn pending(&self) -> Vec<(String, u64)> {
        self.app
            .runtime
            .lock()
            .unwrap()
            .live_finals
            .iter()
            .map(|s| (s.id.clone(), s.start_ms))
            .collect()
    }
    fn report(&self) -> backlog::BacklogReport {
        self.app.backlog.lock().unwrap().report().1
    }
    fn finish(&mut self) -> Result<()> {
        self.tx.take();
        let result = self.worker.take().unwrap().join().unwrap();
        self.writer.take().unwrap().finish();
        result
    }
}

#[test]
fn recognizer_keeps_consuming_while_the_database_is_write_locked_past_the_budget() {
    let mut fixture = Fixture::start(LiveSaveBudget::PRODUCTION, None);
    fixture.send(0);
    wait(|| fixture.saved().len() == 1);
    let first = fixture.saved();

    // Another connection holds the write lock well past the ~1 s live budget.
    let blocker = store::open(&fixture.app.root).unwrap();
    blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
    let locked = Instant::now();
    for second in 1..=20 {
        fixture.send(second);
    }
    // The recognizer consumed everything at once, without waiting for SQLite.
    wait(|| fixture.accepted.load(Ordering::SeqCst) == 21 && fixture.pending().len() == 20);
    assert!(
        locked.elapsed() < Duration::from_millis(900),
        "{:?}",
        locked.elapsed()
    );
    wait(|| {
        fixture
            .report()
            .warnings
            .iter()
            .any(|w| w.starts_with("Live transcript lines could not be saved yet"))
    });
    thread::sleep(Duration::from_millis(1_500));
    let pending = fixture.pending();
    assert_eq!(pending.len(), 20);
    assert_eq!(fixture.saved(), first);
    assert!(fixture.app.inference_allowed(backlog::Worker::Stt));

    blocker.execute_batch("ROLLBACK").unwrap();
    // Carried-forward lines are saved by the timed retry, in order, with the
    // IDs minted (and shown) before the first attempt.
    wait(|| fixture.saved().len() == 21);
    let mut expected = first.clone();
    expected.extend(pending);
    assert_eq!(fixture.saved(), expected);
    fixture.finish().unwrap();
    assert!(fixture.pending().is_empty());
    let report = fixture.report();
    assert!(report.gaps.is_empty(), "{:?}", report.gaps);
    let warning = report
        .warnings
        .iter()
        .find(|w| w.starts_with("Live transcript lines could not be saved yet"))
        .unwrap();
    assert!(warning.contains("SQLITE_BUSY"), "{warning}");
}

#[test]
fn deterministic_save_error_drops_only_that_line_as_a_gap_with_a_warning() {
    let mut fixture = Fixture::start(TEST_BUDGET, Some(1000));
    for second in 0..4 {
        fixture.send(second);
    }
    wait(|| fixture.saved().len() == 3 && fixture.pending().is_empty());
    assert_eq!(
        fixture
            .saved()
            .iter()
            .map(|(_, start)| *start)
            .collect::<Vec<_>>(),
        [0, 2000, 3000]
    );
    fixture.send(4);
    wait(|| fixture.saved().len() == 4);
    fixture.finish().unwrap();
    let report = fixture.report();
    assert_eq!(report.gaps.len(), 1, "{:?}", report.gaps);
    let gap = &report.gaps[0];
    assert_eq!(gap.worker, "stt");
    assert_eq!(gap.track, "system");
    assert_eq!(gap.reason, SEGMENT_REJECTED_GAP);
    assert_eq!((gap.start_ms, gap.end_ms), (1000, 1900));
    assert!(
        report
            .warnings
            .iter()
            .any(|w| w == &rejected_warning("Word timestamp is outside its segment.")),
        "{:?}",
        report.warnings
    );
}

#[test]
fn lines_still_unsaved_at_the_close_deadline_become_persistence_failed_gaps() {
    let mut fixture = Fixture::start(TEST_BUDGET, None);
    let blocker = store::open(&fixture.app.root).unwrap();
    blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
    fixture.send(7);
    wait(|| fixture.pending().len() == 1);
    let started = Instant::now();
    fixture.finish().unwrap();
    assert!(started.elapsed() >= CLOSE_DEADLINE);
    blocker.execute_batch("ROLLBACK").unwrap();
    assert!(fixture.saved().is_empty());
    assert!(fixture.pending().is_empty());
    let report = fixture.report();
    assert!(
        report
            .gaps
            .iter()
            .any(|g| g.reason == PERSISTENCE_FAILED_GAP && g.start_ms == 7000),
        "{:?}",
        report.gaps
    );
    assert!(report.warnings.iter().any(|w| w == DROPPED_WARNING));
}

#[test]
fn queue_overflow_turns_the_oldest_unsaved_lines_into_gaps() {
    let temp = TempDir::new().unwrap();
    let app = AppService::new(temp.path().join("data"), "missing-helper".into()).unwrap();
    let recording = vt_record::Recording::create(&app.root, "live").unwrap();
    store::create_meeting(&app.root, "live", "Synthetic overflow", "remote").unwrap();
    recording.finish().unwrap();
    let run = store::detail(&app.root, "live")
        .unwrap()
        .active_run_id
        .unwrap();
    let blocker = store::open(&app.root).unwrap();
    blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
    let writer =
        TranscriptWriter::start(app.clone(), "live".into(), run.clone(), TEST_BUDGET).unwrap();
    let sink = writer.sink();
    for n in 0..(QUEUE_LIMIT as u64 + 3) {
        let segment = Segment {
            id: format!("line-{n:04}"),
            track: "mic".into(),
            start_ms: n * 1000,
            end_ms: n * 1000 + 500,
            text: format!("Line {n}"),
            speaker: "Me".into(),
            words: vec![],
        };
        app.runtime
            .lock()
            .unwrap()
            .live_finals
            .push(segment.clone());
        sink.push(0, TrackKind::Mic, segment);
    }
    blocker.execute_batch("ROLLBACK").unwrap();
    writer.finish();
    let report = app.backlog.lock().unwrap().report().1;
    let dropped: Vec<_> = report
        .gaps
        .iter()
        .filter(|g| g.reason == PERSISTENCE_FAILED_GAP)
        .collect();
    assert!(!dropped.is_empty());
    // Overflow evicts the oldest lines not already being saved by the writer
    // (at most one batch), never the newest.
    assert!(
        dropped
            .iter()
            .all(|g| g.start_ms < (BATCH_LIMIT as u64 + 3) * 1000),
        "{dropped:?}"
    );
    assert!(report.warnings.iter().any(|w| w == DROPPED_WARNING));
    let db = store::open(&app.root).unwrap();
    let saved: i64 = db
        .query_row(
            "SELECT COUNT(*) FROM app_run_segments WHERE run_id=?1",
            [&run],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(saved as usize, QUEUE_LIMIT);
    let newest: bool = db
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM app_run_segments WHERE run_id=?1 AND id=?2)",
            params![run, format!("line-{:04}", QUEUE_LIMIT + 2)],
            |r| r.get(0),
        )
        .unwrap();
    assert!(newest);
    assert!(app.runtime.lock().unwrap().live_finals.is_empty());
}

use rusqlite::params;

// ---- Full recording sessions with a synthetic capture helper ----

const SECOND: u64 = 1_000_000_000;
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
#[cfg(unix)]
fn helper(temp: &TempDir) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = temp.path().join("fake-helper");
    fs::write(&path,r#"#!/usr/bin/env python3
import json,pathlib,select,sys,time
folder=pathlib.Path(__file__).parent
def emit(value): print(json.dumps(value),flush=True)
for track in ['mic','system']:
    emit({'type':'format','track':track,'sample_rate':16000,'channels':1,'epoch':0,'start_ns':0})
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
    resources: Arc<Resources>,
    id: String,
}
#[cfg(unix)]
impl Session {
    fn start(stt: bool, emitted: Arc<Mutex<Vec<u64>>>) -> Self {
        let temp = TempDir::new().unwrap();
        let resources = Arc::new(Resources(AtomicU64::new(0)));
        let app =
            AppService::with_resources(temp.path().join("data"), helper(&temp), resources.clone())
                .unwrap();
        *app.test_workers.lock().unwrap() = Some(Arc::new(backlog::TestWorkers {
            stt,
            speakers: false,
            // One final line per 400 ms of system audio.
            accept: Arc::new(move |_, message| match message {
                Inference::Audio {
                    track: TrackKind::System,
                    start_ms,
                    ..
                } if start_ms % 400 == 0 => {
                    emitted.lock().unwrap().push(*start_ms);
                    Ok(vec![vt_stt::TranscriptEvent {
                        track: TrackKind::System,
                        start_ms: *start_ms,
                        end_ms: start_ms + 20,
                        text: format!("Session line {start_ms}"),
                        is_final: true,
                        words: vec![],
                    }])
                }
                _ => Ok(vec![]),
            }),
            speaker_turns: None,
            live_save: TEST_BUDGET,
        }));
        app.runtime.lock().unwrap().model.installed = true;
        let id = app
            .start(
                "Synthetic live transcript session".into(),
                CaptureMode::Remote,
                true,
                None,
            )
            .unwrap()
            .recording
            .unwrap()
            .id;
        Self {
            temp,
            app,
            resources,
            id,
        }
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
}
#[cfg(unix)]
impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.app.stop();
        self.app.wait_for_shutdown();
    }
}

#[cfg(unix)]
#[test]
fn stop_drains_every_final_into_the_live_run_before_it_is_sealed() {
    let emitted = Arc::new(Mutex::new(Vec::new()));
    let session = Session::start(true, emitted.clone());
    session.feed(100);
    wait(|| emitted.lock().unwrap().len() >= 5);
    // Both tracks' first 2 s chunk is written before the lock is taken.
    wait(|| {
        store::open(&session.app.root)
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM audio_chunks WHERE meeting_id=?1 AND state='ready'",
                [&session.id],
                |r| r.get::<_, i64>(0),
            )
            .unwrap()
            >= 2
    });
    // Contention keeps lines queued when stop arrives. The lock is held for
    // well under the recorder's 5 s chunk-write budget (a chunk flush may wait
    // on it, as it would on any writer), and is released after stop.
    let blocker = store::open(&session.app.root).unwrap();
    blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
    let locked = Instant::now();
    session.feed(150);
    wait(|| !session.app.runtime.lock().unwrap().live_finals.is_empty());
    // The UI shows queued lines before they are saved, as it did when finals
    // were written synchronously.
    let queued = session
        .app
        .runtime
        .lock()
        .unwrap()
        .live_finals
        .iter()
        .map(|s| s.start_ms)
        .collect::<Vec<_>>();
    let shown = session
        .app
        .detail(&session.id)
        .unwrap()
        .segments
        .iter()
        .map(|s| s.start_ms)
        .collect::<Vec<_>>();
    assert!(
        queued.iter().all(|start| shown.contains(start)),
        "{shown:?} {queued:?}"
    );
    let stopper = {
        let app = session.app.clone();
        thread::spawn(move || {
            let _ = app.stop();
        })
    };
    thread::sleep(Duration::from_millis(500));
    assert!(
        locked.elapsed() < Duration::from_secs(4),
        "{:?}",
        locked.elapsed()
    );
    blocker.execute_batch("ROLLBACK").unwrap();
    stopper.join().unwrap();
    session.stop_and_wait();
    let db = store::open(&session.app.root).unwrap();
    let (status, run): (String, String) = db
        .query_row(
            "SELECT status,id FROM app_processing_runs WHERE meeting_id=?1 AND kind='live'",
            [&session.id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(status, "complete");
    let mut query = db
        .prepare("SELECT start_ms FROM app_run_segments WHERE run_id=?1 ORDER BY start_ms")
        .unwrap();
    let saved = query
        .query_map([&run], |r| r.get::<_, i64>(0))
        .unwrap()
        .map(|v| v.unwrap() as u64)
        .collect::<Vec<_>>();
    let mut expected = emitted.lock().unwrap().clone();
    expected.sort();
    assert!(!expected.is_empty());
    assert_eq!(saved, expected);
    let report = store::detail(&session.app.root, &session.id)
        .unwrap()
        .inference_health
        .unwrap();
    assert!(
        report
            .gaps
            .iter()
            .all(|g| g.reason != PERSISTENCE_FAILED_GAP
                && g.reason != SEGMENT_REJECTED_GAP
                && g.reason != "worker_failed"),
        "{:?}",
        report.gaps
    );
    assert!(session.app.runtime.lock().unwrap().live_finals.is_empty());
}

/// Before: a lock held past the 5 s busy timeout failed `set_duration` and
/// ended the recording. Now periodic duration and health writes use a short
/// timeout and skip on lock contention, with one warning.
#[cfg(unix)]
#[test]
fn busy_duration_and_health_writes_do_not_end_the_recording() {
    let session = Session::start(false, Arc::new(Mutex::new(Vec::new())));
    session.feed(50);
    thread::sleep(Duration::from_millis(700));
    let blocker = store::open(&session.app.root).unwrap();
    blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
    // Make the periodic health snapshot due while the lock is held.
    session.resources.0.store(31 * SECOND, Ordering::Relaxed);
    let held = Instant::now();
    wait(|| {
        session
            .app
            .backlog
            .lock()
            .unwrap()
            .report()
            .1
            .warnings
            .iter()
            .any(|w| w.starts_with(crate::session::HOUSEKEEPING_BUSY_WARNING))
    });
    // Hold past the old 5 s busy timeout so the old code would have failed.
    while held.elapsed() < Duration::from_millis(5_600) {
        thread::sleep(Duration::from_millis(50));
    }
    let status = session
        .app
        .runtime
        .lock()
        .unwrap()
        .current
        .as_ref()
        .map(|c| c.view.status.clone());
    assert_eq!(status.as_deref(), Some("recording"));
    blocker.execute_batch("ROLLBACK").unwrap();
    session.feed(150);
    thread::sleep(Duration::from_millis(500));
    session.stop_and_wait();
    let meeting = store::detail(&session.app.root, &session.id)
        .unwrap()
        .meeting;
    assert_ne!(meeting.status, "failed", "{:?}", meeting.error);
    let warnings = session.app.backlog.lock().unwrap().report().1.warnings;
    let busy = warnings
        .iter()
        .filter(|w| w.starts_with(crate::session::HOUSEKEEPING_BUSY_WARNING))
        .collect::<Vec<_>>();
    assert_eq!(busy.len(), 1, "{warnings:?}");
    assert!(busy[0].contains("SQLITE_BUSY"), "{}", busy[0]);
}
