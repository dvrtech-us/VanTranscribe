# Live speaker saves survive database lock contention

- Date: 2026-11-10
- Feature: speaker-turns
- Related code: crates/vt-session/src/speaker_store.rs (`save_live_updates_with`, `LiveSaveBudget`), crates/vt-session/src/speaker_pipeline.rs (`LivePublisher`), crates/vt-session/src/store.rs (`sqlite_busy`, `describe_error`), crates/vt-session/src/session.rs (`worker_failure_warning`)

## Context

The first real live call (527.6 s, remote, microphone and system audio) on 2026-11-09 lost live speaker labels at about 272 s, and its automatic refinement failed. The stored text was "database is locked".

The live speaker save used a deferred transaction that read and then wrote. In WAL mode, if another connection commits between the read and the write, that fails at once with SQLITE_BUSY_SNAPSHOT (extended code 517) without invoking the busy timeout. The stored text could not tell this apart from a busy-timeout expiry, so the exact cause of the 2026-11-09 failure is not proven. A regression test demonstrates the 517 mechanism. Any error ended the speaker worker for the rest of the recording.

## Decisions

- The live save takes an IMMEDIATE transaction on a fresh connection, so a concurrent commit makes it wait in the busy handler instead of failing on a snapshot upgrade.
- On busy/locked it retries the whole save, up to 3 attempts, each with a 1.5 s busy timeout, with 100 ms then 200 ms backoff. Worst case is 4.8 s, under the 10 s live lag limit. Upserts make a retry idempotent.
- If the save still fails with a lock, the worker stays alive. Unsaved turns are kept (at most 2048) and written with the next successful save. Turns that must be dropped (overflow, or still unsaved when the worker ends without a final reconciled save) are recorded as inference gaps with reason `persistence_failed`.
- A health warning is added: "Live speaker labels could not be saved yet; they are kept and retried with the next update. Raw audio is preserved. Cause: <error> (SQLite extended code <n> <NAME>)".
- Fatal worker failures add "Live inference worker <thread-name> stopped: <error>[ (SQLite extended code <n> <NAME>)]", with the cause capped at 300 characters, so the extended code is visible next time.
- Engine errors and non-lock database errors remain fatal (`worker_failed`). A failed worker is still not restarted.

## Alternatives Considered

- Make every write transaction IMMEDIATE. Rejected: the reconciled save reads the full turn history before writing and would hold the write lock for that time, and the recorder's chunk writer deliberately does not retry after its 5 s lock budget.
- Restart the diarizer after a failure. Out of scope.

## Consequences

- Positive: lock contention no longer ends live speaker labelling, and error text now carries the SQLite extended code.
- Negative: the other deferred read-then-write transactions in `vt-session`, including the reconciled save at finish and the refine-time saves, are unchanged and not retried. The cause of the 2026-11-09 failure remains unproven. If locks outlast the retries, live labels can have gaps until the next successful save or until refinement.
