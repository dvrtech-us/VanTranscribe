# Establish the M0 Recording Foundation

- Date: 2026-11-06
- Feature: durable-recording
- Related code: `crates/vt-core/src/lib.rs`, `crates/vt-record/src/lib.rs`, `crates/vt-record/examples/synthetic.rs`, `documentation/features/recording/durable-recording/sql/001-recording.sql`
- State: Working draft for this unmerged implementation change

## Context

The approved design separates recording from inference and requires independent, recoverable native-rate audio chunks. The existing architecture reviews and owner decisions are recorded in `documentation/planning/Architecture/`. This increment starts that approved M0 scope with a recorder that can be exercised deterministically before native capture, UI and model work.

The design's two-second chunk size does not itself establish a universal crash-loss bound: upstream queues and scheduling can add uncommitted audio. Floating-point capture also requires an explicit integer conversion before FLAC can preserve samples losslessly.

## Decisions

- Introduce dependency-free `vt-core` contracts for tracks, format, epoch-local frame positions, monotonic session nanoseconds, bounded worker-owned blocks and gap reasons.
- Convert finite samples to signed PCM24 with explicit clamping and rounding, retain native rate/channels, and record clipping counts. Define FLAC losslessness relative to the converted integer samples.
- Introduce `vt-record` with a minimal versioned SQLite recording schema in M0, ahead of the broader transcript store planned for M1. It stores two logical tracks through manifest rows rather than introducing the later transcript/run schema prematurely.
- Commit identity, exact byte hash, destination and timing metadata as `pending` before creating an audio file. Then order partial-file write and sync, rename, directory sync and the `ready` commit. Recovery can reconcile either side of an interrupted publication without inventing missing metadata.
- Serialize a meeting's cooperating writer and recovery process with an OS file lock. A failed reserved publication requires dropping the writer and recovering rather than continuing over its range.
- Validate hashes during recovery. Adopt matching pending final/partial files, or preserve the observed time range as an explicit gap. Retain damaged files for inspection.
- Clear path/checksum from gap rows so downstream consumers cannot mistake damaged files for readable audio. If a verified partial and damaged final coexist, quarantine the damaged final before adopting the partial. Reject unsupported platforms before output mutation.
- Require exact per-track timeline adjacency and explicit gap rows. Epoch changes reset frame indexes while preserving the session time origin; unknown device-absence frame counts may be zero.
- Provide a bounded synthetic example and tests through the public recorder API. Mark M0 as partial; hardware capture, lifecycle integration, inference benchmarks and a measured loss envelope remain future work.

- Reproduce and fix the concurrent first-initialization WAL race without weakening test concurrency. The original four-thread test failed 14 of 20 isolated runs; diagnostic instrumentation located the error at `journal_mode=WAL`, before `BEGIN IMMEDIATE`, despite the already-installed timeout. SQLite can bypass its busy handler for a lock upgrade. Retry only initialization Busy/Locked errors with fresh connections, one monotonic deadline, refreshed phase allowances and rollback-safe schema transactions. Preserve all normal writer/recovery/crash rules.

- Bound constructed predictive frame/stream sizes before serialization/publication and use standards-conformant verbatim FLAC for pathological predictors or native rates above the compression library ceiling. A generated transient reproduced a 33,662,133-byte two-second mono file with truncated 24-bit frame-size metadata. Preserve the original PCM24/native-rate contract and SHA-256 rather than weakening the decoder, resampling or mislabeling the source.

## Alternatives Considered

- **UI-first skeleton:** useful for interface work, but it would not exercise the recording durability contract. The recorder foundation establishes that contract first without changing the full roadmap.
- **Publish files before storing metadata:** leaves a crash window where an orphan file has no authoritative timing or format record. A durable pending reservation makes the expected file recoverable.
- **Treat every two-second file as a two-second crash-loss guarantee:** does not account for capture backlog or encoding/sync latency. Measure the complete native pipeline in M0 before asserting the limit.
- **Describe float-to-FLAC as bit-exact preservation of the capture representation:** inaccurate for arbitrary floating-point samples. The chosen integer conversion is explicit and testable.

## Consequences

- Positive: deterministic recording and recovery behavior can be validated independently of audio permissions, hardware and inference. Separate tracks and explicit gaps preserve timeline meaning.
- Positive: the implementation extends the approved durable-recording design without reducing the full roadmap.
- Negative: publication performs blocking encoding, hashing, database and filesystem work and requires a worker thread. Owned blocks are not a callback transport.
- Negative: this increment writes unencrypted output using ordinary filesystem permissions. Encryption, Opus compaction and Windows directory publication remain planned.
- Negative: deterministic recovery checks do not establish hardware drift, real-time scheduling bounds or a complete M0 exit result.

## Validation and review

- Claude Opus performed the final code review after the implementation agent completed, finding no blocking defects. Its non-blocking suggestions were addressed with stricter gap metadata, damaged-file preservation, early platform rejection, documented timestamp rounding and concurrent-meeting tests.
- `cargo test --workspace --all-targets --offline`: 21 tests passed; the subprocess helper is explicitly ignored in the parent test run and invoked at all four boundaries by the SIGKILL test.
- Independent Claxon decoding verifies PCM24 samples, native rates/channels and a short final chunk. Tests cover repeated recovery, poisoning, corrupt/missing files, unknown meetings, concurrent schema initialization and explicit epoch transitions.
- A three-second synthetic CLI run published six independent Mic/System chunks; recovery verified all six with no new gaps.


## Initialization-race validation

The strengthened first-open test uses eight barrier-synchronized writers and 20 fresh roots per invocation; each writer publishes/finishes a generated FLAC chunk, and every meeting is recovered/verified. Twenty standalone invocations passed: 400 fresh manifests and 3,200 concurrent meeting writes/recoveries, in 170.85 seconds. The full recorder suite passes (16 tests; the subprocess helper remains intentionally ignored in the parent run) with the existing SIGKILL publication-boundary, poisoning and writer/recovery lock tests. Short-budget persistent-lock and unsupported-version tests verify bounded failure, no half-created schema and preservation of existing version/data. `cargo clippy -p vt-record --all-targets --offline -- -D warnings` passes. No test was serialized or ignored to hide the original failure.

## Encoder regression validation

A generated delayed-echo/short-reply transient independently reproduced the oversized predictor file. Tests require bounded output and exact Claxon PCM24 decoding at 16 kHz/mono, 48 kHz/stereo and 192 kHz/eight channels. The explicit fallback preserves the native-rate contract; no private audio or output device is used. Tiny final native chunks also reproduced the library's invalid minimum-block-size metadata; the same standards-conformant fallback handles these without padding. The optional reference `flac` CLI was unavailable, but an installed FFmpeg independently decoded fallback output bit-exactly, including full two-second 192 kHz/eight-channel audio.

## Successful-finish lock handoff follow-up

A later parallel full-suite failure returned the recorder's `Busy` variant on immediate recovery after successful finish. Local standard-library source and primary Rust/Apple documentation show that duplicate/forked descriptors reference one shared flock, while close-on-exec does not eliminate the pre-exec descriptor interval. Retaining `Recording`'s actual lock alias through finish reproduced the failure deterministically, independently of scheduler timing. The original concurrent spawn responsible remains unproven.

After committing final status and marking the consumed writer non-writable, finish now explicitly unlocks. No recording operation follows it. The original independent-decoder regression requires active-writer recovery to remain Busy, then immediate recovery to verify all three generated FLAC chunks while the descriptor alias stays open. This changes successful handoff only: no global recovery retry, destructor change, weakened active-writer exclusion or publication/crash policy is introduced.
