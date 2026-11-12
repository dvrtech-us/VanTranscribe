# The audio recorder runs on its own thread

- Date: 2026-11-11
- Feature: durable-recording (with recording-health, macos-capture, desktop-session and database-access)
- Related code: crates/vt-session/src/recorder_queue.rs (`RecorderQueue`, `RECORDER_QUEUE_LIMIT`, `RECORDER_QUEUE_MAX_BYTES`, `RECORDER_DRAIN_DEADLINE`, `ABANDON_GRACE`, `ABORT_WAIT`, `RECORDER_STOP_BOUND`, `RECORDER_RECOVERY_PENDING`), crates/vt-session/src/session.rs (`TrackBuffer::flush`, `poll_recorder`, `publish_recorder`, `drain_recorder`, `late_recovery`), crates/vt-session/src/store.rs (`RECORDER_PROGRESS_SCHEMA`, `save_recorder_progress`, `record_unsaved_tail`, `recover_interrupted`), crates/vt-session/src/lib.rs (`RecorderView`, `Snapshot.recorder`, `wait_for_shutdown`), crates/vt-session/src/capture_health.rs (`RecorderHealth`), crates/vt-record/src/lib.rs (`Recording::set_abort_signal`, `Recording::record_unsaved_tail`), crates/vt-core/src/lib.rs (`GapReason::RecorderBacklog`, `GapReason::RecorderInterrupted`)

## Context

This completes the sequence recorded on 2026-11-11: [a database lock cannot end a recording](2026-11-11-database-lock-cannot-end-a-recording.md) (manifest writes retry a lock for 60 s), then [one shared write connection](../../../storage/database-access/trail/2026-11-11-one-shared-write-connection.md), then this step, which both earlier entries named as the remaining exposure. Those entries stay as written. While the recorder waited on the capture consumer loop, nothing else was consumed: the channel filled, the shown duration stalled and live inference starved.

## Decisions

- One `audio-recorder` thread owns the `Recording`. The capture loop hands it chunks and gaps through one FIFO, bounded at 60 s per track (`RECORDER_QUEUE_LIMIT`) and 64 MiB (`RECORDER_QUEUE_MAX_BYTES`); worst case for 48 kHz mono plus 48 kHz stereo f32 is 34.56 MB.
- Overflow drops the incoming block, never queued audio, and replaces it with a `recorder_backlog` gap with exact times. Drop hysteresis (keep dropping until the backlog is at most half the bound) and merging of consecutive same-reason gaps avoid hundreds of tiny gaps.
- The loop computes end time, sequence, epoch and gap count itself and polls status without blocking. The chunk-clock row is written on the recorder thread with a short writer wait and counts toward max write time. The disk budget counts queued bytes.
- Stop returns at once; the recorder drains and finishes; the live run is sealed afterwards. `RECORDER_DRAIN_DEADLINE` (90 s), then a 5 s grace (a Close that completes there is a success), then cancellable lock waits (`Recording::set_abort_signal`) and a 2 s wait. A thread still stuck reports `RECORDER_RECOVERY_PENDING` and recovers the meeting itself when it ends; startup recovery also revisits meetings failed in the app but still `recording` in the manifest.
- Quit waits up to 35 s plus `RECORDER_STOP_BOUND` (97 s) for a draining recorder.
- Audio unsaved at a quit or crash is recorded, not hidden: marker table `app_recorder_progress` (created in code, not in the documentation SQL files), written while a backlog exists and at stop, cleared after a drain; recovery records a `recorder_interrupted` gap, invents no audio and sets the meeting `recovered` with a warning.
- New fields: `Snapshot.recorder` (`RecorderView`) and `CaptureHealth.recorder` (`RecorderHealth`); the UI shows a "Saving audio" state after Stop (see [desktop session](../../../app/desktop-session/desktop-session.md)).

## Alternatives Considered

- Leave the recorder on the capture loop with only the 60 s lock retry: a lock or disk stall still froze capture, duration and live inference.
- Per-meeting recorder manifest mirrored into the main database, or priority classes in one write queue: rejected on 2026-11-10 (see the earlier entry).
- Evict old queued audio on overflow: rejected; dropping the incoming block keeps the queue contiguous and the loss exactly describable.

## Consequences

- Positive: development-machine measurement (from the implementing agent's report, not a guarantee): with an 8 s database write lock, the old code blocked one capture-loop pass for about 6.6 s, the 128-slot channel filled, the shown duration advanced 1.5 s and live inference received 296 packets; with the recorder thread no pass reached 500 ms, the channel never filled, the duration advanced the full 8 s and live inference received 1,604 packets.
- Negative: up to 60 s of audio per track can exist only in memory during a stall, so a crash during a stall can lose more than before (previously about 12.5 s of buffering). With a healthy disk durability timing is unchanged apart from a thread hand-off. The progress marker can be stale and under-report if the database stayed locked until the process died.
- Review record: an Opus review found no blocker (quit during a drain, stuck-recorder recovery, drop fragmentation, panic outside the catch). A Grok review additionally found that abandon pre-empted a completing close so a recording that had finished writing could be marked failed, that the manifest lock wait ignored abandon so recovery could return Busy and never be retried, and that quit waited less than the drain. All were fixed before commit.
- Not verified: no real recording has exercised this.
