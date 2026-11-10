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
- Note timing metadata, note content and FTS update publish atomically.
- Finishing duration freezes at the last observed source endpoint, retaining native gap/pause semantics.
- Capture and DSP messages contain at most twenty milliseconds of valid native audio; STT/speaker messages contain at most ten milliseconds. Invalid oversized inputs are rejected.
- Sequential global-backlog load shedding above ten seconds remains pending and is not implied by the queue bounds.

- Pending/purged audio is authoritative unavailable state and is never repaired into fabricated gaps by saved recovery/inference. Confirmed retention recovery precedes interrupted staging/audio cleanup at startup.
- Waveforms release the runtime mutex during decoding and hold an RAII reader lease; maintenance is blocked until all such readers finish, including error/panic cleanup.
- Text writes/exports are blocked during maintenance. Note readback preserves optional provenance, and text exports distinguish untimed, approximate, unknown and exact timestamps.

- Global inference lag is the maximum same-track difference between accepted raw source time and actual DSP/STT/speaker input completion; silent/VAD-suppressed input acknowledges completion and absent workers are excluded.
- Strictly greater than ten seconds of lag with fresh input must persist two monotonic seconds before shedding; subsequent ordered stages are at least five seconds apart. Order is partial publication, configured speakers, live STT. Finals survive stage one; all raw capture survives inference shedding.
- Source gaps, format/epoch changes and pause reset the lag basis and reject old generation tokens. Paused/stale source cannot escalate; no stage automatically reactivates during a recording.
- Queue drops/resource shedding create bounded per-run inference evidence, never fabricated raw gaps. Audit persistence runs off the capture thread and finishes before run sealing. Historical evidence stays with its source run; later refinement is separate. `complete` seals only the audit ledger: periodic snapshots are false, mandatory final snapshots true. Missing expected or unsealed historical ledgers remain explicitly incomplete after restart, with no invented gap ranges. Final write failure appends a warning; if even warning persistence fails, a ready in-memory warning is retained and automatic jobs are skipped.
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
| Per-run inference evidence bounds | 128 coalesced source ranges, 12 distinct warnings; explicit truncation |
| Shared native clock | Protocol 3 `source_clock_uptime_raw`; no callback-arrival origin |
| Legacy timing | Explicit latest-audio fallback, no wall-clock estimate |
| Capture/DSP/STT/speaker queue bounds | 2.56 / 12 / 6 / 6 seconds aggregate audio |

## Access and verification

Health and timing are local metadata and inherit meeting/note ownership. No process/device identity or audio content belongs in health diagnostics. Tests use generated temporary recordings, fake helpers and injected space/clock readings; they never fill a disk or open audio devices. Injected blocked consumers prove actual ordered shedding while decoded native raw audio remains exact. Long-call drift and full M0 hardware/lifecycle gates remain open.

- Initial disk suspension disables the expected-inference-audit marker and is retained in capture provenance; intentional suspension does not create a false missing-audit warning.
- Pause context resets use pause-specific notices, distinct from native source-discontinuity notices.

- Detection observation freshness uses a separate suspend-aware clock. Audio source epochs remain raw uptime; the two namespaces are never substituted for each other.
- A missing observation clock cannot authorize detected/automatic capture and does not disable ordinary manual recording.
