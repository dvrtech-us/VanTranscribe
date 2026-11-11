//! Live transcript persistence at the store level: the lock mechanism behind
//! the lost live transcript, concurrent commits, and search-row equivalence.
use super::*;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tempfile::TempDir;

fn fixture() -> TempDir {
    let temp = TempDir::new().unwrap();
    initialize(temp.path()).unwrap();
    let recording = vt_record::Recording::create(temp.path(), "fixture").unwrap();
    create_meeting(temp.path(), "fixture", "Meeting fixture", "remote").unwrap();
    recording.finish().unwrap();
    temp
}
fn live_run(root: &Path) -> String {
    detail(root, "fixture").unwrap().active_run_id.unwrap()
}
fn line(id: &str, start_ms: u64, text: &str) -> Segment {
    Segment {
        id: id.into(),
        track: "system".into(),
        start_ms,
        end_ms: start_ms + 900,
        text: text.into(),
        speaker: "Them".into(),
        words: vec![TranscriptWord {
            text: text.into(),
            start_ms,
            end_ms: start_ms + 900,
        }],
    }
}
fn fts_rows(db: &Connection) -> Vec<(String, String, String, String)> {
    let mut q = db
        .prepare("SELECT meeting_id,source,ref_id,text FROM app_search_fts ORDER BY meeting_id,source,ref_id,text")
        .unwrap();
    q.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}
fn segment_ids(db: &Connection, run: &str) -> Vec<String> {
    let mut q = db
        .prepare("SELECT id FROM app_run_segments WHERE run_id=?1 ORDER BY start_ms,id")
        .unwrap();
    q.query_map([run], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

/// Mechanism record for the lost live transcript: the original live save was a
/// DEFERRED transaction that read the active run before writing. In WAL, once
/// another connection commits in between, the read->write upgrade fails at once
/// with SQLITE_BUSY_SNAPSHOT (517), never consulting the busy handler.
#[test]
fn old_save_segment_shape_fails_with_busy_snapshot_after_a_concurrent_commit() {
    let temp = fixture();
    let root = temp.path();
    let mut live = open(root).unwrap();
    let other = open(root).unwrap();
    let tx = live.transaction().unwrap();
    let run: String = tx.query_row("SELECT r.id FROM app_processing_runs r JOIN app_meeting_revision v ON v.active_run_id=r.id WHERE v.meeting_id=?1 AND r.kind='live' AND r.status='running'",["fixture"],|r|r.get(0)).unwrap();
    // The recorder (or any housekeeping write) commits between read and write.
    other
        .execute(
            "UPDATE app_meetings SET duration_ms=duration_ms+1 WHERE id='fixture'",
            [],
        )
        .unwrap();
    let started = std::time::Instant::now();
    let error = insert_segments(&tx, &run, &[line("late", 0, "Late line")]).unwrap_err();
    assert!(started.elapsed() < Duration::from_secs(1));
    assert!(sqlite_busy(&error));
    assert_eq!(
        sqlite_codes(&error).map(|(_, extended)| extended),
        Some(517)
    );
    assert_eq!(
        describe_error(&error),
        "database is locked (SQLite extended code 517 SQLITE_BUSY_SNAPSHOT)"
    );
}

#[test]
fn live_segments_save_while_another_connection_commits_continuously() {
    let temp = fixture();
    let root = temp.path().to_path_buf();
    let run = live_run(&root);
    let stop = Arc::new(AtomicBool::new(false));
    let writer = {
        let root = root.clone();
        let stop = stop.clone();
        std::thread::spawn(move || {
            // Mirrors the recorder's autocommit, synchronous=FULL writes at a far
            // higher rate than its real ~1 commit/s.
            let db = open(&root).unwrap();
            db.pragma_update(None, "synchronous", "FULL").unwrap();
            let mut commits = 0u64;
            while !stop.load(Ordering::Relaxed) {
                db.execute(
                    "UPDATE app_meetings SET duration_ms=duration_ms+1 WHERE id='fixture'",
                    [],
                )
                .unwrap();
                commits += 1;
                std::thread::sleep(Duration::from_millis(1));
            }
            commits
        })
    };
    let mut failures = Vec::new();
    let mut expected = Vec::new();
    for i in 0..150u64 {
        let id = format!("line-{i:03}");
        let text = if i == 77 {
            "Quarterly zebracorn milestone".to_owned()
        } else {
            format!("Synthetic live line {i}")
        };
        match save_live_segments(
            &root,
            "fixture",
            &run,
            &[line(&id, i * 1000, &text)],
            LiveSaveBudget::PRODUCTION,
        ) {
            Ok(added) => assert_eq!(added, 1),
            Err(error) => failures.push(describe_error(&error)),
        }
        expected.push(id);
    }
    stop.store(true, Ordering::Relaxed);
    let commits = writer.join().unwrap();
    assert!(failures.is_empty(), "{failures:?}");
    assert!(commits > 0);
    let db = open(&root).unwrap();
    assert_eq!(segment_ids(&db, &run), expected);
    let found = search_meetings(&root, "zebracorn").unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].id, "fixture");
    assert_eq!(search_meetings(&root, "Synthetic").unwrap().len(), 1);
}

fn insert_summary(db: &Connection, id: &str, run: &str, stale: bool, text: &str) {
    db.execute("INSERT INTO app_summaries(id,meeting_id,run_id,correction_version,provider,model,template,content_json,content_md,is_stale,created_at) VALUES (?1,'fixture',?2,0,'synthetic','synthetic','default','{}',?3,?4,'2026-01-01T00:00:00Z')",params![id,run,text,stale]).unwrap();
}
fn rebuilt(root: &Path) -> Vec<(String, String, String, String)> {
    let mut db = open(root).unwrap();
    let tx = db.transaction().unwrap();
    rebuild_fts(&tx, "fixture").unwrap();
    let rows = fts_rows(&tx);
    tx.rollback().unwrap();
    rows
}

/// The incremental live path leaves exactly the search rows `rebuild_fts`
/// would derive: title, notes, segments, and no summary once it is stale.
#[test]
fn incremental_search_rows_equal_rebuild_including_stale_summaries() {
    let temp = fixture();
    let root = temp.path();
    let run = live_run(root);
    save_note(root, "fixture", 0, "Synthetic note text").unwrap();
    save_segment(root, "fixture", &line("before", 0, "Earlier line")).unwrap();
    {
        let db = open(root).unwrap();
        insert_summary(&db, "fresh", &run, false, "Fresh summary words");
        insert_summary(&db, "old", &run, true, "Old summary words");
        let mut db = db;
        let tx = db.transaction().unwrap();
        rebuild_fts(&tx, "fixture").unwrap();
        tx.commit().unwrap();
        let rows = fts_rows(&db);
        assert!(rows.iter().any(|r| r.1 == "summary" && r.2 == "fresh"));
        assert!(!rows.iter().any(|r| r.2 == "old"));
    }
    let batch = [
        line("next-1", 1000, "Second line"),
        line("next-2", 2000, "Third line"),
    ];
    assert_eq!(
        save_live_segments(root, "fixture", &run, &batch, LiveSaveBudget::PRODUCTION).unwrap(),
        2
    );
    let db = open(root).unwrap();
    let incremental = fts_rows(&db);
    assert_eq!(incremental, rebuilt(root));
    assert!(!incremental.iter().any(|r| r.1 == "summary"));
    let stale: i64 = db
        .query_row(
            "SELECT COUNT(*) FROM app_summaries WHERE meeting_id='fixture' AND is_stale=1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(stale, 2);

    // A retried batch is idempotent: no duplicate rows or search entries.
    assert_eq!(
        save_live_segments(root, "fixture", &run, &batch, LiveSaveBudget::PRODUCTION).unwrap(),
        0
    );
    assert_eq!(fts_rows(&db), incremental);
    assert_eq!(segment_ids(&db, &run), ["before", "next-1", "next-2"]);
}

#[test]
fn incremental_search_rows_equal_rebuild_without_summaries() {
    let temp = fixture();
    let root = temp.path();
    let run = live_run(root);
    save_note(root, "fixture", 0, "Synthetic note text").unwrap();
    for i in 0..3u64 {
        save_live_segments(
            root,
            "fixture",
            &run,
            &[line(&format!("only-{i}"), i * 1000, "Live words")],
            LiveSaveBudget::PRODUCTION,
        )
        .unwrap();
    }
    assert_eq!(fts_rows(&open(root).unwrap()), rebuilt(root));
}

#[test]
fn deterministic_live_save_errors_are_not_lock_errors() {
    let temp = fixture();
    let root = temp.path();
    let run = live_run(root);
    let mut bad = line("bad", 1000, "Bad words");
    bad.words[0].end_ms = 5000;
    let error =
        save_live_segments(root, "fixture", &run, &[bad], LiveSaveBudget::PRODUCTION).unwrap_err();
    assert!(!sqlite_busy(&error));
    assert_eq!(error.to_string(), "Word timestamp is outside its segment.");
    complete_live_run(root, "fixture").unwrap();
    let error = save_live_segments(
        root,
        "fixture",
        &run,
        &[line("late", 0, "Late")],
        LiveSaveBudget::PRODUCTION,
    )
    .unwrap_err();
    assert!(!sqlite_busy(&error));
    assert_eq!(error.to_string(), "Only running runs accept segments.");
}

#[test]
fn live_save_budget_waits_about_one_second_at_most() {
    let worst = LiveSaveBudget::PRODUCTION.worst_case();
    assert_eq!(worst, Duration::from_millis(1_050));
    // A held write lock costs the whole budget, then reports lock contention.
    let temp = fixture();
    let root = temp.path();
    let run = live_run(root);
    let blocker = open(root).unwrap();
    blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
    let started = std::time::Instant::now();
    let error = save_live_segments(
        root,
        "fixture",
        &run,
        &[line("held", 0, "Held")],
        LiveSaveBudget::PRODUCTION,
    )
    .unwrap_err();
    let waited = started.elapsed();
    blocker.execute_batch("ROLLBACK").unwrap();
    assert!(sqlite_busy(&error), "{}", describe_error(&error));
    assert!(waited >= Duration::from_millis(900), "{waited:?}");
    assert!(waited < Duration::from_millis(2_000), "{waited:?}");
}

/// The same race through the shared writer: the write lock is taken at BEGIN,
/// so a commit by another connection (the recorder) between the closure's read
/// and its write waits for the closure instead of breaking it.
#[test]
fn a_shared_write_cannot_lose_its_snapshot_to_a_concurrent_commit() {
    let temp = fixture();
    let root = temp.path().to_path_buf();
    let (tx_started, rx_started) = std::sync::mpsc::channel();
    let committed = write(&root, |tx| {
        let run: String = tx.query_row("SELECT r.id FROM app_processing_runs r JOIN app_meeting_revision v ON v.active_run_id=r.id WHERE v.meeting_id=?1 AND r.kind='live' AND r.status='running'",["fixture"],|r|r.get(0))?;
        let other = {
            let root = root.clone();
            std::thread::spawn(move || {
                let db = open(&root).unwrap();
                tx_started.send(()).unwrap();
                db.execute(
                    "UPDATE app_meetings SET duration_ms=duration_ms+1 WHERE id='fixture'",
                    [],
                )
            })
        };
        rx_started.recv().unwrap();
        std::thread::sleep(Duration::from_millis(100));
        insert_segments(tx, &run, &[line("late", 0, "Late line")])?;
        Ok(other)
    })
    .unwrap();
    assert_eq!(committed.join().unwrap().unwrap(), 1);
    assert_eq!(
        segment_ids(&open(&root).unwrap(), &live_run(&root)),
        ["late"]
    );
}

/// In-process contention: another writer closure holds the shared writer for
/// longer than the live budget. The save waits at most the budget, then
/// reports lock contention (so the transcript writer keeps the line queued).
#[test]
fn live_save_budget_also_bounds_waiting_for_the_shared_writer() {
    let temp = fixture();
    let root = temp.path().to_path_buf();
    let run = live_run(&root);
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    let holder = {
        let root = root.clone();
        std::thread::spawn(move || {
            write(&root, |_| {
                started_tx.send(()).unwrap();
                let _ = release_rx.recv();
                Ok(())
            })
        })
    };
    started_rx.recv().unwrap();
    let started = std::time::Instant::now();
    let error = save_live_segments(
        &root,
        "fixture",
        &run,
        &[line("held", 0, "Held")],
        LiveSaveBudget::PRODUCTION,
    )
    .unwrap_err();
    let waited = started.elapsed();
    release_tx.send(()).unwrap();
    holder.join().unwrap().unwrap();
    assert!(sqlite_busy(&error), "{}", describe_error(&error));
    assert!(waited >= Duration::from_millis(900), "{waited:?}");
    assert!(waited < Duration::from_millis(2_000), "{waited:?}");
    // Once released, the same save succeeds.
    assert_eq!(
        save_live_segments(
            &root,
            "fixture",
            &run,
            &[line("held", 0, "Held")],
            LiveSaveBudget::PRODUCTION
        )
        .unwrap(),
        1
    );
}

fn search_hits(root: &Path, query: &str) -> usize {
    search_meetings(root, query).unwrap().len()
}

/// During an active recording, user edits do not rebuild the whole meeting's
/// search rows in front of the recorder: notes and titles update their own
/// rows at once, other edits are deferred, and sealing the live run at stop
/// leaves exactly the rows a full rebuild derives.
#[test]
fn edits_during_a_recording_defer_the_search_rebuild_until_the_live_run_is_sealed() {
    let temp = fixture();
    let root = temp.path();
    let run = live_run(root);
    save_live_segments(
        root,
        "fixture",
        &run,
        &[line("one", 0, "Original wording")],
        LiveSaveBudget::PRODUCTION,
    )
    .unwrap();
    // Incremental: searchable immediately.
    save_note(root, "fixture", 0, "Notedzebra").unwrap();
    assert_eq!(search_hits(root, "Notedzebra"), 1);
    rename(root, "fixture", "Renamedokapi").unwrap();
    assert_eq!(search_hits(root, "Renamedokapi"), 1);
    assert_eq!(search_hits(root, "fixture"), 0, "old title row replaced");
    // Deferred: the correction is saved, its search row follows at stop.
    correct_segment(root, "fixture", &run, "one", "Correctedquokka").unwrap();
    assert_eq!(search_hits(root, "Correctedquokka"), 0);
    assert_eq!(
        detail(root, "fixture").unwrap().segments[0].text,
        "Correctedquokka"
    );
    complete_live_run(root, "fixture").unwrap();
    assert_eq!(search_hits(root, "Correctedquokka"), 1);
    assert_eq!(search_hits(root, "Original"), 0);
    assert_eq!(fts_rows(&open(root).unwrap()), rebuilt(root));
    // After the seal, edits rebuild in full again.
    correct_segment(root, "fixture", &run, "one", "Laterlemur").unwrap();
    assert_eq!(search_hits(root, "Laterlemur"), 1);
}

/// An interrupted recording catches up its deferred search rebuild at
/// startup recovery.
#[test]
fn startup_recovery_rebuilds_search_deferred_by_an_interrupted_recording() {
    let temp = fixture();
    let root = temp.path();
    let run = live_run(root);
    save_live_segments(
        root,
        "fixture",
        &run,
        &[line("one", 0, "Original wording")],
        LiveSaveBudget::PRODUCTION,
    )
    .unwrap();
    correct_segment(root, "fixture", &run, "one", "Recoveredquokka").unwrap();
    assert_eq!(search_hits(root, "Recoveredquokka"), 0);
    recover_interrupted(root).unwrap();
    assert_eq!(search_hits(root, "Recoveredquokka"), 1);
    assert_eq!(fts_rows(&open(root).unwrap()), rebuilt(root));
}
