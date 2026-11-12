# Database access

Every in-process access to the main database `<root>/vantranscribe.db` in `crates/vt-session`, `crates/vt-speakers` and `src-tauri` goes through `crates/vt-session/src/db.rs` (re-exported by `store.rs` as `store::write`, `store::write_with` and `store::read`). The schema itself is documented by [processing runs](../processing-runs/processing-runs.md) and [desktop session](../../app/desktop-session/desktop-session.md#database-schema). The recorder (`crates/vt-record`) is the one exception, see [Recorder contention](#recorder-contention).

Updated after two adversarial reviews (see the trail entry). This is uncommitted work on `docs/initial-design`. No real recording has exercised it; coverage is the `db_tests.rs` tests below.

## Technical flow

**One writer per database file.** The connection is handed out strictly in arrival (FIFO) order through a ticket queue (`Slot.queue`); a waiter that times out removes its ticket and wakes the others; a panicking holder still returns the connection (`Lease::drop`). If a connection cannot be returned to autocommit after a failed rollback (`reset`), it is discarded ("database writer: ROLLBACK failed; discarding the connection to reopen it") and the next write reopens it with WAL and foreign keys re-established, so the writer is one connection at a time, not necessarily one for the process lifetime. `Writer::for_root(root)` returns the live `Arc<Writer>` for the canonicalized database path (`database_path`, so `/var/...` and `/private/var/...` share one writer), opening it if none is alive. The registry holds `Weak` pointers; while any handle is alive every lookup returns the same writer. `AppService` holds one (`_db_writer`) for its lifetime. `Writer::open` opens the connection, sets the busy timeout to `WRITE_WAIT`, runs `PRAGMA journal_mode=WAL` and fails with "The database could not enter WAL mode (journal_mode=<mode>)." if SQLite does not report `wal`, then enables `foreign_keys`.

**Writes.** `store::write(root, |tx| ...)` runs one `BEGIN IMMEDIATE` transaction on the writer's single connection: commit when the closure returns `Ok`, rollback on `Err` or panic (the `Lease` guard rolls back a transaction left open and returns the connection). `store::write_with(root, wait, ...)` bounds the total wait: first for the in-process writer (`checkout`), then, with what remains, for SQLite's write lock held by another connection (the recorder). Either expiry is reported as `SQLITE_BUSY` with the message "database is locked: the shared database writer stayed busy for <wait>" (writer expiry) or SQLite's own busy error, so `store::sqlite_busy` still classifies it as lock contention. The default `wait` is `WRITE_WAIT` (30 s). Wait budgets: while the calling thread holds an `OrderedMutex` (`runtime` or `backlog`; per-thread guard count `held_app_locks`) `store::write` uses `HELD_LOCK_WRITE_WAIT` (5 s, `default_wait`), and an explicit longer `write_with` panics in debug builds ("lock-order violation: database write waiting up to ..."). On the capture loop (`CaptureLoopScope`, entered in `session.rs`) waiting in the in-process queue is capped at `CAPTURE_LOOP_QUEUE_WAIT` (5 ms); the wait is one total budget and the SQLite lock wait (the recorder) gets the remainder, so `LOOP_BUSY_TIMEOUT` (250 ms) still bounds it.

**Reads.** `store::read(root, |conn| ...)` opens a fresh `query_only` connection (`READ_BUSY_TIMEOUT` 5 s), runs the closure inside one read transaction (a snapshot-consistent view) and commits. A read never waits for the writer and sees only committed data, so a read closure cannot see an uncommitted write made elsewhere.

**Rules.**

- No nested access: calling `write` or `read` inside a `write`/`read` closure on the same thread fails at once with `NestedAccess` ("nested database <inner> inside a database <outer> closure on the same thread; pass the open transaction to the helper instead"). Helpers take the open `&Transaction`/`&Connection`.
- The database is the innermost lock. `AppService::runtime` and `AppService::backlog` are `OrderedMutex` (`db.rs`) and may be held while calling `write`/`read` (with the 5 s cap above), but must never be acquired inside a closure; `OrderedMutex::lock` panics in debug builds ("lock-order violation: ... acquired inside a database <kind> closure").
- Closures do database work only: no file system, Keychain, network, sleeps or channel waits. This is a convention checked by review; only nesting and lock order are enforced at runtime.
- Debug builds print "slow database write: ..." for a write holding the writer longer than `SLOW_WRITE` (250 ms), splitting closure/commit time from time waiting for another connection's lock. `db::longest_write()` exposes the longest closure for tests.
- `store::open` is `#[cfg(test)]` only (a plain connection for lock-holder fixtures). The test `no_production_code_opens_its_own_database_connection` walks `crates/` recursively and `src-tauri`, skips only files it detects as test-only, and flags `Connection::open(`, `open_with_flags`, `open_in_memory`, aliased `Connection` imports (`Connection as `, `= Connection;`, `= rusqlite::Connection`), `sqlite3_open` and `store::open(` outside an allow-list: the writer and reader openers in `db.rs`, one test helper in `store.rs`, the recorder's durable connection in `vt-record`, and `vt-playback/src/manifest.rs`. It also asserts that vt-playback opens `vantranscribe.db` with `SQLITE_OPEN_READ_ONLY`.

## Behaviour that changed

| Before | Now |
|---|---|
| Each write opened its own connection; concurrent in-process writers failed with "database is locked" after a 5 s busy timeout | In-process writers queue on one connection; default wait is 30 s |
| Reads on ad-hoc connections, possibly spanning several statements without a snapshot | Each `read` closure is one snapshot-consistent transaction |
| Commands held `runtime` across their database write | Rename, notes, bleed review, speaker edits, corrections, voice-memory commands and others release `runtime` before writing (see [desktop session](../../app/desktop-session/desktop-session.md#database-access-from-commands)) |
| Every user edit rebuilt the whole meeting's search index | During an active recording only note/title rows are written; other edits defer the rebuild until stop (next section) |
| Only the recorder set WAL | The shared writer sets and verifies WAL for the main database |

## Restructured callers

| Caller | Shape |
|---|---|
| `retention::commit` | Preview read and `confirmable` check, then the recording lease (`lock_meeting`) and file inventory outside the database, then one confirming write that re-checks the preview and fingerprint. The lease is held until after that write commits; commit-before-delete order is unchanged |
| `retention::make_plan`, `preview_policy` | Reads and file inspection alternate; file work never runs inside a closure |
| `voice_memory::prepare_key` | Keychain `load`/`create` run outside every closure |
| `echo_store` route lookups | Run through `write_with`/`read` |
| `store::recover_interrupted` | Staging cleanup and abandoning running runs in one transaction |
| `preferences::load` | Runs on the writer because it creates the settings table |

## Search index during a recording

`store::recording_active` is true while the meeting is `recording` or `paused` and its active live run is `running`. In that state `save_note` and `rename` write only their own `app_search_fts` row, and every other edit (speaker rename, constraints, reviews, corrections, bleed reviews) calls `refresh_search_after_edit`, which skips the whole-meeting `rebuild_fts` and only deletes summary rows that are no longer current. The full rebuild runs once when the live run is sealed (`complete_live_run_with`), on a failed stop (`rebuild_search_with`) or at startup recovery (`recover_interrupted`). User-visible consequence: those edits made during a recording become searchable when the recording stops. Reason: a whole-meeting rebuild would hold the writer and SQLite's write lock in front of the recorder's chunk writes and, with `runtime` held, stall capture.

Measurement, not a guarantee: the test `long_meeting_publish_and_search_rebuild_times` (`db_tests.rs`, debug build, development machine) printed `rebuild_fts` about 37 ms for 3,000 segments and about 110 ms for 10,000; `publish_staged` about 60-85 ms and about 210 ms.

## Recorder contention

`vt-record` keeps its own connection (`synchronous=FULL`, `fullfsync=ON`) and still contends with the writer at the SQLite level. The writer's `wait` and the recorder's 60 s `MANIFEST_LOCK_BUDGET` retry cover that ([durable recording](../../recording/durable-recording/durable-recording.md#manifest-write-lock-tolerance)). The main database's synchronous level is unchanged. Stop-gap lock handling (`sqlite_busy`, `retry_busy`, `tolerate_busy`, `LOOP_BUSY_TIMEOUT` 250 ms, `LiveSaveBudget::PRODUCTION`) is kept and now rarely exercised; its removal is a later step.

## Known limits

- While the writer waits on the recorder's SQLite lock, every other in-process writer queues behind it.
- A write may still be made while `runtime` is held (recording start `create_meeting_with_engines`, enrollment's final write, `select_live_model`; none can run during capture); it waits at most 5 s, but that can hold up code needing `runtime`. The three meeting-detection settings commands hold the `meeting_detection` lock across a small write.
- The recorder now runs on its own `audio-recorder` thread behind a bounded in-memory queue (2026-11-11, see [durable recording](../../recording/durable-recording/durable-recording.md#recorder-thread-and-bounded-queue)); the capture loop no longer waits for its SQLite lock. The recorder still keeps its own connection. Its chunk-clock row and the `app_recorder_progress` marker go through this shared writer (the clock row from the recorder thread, the marker from the capture loop and at stop).
- The pattern follows the `sqlite-rwc` crate; it is not used because it is AGPL-3.0-only and this project is MIT.

## Key types and entry points

| Item | File | Responsibility |
|---|---|---|
| `Writer`, `Lease`, `write`, `write_with`, `read`, `database_path` | `crates/vt-session/src/db.rs` | Shared writer, transactions, snapshot reads, path identity |
| `NestedAccess`, `ScopeGuard`, `OrderedMutex`, `OrderedGuard`, `held_app_locks`, `CaptureLoopScope`, `assert_outside_db_closure` | `crates/vt-session/src/db.rs` | Nesting, lock-order and wait-budget enforcement |
| `WRITE_WAIT` 30 s, `HELD_LOCK_WRITE_WAIT` 5 s, `CAPTURE_LOOP_QUEUE_WAIT` 5 ms, `READ_BUSY_TIMEOUT` 5 s, `SLOW_WRITE` 250 ms, `DB_FILE` | `crates/vt-session/src/db.rs` | Constants |
| `store::{write, write_with, read}`, `sqlite_busy`, `retry_busy`, `LOOP_BUSY_TIMEOUT` | `crates/vt-session/src/store.rs` | Re-exports and the retained lock classification and retry helpers |

## Tests

`crates/vt-session/src/db_tests.rs` (27 tests) covers arrival-order handoff, a timed-out waiter not blocking those behind it, a panicking holder handing the writer on, a failed rollback discarding and reopening the connection, the 5 s cap under an application lock and the debug panic for a longer explicit wait, the capture-loop queue cap and its unchanged SQLite budget, WAL/foreign keys, concurrent writers and readers, snapshot reads during a long write, read-only readers, nested access failing fast while the outer write commits, rollback on error and panic, bounded wait reporting SQLITE_BUSY, application locks around (allowed) and inside (caught) closures, one writer for every spelling of a folder and across service restart, the recorder recording through a long shared write, the stray-connection scan and the long-meeting publish and search-rebuild timing test.

## Access and routes

Local only; no routes, tables or SQL artifacts. Access control is the OS account and the per-root desktop lock.
