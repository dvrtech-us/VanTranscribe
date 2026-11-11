# A database lock cannot end a recording

- Date: 2026-11-11
- Feature: durable-recording (with recording-health, live-english and desktop-session)
- Related code: crates/vt-record/src/lib.rs (`MANIFEST_LOCK_BUDGET`, `Recording::retry_lock`, `insert_row`, `LockWaits`), crates/vt-session/src/session.rs (`PendingRoutes`, `AFTER_STOP_BUDGET`, `finish_live_run`, `tolerate_busy`, `note_recorder_waits`), crates/vt-session/src/store.rs (`save_clock`, `set_status_with`, `complete_live_run_with`, `LOOP_BUSY_TIMEOUT`), crates/vt-session/src/echo_store.rs (`record_route_with`, `seal_routes_with`), crates/vt-session/src/lib.rs (`recording_budget`)

## Context

The 2026-11-10 [live transcript persistence](../../../transcription/live-english/trail/2026-11-10-live-transcript-persistence.md) and [live speaker save](../../../diarization/speaker-turns/trail/2026-11-10-live-speaker-save-lock-contention.md) entries recorded a known remaining exposure: the recorder's chunk writer waited at most 5 s for the database, so a longer lock across a chunk flush ended the recording, and `save_clock` on flush was fatal. Those entries stay as written; this entry supersedes that statement.

Two adversarial reviews on 2026-11-10 of a larger design rejected (1) a per-meeting recorder manifest database mirrored into the main database (two sources of truth, and it does not survive a disk stall) and (2) priority classes in a single write-owner queue (a stop or abandon jumping ahead of queued transcript rows would have those rows rejected by the run-immutability triggers). This change is the agreed smallest step.

## Decisions

- Recorder manifest writes (pending insert, ready update, gap insert, finish) retry SQLITE_BUSY/SQLITE_LOCKED, including extended codes such as 517, with 250 ms attempts and 5 ms to 100 ms backoff for up to `MANIFEST_LOCK_BUDGET` (60 s, injectable through `set_lock_budget`), then poison the writer as before. Retries are idempotent: a retried insert accepts its own identical row once. `validate_next`, `recover`, `synchronous=FULL`, `fullfsync` and pending-before-file ordering are unchanged.
- The wait count and longest wait are exposed (`Recording::lock_waits()`), copied into `Runtime::recorder_lock_waits`, and one warning (`RECORDER_LOCK_WAIT_WARNING`) is raised when a wait exceeds 1 s.
- While the recorder waits, the capture consumer loop is blocked: the 128-slot channel (about 2.56 s), then the pipe, then the helper's ring of at least 10 s; after roughly 12.5 s the helper drops frames and reports an explicit `overflow` gap and the recording continues. A stop request is not handled until the pending write finishes.
- Flush-path health persistence (`recording_budget`) and `save_clock` are best effort on lock errors: 250 ms timeout, skipped, with the single per-recording housekeeping warning, whose text is now: "Recording details (duration, status, health, chunk clock or output route) could not be saved while the database was busy; recording continues and periodic details are saved again on the next update." followed by " Cause: <error>".
- Output route changes that cannot be saved are kept in order (bound `MAX_PENDING_ROUTES` = 64, original timestamps) and retried (250 ms timeout, at most every 500 ms). At stop they are flushed and the history sealed with `AFTER_STOP_BUDGET` (4 x 5 s, 200/400/800 ms backoff, 21.4 s worst case). The history stays unsealed only on overflow, a non-lock error or a lock outlasting that retry, each with an "Output route history ..." warning.
- Live-run completion and the final status and duration writes at stop use `AFTER_STOP_BUDGET` before the meeting is marked failed.

## Alternatives Considered

- Per-meeting recorder manifest database with a mirror in the main database: rejected (two sources of truth; does not survive a disk stall).
- Priority classes in one write-owner queue: rejected (a stop or abandon overtaking queued transcript rows would have them rejected by the run-immutability triggers).
- Not done, still open: a single shared write connection for the main database (the `sqlite-rwc` crate implements that pattern but is AGPL-3.0-only, incompatible with this MIT project, so it would be built in the store layer), and moving the recorder onto its own thread with a bounded in-memory audio queue so a disk stall backs up that queue instead of the capture loop.

## Consequences

- Positive: a database lock shorter than 60 s costs at worst an explicit audio gap instead of the recording; stop-time writes no longer fail a meeting on a lock shorter than the retry budget.
- Negative: a lock still ends the recording once it outlasts 60 s, and the capture loop (and live inference input) is blocked while the recorder waits. Disk stalls still back up the capture loop because the recorder shares its thread.
- Not verified: no real recording has exercised any of this; coverage is tests that hold a lock on a temporary database.
