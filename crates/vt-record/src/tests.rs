use super::*;
use std::{
    process::{Command, Stdio},
    thread,
    time::Instant,
};
use tempfile::TempDir;
use vt_core::FramePosition;

fn block(
    epoch: u32,
    first_frame: u64,
    start_ns: u64,
    rate: u32,
    channels: u16,
    frames: usize,
) -> AudioBlock {
    let samples = (0..frames * channels as usize)
        .map(|i| ((i as f32 * 0.091).sin() * 0.75) + if i % 31 == 0 { 1.0 } else { 0.0 })
        .collect();
    AudioBlock::new(
        StreamFormat::new(rate, channels).unwrap(),
        FramePosition {
            epoch,
            first_frame,
            host_time_ns: start_ns,
        },
        samples,
    )
    .unwrap()
}

fn stored(root: &Path, meeting: &str) -> Vec<Chunk> {
    read_chunks(&open_database(root).unwrap(), meeting).unwrap()
}

fn assert_decodes(root: &Path, chunk: &Chunk, expected: &[i32]) {
    let mut reader = claxon::FlacReader::open(root.join(chunk.path.as_ref().unwrap())).unwrap();
    let info = reader.streaminfo();
    assert_eq!(info.sample_rate, chunk.format.sample_rate);
    assert_eq!(info.channels, u32::from(chunk.format.channels));
    assert_eq!(info.bits_per_sample, 24);
    assert_eq!(info.samples, Some(chunk.frame_count));
    let actual = reader
        .samples()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(actual, expected);
}

#[test]
fn independent_decoder_preserves_two_native_formats_and_short_final_chunk() {
    let root = TempDir::new().unwrap();
    let mut recording = Recording::create(root.path(), "round-trip").unwrap();
    let mic = block(0, 0, 0, 8_000, 1, 8_000);
    let system = block(0, 0, 0, 44_100, 2, 44_100);
    let a = recording.write_chunk(TrackKind::Mic, &mic).unwrap();
    let b = recording.write_chunk(TrackKind::System, &system).unwrap();
    let tail = block(0, 8_000, a.end_ns, 8_000, 1, 17);
    let c = recording.write_chunk(TrackKind::Mic, &tail).unwrap();
    assert_decodes(root.path(), &a, &pcm24(&mic.samples).unwrap());
    assert_decodes(root.path(), &b, &pcm24(&system.samples).unwrap());
    assert_decodes(root.path(), &c, &pcm24(&tail.samples).unwrap());
    assert!(a.clipped_samples > 0);
    // Model a descriptor inherited by a concurrent child before close-on-exec.
    // While the writer is active this alias must not weaken mutual exclusion.
    let inherited_lock = recording._lock.try_clone().unwrap();
    assert!(matches!(
        Recording::recover(root.path(), "round-trip"),
        Err(RecordError::Busy)
    ));
    recording.finish().unwrap();
    assert_eq!(
        Recording::recover(root.path(), "round-trip")
            .unwrap()
            .verified_chunks,
        3
    );
    // The alias is deliberately still open during immediate recovery.
    drop(inherited_lock);
}

#[test]
fn explicit_pause_and_format_epoch_preserve_timeline() {
    let root = TempDir::new().unwrap();
    let mut r = Recording::create(root.path(), "epochs").unwrap();
    let first = block(0, 0, 0, 8_000, 1, 8_000);
    r.write_chunk(TrackKind::Mic, &first).unwrap();
    r.write_gap(
        TrackKind::Mic,
        first.format,
        Gap {
            epoch: 0,
            first_frame: 8_000,
            frame_count: 0,
            start_ns: 1_000_000_000,
            end_ns: 3_000_000_000,
            reason: GapReason::DeviceChange,
        },
    )
    .unwrap();
    r.write_chunk(
        TrackKind::Mic,
        &block(1, 0, 3_000_000_000, 16_000, 2, 16_000),
    )
    .unwrap();
    let chunks = r.chunks().unwrap();
    assert_eq!(chunks.len(), 3);
    assert_eq!(chunks[1].gap_reason.as_deref(), Some("device_change"));
    assert_eq!(chunks[2].epoch, 1);
    assert_eq!(chunks[2].end_ns, 4_000_000_000);
}

#[test]
fn rejects_implicit_gaps_reversed_time_and_changed_format() {
    let root = TempDir::new().unwrap();
    let mut r = Recording::create(root.path(), "invalid-timeline").unwrap();
    r.write_chunk(TrackKind::Mic, &block(0, 0, 0, 8_000, 1, 8_000))
        .unwrap();
    for candidate in [
        block(0, 8_000, 2_000_000_000, 8_000, 1, 8_000),
        block(0, 7_999, 1_000_000_000, 8_000, 1, 8_000),
        block(0, 8_000, 1_000_000_000, 16_000, 1, 16_000),
        block(2, 0, 1_000_000_000, 8_000, 1, 8_000),
    ] {
        assert!(r.write_chunk(TrackKind::Mic, &candidate).is_err());
    }
    assert_eq!(r.chunks().unwrap().len(), 1);
    r.write_chunk(
        TrackKind::Mic,
        &block(0, 8_000, 1_000_000_000, 8_000, 1, 8_000),
    )
    .unwrap();
}

#[test]
fn rejects_invalid_samples_and_mutated_public_formats_without_reserving() {
    let root = TempDir::new().unwrap();
    let mut r = Recording::create(root.path(), "invalid-samples").unwrap();
    let mut b = block(0, 0, 0, 8_000, 1, 10);
    b.samples[0] = f32::NAN;
    assert!(r.write_chunk(TrackKind::Mic, &b).is_err());
    b.samples[0] = 0.0;
    b.format.channels = 0;
    assert!(r.write_chunk(TrackKind::Mic, &b).is_err());
    assert!(r.chunks().unwrap().is_empty());
}

#[test]
fn rejects_traversal_and_duplicate_meeting_ids() {
    let root = TempDir::new().unwrap();
    for id in ["", "../escape", "a/b", ".", "é"] {
        assert!(Recording::create(root.path(), id).is_err());
    }
    Recording::create(root.path(), "unique")
        .unwrap()
        .finish()
        .unwrap();
    assert!(Recording::create(root.path(), "unique").is_err());
}

#[test]
fn unknown_recovery_does_not_initialize_a_recording_or_database() {
    let root = TempDir::new().unwrap();
    assert!(Recording::recover(root.path(), "unknown").is_err());
    assert!(!root.path().join("vantranscribe.db").exists());
    assert!(!root.path().join("meetings").exists());
    Recording::create(root.path(), "known")
        .unwrap()
        .finish()
        .unwrap();
    assert!(Recording::recover(root.path(), "unknown").is_err());
    assert!(!root.path().join("meetings/unknown").exists());
}

#[test]
fn concurrent_meetings_share_one_initialized_manifest() {
    const WRITERS: usize = 8;
    for round in 0..20 {
        // Every round races the very first WAL transition and schema creation,
        // not merely connections to an already initialized database.
        let root = TempDir::new().unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(WRITERS));
        let workers: Vec<_> = (0..WRITERS)
            .map(|n| {
                let root = root.path().to_owned();
                let barrier = barrier.clone();
                thread::spawn(move || -> Result<()> {
                    barrier.wait();
                    let mut recording = Recording::create(root, &format!("parallel-{n}"))?;
                    recording.write_chunk(TrackKind::Mic, &block(0, 0, 0, 8_000, 1, 80))?;
                    recording.finish()
                })
            })
            .collect();
        // Join everyone before reporting an error so a failed assertion cannot
        // delete the temporary root while the other real writers are running.
        let results = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect::<Vec<_>>();
        for result in results {
            result.unwrap_or_else(|error| panic!("first-open round {round}: {error}"));
        }
        let db = open_database(root.path()).unwrap();
        assert_eq!(
            db.pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            db.pragma_query_value(None, "journal_mode", |row| row.get::<_, String>(0))
                .unwrap(),
            "wal"
        );
        assert_eq!(
            db.pragma_query_value(None, "foreign_keys", |row| row.get::<_, u32>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            db.pragma_query_value(None, "busy_timeout", |row| row.get::<_, u32>(0))
                .unwrap(),
            5000
        );
        assert_eq!(
            db.query_row("SELECT COUNT(*) FROM meetings", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            WRITERS as i64
        );
        assert_eq!(
            db.query_row(
                "SELECT COUNT(*) FROM audio_chunks WHERE state='ready'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            WRITERS as i64
        );
        drop(db);
        for n in 0..WRITERS {
            assert_eq!(
                Recording::recover(root.path(), &format!("parallel-{n}"))
                    .unwrap()
                    .verified_chunks,
                1
            );
        }
    }
}

#[test]
fn a_persistent_initialization_lock_exhausts_one_budget_without_partial_schema() {
    let root = TempDir::new().unwrap();
    let holder = Connection::open(root.path().join("vantranscribe.db")).unwrap();
    holder.execute_batch("BEGIN EXCLUSIVE").unwrap();
    let (sender, receiver) = std::sync::mpsc::channel();
    let path = root.path().to_owned();
    let worker = thread::spawn(move || {
        sender
            .send(open_database_with_budget(&path, Duration::from_millis(60)).map(|_| ()))
            .unwrap();
    });
    // Keep the conflicting transaction open until initialization returns. A
    // reset timeout/infinite retry cannot accidentally pass after lock release.
    let result = receiver.recv_timeout(Duration::from_secs(2));
    holder.execute_batch("ROLLBACK").unwrap();
    worker.join().unwrap();
    let error = result
        .expect("initialization must respect its total lock budget")
        .unwrap_err();
    assert!(
        matches!(error,RecordError::Database(rusqlite::Error::SqliteFailure(error,_)) if error.code==rusqlite::ErrorCode::DatabaseBusy)
    );
    assert_eq!(
        holder
            .pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        holder
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table'",
                [],
                |row| row.get::<_, u32>(0)
            )
            .unwrap(),
        0
    );
    drop(holder);
    // A failed attempt leaves no held connection/transaction; the same root can
    // initialize successfully once the unrelated lock is gone.
    open_database(root.path()).unwrap();
}

#[test]
fn unsupported_manifest_version_preserves_existing_schema_and_data() {
    let root = TempDir::new().unwrap();
    let existing = Connection::open(root.path().join("vantranscribe.db")).unwrap();
    existing.execute_batch("CREATE TABLE retained(value TEXT NOT NULL); INSERT INTO retained VALUES ('synthetic evidence'); PRAGMA user_version=9;").unwrap();
    let error = open_database_with_budget(root.path(), Duration::from_millis(60)).unwrap_err();
    assert!(
        matches!(error,RecordError::Invalid(message) if message=="unsupported manifest version 9")
    );
    assert_eq!(
        existing
            .pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
            .unwrap(),
        9
    );
    assert_eq!(
        existing
            .query_row("SELECT value FROM retained", [], |row| row
                .get::<_, String>(0))
            .unwrap(),
        "synthetic evidence"
    );
    assert_eq!(
        existing
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name IN ('meetings','audio_chunks')",
                [],
                |row| row.get::<_, u32>(0)
            )
            .unwrap(),
        0
    );
}

#[test]
fn fractional_sample_duration_chains_the_published_endpoint() {
    let root = TempDir::new().unwrap();
    let mut recording = Recording::create(root.path(), "fractional").unwrap();
    let first = recording
        .write_chunk(TrackKind::Mic, &block(0, 0, 0, 44_100, 1, 1))
        .unwrap();
    assert_eq!(first.end_ns, 22_675);
    let second = recording
        .write_chunk(TrackKind::Mic, &block(0, 1, first.end_ns, 44_100, 1, 1))
        .unwrap();
    assert_eq!(second.start_ns, first.end_ns);
}

#[test]
fn valid_pending_partial_replaces_damaged_final_without_discarding_evidence() {
    let root = TempDir::new().unwrap();
    let mut recording = Recording::create(root.path(), "adopt-partial").unwrap();
    assert!(
        recording
            .write_observed(TrackKind::Mic, &block(0, 0, 0, 8_000, 1, 80), |stage| {
                if stage == PublishStage::TempSynced {
                    Err(std::io::Error::other("interrupted").into())
                } else {
                    Ok(())
                }
            })
            .is_err()
    );
    let path = root
        .path()
        .join("meetings/adopt-partial/audio/mic/0-0.flac");
    fs::write(&path, b"damaged destination").unwrap();
    drop(recording);
    assert_eq!(
        Recording::recover(root.path(), "adopt-partial")
            .unwrap()
            .adopted_chunks,
        1
    );
    assert_eq!(
        fs::read(path.with_extension("flac.damaged.0")).unwrap(),
        b"damaged destination"
    );
    assert_decodes(
        root.path(),
        &stored(root.path(), "adopt-partial")[0],
        &pcm24(&block(0, 0, 0, 8_000, 1, 80).samples).unwrap(),
    );
}

#[test]
fn writer_and_recovery_are_mutually_exclusive() {
    let root = TempDir::new().unwrap();
    let writer = Recording::create(root.path(), "locked").unwrap();
    assert!(matches!(
        Recording::create(root.path(), "locked"),
        Err(RecordError::Busy)
    ));
    assert!(matches!(
        Recording::recover(root.path(), "locked"),
        Err(RecordError::Busy)
    ));
    drop(writer);
    assert!(Recording::recover(root.path(), "locked").is_ok());
}

#[test]
fn failed_publication_poisoning_and_incomplete_file_become_gap() {
    let root = TempDir::new().unwrap();
    let mut r = Recording::create(root.path(), "failed-write").unwrap();
    let audio = block(0, 0, 0, 8_000, 1, 8_000);
    let error = r.write_observed(TrackKind::Mic, &audio, |stage| {
        if stage == PublishStage::Reserved {
            Err(std::io::Error::from(std::io::ErrorKind::StorageFull).into())
        } else {
            Ok(())
        }
    });
    assert!(error.is_err());
    assert!(r.write_chunk(TrackKind::System, &audio).is_err());
    let partial = root
        .path()
        .join("meetings/failed-write/audio/mic/0-0.flac.partial");
    fs::write(&partial, b"truncated FLAC data").unwrap();
    assert!(r.finish().is_err());
    let report = Recording::recover(root.path(), "failed-write").unwrap();
    assert_eq!(report.gaps, 1);
    assert!(partial.exists(), "damaged evidence must not be deleted");
    assert_eq!(stored(root.path(), "failed-write")[0].state, "gap");
    assert_eq!(
        Recording::recover(root.path(), "failed-write")
            .unwrap()
            .gaps,
        0
    );
}

#[test]
fn missing_and_corrupt_ready_files_are_explicit_gaps() {
    let root = TempDir::new().unwrap();
    let mut r = Recording::create(root.path(), "damage").unwrap();
    let a = r
        .write_chunk(TrackKind::Mic, &block(0, 0, 0, 8_000, 1, 8_000))
        .unwrap();
    let b = r
        .write_chunk(TrackKind::System, &block(0, 0, 0, 8_000, 1, 8_000))
        .unwrap();
    r.finish().unwrap();
    fs::remove_file(root.path().join(a.path.unwrap())).unwrap();
    let damaged = root.path().join(b.path.unwrap());
    fs::write(&damaged, b"damaged").unwrap();
    assert_eq!(Recording::recover(root.path(), "damage").unwrap().gaps, 2);
    assert!(damaged.exists());
    assert!(
        stored(root.path(), "damage")
            .iter()
            .all(|c| c.state == "gap" && c.path.is_none() && c.sha256.is_none())
    );
    let db = open_database(root.path()).unwrap();
    let status: String = db
        .query_row("SELECT status FROM meetings WHERE id='damage'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(status, "recovered");
}

/// Holds SQLite's write lock from another connection for `held`, returning once
/// the lock is taken.
fn hold_write_lock(root: &Path, held: Duration) -> thread::JoinHandle<()> {
    let (taken, ready) = std::sync::mpsc::channel();
    let path = root.join("vantranscribe.db");
    let holder = thread::spawn(move || {
        let db = Connection::open(path).unwrap();
        db.execute_batch("BEGIN IMMEDIATE").unwrap();
        taken.send(()).unwrap();
        thread::sleep(held);
        db.execute_batch("ROLLBACK").unwrap();
    });
    ready.recv().unwrap();
    holder
}

fn busy(error: &RecordError) -> bool {
    matches!(error, RecordError::Database(rusqlite::Error::SqliteFailure(e, _)) if e.code == rusqlite::ErrorCode::DatabaseBusy)
}

#[test]
fn lock_errors_including_busy_snapshot_are_retryable_and_others_are_not() {
    for code in [5, 6, 261, 517] {
        let error = RecordError::Database(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(code),
            None,
        ));
        assert!(transient_database_lock(&error), "{code}");
    }
    for code in [10, 11, 19] {
        let error = RecordError::Database(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(code),
            None,
        ));
        assert!(!transient_database_lock(&error), "{code}");
    }
}

/// Before: the pending insert waited 5 s, then failed and poisoned the writer.
#[test]
fn pending_insert_retries_a_lock_without_duplicating_the_row() {
    let root = TempDir::new().unwrap();
    let mut r = Recording::create(root.path(), "insert-retry").unwrap();
    let first = r
        .write_chunk(TrackKind::Mic, &block(0, 0, 0, 8_000, 1, 80))
        .unwrap();
    // Longer than one 250 ms attempt, so the first INSERT fails and is retried.
    let holder = hold_write_lock(root.path(), Duration::from_millis(700));
    let second = r
        .write_chunk(TrackKind::Mic, &block(0, 80, first.end_ns, 8_000, 1, 80))
        .unwrap();
    holder.join().unwrap();
    assert_eq!(second.state, "ready");
    let waits = r.lock_waits();
    assert_eq!(waits.count, 1, "{waits:?}");
    assert!(waits.max >= Duration::from_millis(400), "{waits:?}");
    let rows = stored(root.path(), "insert-retry");
    assert_eq!(
        rows.iter()
            .map(|c| (c.seq, c.state.as_str()))
            .collect::<Vec<_>>(),
        [(0, "ready"), (1, "ready")]
    );
    r.write_chunk(TrackKind::Mic, &block(0, 160, second.end_ns, 8_000, 1, 80))
        .unwrap();
    r.finish().unwrap();
    assert_eq!(
        Recording::recover(root.path(), "insert-retry")
            .unwrap()
            .verified_chunks,
        3
    );
}

/// A retry after an attempt whose outcome is unknown must accept the row an
/// earlier attempt committed, once, and must refuse a different row.
#[test]
fn a_retried_insert_accepts_its_own_committed_row_and_rejects_another() {
    let root = TempDir::new().unwrap();
    let mut r = Recording::create(root.path(), "insert-idempotent").unwrap();
    let chunk = r
        .write_chunk(TrackKind::Mic, &block(0, 0, 0, 8_000, 1, 80))
        .unwrap();
    let pending = Chunk {
        state: "pending".into(),
        ..chunk.clone()
    };
    r.db.execute(
        "UPDATE audio_chunks SET state='pending' WHERE meeting_id='insert-idempotent'",
        [],
    )
    .unwrap();
    insert_row(&r.db, "insert-idempotent", &pending, "audio", true).unwrap();
    assert!(insert_row(&r.db, "insert-idempotent", &pending, "audio", false).is_err());
    let other = Chunk {
        sha256: Some("0".repeat(64)),
        ..pending.clone()
    };
    assert!(matches!(
        insert_row(&r.db, "insert-idempotent", &other, "audio", true),
        Err(RecordError::Invalid(_))
    ));
    assert_eq!(stored(root.path(), "insert-idempotent").len(), 1);
}

/// Before: the ready update waited 5 s, then failed and poisoned the writer.
#[test]
fn ready_update_retries_a_lock_after_the_file_is_published() {
    let root = TempDir::new().unwrap();
    let mut r = Recording::create(root.path(), "ready-retry").unwrap();
    let mut holder = None;
    let path = root.path().to_owned();
    let chunk = r
        .write_observed(TrackKind::Mic, &block(0, 0, 0, 8_000, 1, 80), |stage| {
            if stage == PublishStage::Renamed {
                holder = Some(hold_write_lock(&path, Duration::from_millis(700)));
            }
            Ok(())
        })
        .unwrap();
    holder.unwrap().join().unwrap();
    assert_eq!(chunk.state, "ready");
    assert_eq!(r.lock_waits().count, 1);
    let rows = stored(root.path(), "ready-retry");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].state, "ready");
    assert!(
        !root
            .path()
            .join("meetings/ready-retry/audio/mic/0-0.flac.partial")
            .exists()
    );
    r.write_gap(
        TrackKind::Mic,
        chunk.format,
        Gap {
            epoch: 0,
            first_frame: 80,
            frame_count: 0,
            start_ns: chunk.end_ns,
            end_ns: chunk.end_ns + 1_000,
            reason: GapReason::Overflow,
        },
    )
    .unwrap();
    r.finish().unwrap();
    assert_eq!(
        Recording::recover(root.path(), "ready-retry").unwrap(),
        RecoveryReport {
            verified_chunks: 1,
            adopted_chunks: 0,
            gaps: 0
        }
    );
}

#[test]
fn gap_and_finish_retry_a_lock() {
    let root = TempDir::new().unwrap();
    let mut r = Recording::create(root.path(), "gap-retry").unwrap();
    let chunk = r
        .write_chunk(TrackKind::System, &block(0, 0, 0, 8_000, 1, 80))
        .unwrap();
    let holder = hold_write_lock(root.path(), Duration::from_millis(600));
    r.write_gap(
        TrackKind::System,
        chunk.format,
        Gap {
            epoch: 0,
            first_frame: 80,
            frame_count: 0,
            start_ns: chunk.end_ns,
            end_ns: chunk.end_ns + 1_000,
            reason: GapReason::Overflow,
        },
    )
    .unwrap();
    holder.join().unwrap();
    let holder = hold_write_lock(root.path(), Duration::from_millis(600));
    r.finish().unwrap();
    holder.join().unwrap();
    let db = open_database(root.path()).unwrap();
    let status: String = db
        .query_row(
            "SELECT status FROM meetings WHERE id='gap-retry'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(status, "processing");
    assert_eq!(
        stored(root.path(), "gap-retry")
            .iter()
            .map(|c| c.state.as_str())
            .collect::<Vec<_>>(),
        ["ready", "gap"]
    );
}

/// A lock outlasting the total budget still poisons the writer with the lock
/// error, and recovery adopts the published chunk left pending.
#[test]
fn a_lock_past_the_budget_poisons_the_writer_and_recovery_adopts_the_chunk() {
    let root = TempDir::new().unwrap();
    let mut r = Recording::create(root.path(), "budget").unwrap();
    r.set_lock_budget(Duration::from_millis(300));
    let blocker = Connection::open(root.path().join("vantranscribe.db")).unwrap();
    let started = Instant::now();
    let error = r
        .write_observed(TrackKind::Mic, &block(0, 0, 0, 8_000, 1, 80), |stage| {
            if stage == PublishStage::Renamed {
                blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
            }
            Ok(())
        })
        .unwrap_err();
    assert!(busy(&error), "{error}");
    assert!(started.elapsed() < Duration::from_secs(3));
    assert!(r.lock_waits().max >= Duration::from_millis(300));
    assert!(matches!(
        r.write_chunk(TrackKind::System, &block(0, 0, 0, 8_000, 1, 80)),
        Err(RecordError::Invalid(_))
    ));
    blocker.execute_batch("ROLLBACK").unwrap();
    assert!(r.finish().is_err());
    assert_eq!(stored(root.path(), "budget")[0].state, "pending");
    assert_eq!(
        Recording::recover(root.path(), "budget")
            .unwrap()
            .adopted_chunks,
        1
    );
    assert_eq!(stored(root.path(), "budget")[0].state, "ready");

    // A lock at the pending insert past the budget reserves nothing.
    let mut r = Recording::create(root.path(), "budget-insert").unwrap();
    r.set_lock_budget(Duration::from_millis(200));
    blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
    let error = r
        .write_chunk(TrackKind::Mic, &block(0, 0, 0, 8_000, 1, 80))
        .unwrap_err();
    blocker.execute_batch("ROLLBACK").unwrap();
    assert!(busy(&error), "{error}");
    assert!(
        r.write_chunk(TrackKind::Mic, &block(0, 0, 0, 8_000, 1, 80))
            .is_err()
    );
    drop(r);
    assert!(stored(root.path(), "budget-insert").is_empty());
}

#[test]
fn sql_integer_overflow_is_rejected_before_publication() {
    let root = TempDir::new().unwrap();
    let mut r = Recording::create(root.path(), "overflow").unwrap();
    let b = block(0, 0, i64::MAX as u64, 8_000, 1, 8_000);
    assert!(r.write_chunk(TrackKind::Mic, &b).is_err());
    assert!(r.chunks().unwrap().is_empty());
}

// The parent kills this isolated test process while the file lock and SQLite
// connection are still live. Unlike returning an injected error, SIGKILL does
// not run destructors or close SQLite cooperatively.
#[test]
#[ignore = "subprocess harness; invoked explicitly by the SIGKILL test"]
fn crash_child() {
    let Ok(dir) = std::env::var("VT_RECORD_CRASH_ROOT") else {
        return;
    };
    let stage = std::env::var("VT_RECORD_CRASH_STAGE").unwrap();
    let root = Path::new(&dir);
    let mut r = Recording::create(root, "crash").unwrap();
    let initial = block(0, 0, 0, 8_000, 1, 8_000);
    r.write_chunk(TrackKind::Mic, &initial).unwrap();
    r.write_chunk(TrackKind::System, &initial).unwrap();
    r.write_observed(
        TrackKind::Mic,
        &block(0, 8_000, 1_000_000_000, 8_000, 1, 8_000),
        |point| {
            if format!("{point:?}") == stage {
                fs::write(root.join("at-boundary"), b"ready").unwrap();
                loop {
                    thread::sleep(Duration::from_secs(1));
                }
            }
            Ok(())
        },
    )
    .unwrap();
    panic!("requested crash stage was never reached");
}

#[cfg(unix)]
#[test]
fn sigkill_at_each_publication_boundary_recovers_idempotently() {
    for stage in ["Reserved", "TempSynced", "Renamed", "Committed"] {
        let root = TempDir::new().unwrap();
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "tests::crash_child", "--ignored", "--nocapture"])
            .env("VT_RECORD_CRASH_ROOT", root.path())
            .env("VT_RECORD_CRASH_STAGE", stage)
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        while !root.path().join("at-boundary").exists() {
            if let Some(status) = child.try_wait().unwrap() {
                panic!("crash worker exited early: {status}");
            }
            if Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("worker did not reach {stage}");
            }
            thread::sleep(Duration::from_millis(10));
        }
        child.kill().unwrap();
        assert!(!child.wait().unwrap().success());
        let report = Recording::recover(root.path(), "crash").unwrap();
        assert_eq!(report.gaps, usize::from(stage == "Reserved"));
        assert_eq!(
            report.adopted_chunks,
            usize::from(stage == "TempSynced" || stage == "Renamed")
        );
        let chunks = stored(root.path(), "crash");
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks.iter().filter(|c| c.state == "pending").count(), 0);
        assert_eq!(
            chunks.iter().filter(|c| c.state == "ready").count(),
            if stage == "Reserved" { 2 } else { 3 }
        );
        let initial = block(0, 0, 0, 8_000, 1, 8_000);
        for track in [TrackKind::Mic, TrackKind::System] {
            let chunk = chunks
                .iter()
                .find(|c| c.track == track && c.seq == 0)
                .unwrap();
            assert_decodes(root.path(), chunk, &pcm24(&initial.samples).unwrap());
        }
        let again = Recording::recover(root.path(), "crash").unwrap();
        assert_eq!(again.adopted_chunks, 0);
        assert_eq!(again.gaps, 0);
        assert_eq!(stored(root.path(), "crash"), chunks);
    }
}

#[test]
fn transient_colored_pcm_never_publishes_pathological_predictor_frames() {
    // This two-second source reproduces flacenc's underestimated ApproxEnt
    // fixed-predictor choice during a short near-end reply over delayed echo.
    let mut far = vec![0.0; 12 * 16000];
    let (mut seed, mut seed2) = (0x123456789abcdefu64, 0x9876543210abcdefu64);
    let (mut low, mut low2) = (0.0, 0.0);
    let colored = |seed: &mut u64, low: &mut f32| {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 7;
        *seed ^= *seed << 17;
        let white = (*seed >> 40) as f32 / (1u32 << 23) as f32 - 1.0;
        *low = 0.85 * *low + 0.15 * white;
        *low
    };
    let mut source = Vec::with_capacity(32000);
    for i in 0..far.len() {
        let t = i as f32 / 16000.0;
        let envelope = 0.35 + 0.65 * (t * std::f32::consts::TAU * 2.3).sin().abs();
        far[i] = envelope
            * (0.32 * colored(&mut seed, &mut low)
                + 0.12 * (t * std::f32::consts::TAU * 173.0).sin()
                + 0.07 * (t * std::f32::consts::TAU * 431.0).sin());
        let near = if (8 * 16000..10 * 16000).contains(&i)
            || (11 * 16000..11 * 16000 + 4000).contains(&i)
        {
            0.45 * colored(&mut seed2, &mut low2) + 0.13 * (t * std::f32::consts::TAU * 283.0).sin()
        } else {
            0.0
        };
        if i >= 10 * 16000 {
            source.push(near + 0.65 * far[i - 480] + 0.15 * far[i - 640]);
        }
    }
    for (rate, channels) in [(16000, 1), (48000, 2), (192000, 8)] {
        let temp = TempDir::new().unwrap();
        let mut recording = Recording::create(temp.path(), "predictor-regression").unwrap();
        let samples: Vec<f32> = source
            .iter()
            .flat_map(|value| {
                (0..channels).map(move |channel| *value * (1.0 - channel as f32 / 16.0))
            })
            .collect();
        let expected = vt_core::pcm24(&samples).unwrap();
        let chunk = recording
            .write_chunk(
                TrackKind::Mic,
                &AudioBlock::new(
                    StreamFormat::new(rate, channels).unwrap(),
                    FramePosition {
                        epoch: 0,
                        first_frame: 0,
                        host_time_ns: 0,
                    },
                    samples,
                )
                .unwrap(),
            )
            .unwrap();
        recording.finish().unwrap();
        let bytes = fs::read(temp.path().join(chunk.path.unwrap())).unwrap();
        assert!(
            bytes.len() <= expected.len() * 3 + 4096,
            "predictor exceeded verbatim bound at {rate}Hz/{channels}ch: {} bytes",
            bytes.len()
        );
        let mut decoder = claxon::FlacReader::new(std::io::Cursor::new(bytes)).unwrap();
        assert_eq!(decoder.streaminfo().sample_rate, rate);
        assert_eq!(decoder.streaminfo().channels, u32::from(channels));
        assert_eq!(decoder.streaminfo().bits_per_sample, 24);
        assert_eq!(decoder.streaminfo().samples, Some(32000));
        let actual = decoder
            .samples()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(actual, expected, "PCM24 changed at {rate}Hz/{channels}ch");
        assert_eq!(
            Recording::recover(temp.path(), "predictor-regression")
                .unwrap()
                .verified_chunks,
            1
        );
    }
}

#[test]
fn short_final_native_chunks_publish_valid_streaminfo_without_padding_pcm() {
    for frames in [1, 15, 16, 4097, 8207] {
        let temp = TempDir::new().unwrap();
        let mut recording = Recording::create(temp.path(), "short-native").unwrap();
        let samples: Vec<f32> = (0..frames)
            .map(|n| (n as f32 * 0.071).sin() * 0.2)
            .collect();
        let expected = vt_core::pcm24(&samples).unwrap();
        let chunk = recording
            .write_chunk(
                TrackKind::Mic,
                &AudioBlock::new(
                    StreamFormat::new(16000, 1).unwrap(),
                    FramePosition {
                        epoch: 0,
                        first_frame: 0,
                        host_time_ns: 0,
                    },
                    samples,
                )
                .unwrap(),
            )
            .unwrap();
        recording.finish().unwrap();
        let bytes = fs::read(temp.path().join(chunk.path.unwrap())).unwrap();
        let mut decoder = claxon::FlacReader::new(std::io::Cursor::new(bytes)).unwrap();
        assert!(decoder.streaminfo().min_block_size >= 16);
        assert_eq!(decoder.streaminfo().samples, Some(frames as u64));
        assert_eq!(
            decoder
                .samples()
                .collect::<std::result::Result<Vec<_>, _>>()
                .unwrap(),
            expected
        );
    }
}
