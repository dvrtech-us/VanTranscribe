//! The recorder thread on its own: a real `vt_record::Recording` in a temp
//! directory, driven through the queue with a test hook that can stall,
//! fail or panic before an item is written.
use super::*;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tempfile::TempDir;
use vt_core::FramePosition;
use vt_record::RecoveryReport;

const RATE: u32 = 8_000;
const SECOND: u64 = 1_000_000_000;

fn format() -> StreamFormat {
    StreamFormat::new(RATE, 1).unwrap()
}
/// `frames` of audio for `epoch` starting at `first` frames / `start` ns.
fn block(epoch: u32, first: u64, start: u64, frames: usize) -> AudioBlock {
    AudioBlock::new(
        format(),
        FramePosition {
            epoch,
            first_frame: first,
            host_time_ns: start,
        },
        vec![0.125; frames],
    )
    .unwrap()
}
fn no_clock() -> ClockSink {
    Box::new(|_, _| Ok(()))
}
fn no_cleanup() -> Box<dyn FnOnce() + Send> {
    Box::new(|| ())
}
/// A hook that holds every item while `stalled` is set.
fn stall_hook(stalled: Arc<AtomicBool>) -> Arc<dyn Fn(TrackKind, bool) + Send + Sync> {
    Arc::new(move |_, _| {
        while stalled.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(2));
        }
    })
}
fn config(hook: Arc<dyn Fn(TrackKind, bool) + Send + Sync>) -> Config {
    Config {
        before: Some(hook),
        ..Config::default()
    }
}
fn wait_until(mut condition: impl FnMut() -> bool) {
    let until = Instant::now() + Duration::from_secs(15);
    while !condition() {
        assert!(Instant::now() < until, "recorder queue fixture timed out");
        std::thread::sleep(Duration::from_millis(2));
    }
}
type Row = (String, u64, u32, u64, u64, u64, u64, String, Option<String>);
fn rows(root: &std::path::Path, meeting: &str) -> Vec<Row> {
    let db = rusqlite::Connection::open(root.join("vantranscribe.db")).unwrap();
    let mut query = db
        .prepare("SELECT track,seq,epoch,first_frame,frame_count,start_ns,end_ns,state,gap_reason FROM audio_chunks WHERE meeting_id=?1 ORDER BY track,seq")
        .unwrap();
    query
        .query_map([meeting], |r| {
            Ok((
                r.get(0)?,
                r.get::<_, i64>(1)? as u64,
                r.get(2)?,
                r.get::<_, i64>(3)? as u64,
                r.get::<_, i64>(4)? as u64,
                r.get::<_, i64>(5)? as u64,
                r.get::<_, i64>(6)? as u64,
                r.get(7)?,
                r.get(8)?,
            ))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect()
}
#[allow(clippy::too_many_arguments)]
fn row(
    track: &str,
    seq: u64,
    epoch: u32,
    first: u64,
    frames: u64,
    start: u64,
    end: u64,
    state: &str,
    reason: Option<&str>,
) -> Row {
    (
        track.into(),
        seq,
        epoch,
        first,
        frames,
        start,
        end,
        state.into(),
        reason.map(Into::into),
    )
}

/// Everything queued while the recorder is stalled reaches the manifest in
/// the loop's order: per-track contiguity, helper gaps and a new epoch.
#[test]
fn queued_chunks_gaps_and_epochs_reach_the_manifest_in_order() {
    let root = TempDir::new().unwrap();
    let stalled = Arc::new(AtomicBool::new(true));
    let recording = Recording::create(root.path(), "ordered").unwrap();
    let queue = RecorderQueue::start(
        recording,
        no_clock(),
        config(stall_hook(stalled.clone())),
        no_cleanup(),
    )
    .unwrap();
    let half = SECOND / 2;
    assert_eq!(
        queue
            .push_chunk(TrackKind::Mic, block(0, 0, 0, 4_000), None)
            .unwrap(),
        Admission::Queued
    );
    queue
        .push_chunk(TrackKind::System, block(0, 0, 0, 4_000), None)
        .unwrap();
    // Helper overflow on the mic, then the mic continues in epoch 0.
    queue
        .push_gap(
            TrackKind::Mic,
            format(),
            Gap {
                epoch: 0,
                first_frame: 4_000,
                frame_count: 0,
                start_ns: half,
                end_ns: SECOND,
                reason: GapReason::Overflow,
            },
        )
        .unwrap();
    queue
        .push_chunk(TrackKind::Mic, block(0, 4_000, SECOND, 4_000), None)
        .unwrap();
    // Device change on the system track: gap, then epoch 1 from frame 0.
    queue
        .push_gap(
            TrackKind::System,
            format(),
            Gap {
                epoch: 0,
                first_frame: 4_000,
                frame_count: 0,
                start_ns: half,
                end_ns: 2 * SECOND,
                reason: GapReason::DeviceChange,
            },
        )
        .unwrap();
    queue
        .push_chunk(TrackKind::System, block(1, 0, 2 * SECOND, 4_000), None)
        .unwrap();
    // Pause then resume on the mic.
    queue
        .push_gap(
            TrackKind::Mic,
            format(),
            Gap {
                epoch: 0,
                first_frame: 8_000,
                frame_count: 0,
                start_ns: SECOND + half,
                end_ns: 3 * SECOND,
                reason: GapReason::Pause,
            },
        )
        .unwrap();
    queue
        .push_chunk(TrackKind::Mic, block(0, 8_000, 3 * SECOND, 4_000), None)
        .unwrap();
    assert_eq!(queue.status().queued_ns, SECOND + half);
    stalled.store(false, Ordering::SeqCst);
    queue.finish(|_| ()).unwrap();
    assert_eq!(
        rows(root.path(), "ordered"),
        vec![
            row("mic", 0, 0, 0, 4_000, 0, half, "ready", None),
            row("mic", 1, 0, 4_000, 0, half, SECOND, "gap", Some("overflow")),
            row(
                "mic",
                2,
                0,
                4_000,
                4_000,
                SECOND,
                SECOND + half,
                "ready",
                None
            ),
            row(
                "mic",
                3,
                0,
                8_000,
                0,
                SECOND + half,
                3 * SECOND,
                "gap",
                Some("pause")
            ),
            row(
                "mic",
                4,
                0,
                8_000,
                4_000,
                3 * SECOND,
                3 * SECOND + half,
                "ready",
                None
            ),
            row("system", 0, 0, 0, 4_000, 0, half, "ready", None),
            row(
                "system",
                1,
                0,
                4_000,
                0,
                half,
                2 * SECOND,
                "gap",
                Some("device_change")
            ),
            row(
                "system",
                2,
                1,
                0,
                4_000,
                2 * SECOND,
                2 * SECOND + half,
                "ready",
                None
            ),
        ]
    );
    assert_eq!(
        Recording::recover(root.path(), "ordered").unwrap(),
        RecoveryReport {
            verified_chunks: 5,
            adopted_chunks: 0,
            gaps: 0
        }
    );
}

/// Past the bound, whole incoming blocks are dropped (queued ones are kept)
/// and one `recorder_backlog` gap covers exactly the dropped frames and time.
#[test]
fn a_full_queue_drops_whole_blocks_into_one_exact_recorder_backlog_gap() {
    let root = TempDir::new().unwrap();
    let stalled = Arc::new(AtomicBool::new(true));
    let recording = Recording::create(root.path(), "bounded").unwrap();
    let queue = RecorderQueue::start(
        recording,
        no_clock(),
        Config {
            queue_limit: Duration::from_secs(1),
            ..config(stall_hook(stalled.clone()))
        },
        no_cleanup(),
    )
    .unwrap();
    let half = SECOND / 2;
    let mut admissions = Vec::new();
    for n in 0..6u64 {
        admissions.push(
            queue
                .push_chunk(TrackKind::Mic, block(0, n * 4_000, n * half, 4_000), None)
                .unwrap(),
        );
    }
    assert_eq!(
        admissions,
        vec![
            Admission::Queued,
            Admission::Queued,
            Admission::Dropped { new_gap: true },
            Admission::Dropped { new_gap: false },
            Admission::Dropped { new_gap: false },
            Admission::Dropped { new_gap: false },
        ]
    );
    let status = queue.status();
    assert_eq!((status.drops, status.dropped_ns), (4, 2 * SECOND));
    assert_eq!(status.queued_ns, SECOND);
    assert_eq!(status.max_queued_ns, SECOND);
    // Encoded upper bound of the two queued blocks (PCM24 + per-chunk slack).
    assert_eq!(status.pending_bytes, 2 * (4_000 * 3 + 4_096));
    stalled.store(false, Ordering::SeqCst);
    wait_until(|| queue.status().queued_ns == 0);
    // The gap was written; the next block is queued and joins it exactly.
    assert_eq!(
        queue
            .push_chunk(TrackKind::Mic, block(0, 24_000, 3 * SECOND, 4_000), None)
            .unwrap(),
        Admission::Queued
    );
    queue.finish(|_| ()).unwrap();
    assert_eq!(
        rows(root.path(), "bounded"),
        vec![
            row("mic", 0, 0, 0, 4_000, 0, half, "ready", None),
            row("mic", 1, 0, 4_000, 4_000, half, SECOND, "ready", None),
            row(
                "mic",
                2,
                0,
                8_000,
                16_000,
                SECOND,
                3 * SECOND,
                "gap",
                Some("recorder_backlog")
            ),
            row(
                "mic",
                3,
                0,
                24_000,
                4_000,
                3 * SECOND,
                3 * SECOND + half,
                "ready",
                None
            ),
        ]
    );
}

/// A non-lock I/O error on the recorder thread is visible to the loop at once
/// (no join, no wait) and is the error `finish` returns.
#[test]
fn a_fatal_write_error_is_reported_without_waiting_and_ends_the_recorder() {
    let root = TempDir::new().unwrap();
    let recording = Recording::create(root.path(), "fatal").unwrap();
    let audio = root.path().join("meetings/fatal/audio/mic");
    let queue = RecorderQueue::start(
        recording,
        no_clock(),
        config(Arc::new(move |_, _| {
            let _ = std::fs::remove_dir_all(&audio);
        })),
        no_cleanup(),
    )
    .unwrap();
    queue
        .push_chunk(TrackKind::Mic, block(0, 0, 0, 4_000), None)
        .unwrap();
    wait_until(|| queue.status().failed.is_some());
    let failed = queue.status().failed.unwrap();
    assert!(failed.contains("No such file or directory"), "{failed}");
    // Later work is refused (the loop then stops) instead of being queued
    // behind a dead writer.
    assert_eq!(
        queue.push_chunk(TrackKind::Mic, block(0, 4_000, SECOND / 2, 4_000), None),
        Err(failed.clone())
    );
    assert_eq!(queue.status().queued_ns, 0);
    assert_eq!(queue.finish(|_| ()), Err(failed));
    // The pending reservation is reconciled by recovery as a gap.
    let report = Recording::recover(root.path(), "fatal").unwrap();
    assert_eq!(report.gaps, 1);
}

/// A panic on the recorder thread becomes a fatal error, not a hang, and the
/// meeting lock is released so recovery can run.
#[test]
fn a_recorder_panic_becomes_a_fatal_error_and_releases_the_meeting() {
    let root = TempDir::new().unwrap();
    let recording = Recording::create(root.path(), "panic").unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let queue = RecorderQueue::start(
        recording,
        no_clock(),
        config(Arc::new(move |_, _| {
            if seen.fetch_add(1, Ordering::SeqCst) == 1 {
                panic!("synthetic recorder fault");
            }
        })),
        no_cleanup(),
    )
    .unwrap();
    queue
        .push_chunk(TrackKind::Mic, block(0, 0, 0, 4_000), None)
        .unwrap();
    queue
        .push_chunk(TrackKind::Mic, block(0, 4_000, SECOND / 2, 4_000), None)
        .unwrap();
    wait_until(|| queue.status().failed.is_some());
    let failed = queue.status().failed.unwrap();
    assert_eq!(
        failed,
        format!("{RECORDER_PANIC_ERROR}: synthetic recorder fault")
    );
    let started = Instant::now();
    assert_eq!(queue.finish(|_| ()), Err(failed));
    assert!(started.elapsed() < Duration::from_secs(1));
    assert_eq!(
        Recording::recover(root.path(), "panic").unwrap(),
        RecoveryReport {
            verified_chunks: 1,
            adopted_chunks: 0,
            gaps: 0
        }
    );
}

/// The process "dies" (the queue is dropped without finish) with items still
/// queued: only chunks written before that exist, and recovery yields a
/// consistent, contiguous manifest.
#[test]
fn dying_with_items_queued_leaves_only_written_chunks_and_a_consistent_recovery() {
    let root = TempDir::new().unwrap();
    let stalled = Arc::new(AtomicBool::new(false));
    let written = Arc::new(AtomicUsize::new(0));
    let (hook_stalled, hook_written) = (stalled.clone(), written.clone());
    let recording = Recording::create(root.path(), "crash").unwrap();
    let queue = RecorderQueue::start(
        recording,
        no_clock(),
        config(Arc::new(move |_, _| {
            // The second item stalls: the first is written, the rest wait.
            if hook_written.fetch_add(1, Ordering::SeqCst) == 1 {
                hook_stalled.store(true, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(300));
            }
        })),
        no_cleanup(),
    )
    .unwrap();
    for n in 0..5u64 {
        queue
            .push_chunk(
                TrackKind::System,
                block(0, n * 4_000, n * SECOND / 2, 4_000),
                None,
            )
            .unwrap();
    }
    wait_until(|| stalled.load(Ordering::SeqCst));
    queue.abandon_and_wait();
    // The item already in hand was completed; nothing after it was started.
    let files = std::fs::read_dir(root.path().join("meetings/crash/audio/system"))
        .unwrap()
        .count();
    assert_eq!(files, 2);
    assert_eq!(
        Recording::recover(root.path(), "crash").unwrap(),
        RecoveryReport {
            verified_chunks: 2,
            adopted_chunks: 0,
            gaps: 0
        }
    );
    let rows = rows(root.path(), "crash");
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|r| r.7 == "ready"));
    assert_eq!(rows[1].5, rows[0].6);
}

/// Stop with a recorder stalled past the drain deadline returns an explicit
/// error at the deadline; the thread stops after its current write, and
/// recovery adopts exactly what was written.
#[test]
fn a_drain_past_its_deadline_fails_explicitly_and_recovery_keeps_what_was_written() {
    let root = TempDir::new().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let recording = Recording::create(root.path(), "deadline").unwrap();
    let queue = RecorderQueue::start(
        recording,
        no_clock(),
        Config {
            drain_deadline: Duration::from_millis(300),
            ..config(Arc::new(move |_, _| {
                if seen.fetch_add(1, Ordering::SeqCst) == 1 {
                    std::thread::sleep(Duration::from_millis(1_500));
                }
            }))
        },
        no_cleanup(),
    )
    .unwrap();
    for n in 0..4u64 {
        queue
            .push_chunk(
                TrackKind::Mic,
                block(0, n * 4_000, n * SECOND / 2, 4_000),
                None,
            )
            .unwrap();
    }
    let started = Instant::now();
    let mut saving = Vec::new();
    let error = queue
        .finish(|status| saving.push(status.queued_ns))
        .unwrap_err();
    assert!(error.starts_with(RECORDER_DRAIN_ERROR), "{error}");
    // Deadline, then at most the grace period for the stalled write to end.
    assert!(started.elapsed() >= Duration::from_millis(300));
    assert!(started.elapsed() < Duration::from_secs(4));
    assert!(saving.iter().any(|queued| *queued > 0), "{saving:?}");
    assert_eq!(
        Recording::recover(root.path(), "deadline").unwrap(),
        RecoveryReport {
            verified_chunks: 2,
            adopted_chunks: 0,
            gaps: 0
        }
    );
}

/// A recorder slightly slower than real time (~0.9x, 10x time-compressed
/// here): once the bound is reached, drops come in a few long episodes (each
/// one gap) instead of alternating drop/accept with one small gap per drop.
#[test]
fn a_slightly_slow_recorder_drops_in_few_long_gaps_not_many_small_ones() {
    let root = TempDir::new().unwrap();
    let recording = Recording::create(root.path(), "slow-disk").unwrap();
    // Chunks start at most every 27.5 ms while 0.25 s blocks (25 ms of wall
    // time each) arrive every 25 ms: 0.909x real time.
    let next = Arc::new(Mutex::new(Instant::now()));
    let pace = next.clone();
    let queue = RecorderQueue::start(
        recording,
        no_clock(),
        Config {
            queue_limit: Duration::from_secs(2),
            ..config(Arc::new(move |_, chunk| {
                if !chunk {
                    return;
                }
                let at = *pace.lock().unwrap();
                let now = Instant::now();
                if at > now {
                    std::thread::sleep(at - now);
                }
                *pace.lock().unwrap() = at.max(now) + Duration::from_micros(27_500);
            }))
        },
        no_cleanup(),
    )
    .unwrap();
    let quarter = SECOND / 4;
    let started = Instant::now();
    for n in 0..240u64 {
        let due = started + Duration::from_millis(25 * n);
        if let Some(wait) = due.checked_duration_since(Instant::now()) {
            std::thread::sleep(wait);
        }
        queue
            .push_chunk(
                TrackKind::Mic,
                block(0, n * 2_000, n * quarter, 2_000),
                None,
            )
            .unwrap();
    }
    let status = queue.status();
    queue.finish(|_| ()).unwrap();
    let rows = rows(root.path(), "slow-disk");
    let gaps = rows
        .iter()
        .filter(|r| r.8.as_deref() == Some("recorder_backlog"))
        .collect::<Vec<_>>();
    // Contiguous, exact: every frame and nanosecond is audio or a gap.
    let (mut frame, mut end) = (0, 0);
    for r in &rows {
        assert_eq!((r.3, r.5), (frame, end), "{rows:?}");
        frame += r.4;
        end = r.6;
    }
    assert_eq!((frame, end), (480_000, 60 * SECOND));
    let gap_ns: u64 = gaps.iter().map(|r| r.6 - r.5).sum();
    assert_eq!(gap_ns, status.dropped_ns);
    assert!(status.drops > 0, "the recorder was meant to fall behind");
    eprintln!(
        "slow recorder: {} drops in {} gaps ({:?} s each)",
        status.drops,
        gaps.len(),
        gaps.iter()
            .map(|r| (r.6 - r.5) as f64 / 1e9)
            .collect::<Vec<_>>()
    );
    // Without hysteresis every drop was its own 0.25 s gap (16 of 16 here).
    assert!(
        gaps.len() * 4 <= status.drops as usize,
        "{} gaps for {} drops",
        gaps.len(),
        status.drops
    );
    // Each episode lasts until the backlog halves (1 s of the 2 s bound),
    // except one cut short by the end of the input.
    assert!(
        gaps.iter()
            .all(|r| r.6 - r.5 >= SECOND || r.6 == 60 * SECOND),
        "{gaps:?}"
    );
}

fn manifest_status(root: &std::path::Path, meeting: &str) -> String {
    rusqlite::Connection::open(root.join("vantranscribe.db"))
        .unwrap()
        .query_row("SELECT status FROM meetings WHERE id=?1", [meeting], |r| {
            r.get(0)
        })
        .unwrap()
}

/// The last write completes after the drain deadline but within the grace:
/// the queue is then empty, so the manifest is finished and stop succeeds.
#[test]
fn a_close_completing_after_the_drain_deadline_is_reported_as_success() {
    let root = TempDir::new().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let recording = Recording::create(root.path(), "late-close").unwrap();
    let queue = RecorderQueue::start(
        recording,
        no_clock(),
        Config {
            drain_deadline: Duration::from_millis(300),
            abandon_grace: Duration::from_secs(3),
            ..config(Arc::new(move |_, _| {
                if seen.fetch_add(1, Ordering::SeqCst) == 1 {
                    std::thread::sleep(Duration::from_millis(800));
                }
            }))
        },
        no_cleanup(),
    )
    .unwrap();
    for n in 0..2u64 {
        queue
            .push_chunk(
                TrackKind::Mic,
                block(0, n * 4_000, n * SECOND / 2, 4_000),
                None,
            )
            .unwrap();
    }
    let started = Instant::now();
    assert_eq!(queue.finish(|_| ()), Ok(()));
    assert!(started.elapsed() >= Duration::from_millis(700));
    assert_eq!(manifest_status(root.path(), "late-close"), "processing");
    assert_eq!(rows(root.path(), "late-close").len(), 2);
}

/// Abandon while the recorder waits for SQLite's write lock (held by another
/// connection for longer than any deadline): the wait is cancelled, the
/// meeting lock is released promptly, and recovery then succeeds.
#[test]
fn abandon_during_a_manifest_lock_wait_releases_the_meeting_promptly() {
    let root = TempDir::new().unwrap();
    let recording = Recording::create(root.path(), "lock-abort").unwrap();
    let queue = RecorderQueue::start(
        recording,
        no_clock(),
        Config {
            drain_deadline: Duration::from_millis(300),
            abandon_grace: Duration::from_millis(300),
            ..Config::default()
        },
        no_cleanup(),
    )
    .unwrap();
    let blocker = rusqlite::Connection::open(root.path().join("vantranscribe.db")).unwrap();
    blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
    queue
        .push_chunk(TrackKind::Mic, block(0, 0, 0, 4_000), None)
        .unwrap();
    queue
        .push_chunk(TrackKind::Mic, block(0, 4_000, SECOND / 2, 4_000), None)
        .unwrap();
    let started = Instant::now();
    let error = queue.finish(|_| ()).unwrap_err();
    // Deadline + grace + cancellation (one 250 ms attempt), not 60 s.
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
    assert!(error.starts_with(RECORDER_DRAIN_ERROR), "{error}");
    assert!(!error.contains(RECORDER_RECOVERY_PENDING), "{error}");
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(root.path().join("meetings/lock-abort/.recording.lock"))
        .unwrap();
    lock.try_lock().unwrap();
    drop(lock);
    blocker.execute_batch("ROLLBACK").unwrap();
    Recording::recover(root.path(), "lock-abort").unwrap();
    assert_eq!(manifest_status(root.path(), "lock-abort"), "recovered");
    assert!(
        rows(root.path(), "lock-abort")
            .iter()
            .all(|r| r.7 != "pending")
    );
}

/// A panic in the thread's own bookkeeping (outside the per-item guard) still
/// ends it with the panic as its fatal error, without a drain timeout.
#[test]
fn a_panic_in_the_recorder_bookkeeping_is_a_fatal_error_not_a_timeout() {
    let root = TempDir::new().unwrap();
    let recording = Recording::create(root.path(), "book-panic").unwrap();
    let queue = RecorderQueue::start(
        recording,
        no_clock(),
        Config {
            after: Some(Arc::new(|| panic!("synthetic bookkeeping fault"))),
            ..Config::default()
        },
        no_cleanup(),
    )
    .unwrap();
    queue
        .push_chunk(TrackKind::Mic, block(0, 0, 0, 4_000), None)
        .unwrap();
    wait_until(|| queue.status().failed.is_some());
    let started = Instant::now();
    assert_eq!(
        queue.finish(|_| ()),
        Err(format!(
            "{RECORDER_PANIC_ERROR}: synthetic bookkeeping fault"
        ))
    );
    assert!(started.elapsed() < Duration::from_secs(1));
}

/// A flapping device (a device-change gap and a new epoch, again and again,
/// with no audio between) extends one queued gap instead of growing the queue.
#[test]
fn consecutive_helper_gaps_on_one_track_extend_one_queued_gap() {
    let root = TempDir::new().unwrap();
    let stalled = Arc::new(AtomicBool::new(true));
    let recording = Recording::create(root.path(), "flapping").unwrap();
    let queue = RecorderQueue::start(
        recording,
        no_clock(),
        config(stall_hook(stalled.clone())),
        no_cleanup(),
    )
    .unwrap();
    queue
        .push_chunk(TrackKind::Mic, block(0, 0, 0, 4_000), None)
        .unwrap();
    // The loop's bookkeeping, as in `TrackBuffer::gap_until`/`set_format`.
    let (mut epoch, mut frame, mut end) = (0u32, 4_000u64, SECOND / 2);
    let mut rows_added = 0;
    for _ in 0..1_000 {
        let admitted = queue
            .push_gap(
                TrackKind::Mic,
                format(),
                Gap {
                    epoch,
                    first_frame: frame,
                    frame_count: 0,
                    start_ns: end,
                    end_ns: end + 1_000_000,
                    reason: GapReason::DeviceChange,
                },
            )
            .unwrap();
        rows_added += usize::from(!admitted.merged);
        end += 1_000_000;
        epoch = admitted.epoch + 1;
        frame = 0;
    }
    assert_eq!(rows_added, 1);
    queue
        .push_chunk(TrackKind::Mic, block(epoch, frame, end, 4_000), None)
        .unwrap();
    // The first chunk is being written (stalled); one gap and one chunk wait.
    assert_eq!(queue.shared.lock().queue.len(), 2);
    stalled.store(false, Ordering::SeqCst);
    queue.finish(|_| ()).unwrap();
    let half = SECOND / 2;
    assert_eq!(
        rows(root.path(), "flapping"),
        vec![
            row("mic", 0, 0, 0, 4_000, 0, half, "ready", None),
            row(
                "mic",
                1,
                0,
                4_000,
                0,
                half,
                half + SECOND,
                "gap",
                Some("device_change")
            ),
            row(
                "mic",
                2,
                1,
                0,
                4_000,
                half + SECOND,
                2 * SECOND,
                "ready",
                None
            ),
        ]
    );
}
