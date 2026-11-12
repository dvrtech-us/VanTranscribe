# Database access baseline

## Invariants

- Production code in `vt-session`, `vt-speakers` and `src-tauri` never opens its own database connection; all access is `store::write`, `store::write_with` or `store::read`. The scan test `no_production_code_opens_its_own_database_connection` enforces this with an allow-list that may shrink, never grow.
- One `Writer` (one write connection at a time) exists per canonicalized database file while any handle is alive. `AppService` holds it for its lifetime. A connection that cannot return to autocommit after a failed rollback is discarded and the next write reopens it (WAL and foreign keys re-established).
- The writer is handed out in arrival (FIFO) order; a waiter that times out removes itself; a panicking holder still returns the connection.
- While the calling thread holds `runtime` or `backlog`, a write waits at most `HELD_LOCK_WRITE_WAIT` (5 s); an explicit longer `write_with` panics in debug builds.
- On the capture loop the in-process queue wait is at most `CAPTURE_LOOP_QUEUE_WAIT` (5 ms); the whole wait is one budget and the SQLite lock wait gets the remainder.
- During an active recording (`recording_active`), only note and title edits write their own search row; all other edits skip the whole-meeting search rebuild and drop only non-current summary rows. The full rebuild runs once at stop (`complete_live_run_with`), on a failed stop (`rebuild_search_with`) or at startup recovery, so those edits are searchable only after the recording stops.
- Commands that may run during a recording do not hold `runtime` across their write.
- The writer enables `foreign_keys` and `journal_mode=WAL` and fails opening if WAL is not reported.
- Each `write` is one `BEGIN IMMEDIATE` transaction: committed when the closure returns `Ok`, rolled back on error or panic, and the writer is always returned.
- Each `read` is one snapshot-consistent `query_only` transaction on a separate connection; it never waits for the writer and sees committed data only.
- `write`/`read` inside a `write`/`read` closure on the same thread fails with `NestedAccess`.
- `AppService::runtime` and `backlog` (`OrderedMutex`) may be held around database calls, never acquired inside a closure (debug assertion).
- Closures contain database work only: no file system, Keychain, network, sleeps or channel waits.
- Lock contention, whether the writer or SQLite's lock, surfaces as `SQLITE_BUSY` and is classified by `store::sqlite_busy`.
- Retention commit order is unchanged: confirming write, then file moves, then database deletion before trash removal; the recording lease is held until the confirming write commits.

## Failure rules

| Condition | Result |
|---|---|
| Writer not available within `wait` | Error "database is locked: the shared database writer stayed busy for <wait>" (SQLITE_BUSY) |
| SQLite's write lock held by another connection beyond the remaining `wait` | SQLITE_BUSY from SQLite |
| WAL cannot be enabled | Error "The database could not enter WAL mode (journal_mode=<mode>)." |
| Nested `write`/`read` | `NestedAccess` error |
| `OrderedMutex` locked inside a closure (debug) | Panic |
| Explicit `write_with` wait over 5 s while holding `runtime`/`backlog` (debug) | Panic |
| Failed ROLLBACK on return | Connection discarded, next write reopens it |
| Write holds the writer over 250 ms (debug) | "slow database write" line on stderr |

## Defaults

| Setting | Value |
|---|---|
| `WRITE_WAIT` (default writer wait and writer busy timeout) | 30 s |
| `HELD_LOCK_WRITE_WAIT` (write while `runtime`/`backlog` is held) | 5 s |
| `CAPTURE_LOOP_QUEUE_WAIT` (in-process queue wait on the capture loop) | 5 ms |
| `READ_BUSY_TIMEOUT` | 5 s |
| `SLOW_WRITE` | 250 ms |
| Capture-loop housekeeping wait (`LOOP_BUSY_TIMEOUT`) | 250 ms total; at most 5 ms of it queueing for the writer, the remainder for SQLite's lock |
| Main database synchronous level | Unchanged (not raised to FULL) |

## Known limits

While the writer waits for the recorder's SQLite lock, other in-process writers queue behind it. Recording start, enrollment's final write and `select_live_model` still write under `runtime` (5 s cap; none can run during capture). The recorder keeps its own connection but runs on its own `audio-recorder` thread, not the capture loop (see [durable recording](../../recording/durable-recording/durable-recording.md#recorder-thread-and-bounded-queue)); no real recording has exercised the shared writer or the recorder thread.
