# Recording Health and Source Timing Baseline

## Flow invariants

- Recording preflight verifies at least 100,000,000 available bytes before opening the helper or creating a meeting; unknown availability blocks startup explicitly.
- Available bytes mean unprivileged available filesystem blocks times fragment size, with checked arithmetic.
- Active recording checks availability at least every thirty seconds and before each buffered chunk publication. A chunk cannot reserve pending metadata unless its upper bound plus 2 MB fits known available space.
- Below 2,000,000,000 bytes a warning is published; below 500,000,000 live inference is suspended and raw capture continues; below 100,000,000 capture stops and drains accepted raw frames.
- Inference suspension clears live partials, prevents new model setup and ends queued model/DSP work. It remains sticky until the next recording; recovered space can permit saved processing.
- Raw audio, native formats, hardware timestamps and frame indexes remain unchanged by health/timing controls.
- Current source time comes from a validated helper Mach/raw-uptime origin and the same OS monotonic clock. App startup and model-finalization elapsed time never supply note/audio timestamps.
- Missing/unknown/changed clocks stay explicitly approximate. A note before any source-clock/audio observation is rejected. Observed-audio notes carry unsynchronized provenance.
- Saved playback bookmarks use the matching playback cursor without starting output. Other saved text notes are explicitly untimed at zero.
- Note timing metadata, note content and FTS update publish atomically. During an active recording the FTS update is the note's own row only; the command writes without holding `runtime` (the capture loop needs it for source-clock events).
- Finishing duration freezes at the last observed source endpoint, retaining native gap/pause semantics.
- Capture and DSP messages contain at most twenty milliseconds of valid native audio; STT/speaker messages contain at most ten milliseconds. Invalid oversized inputs are rejected.
- Sequential global-backlog load shedding above ten seconds remains pending and is not implied by the queue bounds.

- Pending/purged audio is authoritative unavailable state and is never repaired into fabricated gaps by saved recovery/inference. Confirmed retention recovery precedes interrupted staging/audio cleanup at startup.
- Waveforms release the runtime mutex during decoding and hold an RAII reader lease; maintenance is blocked until all such readers finish, including error/panic cleanup.
- Text writes/exports are blocked during maintenance. Note readback preserves optional provenance, and text exports distinguish untimed, approximate, unknown and exact timestamps.

- Global inference lag is the maximum same-track difference between accepted raw source time and actual DSP/STT/speaker input completion; silent/VAD-suppressed input acknowledges completion and absent workers are excluded.
- Strictly greater than ten seconds of lag with fresh input must persist two monotonic seconds before shedding; subsequent ordered stages are at least five seconds apart. Order is partial publication, configured speakers, live STT. Finals survive stage one; all raw capture survives inference shedding.
- Source gaps, format/epoch changes and pause reset the lag basis and reject old generation tokens. Paused/stale source cannot escalate; a stage disabled by backlog shedding or disk suspension does not reactivate during a recording.
- Queue drops/resource shedding create bounded per-run inference evidence, never fabricated raw gaps. Audit persistence runs off the capture thread and finishes before run sealing. Historical evidence stays with its source run; later refinement is separate. `complete` seals only the audit ledger: periodic snapshots are false, mandatory final snapshots true. Missing expected or unsealed historical ledgers remain explicitly incomplete after restart, with no invented gap ranges. Final write failure appends a warning; if even warning persistence fails, a ready in-memory warning is retained and automatic jobs are skipped.
- The speech worker recovers in place from a per-message recognizer error: it records a `recognizer_error` gap and a non-terminal failure, resets the track, and rebuilds the engine only for an engine error or a failed reset. The same cause twice in a row is terminal; at most 20 recoveries per recording (`recovery_budget_exhausted` beyond that). The speaker worker is never restarted. Backlog shedding is sticky for the recording.
- Live transcription state (`RecordingView.live_transcription`) is `active` until it stops for a named `LiveStopReason`, then `stopped` with `since_source_ms` for the rest of the recording. A recording where it was not configured reports none.
- Each worker's first and latest failure are kept in the health report `failures[]` (cause at most 300 characters, SQLite extended code when present), outside the bounded gap list and warnings.
- Gaps of the same worker, track and reason merge when adjacent within 1 ms regardless of generation; one worker holds at most 42 of the 128 gap entries.
- Every recording has a health report, including recordings without live models. Its `capture` section keeps at most 32 entries per list (rebuilds, slow calls, epochs, stalls) with exact totals.
- The helper is never stopped or restarted because of a stall. `capture.state` is derived only while recording and not paused, only after a first heartbeat: 5 s without a heartbeat is `helper_silent`, 5 s of heartbeats without callback progress is `no_audio_callbacks`, and a session-loop iteration of 500 ms or more is the consumer's delay (`consumer_slow` stall record at 5 s), never blamed on the helper.
- Housekeeping writes on the recording loop (duration, unforced health, source timing, pause/resume status) wait at most 250 ms in total (`store::write_with`; at most 5 ms of that in the in-process writer queue on the capture loop, the rest for the recorder's SQLite lock) and skip on a lock error with one warning per recording; non-lock errors remain fatal. On the chunk flush path `save_clock` and the forced health persist also use the 250 ms timeout and are skipped on a lock error (same single warning, `HOUSEKEEPING_BUSY_WARNING`). A lock no longer ends a recording within `MANIFEST_LOCK_BUDGET` (60 s): the recorder retries its manifest writes (pending insert, ready update, gap, finish) and only a lock outlasting that budget, or a non-lock error, poisons the writer. While a write waits the capture consumer loop is blocked; the 128-slot channel (2.56 s), the pipe and the helper's ring (at least 10 s) absorb it, after roughly 12.5 s the helper reports an explicit `overflow` gap and the recording continues. A stop request waits for the pending write. A single wait over 1 s raises `RECORDER_LOCK_WAIT_WARNING` once. See [durable recording](../../recording/durable-recording/durable-recording.md#manifest-write-lock-tolerance).
- The final echo-info save retries briefly (3 x 1 s) and a lock is a warning, not a failure.
- Live transcript lines that could not be saved are `segment_rejected` (deterministic error) or `persistence_failed` (queue overflow, or unsaved at the 5 s close deadline) inference gaps; the raw recording is unaffected.
- Live speaker turns that could not be saved because of database lock contention are recorded as inference gaps with reason `persistence_failed` (see [speaker turns](../../diarization/speaker-turns/speaker-turns.md)); the raw recording is unaffected.

## Flag rules

| Condition | Severity | Blocks? |
|---|---|---|
| `<2 GB`, `>=500 MB` | Warning | No capture/inference block |
| `<500 MB`, `>=100 MB` | Warning; sticky inference suspension | Live inference and saved processing; raw capture continues |
| `<100 MB` | Critical; clean stop | New capture and continued producer input after shutdown/drain |
| Availability unknown | Explicit warning | New capture/saved processing; active inference suspended |
| Buffered chunk upper bound plus margin does not fit | Recording error before pending reservation | That publication; existing recovery policy applies |
| Legacy/unknown/unavailable clock | Approximate provenance | Notes before any source observation; later fallback is explicit |
| Fresh source and sustained `>10s` inference lag | Ordered sticky live degradation | Partials, then speakers, then live STT; raw capture continues |
| Speech recognizer error (new cause) | Warning; `recognizer_error` gap and failure entry | Short stretch of live text; worker continues |
| Same recognizer cause twice in a row, or 20 recoveries used | Terminal; live transcription `stopped` (`worker_failed` / `recovery_budget_exhausted`) | Live STT for the rest of the recording; raw capture continues |
| Live speaker worker error | Terminal; not restarted | Live speaker labels for the rest of the recording |
| Database lock (shared writer busy, or the recorder's SQLite lock) on a recording-loop housekeeping write, `save_clock`, flush-path health persist or output route save | One warning (`HOUSEKEEPING_BUSY_WARNING`); write skipped (route changes are kept and retried) | No |
| Database lock on a recorder manifest write | Retried up to 60 s; a wait over 1 s warns once (`RECORDER_LOCK_WAIT_WARNING`) | Capture loop blocked while waiting; the helper may report an `overflow` gap after about 12.5 s |
| Database lock on a recorder manifest write outlasting 60 s | Writer poisoned; recording ends | Further writes; recovery adopts what was published |
| No helper heartbeat or callback progress for 5 s | `capture.state` stalled; record in report | Observation only; capture is not touched |
| Note metadata/FTS failure | Atomic rollback | Entire new note publication |

## Ordered side effects

1. Initialize additive health/timing schema and read actual availability.
2. Check consent, helper availability, disk preflight and existing runtime/capture exclusion.
3. Create recording and live run; reset source timeline and inference-suspension state for that recording.
4. Accept native clock/audio evidence, publish health state and persist periodic/transitional metadata.
5. Suspend inference under pressure while raw recording continues; before each chunk, check its space budget.
6. On critical pressure, request helper Stop, drain accepted raw input, flush and seal the run. Freeze source duration and suppress automatic jobs until space permits.

## Defaults

| Setting | Default |
|---|---|
| Disk thresholds | Decimal 2 GB / 500 MB / 100 MB |
| Background recording poll | Thirty monotonic seconds; per-chunk checks additionally apply |
| Chunk publication margin | 2,000,000 bytes plus bounded PCM24/headers |
| Backlog confirmation / stage interval / input freshness | 2 / 5 / 2 monotonic seconds |
| Per-run inference evidence bounds | 128 coalesced source ranges (42 per worker), 12 distinct warnings; explicit truncation; 2 failures per worker; 32 entries per capture list |
| Speech recovery budget | 20 per recording; same cause twice in a row is terminal |
| Stall / slow-iteration thresholds | 5 s / 500 ms |
| Recording-loop housekeeping lock wait | 250 ms |
| Recorder manifest write lock budget | `MANIFEST_LOCK_BUDGET` 60 s total; 250 ms per attempt; 5 ms doubling to 100 ms backoff |
| Lock-wait warning threshold | 1 s |
| Shared native clock | Protocol 3 `source_clock_uptime_raw`; no callback-arrival origin |
| Legacy timing | Explicit latest-audio fallback, no wall-clock estimate |
| Capture/DSP/STT/speaker queue bounds | 2.56 / 12 / 6 / 6 seconds aggregate audio |

## Access and verification

Health and timing are local metadata and inherit meeting/note ownership. No process/device identity or audio content belongs in health diagnostics. Tests use generated temporary recordings, fake helpers and injected space/clock readings; they never fill a disk or open audio devices. Injected blocked consumers prove actual ordered shedding while decoded native raw audio remains exact. Long-call drift and full M0 hardware/lifecycle gates remain open.

- Initial disk suspension disables the expected-inference-audit marker and is retained in capture provenance; intentional suspension does not create a false missing-audit warning.
- Pause context resets use pause-specific notices, distinct from native source-discontinuity notices.

- Detection observation freshness uses a separate suspend-aware clock. Audio source epochs remain raw uptime; the two namespaces are never substituted for each other.
- A missing observation clock cannot authorize detected/automatic capture and does not disable ordinary manual recording.
