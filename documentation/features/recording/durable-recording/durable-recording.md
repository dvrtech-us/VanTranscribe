# Durable Recording

This recorder library records bounded, worker-owned audio blocks as independent FLAC files and a SQLite manifest. Mic and System remain separate tracks. It provides a synthetic CLI and interrupted-publication recovery, and is used by the desktop capture/inference pipeline. Native capture, callback queues, UI and inference belong to separate features; encryption and compaction remain open.

## User flow

1. With stable Rust 1.91 or newer, run `cargo run -p vt-record --example synthetic -- <root> <meeting-id> [seconds]`. The optional duration accepts 1–60 seconds and defaults to four seconds.
2. The example creates a new meeting, generates separate synthetic Mic and System signals, and publishes one-second chunks at 48 kHz, with mono Mic and stereo System. It does not request capture permissions or open audio devices.
3. Audio files appear under `<root>/meetings/<meeting-id>/audio/<track>/<epoch>-<seq>.flac`; `<root>/vantranscribe.db` stores the manifest. A successful end changes the meeting from `recording` to `processing`, making the audio available for later jobs.
4. Verify or reconcile a meeting with `cargo run -p vt-record --example synthetic -- --recover <root> <meeting-id>`. Recovery reports verified files, adopted publications and newly recorded gaps. It does not resume recording.
5. Use a new meeting ID for the next recording. IDs contain 1–64 ASCII letters, digits, hyphens or underscores. Existing meeting IDs are rejected.

## Technical flow

### Audio contracts

`vt-core::StreamFormat` accepts 8,000–192,000 Hz and 1–8 interleaved channels. `AudioBlock` is nonempty, contains whole frames and is capped at two seconds. One frame contains one scalar sample per channel. Frame indexes are local to an epoch and reset to zero when the epoch increments. Platform adapters must supply elapsed monotonic session nanoseconds in `FramePosition::host_time_ns`; raw hardware ticks are not accepted as that unit.

`Recording::write_chunk` revalidates public block fields. For each track, the first block starts at epoch 0, frame 0. Subsequent time ranges must join exactly. Within an epoch, frame ranges are contiguous and format is fixed. A new epoch increments by one and starts at frame 0. Observed discontinuities require `write_gap`; the writer does not silently fill missing time.

This foundation truncates fractional nanoseconds per block. Callers chain the preceding published `end_ns`, as the synthetic example does. This is a worker timeline contract, not a completed hardware-clock adapter: native capture must separately retain observed hardware timestamps and reconcile drift and discontinuities. Hardware-clock fidelity is not validated by this increment.

`vt-core::pcm24` rejects NaN and infinities. Finite values are clamped to `[-1, 1]`, scaled by `2^23`, rounded to the nearest integer with ties away from zero, and clamped to `[-8388608, 8388607]`. The manifest counts original scalar samples outside `[-1, 1]` as `clipped_samples`. FLAC preserves these converted integers losslessly, rather than arbitrary original floating-point samples. Recording retains native rate and channel order without mixing or resampling.

### Publication and completion

1. `Recording::create` validates the meeting ID, canonicalizes the output root, acquires the meeting lock, initializes the versioned database and creates per-track directories.
2. `write_chunk` validates the next range, converts PCM and encodes FLAC on the worker thread. It computes SHA-256 over the exact encoded bytes.
3. A `pending` manifest row commits before any chunk file is created. It retains identity, format, frame/time ranges, destination, expected hash and clipping count.
4. The writer creates a new `<epoch>-<seq>.flac.partial`, writes its bytes and synchronizes the file.
5. It renames the file to its final `.flac` name, synchronizes the containing directory, then commits `state='ready'`.
6. Every successful `write_chunk` has completed this protocol. `finish` has no hidden audio buffers to flush; it commits the meeting status `processing`, marks the consumed writer non-writable and explicitly unlocks the meeting lock. No recording operation follows that unlock.

Once pending-row insertion begins, a database or publication failure poisons the writer (a SQLite busy/locked error first goes through the retry described under [manifest write lock tolerance](#manifest-write-lock-tolerance), and poisons only when the retry budget is spent), including an uncertain reservation outcome. The caller must drop it and run recovery; continued writes and `finish` are rejected. Validation and encoding errors before reservation do not reserve a timeline range. The current directory-sync implementation requires Unix; creation and recovery reject unsupported platforms before touching output. Windows durable publication has not been implemented.

### Explicit gaps and recovery

`Recording::write_gap` records an explicit time range with reason `pause`, `device_change`, `overflow` or `unavailable`. Its `frame_count` may be zero when a missing device makes the number of absent hardware frames unknown. A gap still has a positive time duration. Per-track sequence numbers advance across both audio and gap rows.

`Recording::recover` takes the same exclusive OS file lock as the writer, so it cannot race a cooperating active recorder. Successful `finish` explicitly releases that shared lock even if a duplicated or transiently inherited descriptor is still open. Merely closing the writer's descriptor can leave the lock held until its last alias closes; close-on-exec prevents inheritance after exec, rather than removing the pre-exec interval. See [Rust File locking](https://doc.rust-lang.org/std/fs/struct.File.html#method.lock) and [Apple flock semantics](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/flock.2.html). Active writers still cause immediate `Busy`; recovery does not retry or bypass that exclusion. It validates each non-gap manifest entry against the expected SHA-256. A matching pending final file is adopted; otherwise a matching partial file is synchronized and renamed before adoption. If a damaged final file already exists, it is retained under `.flac.damaged.<n>` first, without overwriting earlier evidence. Complete files are synchronized before acknowledgment. Missing or mismatched publications become explicit `missing_or_incomplete_chunk` gaps with `path` and `sha256` cleared. Damaged files are retained. Previously resolved gap rows are skipped on subsequent recovery.

Recovery changes an interrupted `recording` meeting to `recovered`; a finished meeting with newly discovered damage also becomes `recovered`. An intact `processing` meeting retains that status. Unknown meeting IDs are rejected rather than initialized. Recovery does not invent the endpoint of an unobserved crash tail, recreate missing audio, or resume capture. The current design therefore makes no universal promise that a process crash loses at most two seconds: upstream capture queues, scheduling and storage contention are not measured in this increment.

## Key types and entry points

| Type or function | Source | Role |
|---|---|---|
| `TrackKind`, `StreamFormat`, `FramePosition`, `AudioBlock` | `crates/vt-core/src/lib.rs` | Track identity, bounded geometry and session-time contracts |
| `pcm24` | `crates/vt-core/src/lib.rs` | Defined float-to-integer conversion |
| `Recording` | `crates/vt-record/src/lib.rs` | Meeting creation, chunk/gap publication, completion and recovery |
| `Chunk`, `Gap`, `RecoveryReport`, `RecordError` | `crates/vt-record/src/lib.rs` | Manifest views, discontinuities, recovery result and failures |
| `open_database`, `open_database_with_budget`, `open_database_attempt` | `crates/vt-record/src/lib.rs` | Verified WAL setup, bounded transient lock retries and rollback-safe versioned initialization |
| `synthetic` example | `crates/vt-record/examples/synthetic.rs` | Bounded two-track CLI demonstration and recovery command |

## Integration points

`vt-record` consumes `vt-core` blocks. It uses `flacenc` for 24-bit FLAC, `sha2` for exact-file SHA-256 and bundled SQLite through `rusqlite`. `open_database` installs its busy timeout before any pragma/schema access, enables foreign keys, verifies that `journal_mode=WAL` actually took effect, and enables `synchronous=FULL`/`fullfsync=ON`. Version inspection and schema creation share a rollback-safe immediate transaction and accept manifest version 1.

Concurrent first-open WAL transitions can return `SQLITE_BUSY` before the schema transaction, even with a busy handler installed: SQLite may decline to invoke that handler when waiting would deadlock an upgrade. See the [official busy-handler contract](https://www.sqlite.org/c3ref/busy_handler.html). Initialization therefore retries only SQLite `DatabaseBusy`/`DatabaseLocked`, using a fresh connection after each failed attempt. The failed connection and any schema transaction are dropped/rolled back before sleeping, so no read/upgrade/schema lock is retained between attempts.

All attempts share one five-second monotonic lock deadline. The remaining busy allowance is refreshed before each potentially blocking setup/transaction phase; retry backoff starts at 1 ms and doubles to a 25 ms cap, clipped to the same remaining budget. Expiry returns the original lock error. Unsupported versions, malformed databases, I/O and other errors are returned without retry; a rejected version never receives a partial recorder schema or reset version. On success, the connection's normal five-second busy timeout is restored. This retry applies only to initialization, never to recording reservations or file publication with an uncertain outcome.

Capture adapters must normalize clocks, provide explicit gaps and epoch changes, and call the recorder from a worker. The owned blocks and recorder API allocate, encode, hash, lock, access SQLite and write files; they are not callback-safe. The recorder itself does not run inference; the desktop session supplies that separate worker boundary.

## Routes and access control

There are no HTTP routes, desktop commands, accounts or application authorization roles. The CLI and library run as the local OS user. Output and locks use ordinary filesystem permissions inherited from the process; encryption and restricted application-specific ACLs are not implemented. Generated audio is unencrypted. No network transmission occurs in this recording path.

## Database schema

`meetings` stores recording lifecycle state:

| Column | Description |
|---|---|
| `id` | Meeting identifier, primary key |
| `status` | `recording`, `processing` or `recovered` |

`audio_chunks` stores publications and gaps:

| Column | Description |
|---|---|
| `meeting_id` | Foreign key to `meetings.id`; part of the primary key |
| `track` | `mic` or `system`; part of the primary key |
| `seq` | Nonnegative per-track sequence; part of the primary key |
| `epoch` | Nonnegative device/format generation |
| `first_frame` | Epoch-local first frame index |
| `frame_count` | Frames covered; may be zero for a gap with an unknown count |
| `sample_rate`, `channels` | Native format, 8,000–192,000 Hz and 1–8 channels |
| `start_ns`, `end_ns` | Session nanoseconds, with a strictly positive range |
| `kind` | `audio` or `gap` |
| `state` | `pending`, `ready` or `gap` |
| `path` | Root-relative expected chunk path; NULL for gaps |
| `sha256` | Hash of encoded bytes; required for audio publications, NULL for gaps |
| `gap_reason` | Explicit discontinuity or recovery failure reason |
| `clipped_samples` | Number of finite scalar input samples outside `[-1, 1]` |

SQLite integers are signed 64-bit values; the recorder rejects ranges exceeding that representation. Audio rows require a path, hash and positive frame count. Gap rows require a reason and NULL path/hash. `PRAGMA user_version=1` identifies this foundation schema; the broader transcript schema in the design is not implemented here.

## SQL artifacts

- [001-recording.sql](sql/001-recording.sql): executable initial schema, constraints and manifest version.

## Current limits and validation

Unit and integration coverage belongs beside `vt-core` and `vt-record`; the synthetic CLI exercises the same public writer API. Hardware drift, permissions, device following, callback allocation bounds, real SIGKILL capture loss and inference contention require later native tests. There is no at-rest encryption, lossless retention setting, Opus compaction or arbitrary-file import in this increment.


The first-open concurrency regression uses a barrier and eight independent meeting writers, each writing/finishing/recovering a real generated FLAC chunk. Every test invocation repeats this on 20 fresh database roots, checking one version-1 WAL schema, the configured timeout/foreign keys and all expected meetings/ready rows. Additional tests keep a conflicting exclusive transaction open beyond a short test-only initialization budget, verify no partial schema, then initialize the same root after release; an unsupported-version fixture retains its original table/data/version. Existing publication poisoning, writer/recovery exclusion and SIGKILL recovery tests remain unchanged.

## Manifest write lock tolerance

`Recording::retry_lock` wraps every manifest write of a writable `Recording`: the pending/gap row insert (`insert` -> `insert_row`), the `ready` update after the rename, and the `processing` update in `finish`. Each attempt is one autocommit statement with a busy timeout of `LOCK_ATTEMPT_TIMEOUT` (250 ms, clipped to the remaining budget). On SQLITE_BUSY or SQLITE_LOCKED, including extended codes such as 517 (SQLITE_BUSY_SNAPSHOT, which bypasses the busy handler), it sleeps 5 ms doubling to at most 100 ms and tries again until `MANIFEST_LOCK_BUDGET` (60 s, `pub const`; tests override it with the `#[doc(hidden)]` `Recording::set_lock_budget`) is spent. It then returns the last lock error and the writer is poisoned exactly as for any other manifest failure (the caller drops it and recovery adopts a verified pending file or records a gap). Other errors return at once. The connection's 5 s busy timeout is restored after each write for reads and recovery.

Retries are idempotent. A retried insert first reads the row by `(meeting_id, track, seq)`: an identical row means an earlier attempt committed and is accepted once; a different row is `RecordError::Invalid("manifest row changed during a lock retry")`. The `ready` update only touches that chunk's row and the file is already in place. `validate_next`, `recover`, `synchronous=FULL`, `fullfsync=ON` and the order "pending row before any file, then file sync, rename, directory sync, `ready`" are unchanged.

`Recording::lock_waits()` returns `LockWaits { count, max }`: `count` is the number of writes that hit at least one lock error and were retried, `max` the longest single write including all its attempts. `vt-session` copies it after each chunk or gap write (`note_recorder_waits`) into `Runtime::recorder_lock_waits` and raises `RECORDER_LOCK_WAIT_WARNING` once when `max` exceeds 1 s.

Consequence for capture: the retry runs on the capture consumer loop, so while it waits nothing else is consumed. The 128-slot capture channel (20 ms packets, at most 2.56 s) fills first, then the helper's stdout pipe, then the helper's ring (at least 10 s); after roughly 12.5 s the helper drops frames and reports an explicit `overflow` gap and the recording continues once the write succeeds. A stop request is not handled until the pending write finishes. The recorder is not on its own thread and has no in-memory audio queue. No real recording has exercised this; coverage is `crates/vt-record/src/tests.rs` (retry of insert, ready update, gap and finish; idempotent re-insert; budget exhaustion poisons and recovery adopts the chunk) with a lock held on a temporary database.

## Bounded native FLAC encoding

The recorder retains native PCM24 at 8–192 kHz and 1–8 channels. Ordinary chunks up to 96 kHz use `flacenc` compression. Before serialization or a pending manifest reservation, the recorder checks actual constructed minimum/maximum frame sizes and total encoded size against PCM24 plus bounded frame headers. A transient fixture exposed a predictor producing a 33.7 MB frame stream for two seconds of mono PCM; its 24-bit STREAMINFO frame-size field truncated and strict readers rejected it. Predictor-cost configuration alone did not remove the defect.

Oversized predictive output, final chunks whose compressed STREAMINFO block bound falls below the required 16 samples, and native rates above `flacenc`'s 96 kHz implementation ceiling use the small verbatim FLAC encoder in `crates/vt-record/src/verbatim.rs`. It retains each signed PCM24 sample, independent channels and the actual native rate; it does not resample or relabel audio. Fixed frames contain at most 4,096 samples/channel, valid frame/header CRCs and bounded STREAMINFO sizes. The optional FLAC MD5 is all zero (unknown); manifest SHA-256 still covers the exact stored file. This is a bounded in-memory encoding fallback before publication, not a retry of an uncertain manifest/file commit. If encoding cannot produce a valid bounded representation, no pending/ready audio row is created.

Tiny final-chunk regressions cover 1/15/16 samples and short tails after full blocks without padding. Regression fixtures cover the pathological transient at 16 kHz/mono, 48 kHz/stereo and 192 kHz/eight channels, exact independent Claxon PCM24 decoding and file-size bounds. Saved-inference tests also verify unchanged source FLAC bytes/hashes after actual echo processing. These tests write only generated temporary audio and open no output devices. The format contract follows [RFC 9639](https://www.rfc-editor.org/rfc/rfc9639.html).

### Successful-finish lock handoff regression

The independent-decoder regression retains a duplicate of the actual writer lock descriptor. It first requires recovery to return `Busy` while the writer is active, then finishes and immediately verifies all three original FLAC chunks while that duplicate remains open. Before explicit unlock, this deterministic case failed with `RecordError::Busy`; after unlock it succeeds. This establishes the descriptor-alias mechanism. It does not identify which concurrent process-spawn interval caused the earlier full-suite incident. The change applies only after successful final status commit; publication poisoning, SIGKILL recovery and ordinary recovery exclusion retain their existing behavior.
