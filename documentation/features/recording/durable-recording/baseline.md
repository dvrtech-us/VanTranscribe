# Durable Recording Baseline

## Flow invariants

- Mic and System use independent frame ranges, sequence numbers and files. Processing never mixes the tracks.
- Audio blocks are nonempty whole interleaved frames, with native rates of 8,000–192,000 Hz, 1–8 channels and at most two seconds of samples.
- Frame indexes count one frame per channel, start at zero and reset only when the epoch increments by one. Session nanoseconds retain one monotonic time origin.
- Within a track, time ranges join exactly. Missing time is represented by an explicit gap. A gap has positive duration and may carry zero frames when the absent frame count is unknown.
- Fractional nanoseconds are truncated per block; callers chain published endpoints. A future native adapter must retain hardware-clock observations separately and reconcile drift rather than substituting callback-arrival time.
- Floating-point conversion rejects NaN/infinity, clamps finite input to `[-1,1]`, scales by `2^23`, rounds ties away from zero and clamps to signed PCM24. FLAC preserves that integer representation.
- A pending manifest reservation commits before a chunk file is created. A successful publication completes file sync, rename, directory sync and the ready transaction, in that order.
- Failure while inserting a pending reservation or publishing its file prevents the writer from continuing or finishing until it is dropped and recovery runs.
- Recovery validates exact encoded bytes with SHA-256, adopts complete pending files and records explicit gaps for missing/incomplete publications. It retains damaged files and does not infer an unobserved crash tail.
- Gap rows never advertise a readable audio path or checksum. A verified partial file can replace a damaged final file only after retaining the damaged file under a unique `.flac.damaged.<n>` name.
- A per-meeting OS lock excludes another cooperating writer or recovery process. Duplicated/inherited descriptors share that lock; ordinary drop/process death releases it when its last reference closes. Successful consumed-writer `finish` explicitly unlocks after its final status commit, before returning, even if an alias remains open.
- The busy handler is installed before any initialization pragma/schema access. WAL mode must be verified before returning a recording connection.
- Concurrent first-open `DatabaseBusy`/`DatabaseLocked` may retry initialization only, using fresh connections and one shared five-second monotonic deadline including SQLite busy waits and retry sleeps.
- A failed initialization drops its connection and rolls back any immediate schema transaction before retrying. Version inspection and schema creation are atomic; unsupported versions/data are not overwritten.
- Non-lock errors are not retried. Audio reservations, file publication and writer-poisoning semantics retain their original failure behavior.
- Manifest writes (pending/gap insert, `ready` update, `processing` update) retry SQLITE_BUSY/SQLITE_LOCKED, including extended codes such as 517, with a 250 ms per-attempt timeout and 5 ms doubling to 100 ms backoff until `MANIFEST_LOCK_BUDGET` (60 s) is spent; only then is the writer poisoned.
- A retried insert accepts its own identical committed row exactly once and rejects a different row; the retry never inserts a second row.
- Lock retries do not change `validate_next`, `recover`, `synchronous=FULL`, `fullfsync=ON` or the pending-row-before-file ordering.
- `Recording::lock_waits()` reports the number of retried writes and the longest write; a retry runs on the `Recording` owner's thread, which in the app is the `audio-recorder` thread, not the capture loop.
- No API in this increment is intended for a real-time audio callback.
- In the app, all `Recording` work runs on one `audio-recorder` thread fed by one FIFO; the capture loop never waits for it. Per track at most `RECORDER_QUEUE_LIMIT` (60 s) and in total `RECORDER_QUEUE_MAX_BYTES` (64 MiB) of audio wait; a block over either bound is dropped (queued audio is never evicted) and replaced by an exact `recorder_backlog` gap. Once a track drops it keeps dropping into one gap until its backlog is at most half the bound. Consecutive queued gaps of the same reason merge.
- Stop returns immediately; the recorder drains and finishes; the live run is sealed only after that. The drain waits `RECORDER_DRAIN_DEADLINE` (90 s), then `ABANDON_GRACE` (5 s), then `ABORT_WAIT` (2 s). A Close that completes after the deadline is still a success; abandon applies only while work remains.
- Quit waits for a draining recorder (up to 35 s plus `RECORDER_STOP_BOUND`, 97 s).
- A recorder error or panic ends the recording promptly with the error recorded. After a panic the manifest is never marked finished.
- While a backlog exists the queued span is saved to `app_recorder_progress` (every 2 s and at stop) and cleared after a successful drain. Recovery turns a span beyond the manifest's last row into a `recorder_interrupted` gap, invents no audio, and marks the meeting `recovered` with a warning. A marker can be stale and under-report if the database stayed locked until the process died.

## Flag and failure rules

| Condition | Severity | Blocks? |
|---|---|---|
| Invalid meeting ID or existing meeting ID at creation | Error | Creation |
| Active meeting lock | Busy | Another writer or recovery |
| Invalid format, block geometry, nonfinite conversion input or overflowing range | Error | The submitted write |
| Timestamp hole, overlap, unsupported epoch jump or frame discontinuity | Error | The submitted write; caller must describe the transition explicitly |
| Finite sample outside `[-1,1]` | Counted conversion clipping | No; stored in `clipped_samples` |
| Pending reservation or subsequent file/database/publication failure | Error; writer requires recovery | Further writes and finish |
| Manifest write hits SQLITE_BUSY/SQLITE_LOCKED | Retried up to 60 s | The calling thread while it waits |
| Manifest write lock outlasts 60 s | Last lock error; writer poisoned | Further writes and finish; recovery runs |
| Recorder backlog at least 5 s | One warning (`RECORDER_BACKLOG_WARNING`) | No |
| Recorder queue full (60 s per track or 64 MiB) | One warning (`RECORDER_DROP_WARNING`); `recorder_backlog` gap | That audio is lost, exactly marked |
| Drain not finished after `RECORDER_DRAIN_DEADLINE` | `RECORDER_DRAIN_ERROR`; `RECORDER_RECOVERY_PENDING` if the thread is still busy | The meeting fails; unwritten audio becomes a `recorder_interrupted` gap at recovery |
| Recorder error or panic | Recording ends with the error | Further audio; recovery runs |
| Pending file matches its expected hash | Recoverable publication | No; recovery adopts it |
| Missing or hash-mismatched chunk | Explicit `missing_or_incomplete_chunk` gap | That audio range is unavailable |
| Transient SQLite Busy/Locked during initialization | Bounded retry | Creation/opening after one shared five-second budget expires |
| Initialization non-lock error or WAL cannot be enabled | Error, no retry | Opening; no partial recorder schema is retained |
| Unsupported manifest version | Error | Database opening |
| Non-Unix durable directory publication | Unsupported | Creation/recovery before output mutation |

## Side effects in order

1. Creation validates identity, obtains the meeting lock, initializes/validates WAL and the versioned SQLite schema with bounded transient-lock retries, creates track directories and inserts `status='recording'`.
2. Audio publication validates input, encodes and hashes it, commits `pending`, writes a new partial file, syncs it, renames it, syncs its directory and commits `ready`.
3. Explicit gap publication inserts a `gap` row with a reason; it creates no audio file.
4. Completion commits the meeting `processing`, marks the consumed writer non-writable and explicitly unlocks its shared meeting lock. No recorder operation follows unlock; all successful chunk writes were already committed.
5. In the app, a stop drains the recorder thread before the live run is sealed; unsaved audio left by a quit or crash is recorded by recovery as a `recorder_interrupted` gap.
6. Recovery takes the meeting lock, verifies/adopts files or records gaps, and changes an interrupted `recording` meeting or a finished meeting with newly found damage to `recovered`. Repeating recovery does not create duplicate manifest entries or repeatedly convert existing gap rows. Unknown IDs are rejected without initializing a meeting.

## Defaults and representation

| Setting or contract | Value |
|---|---|
| Toolchain baseline | Stable Rust 1.91 or newer |
| Encoded samples | Signed 24-bit integer PCM, native rate/channels |
| File path | `meetings/<id>/audio/<track>/<epoch>-<seq>.flac` |
| Temporary suffix | `.flac.partial` |
| Database | `<root>/vantranscribe.db`, manifest version 1 |
| SQLite | WAL, `synchronous=FULL`, `fullfsync=ON`, foreign keys enabled |
| SQLite busy timeout | Normal returned connections: 5 seconds |
| Shared app writer | The recorder keeps its own connection (this table's settings) and contends with the app's [shared writer](../../storage/database-access/database-access.md) at the SQLite level; the app writer waits up to its own `wait` (30 s default, 5 s while `runtime`/`backlog` is held) for the recorder's lock, the recorder retries up to `MANIFEST_LOCK_BUDGET` |
| Initialization lock budget | One five-second monotonic deadline across all attempts/phases, including busy waits and retry sleep |
| Manifest write lock budget | `MANIFEST_LOCK_BUDGET` 60 s total; `LOCK_ATTEMPT_TIMEOUT` 250 ms per attempt; backoff 5 ms doubling to 100 ms |
| Recorder queue | `RECORDER_QUEUE_LIMIT` 60 s per track; `RECORDER_QUEUE_MAX_BYTES` 64 MiB; backlog warning 5 s; drop hysteresis at half the bound |
| Recorder stop | `RECORDER_DRAIN_DEADLINE` 90 s; `ABANDON_GRACE` 5 s; `ABORT_WAIT` 2 s; `RECORDER_STOP_BOUND` 97 s |
| Initialization retry | Only SQLite Busy/Locked; fresh connection; 1 ms exponential backoff capped at 25 ms and remaining budget |
| Checked recovery file size | At most 64 MiB |
| CLI synthetic duration | Optional value in 1–60 seconds; four seconds by default |
| PCM clipping accounting | Counts original finite scalar samples outside `[-1,1]` |

The two-second block cap is not a measured capture-to-durable loss guarantee. Native capture queues, hardware timing and storage scheduling remain outside the implemented baseline.

## Access control and privacy

- Output remains local to the chosen filesystem root; the recording path performs no network operations.
- Audio and manifest files are unencrypted and use ordinary local OS filesystem permissions. This increment adds no application accounts, ACL policy or encryption keys.
- There are no HTTP routes or desktop capture permissions in the synthetic CLI.
- Feature documentation and examples contain synthetic signals and no personal meeting data.

The lock-handoff regression deliberately keeps a duplicate lock descriptor open through finish and immediate three-chunk recovery. Active recovery remains `Busy`; successful completion hands off immediately without recovery retries. The pre-fix case deterministically reproduced `Busy`, while no specific concurrent spawn is claimed as the original incident's proven cause.
