use super::*;
use crate::store;
use std::{
    sync::{Arc, mpsc},
    thread,
    time::{Duration, Instant},
};
use tempfile::TempDir;

fn fresh() -> TempDir {
    let temp = TempDir::new().unwrap();
    write(temp.path(), |tx| {
        tx.execute_batch("CREATE TABLE t(id INTEGER PRIMARY KEY, who INTEGER, n INTEGER UNIQUE)")?;
        Ok(())
    })
    .unwrap();
    temp
}

fn count(root: &Path) -> i64 {
    read(root, |db| {
        Ok(db.query_row("SELECT COUNT(*) FROM t", [], |r| r.get(0))?)
    })
    .unwrap()
}

/// Holds the shared writer inside a write closure until the returned sender
/// is used (or dropped). Returns once the write transaction has begun.
fn hold_writer(root: &Path, insert: bool) -> (mpsc::Sender<()>, thread::JoinHandle<Result<()>>) {
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let root = root.to_path_buf();
    let handle = thread::spawn(move || {
        write(&root, |tx| {
            if insert {
                tx.execute("INSERT INTO t(who,n) VALUES (-1,-1)", [])?;
            }
            started_tx.send(()).unwrap();
            let _ = release_rx.recv();
            Ok(())
        })
    });
    started_rx.recv().unwrap();
    (release_tx, handle)
}

#[test]
fn the_writer_enables_wal_and_foreign_keys() {
    let temp = fresh();
    let mode: String = read(temp.path(), |db| {
        Ok(db.query_row("PRAGMA journal_mode", [], |r| r.get(0))?)
    })
    .unwrap();
    assert_eq!(mode, "wal");
    let fk: bool = write(temp.path(), |tx| {
        Ok(tx.query_row("PRAGMA foreign_keys", [], |r| r.get(0))?)
    })
    .unwrap();
    assert!(fk);
}

/// Many threads doing read-then-write transactions (the pattern that fails
/// with SQLITE_BUSY_SNAPSHOT on separate DEFERRED connections) plus readers:
/// no lock errors, every row present, and the read-derived values are exact
/// because writes are serialized.
#[test]
fn concurrent_writers_and_readers_never_collide() {
    let temp = fresh();
    let _service_handle = Writer::for_root(temp.path()).unwrap();
    let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let readers = (0..4)
        .map(|_| {
            let root = temp.path().to_path_buf();
            let done = done.clone();
            thread::spawn(move || {
                let mut reads = 0;
                while !done.load(std::sync::atomic::Ordering::Relaxed) {
                    read(&root, |db| {
                        let _: i64 = db.query_row("SELECT COUNT(*) FROM t", [], |r| r.get(0))?;
                        Ok(())
                    })
                    .unwrap();
                    reads += 1;
                }
                reads
            })
        })
        .collect::<Vec<_>>();
    let writers = (0..8)
        .map(|who| {
            let root = temp.path().to_path_buf();
            thread::spawn(move || {
                for _ in 0..50 {
                    write(&root, |tx| {
                        let next: i64 =
                            tx.query_row("SELECT COALESCE(MAX(n),0)+1 FROM t", [], |r| r.get(0))?;
                        tx.execute("INSERT INTO t(who,n) VALUES (?1,?2)", [who, next])?;
                        Ok(())
                    })
                    .unwrap();
                }
            })
        })
        .collect::<Vec<_>>();
    for writer in writers {
        writer.join().unwrap();
    }
    done.store(true, std::sync::atomic::Ordering::Relaxed);
    for reader in readers {
        assert!(reader.join().unwrap() > 0);
    }
    assert_eq!(count(temp.path()), 400);
    let (min, max): (i64, i64) = read(temp.path(), |db| {
        Ok(db.query_row("SELECT MIN(n),MAX(n) FROM t", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?)
    })
    .unwrap();
    assert_eq!((min, max), (1, 400));
}

#[test]
fn reads_see_the_pre_write_state_during_a_long_write_and_writes_after_it() {
    let temp = fresh();
    let (release, writer) = hold_writer(temp.path(), true);
    let started = Instant::now();
    assert_eq!(count(temp.path()), 0, "uncommitted rows are invisible");
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "a read never waits for the writer"
    );
    release.send(()).unwrap();
    writer.join().unwrap().unwrap();
    assert_eq!(count(temp.path()), 1);
    // Read-your-writes: a read after `write` returns sees its commit.
    write(temp.path(), |tx| {
        tx.execute("INSERT INTO t(who,n) VALUES (0,7)", [])?;
        Ok(())
    })
    .unwrap();
    assert_eq!(count(temp.path()), 2);
}

#[test]
fn reads_are_read_only() {
    let temp = fresh();
    let error = read(temp.path(), |db| {
        db.execute("INSERT INTO t(who,n) VALUES (0,1)", [])?;
        Ok(())
    })
    .unwrap_err();
    assert!(error.to_string().contains("readonly"), "{error}");
    assert_eq!(count(temp.path()), 0);
}

#[test]
fn nested_access_fails_fast_with_a_clear_error_and_the_outer_write_still_commits() {
    let temp = fresh();
    let root = temp.path();
    let started = Instant::now();
    write(root, |tx| {
        tx.execute("INSERT INTO t(who,n) VALUES (0,1)", [])?;
        let nested = write(root, |_| Ok(())).unwrap_err();
        assert_eq!(
            nested.downcast_ref::<NestedAccess>(),
            Some(&NestedAccess {
                outer: "write",
                inner: "write"
            })
        );
        assert!(nested.to_string().contains("pass the open transaction"));
        let nested = read(root, |_| Ok(())).unwrap_err();
        assert_eq!(
            nested.downcast_ref::<NestedAccess>(),
            Some(&NestedAccess {
                outer: "write",
                inner: "read"
            })
        );
        // A handle held directly is no way around the rule.
        let handle = Writer::for_root(root);
        assert!(handle.is_err() || handle.unwrap().write_with(WRITE_WAIT, |_| Ok(())).is_err());
        Ok(())
    })
    .unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "no deadlock or wait"
    );
    read(root, |_| {
        let nested = write(root, |_| Ok(())).unwrap_err();
        assert_eq!(
            nested.downcast_ref::<NestedAccess>(),
            Some(&NestedAccess {
                outer: "read",
                inner: "write"
            })
        );
        Ok(())
    })
    .unwrap();
    assert_eq!(count(root), 1);
    // Store helpers that write are rejected the same way.
    let error = write(root, |_| store::set_status(root, "m", "ready", None)).unwrap_err();
    assert!(error.downcast_ref::<NestedAccess>().is_some());
}

#[test]
fn a_failed_or_panicking_closure_rolls_back_and_releases_the_writer() {
    let temp = fresh();
    let root = temp.path().to_path_buf();
    let _handle = Writer::for_root(&root).unwrap();
    let failed: Result<()> = write(&root, |tx| {
        tx.execute("INSERT INTO t(who,n) VALUES (0,1)", [])?;
        anyhow::bail!("rejected")
    });
    assert!(failed.is_err());
    let panicked = thread::spawn({
        let root = root.clone();
        move || {
            let _ = write(&root, |tx| -> Result<()> {
                tx.execute("INSERT INTO t(who,n) VALUES (0,2)", [])?;
                panic!("closure panicked");
            });
        }
    })
    .join();
    assert!(panicked.is_err());
    assert_eq!(count(&root), 0);
    write(&root, |tx| {
        tx.execute("INSERT INTO t(who,n) VALUES (0,3)", [])?;
        Ok(())
    })
    .unwrap();
    assert_eq!(count(&root), 1);
}

#[test]
fn a_bounded_wait_for_a_busy_writer_reports_sqlite_busy() {
    let temp = fresh();
    let (release, writer) = hold_writer(temp.path(), false);
    let started = Instant::now();
    let error = write_with(temp.path(), Duration::from_millis(50), |_| Ok(())).unwrap_err();
    assert!(started.elapsed() < Duration::from_secs(1));
    assert!(store::sqlite_busy(&error), "{error:#}");
    release.send(()).unwrap();
    writer.join().unwrap().unwrap();
}

#[test]
fn application_locks_may_be_held_around_database_access() {
    let temp = fresh();
    let runtime = OrderedMutex::new("runtime", 0);
    let guard = runtime.lock().unwrap();
    write(temp.path(), |tx| {
        tx.execute("INSERT INTO t(who,n) VALUES (0,1)", [])?;
        Ok(())
    })
    .unwrap();
    assert_eq!(count(temp.path()), 1);
    drop(guard);
}

#[cfg(debug_assertions)]
#[test]
#[should_panic(
    expected = "lock-order violation: `runtime` acquired inside a database write closure"
)]
fn acquiring_an_application_lock_inside_a_write_closure_is_caught() {
    let temp = fresh();
    let runtime = OrderedMutex::new("runtime", 0);
    let _ = write(temp.path(), |_| {
        let _guard = runtime.lock();
        Ok(())
    });
}

#[cfg(debug_assertions)]
#[test]
#[should_panic(
    expected = "lock-order violation: `backlog` acquired inside a database read closure"
)]
fn acquiring_an_application_lock_inside_a_read_closure_is_caught() {
    let temp = fresh();
    let backlog = OrderedMutex::new("backlog", 0);
    let _ = read(temp.path(), |_| {
        let _guard = backlog.lock();
        Ok(())
    });
}

#[cfg(unix)]
#[test]
fn every_spelling_of_one_folder_resolves_to_one_writer() {
    let temp = TempDir::new().unwrap();
    let real = temp.path().join("library");
    std::fs::create_dir(&real).unwrap();
    let link = temp.path().join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let first = Writer::for_root(&real).unwrap();
    assert!(Arc::ptr_eq(&first, &Writer::for_root(&link).unwrap()));
    assert!(Arc::ptr_eq(
        &first,
        &Writer::for_root(&real.join(".")).unwrap()
    ));
    // macOS: /var is a symlink to /private/var.
    if let Ok(rest) = real.strip_prefix("/var") {
        let private = Path::new("/private/var").join(rest);
        assert!(Arc::ptr_eq(&first, &Writer::for_root(&private).unwrap()));
    }
    if let Ok(rest) = real.strip_prefix("/private/var") {
        let public = Path::new("/var").join(rest);
        assert!(Arc::ptr_eq(&first, &Writer::for_root(&public).unwrap()));
    }
}

#[cfg(unix)]
#[test]
fn a_service_and_raw_test_paths_share_the_service_writer() {
    let temp = TempDir::new().unwrap();
    let helper = temp.path().join("helper");
    std::fs::write(&helper, b"unused").unwrap();
    let service = crate::AppService::new(temp.path().join("data"), helper).unwrap();
    let raw = temp.path().join("data");
    assert!(Arc::ptr_eq(
        &service._db_writer,
        &Writer::for_root(&raw).unwrap()
    ));
}

#[test]
fn a_restart_reuses_a_lingering_writer_and_never_opens_a_second_one() {
    let temp = fresh();
    let root = temp.path();
    let old_service = Writer::for_root(root).unwrap();
    // A thread of the old instance still holds the writer briefly.
    let lingering = Arc::clone(&old_service);
    drop(old_service);
    let new_service = Writer::for_root(root).unwrap();
    assert!(Arc::ptr_eq(&lingering, &new_service));
    drop(lingering);
    drop(new_service);
    // Once every handle is gone a fresh writer opens and works.
    let reopened = Writer::for_root(root).unwrap();
    reopened
        .write_with(WRITE_WAIT, |tx| {
            tx.execute("INSERT INTO t(who,n) VALUES (0,1)", [])?;
            Ok(())
        })
        .unwrap();
    assert_eq!(count(root), 1);
}

#[cfg(unix)]
#[test]
fn an_app_service_restart_shares_the_writer_with_threads_of_the_old_instance() {
    let temp = TempDir::new().unwrap();
    let helper = temp.path().join("helper");
    std::fs::write(&helper, b"unused").unwrap();
    let data = temp.path().join("data");
    let old = crate::AppService::new(data.clone(), helper.clone()).unwrap();
    let lingering = Arc::clone(&old._db_writer);
    drop(old);
    let new = crate::AppService::new(data, helper).unwrap();
    assert!(Arc::ptr_eq(&lingering, &new._db_writer));
}

fn recorder_fixture() -> (TempDir, vt_record::Recording) {
    let temp = TempDir::new().unwrap();
    store::initialize(temp.path()).unwrap();
    write(temp.path(), |tx| {
        tx.execute_batch("CREATE TABLE t(id INTEGER PRIMARY KEY, who INTEGER, n INTEGER UNIQUE)")?;
        Ok(())
    })
    .unwrap();
    let mut recording = vt_record::Recording::create(temp.path(), "recorded").unwrap();
    recording.set_lock_budget(Duration::from_secs(10));
    (temp, recording)
}

fn block(first_frame: u64) -> vt_core::AudioBlock {
    vt_core::AudioBlock::new(
        vt_core::StreamFormat::new(8000, 1).unwrap(),
        vt_core::FramePosition {
            epoch: 0,
            first_frame,
            host_time_ns: first_frame * 125_000,
        },
        vec![0.25; 800],
    )
    .unwrap()
}

/// The recorder keeps its own durable connection. While the shared writer
/// holds a long write, a chunk write waits (within its retry budget) and then
/// succeeds.
#[test]
fn the_recorder_records_through_a_long_shared_write() {
    let (temp, mut recording) = recorder_fixture();
    let (release, writer) = hold_writer(temp.path(), true);
    let releaser = thread::spawn(move || {
        thread::sleep(Duration::from_millis(600));
        release.send(()).unwrap();
    });
    let started = Instant::now();
    recording
        .write_chunk(vt_core::TrackKind::Mic, &block(0))
        .unwrap();
    assert!(started.elapsed() >= Duration::from_millis(400));
    releaser.join().unwrap();
    writer.join().unwrap().unwrap();
    assert_eq!(recording.chunks().unwrap().len(), 1);
    assert_eq!(count(temp.path()), 1);
}

/// And the shared writer keeps working while the recorder flushes chunks.
#[test]
fn the_shared_writer_works_while_the_recorder_flushes() {
    let (temp, mut recording) = recorder_fixture();
    let recorder = thread::spawn(move || {
        for i in 0..40 {
            recording
                .write_chunk(vt_core::TrackKind::Mic, &block(i * 800))
                .unwrap();
        }
        recording.chunks().unwrap().len()
    });
    for n in 0..100 {
        write(temp.path(), |tx| {
            tx.execute("INSERT INTO t(who,n) VALUES (0,?1)", [n])?;
            Ok(())
        })
        .unwrap();
    }
    assert_eq!(recorder.join().unwrap(), 40);
    assert_eq!(count(temp.path()), 100);
}

/// Every `.rs` file under `dir`, recursively (build output skipped).
fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            if path
                .file_name()
                .is_some_and(|n| n != "target" && n != "node_modules")
            {
                rust_files(&path, out);
            }
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// Files compiled only for tests: integration `tests/`/`benches/` trees and
/// modules declared under `#[cfg(test)]` (with or without `#[path]`),
/// transitively.
fn test_only_files(files: &[PathBuf]) -> std::collections::BTreeSet<PathBuf> {
    let mut test_only = files
        .iter()
        .filter(|path| {
            path.components()
                .any(|c| c.as_os_str() == "tests" || c.as_os_str() == "benches")
        })
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    let declared = |file: &Path, all: bool| -> Vec<PathBuf> {
        let source = std::fs::read_to_string(file).unwrap();
        let lines = source.lines().map(str::trim).collect::<Vec<_>>();
        let dir = file.parent().unwrap();
        let stem = file.file_stem().unwrap().to_string_lossy().to_string();
        let child_dir = if matches!(stem.as_str(), "lib" | "main" | "mod") {
            dir.to_path_buf()
        } else {
            dir.join(&stem)
        };
        let mut out = Vec::new();
        for (i, line) in lines.iter().enumerate() {
            let Some(name) = line
                .strip_prefix("mod ")
                .or_else(|| line.strip_prefix("pub mod "))
                .or_else(|| line.strip_prefix("pub(crate) mod "))
                .and_then(|rest| rest.strip_suffix(';'))
            else {
                continue;
            };
            let attrs = lines[i.saturating_sub(3)..i].to_vec();
            if !all && !attrs.contains(&"#[cfg(test)]") {
                continue;
            }
            let path = attrs.iter().find_map(|a| {
                a.strip_prefix("#[path = \"")
                    .and_then(|rest| rest.strip_suffix("\"]"))
            });
            out.push(match path {
                Some(path) => dir.join(path),
                None => child_dir.join(format!("{name}.rs")),
            });
        }
        out
    };
    for file in files {
        test_only.extend(declared(file, false));
    }
    loop {
        let more = test_only
            .iter()
            .flat_map(|file| declared(file, true))
            .filter(|file| !test_only.contains(file))
            .collect::<Vec<_>>();
        if more.is_empty() {
            break;
        }
        test_only.extend(more);
    }
    test_only
        .into_iter()
        .filter_map(|p| p.canonicalize().ok())
        .collect()
}

/// Source-level guard: no production code anywhere in the workspace opens a
/// connection to the main database except the shared writer and query_only
/// readers (vt-session db.rs), the recorder's own connection (vt-record) and
/// vt-playback's read-only manifest reader. Test-only files are skipped; a
/// `*_tests.rs` file that is not compiled only for tests is scanned.
#[test]
fn no_production_code_opens_its_own_database_connection() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let mut files = Vec::new();
    rust_files(&workspace.join("crates"), &mut files);
    rust_files(&workspace.join("src-tauri"), &mut files);
    let files = files
        .into_iter()
        .map(|p| p.canonicalize().unwrap())
        .collect::<Vec<_>>();
    let test_only = test_only_files(&files);
    // Sanity: the detector sees the test modules it must skip.
    for known in [
        "crates/vt-session/src/db_tests.rs",
        "crates/vt-session/src/tests.rs",
        "crates/vt-session/src/live_segment_store_tests.rs",
        "crates/vt-record/src/tests.rs",
    ] {
        assert!(
            test_only.contains(&workspace.join(known)),
            "{known} not detected as test-only"
        );
    }
    assert!(!test_only.contains(&workspace.join("crates/vt-session/src/store.rs")));
    let forbidden = [
        "Connection::open(",
        "open_with_flags",
        "open_in_memory",
        "Connection as ",
        "= Connection;",
        "= rusqlite::Connection",
        "sqlite3_open",
        "store::open(",
        "store::open_with_timeout(",
        "super::open(",
        "super::open_with_timeout(",
        "super::store::open(",
    ];
    // (file, pattern, occurrences, reason). Shrink, never grow, this list.
    let allowed: &[(&str, &str, usize, &str)] = &[
        (
            "crates/vt-session/src/db.rs",
            "Connection::open(",
            1,
            "the shared writer",
        ),
        (
            "crates/vt-session/src/db.rs",
            "open_with_flags",
            1,
            "query_only reader connections",
        ),
        (
            "crates/vt-session/src/store.rs",
            "Connection::open(",
            1,
            "#[cfg(test)] lock-holder helper",
        ),
        (
            "crates/vt-record/src/lib.rs",
            "Connection::open(",
            1,
            "the recorder's durable connection",
        ),
        (
            "crates/vt-playback/src/manifest.rs",
            "open_with_flags",
            1,
            "read-only manifest reader",
        ),
    ];
    let mut violations = Vec::new();
    let mut scanned = 0;
    for file in &files {
        if test_only.contains(file) {
            continue;
        }
        scanned += 1;
        let name = file
            .strip_prefix(&workspace)
            .unwrap()
            .to_string_lossy()
            .to_string();
        let source = without_inline_test_modules(&std::fs::read_to_string(file).unwrap());
        for pattern in forbidden {
            let found = source.matches(pattern).count();
            let permitted = allowed
                .iter()
                .filter(|(f, p, _, _)| *f == name && *p == pattern)
                .map(|(_, _, n, _)| *n)
                .sum::<usize>();
            if found != permitted {
                violations.push(format!("{name}: {pattern} x{found} (allowed {permitted})"));
            }
        }
    }
    assert!(scanned > 50, "scanned only {scanned} files");
    assert!(
        violations.is_empty(),
        "unexpected database connections: {violations:#?}"
    );
    // vt-playback may open the main database only read-only.
    let manifest =
        std::fs::read_to_string(workspace.join("crates/vt-playback/src/manifest.rs")).unwrap();
    let at = manifest.find("open_with_flags").unwrap();
    let call = &manifest[at..at + manifest[at..].find(")?").unwrap()];
    assert!(call.contains("vantranscribe.db"), "{call}");
    assert!(call.contains("SQLITE_OPEN_READ_ONLY"), "{call}");
    assert!(
        !call.contains("READ_WRITE") && !call.contains("CREATE"),
        "{call}"
    );
    // The plain-connection helpers in store.rs exist only for tests.
    let store = std::fs::read_to_string(workspace.join("crates/vt-session/src/store.rs")).unwrap();
    for helper in [
        "pub(crate) fn open(root: &Path)",
        "pub(crate) fn open_with_timeout(",
    ] {
        let at = store.find(helper).unwrap();
        assert!(
            store[..at].trim_end().ends_with("#[cfg(test)]"),
            "{helper} must stay #[cfg(test)]"
        );
    }
}

/// Removes `#[cfg(test)] mod name { ... }` blocks (closed by a `}` line at
/// column 0, as rustfmt writes them).
fn without_inline_test_modules(source: &str) -> String {
    let mut out = String::new();
    let mut lines = source.lines().peekable();
    while let Some(line) = lines.next() {
        if line == "#[cfg(test)]"
            && lines
                .peek()
                .is_some_and(|next| next.starts_with("mod ") && next.ends_with('{'))
        {
            for skipped in lines.by_ref() {
                if skipped == "}" {
                    break;
                }
            }
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// The longest store transactions in ordinary use: publishing a long refined
/// transcript (corrections, bleed, full search rebuild) and the whole-meeting
/// search rebuild that now runs once when a live run is sealed. Reports times.
#[test]
fn long_meeting_publish_and_search_rebuild_times() {
    for count in [3_000u64, 10_000] {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        store::initialize(root).unwrap();
        let recording = vt_record::Recording::create(root, "long").unwrap();
        store::create_meeting(root, "long", "Long meeting", "remote").unwrap();
        recording.finish().unwrap();
        // Staged in one fixture transaction (the test's own setup).
        write(root, |tx| {
            for i in 0..count {
                tx.execute(
                    "INSERT INTO app_transcription_staging(job_id,id,meeting_id,track,start_ms,end_ms,text,speaker,words_json) VALUES ('job',?1,'long',?2,?3,?4,?5,'','[]')",
                    rusqlite::params![
                        format!("s{i}"),
                        if i % 2 == 0 { "mic" } else { "system" },
                        (i * 1000) as i64,
                        (i * 1000 + 900) as i64,
                        format!("segment number {i} with several words of searchable text"),
                    ],
                )?;
            }
            Ok(())
        })
        .unwrap();
        let started = Instant::now();
        store::publish_staged(root, "job", "long").unwrap();
        let publish = started.elapsed();
        let started = Instant::now();
        store::rebuild_search_with(root, "long", WRITE_WAIT).unwrap();
        let rebuild = started.elapsed();
        eprintln!("{count} segments: publish {publish:?}, rebuild_fts {rebuild:?}");
        assert!(publish < Duration::from_secs(20) && rebuild < Duration::from_secs(20));
        assert_eq!(
            store::detail(root, "long").unwrap().segments.len(),
            count as usize
        );
    }
}

/// A busy in-process writer costs the capture loop a few ms per housekeeping
/// write, not its whole `LOOP_BUSY_TIMEOUT`.
#[test]
fn a_busy_writer_costs_the_capture_loop_a_few_ms_per_write() {
    let temp = fresh();
    let root = temp.path();
    let (release, holder) = hold_writer(root, false);
    let started = Instant::now();
    let error = write_with(root, store::LOOP_BUSY_TIMEOUT, |_| Ok(())).unwrap_err();
    let off_loop = started.elapsed();
    assert!(store::sqlite_busy(&error));
    assert!(off_loop >= Duration::from_millis(200), "{off_loop:?}");
    let per_write = {
        let _scope = CaptureLoopScope::enter();
        let started = Instant::now();
        for _ in 0..20 {
            let error = write_with(root, store::LOOP_BUSY_TIMEOUT, |_| Ok(())).unwrap_err();
            assert!(store::sqlite_busy(&error));
        }
        started.elapsed() / 20
    };
    eprintln!(
        "capture-loop write against a busy writer: {per_write:?} (off the loop: {off_loop:?})"
    );
    assert!(per_write < Duration::from_millis(30), "{per_write:?}");
    release.send(()).unwrap();
    holder.join().unwrap().unwrap();
    // With the writer free, loop writes succeed as before.
    let _scope = CaptureLoopScope::enter();
    write_with(root, store::LOOP_BUSY_TIMEOUT, |tx| {
        tx.execute("INSERT INTO t(who,n) VALUES (0,1)", [])?;
        Ok(())
    })
    .unwrap();
}

/// The SQLite lock wait on the loop keeps the caller's budget: another
/// connection (the recorder) holding the lock still costs up to 250 ms, as
/// before this change.
#[test]
fn the_capture_loop_still_waits_its_budget_for_the_recorder_lock() {
    let temp = fresh();
    let root = temp.path();
    let blocker = store::open(root).unwrap();
    blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
    let _scope = CaptureLoopScope::enter();
    let started = Instant::now();
    let error = write_with(root, store::LOOP_BUSY_TIMEOUT, |_| Ok(())).unwrap_err();
    let waited = started.elapsed();
    blocker.execute_batch("ROLLBACK").unwrap();
    assert!(store::sqlite_busy(&error));
    assert!(
        waited >= Duration::from_millis(200) && waited < Duration::from_secs(1),
        "{waited:?}"
    );
}

/// A connection that cannot be returned to autocommit (a failed ROLLBACK) is
/// discarded and reopened, instead of wedging the only writer with
/// "cannot start a transaction within a transaction".
#[test]
fn a_failed_rollback_discards_the_connection_and_the_next_write_reopens_it() {
    let temp = fresh();
    let writer = Writer::for_root(temp.path()).unwrap();
    let before = REOPENED.load(std::sync::atomic::Ordering::Relaxed);
    SIMULATE_FAILED_ROLLBACK.with(|flag| flag.set(true));
    writer
        .write_with(WRITE_WAIT, |tx| {
            tx.execute("INSERT INTO t(who,n) VALUES (0,1)", [])?;
            Ok(())
        })
        .unwrap();
    let fk: bool = writer
        .write_with(WRITE_WAIT, |tx| {
            tx.execute("INSERT INTO t(who,n) VALUES (0,2)", [])?;
            Ok(tx.query_row("PRAGMA foreign_keys", [], |r| r.get(0))?)
        })
        .unwrap();
    assert!(fk, "the reopened connection keeps foreign keys on");
    assert!(REOPENED.load(std::sync::atomic::Ordering::Relaxed) > before);
    assert_eq!(count(temp.path()), 2);
    let mode: String = read(temp.path(), |db| {
        Ok(db.query_row("PRAGMA journal_mode", [], |r| r.get(0))?)
    })
    .unwrap();
    assert_eq!(mode, "wal");
}

#[test]
fn writes_under_an_application_lock_wait_at_most_five_seconds() {
    assert_eq!(default_wait(), WRITE_WAIT);
    let runtime = OrderedMutex::new("runtime", 0);
    let backlog = OrderedMutex::new("backlog", 0);
    {
        let _runtime = runtime.lock().unwrap();
        assert_eq!(default_wait(), HELD_LOCK_WRITE_WAIT);
        {
            let _backlog = backlog.lock().unwrap();
            assert_eq!(held_app_locks(), 2);
        }
        assert_eq!(default_wait(), HELD_LOCK_WRITE_WAIT);
    }
    assert_eq!(held_app_locks(), 0);
    assert_eq!(default_wait(), WRITE_WAIT);
    // A poisoned lock still counts while its guard is held.
    let poisoned = Arc::new(OrderedMutex::new("runtime", 0));
    let clone = poisoned.clone();
    let _ = thread::spawn(move || {
        let _guard = clone.lock();
        panic!("poison");
    })
    .join();
    let guard = poisoned.lock().unwrap_or_else(|e| e.into_inner());
    assert_eq!(default_wait(), HELD_LOCK_WRITE_WAIT);
    drop(guard);
    assert_eq!(default_wait(), WRITE_WAIT);
}

#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "database write waiting up to 30s while holding `runtime`/`backlog`")]
fn a_long_explicit_write_wait_under_an_application_lock_is_caught() {
    let temp = fresh();
    let runtime = OrderedMutex::new("runtime", 0);
    let _guard = runtime.lock().unwrap();
    let _ = write_with(temp.path(), WRITE_WAIT, |_| Ok(()));
}

fn wait_for_queue(writer: &Writer, len: usize) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while writer.waiting() != len {
        assert!(Instant::now() < deadline, "queue never reached {len}");
        thread::sleep(Duration::from_millis(2));
    }
}

/// Spawns a writer thread that records `label` when it gets the writer, and
/// returns once it is queued.
fn queue_writer(
    writer: &Arc<Writer>,
    label: usize,
    wait: Duration,
    order: &Arc<Mutex<Vec<usize>>>,
) -> thread::JoinHandle<Result<()>> {
    let before = writer.waiting();
    let handle = {
        let writer = writer.clone();
        let order = order.clone();
        thread::spawn(move || {
            writer.write_with(wait, |_| {
                order.lock().unwrap().push(label);
                Ok(())
            })
        })
    };
    wait_for_queue(writer, before + 1);
    handle
}

#[test]
fn waiters_are_served_in_arrival_order() {
    let temp = fresh();
    let writer = Writer::for_root(temp.path()).unwrap();
    let order = Arc::new(Mutex::new(Vec::new()));
    let (release, holder) = hold_writer(temp.path(), false);
    let waiters = (0..8)
        .map(|i| queue_writer(&writer, i, WRITE_WAIT, &order))
        .collect::<Vec<_>>();
    release.send(()).unwrap();
    holder.join().unwrap().unwrap();
    for waiter in waiters {
        waiter.join().unwrap().unwrap();
    }
    assert_eq!(*order.lock().unwrap(), (0..8).collect::<Vec<_>>());
    assert_eq!(writer.waiting(), 0);
}

#[test]
fn a_short_wait_caller_that_arrives_first_is_served_first() {
    let temp = fresh();
    let writer = Writer::for_root(temp.path()).unwrap();
    let order = Arc::new(Mutex::new(Vec::new()));
    let (release, holder) = hold_writer(temp.path(), false);
    let short = queue_writer(&writer, 0, Duration::from_secs(2), &order);
    let long = queue_writer(&writer, 1, WRITE_WAIT, &order);
    release.send(()).unwrap();
    holder.join().unwrap().unwrap();
    short.join().unwrap().unwrap();
    long.join().unwrap().unwrap();
    assert_eq!(*order.lock().unwrap(), [0, 1]);
}

#[test]
fn a_timed_out_waiter_does_not_block_those_behind_it() {
    let temp = fresh();
    let writer = Writer::for_root(temp.path()).unwrap();
    let order = Arc::new(Mutex::new(Vec::new()));
    // Head of the queue times out while the writer is still held.
    let (release, holder) = hold_writer(temp.path(), false);
    let expired = queue_writer(&writer, 0, Duration::from_millis(100), &order);
    let behind = queue_writer(&writer, 1, WRITE_WAIT, &order);
    let error = expired.join().unwrap().unwrap_err();
    assert!(store::sqlite_busy(&error));
    wait_for_queue(&writer, 1);
    release.send(()).unwrap();
    holder.join().unwrap().unwrap();
    behind.join().unwrap().unwrap();
    // Head times out exactly while the writer is free but taken by nobody:
    // the next waiter must still get it promptly.
    let (release, holder) = hold_writer(temp.path(), false);
    let expired = queue_writer(&writer, 2, Duration::from_millis(150), &order);
    let behind = queue_writer(&writer, 3, WRITE_WAIT, &order);
    release.send(()).unwrap();
    holder.join().unwrap().unwrap();
    let started = Instant::now();
    let _ = expired.join().unwrap();
    behind.join().unwrap().unwrap();
    assert!(started.elapsed() < Duration::from_secs(1));
    assert_eq!(writer.waiting(), 0);
    let order = order.lock().unwrap();
    assert_eq!(order[0], 1);
    assert!(order.ends_with(&[3]));
}

#[test]
fn a_panicking_holder_hands_the_writer_to_the_next_waiter() {
    let temp = fresh();
    let writer = Writer::for_root(temp.path()).unwrap();
    let order = Arc::new(Mutex::new(Vec::new()));
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let panicking = {
        let writer = writer.clone();
        thread::spawn(move || {
            let _ = writer.write_with(WRITE_WAIT, |_| -> Result<()> {
                started_tx.send(()).unwrap();
                let _ = release_rx.recv();
                panic!("holder panicked");
            });
        })
    };
    started_rx.recv().unwrap();
    let next = queue_writer(&writer, 0, WRITE_WAIT, &order);
    release_tx.send(()).unwrap();
    assert!(panicking.join().is_err());
    next.join().unwrap().unwrap();
    assert_eq!(*order.lock().unwrap(), [0]);
}
