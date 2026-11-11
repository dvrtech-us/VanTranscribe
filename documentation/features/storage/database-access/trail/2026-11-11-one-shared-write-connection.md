# One shared write connection for the main database

- Date: 2026-11-11
- Feature: database-access (with recording-health, live-english, desktop-session, durable-recording, retention and voice-memory)
- Related code: crates/vt-session/src/db.rs, crates/vt-session/src/db_tests.rs, crates/vt-session/src/store.rs, crates/vt-session/src/retention.rs, crates/vt-session/src/voice_memory.rs, crates/vt-session/src/echo_store.rs, crates/vt-session/src/preferences.rs, crates/vt-session/src/lib.rs

## Context

Every write opened its own connection, so concurrent in-process writers failed with "database is locked" after a 5 s busy timeout, and the code accumulated stop-gaps (`retry_busy`, `tolerate_busy`, `LOOP_BUSY_TIMEOUT`, `LiveSaveBudget`). The [2026-11-11 recorder entry](../../../recording/durable-recording/trail/2026-11-11-database-lock-cannot-end-a-recording.md) left a shared write connection as open work. This is that step; that entry stays as written.

## Decisions

- One `Writer` connection per database file, borrowed for one `BEGIN IMMEDIATE` transaction by `store::write`/`write_with` (default wait `WRITE_WAIT` 30 s). Reads use fresh `query_only` connections inside one read transaction.
- Wait expiry (writer or SQLite lock) is reported as SQLITE_BUSY so existing lock handling keeps working unchanged.
- Rules: no nested access (`NestedAccess`); the database is the innermost lock (`OrderedMutex` for `runtime` and `backlog`, debug assertion); database work only inside closures; debug log above `SLOW_WRITE` (250 ms).
- Writers are looked up by canonicalized path; `AppService` holds its writer for its lifetime.
- Every in-process write and read in `vt-session`, `vt-speakers` and `src-tauri` was converted; `store::open` is test-only and a scan test guards against new connection opens.
- Restructured for the rules: retention `commit`/`make_plan`/`preview_policy` (file work outside closures, lease held until the confirming write commits), voice memory (Keychain outside closures), echo route lookups, `recover_interrupted` in one transaction, `preferences::load` on the writer.
- Review fixes (below): FIFO handoff, a 5 s cap under `runtime`/`backlog`, a 5 ms queue cap on the capture loop, commands no longer hold `runtime` across writes, search rebuild deferred during recording, and discard-and-reopen after a failed rollback.
- The recorder keeps its own connection (`synchronous=FULL`, `fullfsync=ON`); the main database's synchronous level is unchanged. The stop-gap retry/budget code is kept.

## Alternatives Considered

- The `sqlite-rwc` crate (same pattern): not used, it is AGPL-3.0-only and this project is MIT.
- A queued write-owner thread with priority classes (rejected 2026-11-10): queued transcript rows could be rejected by the run-immutability triggers when a stop or abandon jumps ahead.
- A per-meeting recorder manifest database with a mirror (rejected 2026-11-10): two sources of truth, and it does not survive a disk stall.

## Consequences

- Positive: in-process writers queue instead of failing; reads are snapshot-consistent and never wait for the writer; the WAL check lives in one place.
- Negative: while the writer waits on the recorder's SQLite lock every other in-process writer queues behind it; three commands (recording start, enrollment's final write, `select_live_model`) still write under `runtime` with the 5 s cap, and the meeting-detection settings commands hold the `meeting_detection` lock across a small write; edits other than note/title made during a recording are searchable only after stop; the recorder is still on the capture loop (moving it to its own thread with a bounded in-memory audio queue is the planned next step); the stop-gap code is rarely exercised and its removal is later work.
## Review findings and fixes

Two adversarial reviews ran before commit.

- Opus: no blocker. Raised a 30 s wait while `runtime` is held, unfair handoff, and the weak guard test. Fixed: `HELD_LOCK_WRITE_WAIT` (5 s) with a debug panic for longer explicit waits; FIFO ticket queue where a timed-out waiter leaves it and a panicking holder still returns the connection; the guard test walks every workspace crate, flags aliased `Connection` imports and `sqlite3_open`, and asserts vt-playback opens the database read-only.
- Grok, additionally: (1) note, speaker, correction and review commands allowed during a recording held `runtime` across a whole-meeting search rebuild, stalling capture and sitting in front of the recorder's chunk writes. Fixed: those commands release `runtime` before writing, and during a recording only note/title rows are written while other edits defer the rebuild to stop (`refresh_search_after_edit`; measured in `long_meeting_publish_and_search_rebuild_times` on a development machine, not a guarantee). User-visible: those edits become searchable when the recording stops. (2) Capture-loop housekeeping could block audio intake for its full wait. Fixed: `CAPTURE_LOOP_QUEUE_WAIT` (5 ms) caps the in-process queue wait there; the recorder-lock wait keeps the 250 ms budget. (3) A failed rollback would wedge the only writer. Fixed: the connection is discarded and the next write reopens it, so "one connection for the process lifetime" is no longer exact.

## Verification status

- Not verified: no real recording has exercised this; coverage is `db_tests.rs` and the existing suites on temporary databases.
