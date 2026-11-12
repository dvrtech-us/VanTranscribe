//! Full recording sessions (synthetic capture helper, fake live recognizer)
//! with the recorder thread stalled, failing or panicking, plus loop-level
//! ordering checks through a stalled recorder queue.
use super::*;
use crate::recorder_queue::{
    RECORDER_BACKLOG_WARNING, RECORDER_DRAIN_ERROR, RECORDER_DROP_WARNING, RECORDER_PANIC_ERROR,
    RECORDER_RECOVERY_PENDING, TestHook,
};
use rusqlite::params;
use std::{
    fs,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};
use tempfile::TempDir;

/// Real monotonic ticks (so loop-iteration health is a real measurement) and
/// test-controlled free bytes.
struct Resources {
    start: Instant,
    free: AtomicU64,
}
impl crate::health::ResourceProbe for Resources {
    fn free_bytes(&self, _: &Path) -> Result<u64> {
        Ok(self.free.load(Ordering::Relaxed))
    }
    fn uptime_ns(&self) -> Option<u64> {
        None
    }
    fn tick_ns(&self) -> u64 {
        self.start.elapsed().as_nanos() as u64
    }
}

/// 16 kHz mono on both tracks, 20 ms packets up to `target`; real time when
/// `paced` exists. Records the longest single blocking write to stdout (the
/// helper-side backpressure) in `emit_max`.
fn helper(temp: &TempDir) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = temp.path().join("fake-helper");
    fs::write(&path, r#"#!/usr/bin/env python3
import json,pathlib,select,sys,time
folder=pathlib.Path(__file__).parent
worst=0.0
def emit(value):
    global worst
    began=time.monotonic()
    print(json.dumps(value),flush=True)
    took=time.monotonic()-began
    if took>worst:
        worst=took
        (folder/'emit_max.tmp').write_text(repr(worst))
        (folder/'emit_max.tmp').rename(folder/'emit_max')
for track in ['mic','system']:
    emit({'type':'format','track':track,'sample_rate':16000,'channels':1,'epoch':0,'start_ns':0})
packets=0
paced_from=None
while True:
    if select.select([sys.stdin],[],[],0)[0]:
        if sys.stdin.readline().strip()=='stop': break
    try: target=int((folder/'target').read_text())
    except (FileNotFoundError,ValueError): target=0
    due=True
    if (folder/'paced').exists():
        if paced_from is None: paced_from=time.monotonic()-packets*0.02
        due=time.monotonic()-paced_from>=packets*0.02
    if packets<target and due:
        for track in ['system','mic']:
            emit({'type':'audio','track':track,'epoch':0,'first_frame':packets*320,'host_time_ns':packets*20000000,'samples':[0.25 if track=='mic' else -0.125]*320})
        packets+=1
    else: time.sleep(0.002)
"#).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    path
}

#[track_caller]
fn wait(limit: Duration, mut condition: impl FnMut() -> bool) {
    let until = Instant::now() + limit;
    while !condition() {
        assert!(Instant::now() < until, "recorder session fixture timed out");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Stalls every recorder item while set (a slow disk, from the loop's view).
fn stall_hook(stalled: Arc<AtomicBool>) -> Arc<dyn Fn(TrackKind, bool) + Send + Sync> {
    Arc::new(move |_, _| {
        while stalled.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(2));
        }
    })
}

type ManifestRow = (i64, i64, i64, i64, i64, String, Option<String>);

struct Session {
    temp: TempDir,
    app: Arc<AppService>,
    resources: Arc<Resources>,
    id: String,
    /// When the fake recognizer received each audio packet.
    accepted: Arc<Mutex<Vec<Instant>>>,
}
impl Session {
    fn start(hook: TestHook) -> Self {
        let temp = TempDir::new().unwrap();
        let resources = Arc::new(Resources {
            start: Instant::now(),
            free: AtomicU64::new(4_000_000_000),
        });
        let app =
            AppService::with_resources(temp.path().join("data"), helper(&temp), resources.clone())
                .unwrap();
        let accepted = Arc::new(Mutex::new(Vec::new()));
        let seen = accepted.clone();
        *app.test_workers.lock().unwrap() = Some(Arc::new(backlog::TestWorkers {
            stt: true,
            speakers: false,
            accept: Arc::new(move |_, message| {
                if matches!(message, Inference::Audio { .. }) {
                    seen.lock().unwrap().push(Instant::now());
                }
                Ok(vec![])
            }),
            speaker_turns: None,
            live_save: store::LiveSaveBudget::PRODUCTION,
        }));
        *app.recorder_test.lock().unwrap() = Some(hook);
        app.runtime.lock().unwrap().model.installed = true;
        let id = app
            .start(
                "Synthetic recorder session".into(),
                crate::CaptureMode::Remote,
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
            accepted,
        }
    }
    fn feed(&self, packets: u64) {
        fs::write(self.temp.path().join("target"), packets.to_string()).unwrap();
    }
    fn paced(&self) {
        fs::write(self.temp.path().join("paced"), "").unwrap();
    }
    fn emit_max(&self) -> f64 {
        fs::read_to_string(self.temp.path().join("emit_max"))
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0.0)
    }
    fn elapsed_ms(&self) -> u64 {
        self.app.snapshot().unwrap().recording.unwrap().elapsed_ms
    }
    fn recorder(&self) -> crate::RecorderView {
        self.app.snapshot().unwrap().recorder.unwrap_or_default()
    }
    fn meeting(&self) -> Meeting {
        store::detail(&self.app.root, &self.id).unwrap().meeting
    }
    #[track_caller]
    fn wait_done(&self, limit: Duration) {
        wait(limit, || {
            !matches!(self.meeting().status.as_str(), "recording" | "paused")
                && store::detail(&self.app.root, &self.id)
                    .unwrap()
                    .runs
                    .iter()
                    .all(|r| r.status != "running")
        });
    }
    fn live_run_status(&self) -> String {
        store::open(&self.app.root)
            .unwrap()
            .query_row(
                "SELECT status FROM app_processing_runs WHERE meeting_id=?1 AND kind='live'",
                [&self.id],
                |r| r.get(0),
            )
            .unwrap()
    }
    /// (seq, first_frame, frame_count, start_ns, end_ns, state, gap_reason)
    fn rows(&self, track: &str) -> Vec<ManifestRow> {
        let db = store::open(&self.app.root).unwrap();
        let mut query = db
            .prepare("SELECT seq,first_frame,frame_count,start_ns,end_ns,state,gap_reason FROM audio_chunks WHERE meeting_id=?1 AND track=?2 ORDER BY seq")
            .unwrap();
        query
            .query_map(params![self.id, track], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                ))
            })
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }
    /// Every row contiguous in time and frames, numbered from 0; returns the
    /// audio frames written.
    fn contiguous_frames(&self, track: &str) -> i64 {
        let rows = self.rows(track);
        let (mut frame, mut end, mut written) = (0, 0, 0);
        for (n, (seq, first, count, start, stop, state, _)) in rows.iter().enumerate() {
            assert_eq!(*seq, n as i64, "{track} {rows:?}");
            assert_eq!(*first, frame, "{track} {rows:?}");
            assert_eq!(*start, end, "{track} {rows:?}");
            if state != "gap" {
                assert_eq!(state, "ready", "{track} {rows:?}");
                written += count;
            }
            frame += count;
            end = *stop;
        }
        written
    }
    fn clock_seqs(&self, track: &str) -> Vec<i64> {
        let db = store::open(&self.app.root).unwrap();
        let mut query = db
            .prepare(
                "SELECT seq FROM app_audio_clock WHERE meeting_id=?1 AND track=?2 ORDER BY seq",
            )
            .unwrap();
        query
            .query_map(params![self.id, track], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }
    fn manifest_status(&self) -> String {
        store::open(&self.app.root)
            .unwrap()
            .query_row("SELECT status FROM meetings WHERE id=?1", [&self.id], |r| {
                r.get(0)
            })
            .unwrap()
    }
    fn warnings(&self) -> Vec<String> {
        self.app.backlog.lock().unwrap().report().1.warnings
    }
    fn health(&self) -> crate::capture_health::CaptureHealth {
        store::detail(&self.app.root, &self.id)
            .unwrap()
            .inference_health
            .map(|report| report.capture)
            .unwrap_or_default()
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.app.stop();
        self.app.wait_for_shutdown();
    }
}

enum Stall {
    /// Another connection holds SQLite's write lock (the recorder's manifest
    /// writes retry it inside `MANIFEST_LOCK_BUDGET`).
    ManifestLock,
    /// The recorder's file work is delayed (a slow disk).
    FileWrite,
}

/// An 8 s recorder stall during a real-time recording: the loop keeps
/// draining the helper at full rate (no helper-side blocking, no slow loop
/// iteration), live inference keeps being fed, duration keeps advancing, and
/// afterwards every chunk is written, contiguous and ready with no gap.
fn eight_second_stall(stall: Stall) {
    let stalled = Arc::new(AtomicBool::new(false));
    let channel_full = Arc::new(AtomicU64::new(0));
    let session = Session::start(TestHook {
        before: Some(stall_hook(stalled.clone())),
        channel_full: Some(channel_full.clone()),
        ..TestHook::default()
    });
    session.paced();
    session.feed(1_000_000);
    wait(Duration::from_secs(15), || session.elapsed_ms() >= 2_500);
    let blocker = match stall {
        Stall::ManifestLock => {
            let blocker = store::open(&session.app.root).unwrap();
            blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
            Some(blocker)
        }
        Stall::FileWrite => {
            stalled.store(true, Ordering::SeqCst);
            None
        }
    };
    let began = Instant::now();
    let elapsed_before = session.elapsed_ms();
    let fed_before = session.accepted.lock().unwrap().len();
    let mut max_queued = 0;
    // Longest time the shown duration did not move (the UI timer freezing).
    let mut longest_frozen = Duration::ZERO;
    let mut last = (session.elapsed_ms(), Instant::now());
    while began.elapsed() < Duration::from_secs(8) {
        max_queued = max_queued.max(session.recorder().queued_ms);
        let now = session.elapsed_ms();
        if now != last.0 {
            last = (now, Instant::now());
        }
        longest_frozen = longest_frozen.max(last.1.elapsed());
        std::thread::sleep(Duration::from_millis(20));
    }
    let elapsed_during = session.elapsed_ms() - elapsed_before;
    let fed_during = session.accepted.lock().unwrap().len() - fed_before;
    let emit_during = session.emit_max();
    let full_during = channel_full.load(Ordering::Relaxed);
    match blocker {
        Some(blocker) => blocker.execute_batch("ROLLBACK").unwrap(),
        None => stalled.store(false, Ordering::SeqCst),
    }
    wait(Duration::from_secs(30), || {
        session.recorder().queued_ms < 2_100
    });
    std::thread::sleep(Duration::from_millis(500));
    let started_stop = Instant::now();
    session.app.stop().unwrap();
    assert!(started_stop.elapsed() < Duration::from_millis(500));
    session.wait_done(Duration::from_secs(30));
    let meeting = session.meeting();
    assert_ne!(meeting.status, "failed", "{:?}", meeting.error);
    let health = session.health();
    eprintln!(
        "8 s recorder stall: duration advanced {elapsed_during} ms (longest freeze {longest_frozen:?}), {fed_during} packets fed to live inference, capture channel full {full_during} times, helper max blocking write {:.3} s, max loop iteration {} ms ({} >= 500 ms), max recorder backlog {} ms, max chunk write {} ms",
        emit_during,
        health.max_iteration_ms,
        health.slow_iterations,
        health.recorder.max_queued_ms,
        health.recorder.max_write_ms
    );
    // Real time: 8 s of audio per track arrived and was shown while stalled.
    assert!(elapsed_during >= 7_000, "{elapsed_during}");
    // 20 ms packets on two tracks: ~800 in 8 s.
    assert!(fed_during >= 600, "{fed_during}");
    // The 128-slot capture channel never filled and the helper never waited.
    assert_eq!(full_during, 0);
    assert!(emit_during < 1.0, "{emit_during}");
    assert!(
        longest_frozen < Duration::from_secs(1),
        "{longest_frozen:?}"
    );
    assert!(health.max_iteration_ms < 1_000, "{health:?}");
    assert!(max_queued >= 6_000, "{max_queued}");
    assert!(health.recorder.max_queued_ms >= 6_000, "{health:?}");
    assert_eq!(health.recorder.drops, 0);
    for track in ["mic", "system"] {
        let rows = session.rows(track);
        assert!(rows.iter().all(|r| r.5 == "ready"), "{track} {rows:?}");
        assert!(session.contiguous_frames(track) >= 10 * 16_000);
    }
    let warnings = session.warnings();
    assert_eq!(
        warnings
            .iter()
            .filter(|w| *w == RECORDER_BACKLOG_WARNING)
            .count(),
        1,
        "{warnings:?}"
    );
    assert!(!warnings.iter().any(|w| w == RECORDER_DROP_WARNING));
    assert_eq!(session.live_run_status(), "complete");
}

#[test]
fn an_eight_second_manifest_lock_does_not_stall_the_capture_loop() {
    eight_second_stall(Stall::ManifestLock);
}

#[test]
fn an_eight_second_file_write_stall_does_not_stall_the_capture_loop() {
    eight_second_stall(Stall::FileWrite);
}

/// A stall longer than the queue bound: exactly the overflowed blocks become
/// one `recorder_backlog` gap per track, everything else is intact and
/// contiguous, the warning appears once, and the recording continues.
#[test]
fn a_stall_past_the_queue_bound_becomes_one_exact_recorder_backlog_gap() {
    let stalled = Arc::new(AtomicBool::new(true));
    let session = Session::start(TestHook {
        queue_limit: Some(Duration::from_secs(3)),
        before: Some(stall_hook(stalled.clone())),
        ..TestHook::default()
    });
    // 10 s per track while stalled: [0, 2 s) is queued (being written), the
    // next 2 s block would exceed 3 s, so [2 s, 10 s) is dropped.
    session.feed(500);
    wait(Duration::from_secs(15), || {
        session.recorder().dropped_ms == 16_000
    });
    assert_eq!(
        session.meeting().status,
        "recording",
        "the recording continues"
    );
    stalled.store(false, Ordering::SeqCst);
    wait(Duration::from_secs(15), || {
        session.recorder().queued_ms == 0
    });
    // Real time from here, so the recorder keeps up within the 3 s bound.
    session.paced();
    session.feed(700);
    wait(Duration::from_secs(15), || session.elapsed_ms() >= 14_000);
    std::thread::sleep(Duration::from_millis(300));
    let view_gaps = session.app.snapshot().unwrap().recording.unwrap().gaps;
    session.app.stop().unwrap();
    session.wait_done(Duration::from_secs(30));
    assert_ne!(
        session.meeting().status,
        "failed",
        "{:?}",
        session.meeting().error
    );
    let s = 1_000_000_000i64;
    for track in ["mic", "system"] {
        assert_eq!(
            session.rows(track),
            vec![
                (0, 0, 32_000, 0, 2 * s, "ready".into(), None),
                (
                    1,
                    32_000,
                    128_000,
                    2 * s,
                    10 * s,
                    "gap".into(),
                    Some("recorder_backlog".into())
                ),
                (2, 160_000, 32_000, 10 * s, 12 * s, "ready".into(), None),
                (3, 192_000, 32_000, 12 * s, 14 * s, "ready".into(), None),
            ],
            "{track}"
        );
        assert_eq!(session.contiguous_frames(track), 6 * 16_000);
    }
    assert_eq!(view_gaps, 2);
    // Chunk-clock rows carry the loop's predicted sequence: after a drop they
    // still name exactly the manifest's audio rows.
    for track in ["mic", "system"] {
        assert_eq!(session.clock_seqs(track), vec![0, 2, 3], "{track}");
    }
    let health = session.health();
    assert_eq!(
        (health.recorder.drops, health.recorder.dropped_ms),
        (8, 16_000)
    );
    let warnings = session.warnings();
    assert_eq!(
        warnings
            .iter()
            .filter(|w| *w == RECORDER_DROP_WARNING)
            .count(),
        1,
        "{warnings:?}"
    );
}

/// Stop while the recorder is stalled: the stop returns at once, capture ends,
/// the view shows "saving audio" while the live run is still unsealed, and
/// after the stall every queued block is written and the run is sealed.
#[test]
fn stop_during_a_stall_returns_at_once_shows_saving_and_writes_everything() {
    let stalled = Arc::new(AtomicBool::new(true));
    let session = Session::start(TestHook {
        before: Some(stall_hook(stalled.clone())),
        ..TestHook::default()
    });
    session.feed(300);
    wait(Duration::from_secs(15), || {
        session.recorder().queued_ms >= 6_000
    });
    let started = Instant::now();
    session.app.stop().unwrap();
    assert!(started.elapsed() < Duration::from_millis(200));
    wait(Duration::from_secs(10), || session.recorder().saving_audio);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(
        session.app.snapshot().unwrap().recording.unwrap().status,
        "processing"
    );
    // The live run is sealed only after the recorder has finished.
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(session.live_run_status(), "running");
    assert!(session.rows("mic").is_empty());
    stalled.store(false, Ordering::SeqCst);
    session.wait_done(Duration::from_secs(30));
    assert_ne!(
        session.meeting().status,
        "failed",
        "{:?}",
        session.meeting().error
    );
    assert_eq!(session.live_run_status(), "complete");
    for track in ["mic", "system"] {
        assert_eq!(session.contiguous_frames(track), 300 * 320);
        assert!(session.rows(track).iter().all(|r| r.5 == "ready"));
    }
    assert!(!session.recorder().saving_audio);
}

/// Stop with a stall past the drain deadline: the meeting ends with the
/// explicit error, and `recover` adopts exactly what was written.
#[test]
fn stop_with_a_stall_past_the_drain_deadline_fails_explicitly_and_keeps_written_audio() {
    let stalled = Arc::new(AtomicBool::new(true));
    let session = Session::start(TestHook {
        drain_deadline: Some(Duration::from_secs(1)),
        before: Some(stall_hook(stalled.clone())),
        ..TestHook::default()
    });
    session.feed(300);
    wait(Duration::from_secs(15), || {
        session.recorder().queued_ms >= 6_000
    });
    session.app.stop().unwrap();
    wait(Duration::from_secs(10), || session.recorder().saving_audio);
    let release = {
        let stalled = stalled.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(2_500));
            stalled.store(false, Ordering::SeqCst);
        })
    };
    // A failed meeting keeps its unsealed live run, so wait on the status.
    wait(Duration::from_secs(30), || {
        session.meeting().status == "failed"
    });
    release.join().unwrap();
    let meeting = session.meeting();
    assert_eq!(meeting.status, "failed");
    assert!(
        meeting
            .error
            .as_deref()
            .is_some_and(|e| e.starts_with(RECORDER_DRAIN_ERROR)),
        "{:?}",
        meeting.error
    );
    let db = store::open(&session.app.root).unwrap();
    let manifest: String = db
        .query_row(
            "SELECT status FROM meetings WHERE id=?1",
            [&session.id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(manifest, "recovered");
    // The block in hand when the stall began was completed; the rest was lost.
    let written = session.contiguous_frames("mic") + session.contiguous_frames("system");
    assert_eq!(
        written,
        32_000,
        "{:?} {:?}",
        session.rows("mic"),
        session.rows("system")
    );
    // The queued audio that was lost is an explicit gap up to 6 s per track.
    let s = 1_000_000_000i64;
    for track in ["mic", "system"] {
        let last = session.rows(track).pop().unwrap();
        assert_eq!(
            (last.4, last.5.as_str(), last.6.as_deref()),
            (6 * s, "gap", Some("recorder_interrupted")),
            "{track}"
        );
    }
}

fn fails_promptly(hook: Arc<dyn Fn(TrackKind, bool) + Send + Sync>) -> (Meeting, Duration) {
    let session = Session::start(TestHook {
        before: Some(hook),
        ..TestHook::default()
    });
    session.paced();
    session.feed(1_000_000);
    // The first chunk is handed over at 2 s of audio.
    wait(Duration::from_secs(15), || session.elapsed_ms() >= 2_000);
    let started = Instant::now();
    wait(Duration::from_secs(15), || {
        session.meeting().status == "failed"
    });
    (session.meeting(), started.elapsed())
}

#[test]
fn a_recorder_panic_ends_the_recording_promptly_with_the_error_recorded() {
    let (meeting, took) = fails_promptly(Arc::new(|_, _| panic!("synthetic recorder fault")));
    assert_eq!(
        meeting.error.as_deref(),
        Some(format!("{RECORDER_PANIC_ERROR}: synthetic recorder fault").as_str())
    );
    assert!(took < Duration::from_secs(3), "{took:?}");
}

#[test]
fn a_fatal_recorder_io_error_ends_the_recording_promptly_with_its_text() {
    let removed = Arc::new(Mutex::new(None::<std::path::PathBuf>));
    let hook_removed = removed.clone();
    let (meeting, took) = {
        let session_root = Arc::new(Mutex::new(None::<std::path::PathBuf>));
        let root = session_root.clone();
        let hook: Arc<dyn Fn(TrackKind, bool) + Send + Sync> = Arc::new(move |_, chunk| {
            // Remove the meeting's audio folders: the next file create fails.
            if chunk && let Some(meeting) = root.lock().unwrap().clone() {
                let _ = fs::remove_dir_all(meeting.join("audio"));
                *hook_removed.lock().unwrap() = Some(meeting);
            }
        });
        let session = Session::start(TestHook {
            before: Some(hook),
            ..TestHook::default()
        });
        *session_root.lock().unwrap() = Some(session.app.root.join("meetings").join(&session.id));
        session.paced();
        session.feed(1_000_000);
        wait(Duration::from_secs(15), || session.elapsed_ms() >= 2_000);
        let started = Instant::now();
        wait(Duration::from_secs(15), || {
            session.meeting().status == "failed"
        });
        (session.meeting(), started.elapsed())
    };
    assert!(removed.lock().unwrap().is_some());
    let error = meeting.error.unwrap_or_default();
    assert!(error.contains("No such file or directory"), "{error}");
    assert!(took < Duration::from_secs(3), "{took:?}");
}

/// The free-space check counts audio still queued for the recorder: with
/// enough room for one more block but not for the queued ones, the recording
/// is stopped with the disk-space error before anything is written.
#[test]
fn the_disk_budget_counts_audio_still_queued_for_the_recorder() {
    let stalled = Arc::new(AtomicBool::new(true));
    let session = Session::start(TestHook {
        before: Some(stall_hook(stalled.clone())),
        ..TestHook::default()
    });
    session.feed(200);
    wait(Duration::from_secs(15), || {
        session.recorder().queued_ms >= 4_000
    });
    // One 2 s block needs 96,000 + 4,096 B plus the 2 MB reserve; the four
    // queued blocks need another 400,384 B.
    session.resources.free.store(2_300_000, Ordering::Relaxed);
    session.feed(300);
    // The loop has ended (the recorder drains what was queued before it).
    wait(Duration::from_secs(15), || session.recorder().saving_audio);
    stalled.store(false, Ordering::SeqCst);
    wait(Duration::from_secs(15), || {
        session.meeting().status == "failed"
    });
    assert_eq!(
        session.meeting().error.as_deref(),
        Some(
            "There is not enough available disk space to safely publish the buffered audio chunk."
        )
    );
}

/// Pause/resume, helper overflow and device-change gaps, and an epoch change
/// keep their order and contiguity through a stalled recorder queue.
#[test]
fn pause_overflow_device_change_and_epochs_keep_order_through_a_stalled_queue() {
    let temp = TempDir::new().unwrap();
    let service =
        AppService::new(temp.path().to_path_buf(), temp.path().join("unused-helper")).unwrap();
    let id = "queued-order";
    let recording = Recording::create(temp.path(), id).unwrap();
    store::create_meeting(temp.path(), id, "Queued order fixture", "remote").unwrap();
    let stalled = Arc::new(AtomicBool::new(true));
    *service.recorder_test.lock().unwrap() = Some(TestHook {
        before: Some(stall_hook(stalled.clone())),
        ..TestHook::default()
    });
    let recorder = start_recorder(&service, id, recording).unwrap();
    let s = 1_000_000_000u64;
    let mut track = TrackBuffer::new(TrackKind::Mic);
    let rate8 = StreamFormat::new(8_000, 1).unwrap();
    let accept = |track: &mut TrackBuffer, epoch, frame, host_ns, frames| {
        track
            .accept(
                SourcePosition {
                    epoch,
                    frame,
                    host_ns,
                },
                vec![0.125; frames],
                &recorder,
                &service,
                &mut false,
            )
            .unwrap();
    };
    track
        .set_format(rate8, 0, 0, &recorder, &service, &mut false)
        .unwrap();
    accept(&mut track, 0, 0, 0, 8_000);
    track.flush(&recorder, &service, &mut false).unwrap();
    // Pause (as the loop does), then audio resumes 1 s later.
    track.flush(&recorder, &service, &mut false).unwrap();
    track.resume_pending = true;
    accept(&mut track, 0, 8_000, 2 * s, 4_000);
    // Helper overflow: the source skips frames 12,000..20,000.
    accept(&mut track, 0, 20_000, 7 * s / 2, 4_000);
    // Helper gap event, then a new source epoch with a new format.
    track
        .gap_until(
            5 * s,
            GapReason::DeviceChange,
            &recorder,
            &service,
            &mut false,
        )
        .unwrap();
    track
        .set_format(
            StreamFormat::new(16_000, 1).unwrap(),
            1,
            5 * s,
            &recorder,
            &service,
            &mut false,
        )
        .unwrap();
    accept(&mut track, 1, 0, 5 * s, 8_000);
    track.flush(&recorder, &service, &mut false).unwrap();
    // Nothing has been written yet: the loop computed every position itself.
    assert!(recorder.status().queued_ns > 0);
    stalled.store(false, Ordering::SeqCst);
    recorder.finish(|_| ()).unwrap();
    let db = store::open(temp.path()).unwrap();
    let mut query = db
        .prepare("SELECT seq,epoch,first_frame,frame_count,start_ns,end_ns,state,gap_reason FROM audio_chunks WHERE meeting_id=?1 ORDER BY seq")
        .unwrap();
    let rows = query
        .query_map([id], |r| {
            Ok((
                r.get::<_, i64>(0)? as u64,
                r.get::<_, u32>(1)?,
                r.get::<_, i64>(2)? as u64,
                r.get::<_, i64>(3)? as u64,
                r.get::<_, i64>(4)? as u64,
                r.get::<_, i64>(5)? as u64,
                r.get::<_, String>(6)?,
                r.get::<_, Option<String>>(7)?,
            ))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect::<Vec<_>>();
    let ready = |seq, epoch, first, frames, start, end| {
        (
            seq,
            epoch,
            first,
            frames,
            start,
            end,
            "ready".to_string(),
            None,
        )
    };
    let gap = |seq, epoch, first, start, end, reason: &str| {
        (
            seq,
            epoch,
            first,
            0,
            start,
            end,
            "gap".to_string(),
            Some(reason.to_string()),
        )
    };
    assert_eq!(
        rows,
        vec![
            ready(0, 0, 0, 8_000, 0, s),
            gap(1, 0, 8_000, s, 2 * s, "pause"),
            ready(2, 0, 8_000, 4_000, 2 * s, 5 * s / 2),
            gap(3, 0, 12_000, 5 * s / 2, 7 * s / 2, "overflow"),
            ready(4, 0, 12_000, 4_000, 7 * s / 2, 4 * s),
            gap(5, 0, 16_000, 4 * s, 5 * s, "device_change"),
            ready(6, 1, 0, 8_000, 5 * s, 11 * s / 2),
        ]
    );
    service.wait_for_shutdown();
}

/// Quit while the recorder is still saving: shutdown keeps waiting past its
/// usual bound (shortened to 1 s here) until the queued audio is written.
#[test]
fn quitting_during_a_drain_waits_for_the_queued_audio() {
    let stalled = Arc::new(AtomicBool::new(true));
    let session = Session::start(TestHook {
        before: Some(stall_hook(stalled.clone())),
        shutdown_wait: Some(Duration::from_secs(1)),
        ..TestHook::default()
    });
    session.feed(300);
    wait(Duration::from_secs(15), || {
        session.recorder().queued_ms >= 6_000
    });
    let release = {
        let stalled = stalled.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(3));
            stalled.store(false, Ordering::SeqCst);
        })
    };
    let started = Instant::now();
    session.app.wait_for_shutdown();
    let waited = started.elapsed();
    release.join().unwrap();
    assert!(waited >= Duration::from_secs(3), "{waited:?}");
    assert_ne!(
        session.meeting().status,
        "failed",
        "{:?}",
        session.meeting().error
    );
    assert_ne!(session.meeting().status, "recording");
    for track in ["mic", "system"] {
        assert_eq!(session.contiguous_frames(track), 300 * 320);
    }
    assert_eq!(session.manifest_status(), "processing");
}

/// A recorder stuck in file I/O past the deadline, grace and abort wait: the
/// meeting fails saying recovery is pending (not silently), and when the
/// thread finally ends it recovers the manifest and records the unsaved tail.
#[test]
fn a_recorder_stuck_past_the_abort_wait_recovers_itself_when_it_ends() {
    let stalled = Arc::new(AtomicBool::new(true));
    let session = Session::start(TestHook {
        drain_deadline: Some(Duration::from_millis(500)),
        abandon_grace: Some(Duration::from_millis(300)),
        before: Some(stall_hook(stalled.clone())),
        ..TestHook::default()
    });
    session.feed(300);
    wait(Duration::from_secs(15), || {
        session.recorder().queued_ms >= 6_000
    });
    session.app.stop().unwrap();
    wait(Duration::from_secs(20), || {
        session.meeting().status == "failed"
    });
    let error = session.meeting().error.unwrap_or_default();
    assert!(error.starts_with(RECORDER_DRAIN_ERROR), "{error}");
    assert!(error.contains(RECORDER_RECOVERY_PENDING), "{error}");
    // The thread still holds the meeting: nothing could be recovered yet.
    assert_eq!(session.manifest_status(), "recording");
    stalled.store(false, Ordering::SeqCst);
    wait(Duration::from_secs(15), || {
        session.manifest_status() == "recovered"
    });
    wait(Duration::from_secs(15), || {
        ["mic", "system"].iter().all(|track| {
            session
                .rows(track)
                .last()
                .is_some_and(|r| r.6.as_deref() == Some("recorder_interrupted"))
        })
    });
    let s = 1_000_000_000i64;
    assert_eq!(
        session.rows("system"),
        vec![
            (0, 0, 32_000, 0, 2 * s, "ready".into(), None),
            (
                1,
                32_000,
                0,
                2 * s,
                6 * s,
                "gap".into(),
                Some("recorder_interrupted".into())
            ),
        ]
    );
    // The mic track had nothing written: its whole span is the gap.
    assert_eq!(
        session.rows("mic"),
        vec![(
            0,
            0,
            0,
            0,
            6 * s,
            "gap".into(),
            Some("recorder_interrupted".into())
        )]
    );
    assert_eq!(session.meeting().status, "failed");
}

/// The process died (or quit) mid-drain: startup recovery adopts what was
/// written, records the unsaved tail from the durable progress marker as a
/// `recorder_interrupted` gap with a warning, and also revisits a meeting that
/// already failed while its recorder still held the recording.
#[test]
fn startup_recovery_records_unsaved_tails_and_revisits_stuck_failed_meetings() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("data");
    let format = StreamFormat::new(8_000, 1).unwrap();
    let s = 1_000_000_000u64;
    {
        let service = AppService::new(root.clone(), temp.path().join("unused-helper")).unwrap();
        for (id, status) in [("died", "recording"), ("stuck", "failed")] {
            let mut recording = Recording::create(&root, id).unwrap();
            store::create_meeting(&root, id, "Unsaved tail fixture", "remote").unwrap();
            if status == "failed" {
                store::set_status(&root, id, "failed", Some("drain failed")).unwrap();
            }
            recording
                .write_chunk(
                    TrackKind::Mic,
                    &AudioBlock::new(
                        format,
                        FramePosition {
                            epoch: 0,
                            first_frame: 0,
                            host_time_ns: 0,
                        },
                        vec![0.1; 4_000],
                    )
                    .unwrap(),
                )
                .unwrap();
            // Dropped without finish: the process died with audio queued.
            drop(recording);
            store::save_recorder_progress(
                &root,
                id,
                &[store::TrackProgress {
                    track: TrackKind::Mic,
                    start_ns: 0,
                    end_ns: 7 * s / 2,
                    format,
                }],
                Duration::from_secs(1),
            )
            .unwrap();
        }
        service.wait_for_shutdown();
    }
    store::recover_interrupted(&root).unwrap();
    let db = store::open(&root).unwrap();
    for id in ["died", "stuck"] {
        let manifest: String = db
            .query_row("SELECT status FROM meetings WHERE id=?1", [id], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(manifest, "recovered", "{id}");
        let tail: (i64, i64, String) = db
            .query_row(
                "SELECT start_ns,end_ns,gap_reason FROM audio_chunks WHERE meeting_id=?1 AND seq=1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            tail,
            (500_000_000, 3_500_000_000, "recorder_interrupted".into())
        );
        let markers: i64 = db
            .query_row(
                "SELECT COUNT(*) FROM app_recorder_progress WHERE meeting_id=?1",
                [id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(markers, 0);
    }
    let died = store::detail(&root, "died").unwrap().meeting;
    assert_eq!(died.status, "recovered");
    assert_eq!(
        died.error.as_deref(),
        Some(store::unsaved_tail_warning(3 * s).as_str())
    );
    let stuck = store::detail(&root, "stuck").unwrap().meeting;
    assert_eq!(
        (stuck.status.as_str(), stuck.error.as_deref()),
        ("failed", Some("drain failed"))
    );
}

/// A block dropped on one track, then a device change with no queued audio
/// in between: the backlog gap, the device-change gap and the new epoch keep
/// the manifest contiguous.
#[test]
fn a_dropped_block_followed_by_a_device_change_stays_contiguous() {
    let temp = TempDir::new().unwrap();
    let service =
        AppService::new(temp.path().to_path_buf(), temp.path().join("unused-helper")).unwrap();
    let id = "drop-then-device";
    let recording = Recording::create(temp.path(), id).unwrap();
    store::create_meeting(temp.path(), id, "Drop then device fixture", "remote").unwrap();
    let stalled = Arc::new(AtomicBool::new(true));
    *service.recorder_test.lock().unwrap() = Some(TestHook {
        queue_limit: Some(Duration::from_secs(3)),
        before: Some(stall_hook(stalled.clone())),
        ..TestHook::default()
    });
    let recorder = start_recorder(&service, id, recording).unwrap();
    let s = 1_000_000_000u64;
    let mut track = TrackBuffer::new(TrackKind::Mic);
    let accept = |track: &mut TrackBuffer, epoch, frame, host_ns| {
        track
            .accept(
                SourcePosition {
                    epoch,
                    frame,
                    host_ns,
                },
                vec![0.125; 16_000],
                &recorder,
                &service,
                &mut false,
            )
            .unwrap();
    };
    let rate8 = StreamFormat::new(8_000, 1).unwrap();
    track
        .set_format(rate8, 0, 0, &recorder, &service, &mut false)
        .unwrap();
    accept(&mut track, 0, 0, 0); // [0, 2 s): queued (being written)
    accept(&mut track, 0, 16_000, 2 * s); // [2, 4 s): over 3 s, dropped
    // Device change at 5 s with nothing queued after the drop.
    track
        .set_format(rate8, 1, 5 * s, &recorder, &service, &mut false)
        .unwrap();
    accept(&mut track, 1, 0, 5 * s); // still dropping: backlog not halved
    stalled.store(false, Ordering::SeqCst);
    wait(Duration::from_secs(15), || recorder.status().queued_ns == 0);
    accept(&mut track, 1, 16_000, 7 * s); // queued again
    recorder.finish(|_| ()).unwrap();
    let db = store::open(temp.path()).unwrap();
    let mut query = db
        .prepare("SELECT seq,epoch,first_frame,frame_count,start_ns,end_ns,state,gap_reason FROM audio_chunks WHERE meeting_id=?1 ORDER BY seq")
        .unwrap();
    let rows = query
        .query_map([id], |r| {
            Ok((
                r.get::<_, i64>(0)? as u64,
                r.get::<_, u32>(1)?,
                r.get::<_, i64>(2)? as u64,
                r.get::<_, i64>(3)? as u64,
                r.get::<_, i64>(4)? as u64,
                r.get::<_, i64>(5)? as u64,
                r.get::<_, String>(6)?,
                r.get::<_, Option<String>>(7)?,
            ))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect::<Vec<_>>();
    let row = |seq, epoch, first, frames, start, end, state: &str, reason: Option<&str>| {
        (
            seq,
            epoch,
            first,
            frames,
            start,
            end,
            state.to_string(),
            reason.map(str::to_string),
        )
    };
    assert_eq!(
        rows,
        vec![
            row(0, 0, 0, 16_000, 0, 2 * s, "ready", None),
            row(
                1,
                0,
                16_000,
                16_000,
                2 * s,
                4 * s,
                "gap",
                Some("recorder_backlog")
            ),
            row(2, 0, 32_000, 0, 4 * s, 5 * s, "gap", Some("device_change")),
            row(
                3,
                1,
                0,
                16_000,
                5 * s,
                7 * s,
                "gap",
                Some("recorder_backlog")
            ),
            row(4, 1, 16_000, 16_000, 7 * s, 9 * s, "ready", None),
        ]
    );
    service.wait_for_shutdown();
}
