# Live transcript lines are saved off the recognizer thread

- Date: 2026-11-10
- Feature: live-english
- Related code: crates/vt-session/src/live_transcript.rs (`TranscriptWriter`, `TranscriptSink`, `write_loop`, `save_batch`), crates/vt-session/src/store.rs (`save_live_segments`, `insert_live_segments`, `retry_busy`, `open_with_timeout`, `LOOP_BUSY_TIMEOUT`), crates/vt-session/src/speaker_store.rs (`LiveSaveBudget`), crates/vt-session/src/session.rs (`save_transcript`, `tolerate_busy`, `record_session`), crates/vt-session/src/lib.rs (`Runtime::live_finals`, `detail_for_run`), crates/vt-session/src/health.rs, crates/vt-session/src/echo_pipeline.rs

## Context

A real 95.5-minute in-person recording on 2026-11-10 (single microphone) kept recording audio throughout, but the live transcript's last segment ended at 47:41. The live speaker worker had already failed at 6:12 with the cluster capacity error. Ten capture `overflow` gaps occurred at 47:31-47:36 (about 2.1 s lost), which is when the recognizer stopped producing saved text. The recording screen showed "Recording" throughout.

The live speech worker exited and nothing restarted it. The most likely trigger is a database lock on the transcript save during a roughly 10 s stall. This is inferred, not proven: the error text saved at stop was erased by the automatic refine, so the real cause is unrecorded.

Before this change the recognizer thread itself saved every final line with `store::save_segment`: a DEFERRED transaction that read, wrote, and rebuilt the whole meeting's search rows, with a 5 s busy timeout. Any error returned from that save ended the speech worker for the rest of the recording. The recorder's chunk writer (`crates/vt-record`) has a 5 s lock budget and never retries, so a long live save also threatened the recording itself.

## Decisions

- The recognizer thread no longer touches SQLite. `save_transcript` publishes a final line to `Runtime::live_finals` (so the UI shows it at once) and hands it to a `TranscriptSink`. It cannot fail.
- A session-owned writer thread (`live-transcript-writer`, `TranscriptWriter`) drains a bounded queue: 512 unsaved lines (`QUEUE_LIMIT`, queued plus carried after contention), saved in batches of at most 64 (`BATCH_LIMIT`).
- `store::save_live_segments` saves a batch in one IMMEDIATE transaction on a fresh connection with point operations only: a primary-key check that the run is the meeting's running active live run, one idempotent insert per segment (`ON CONFLICT(run_id,id) DO NOTHING`) and one search row per newly inserted segment. Summaries of the meeting are marked stale, and their search rows removed only if summaries exist. For new segments this ends in the same state `rebuild_fts` would produce, because a new segment has no correction or bleed match yet. Segment IDs are minted before the first attempt, so a retried batch never duplicates rows.
- Lock budget is `LiveSaveBudget::PRODUCTION`: 3 attempts, 300 ms busy timeout each, 50 ms then 100 ms backoff, 1.05 s worst case (`store::retry_busy`, each attempt on its own connection). This is the same budget the live speaker publisher now uses; it was 3 x 1.5 s + 100/200 ms = 4.8 s.
- On a lock after the budget the batch is kept and retried after the next arrival or after `RETRY_INTERVAL` (1 s). Health warning: "Live transcript lines could not be saved yet; they are kept and retried. Raw audio is preserved. Cause: <error>".
- A non-lock (deterministic) error is isolated by saving the batch line by line; the bad line is dropped as an STT inference gap with reason `segment_rejected` and the warning "A live transcript line could not be saved and was skipped. Raw audio is preserved; refine saved audio to recover it. Cause: <error>".
- A line evicted from a full queue becomes a `persistence_failed` gap. At stop the writer retries for at most `CLOSE_DEADLINE` (5 s); lines still unsaved become `persistence_failed` gaps. Both add: "Some live transcript lines could not be saved because the database stayed busy. Raw audio is preserved; refine saved audio to recover them."
- Stop closes and joins the writer after the speech worker has been joined and before the inference audit is finalized and the live run is sealed. `AppService::detail_for_run` snapshots the unsaved finals and partials before reading the database and de-duplicates by segment ID, so a line is never shown twice or in neither.
- Housekeeping writes on the recording loop use `store::LOOP_BUSY_TIMEOUT` (250 ms): periodic duration (`set_duration_with`), unforced health (`health::persist`, forced checks keep 5 s), source timing (`health::timing_with`), and pause/resume status. `session::tolerate_busy` skips a lock error, warns once per recording ("Recording details (duration, status or health) could not be saved while the database was busy; recording continues and they are saved again on the next update. Cause: <error>"), and leaves non-lock errors fatal.
- The final echo-info save at stop retries briefly (3 x 1 s, 100/200 ms backoff). A lock is a warning ("Live echo processing details could not be saved because the database stayed busy. Raw audio is preserved. Cause: <error>"), not a worker failure.

## Alternatives Considered

- Retry on the recognizer thread. Rejected: its input queue holds about six seconds of audio, so a blocked save would overflow it and lose audio inference.
- Make every write transaction IMMEDIATE. Rejected: other transactions read a large history before writing and would hold the write lock for that time.
- Keep per-line deferred saves and only catch the error. Rejected: the read-before-write shape can fail at once with a snapshot error that skips the busy timeout, and the whole-meeting search rebuild made each save slower as the meeting grew.
- Move the recorder's chunk writer to the same budget. Out of scope for this change (see Consequences).

## Consequences

- Positive: a database lock can no longer stop or stall recognition; lines are shown immediately, retried, and any loss is recorded as a gap with a cause.
- Negative: known remaining exposure. The recorder's chunk writer (`crates/vt-record`) is unchanged. A database lock held longer than its 5 s budget across a chunk flush still ends the recording, and `save_clock` on flush is still fatal. This change shortens how long live writers can contend for the lock; it does not remove that risk.
- Negative: finals in the queue at an abnormal process exit are lost from live text (raw audio and refine are unaffected).
- Not verified: no real long recording has exercised the writer. The lock behavior is covered by tests that hold a lock on a temporary database; the 2026-11-10 trigger remains inferred.
