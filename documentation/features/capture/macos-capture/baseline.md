# macOS Capture Baseline

## Pipeline invariants

- The helper owns Core Audio objects on its control thread; Rust owns only a child-process transport.
- Remote capture keeps microphone and system audio separate. In-person mode creates no system tap.
- A selected microphone provides the aggregate clock; tap drift compensation is enabled.
- Supported input streams are little-endian Float32 PCM, 8,000–192,000 Hz, with 1–8 microphone channels and two system channels.
- The aggregate's total channel count and all input stream formats are checked before the IOProc starts.
- The IOProc copies into a preallocated SPSC ring and performs no allocations, locks, JSON encoding or pipe IO.
- The ring provides at least ten seconds of capacity for its configured IO quantum. Callback frames above that configured bound are dropped rather than overrun storage.
- Overflow advances native frame indexes and emits explicit gaps; the final pending overflow is reported only after the producer is stopped.
- Worker audio packets retain observed hardware host time and epoch-local frame indexes. Helper restarts reset native origins; the session owns continuous timeline mapping.
- Native property notifications alone never rebuild capture. A snapshot equal to the configured fingerprint is ignored; volatile stream IDs/reserved bytes are not format evidence.
- Actual configuration changes must remain stable for 75 ms. Confirmed input-device death is handled immediately; uncertain reads alone receive the short stability check.
- The fingerprint includes selected input identity/aliveness/rate/channels, aggregate formats/channels/quantum, default output identity/rate/channels and exclusion IDs. Bluetooth profile changes on an unchanged output ID remain detectable.
- Public process-parent traversal determines process AudioObjectID exclusions. Launchd-parented WebKit attribution remains unproven; playback is disabled during capture by the caller.
- A default Bluetooth input is never opened implicitly. A Bluetooth input requires an explicit UID selection.
- System-audio silence does not prove permission denial.
- `request_stop` does not wait for process exit. The caller drains final events to EOF before flushing downstream recording state.
- Callback context storage is not freed while an IOProc might still reference it.

- Every epoch announces anonymous output-route evidence before audio. Missing evidence defaults to unknown/ineligible.
- AEC eligibility requires verified built-in speakers (selected data source, active speaker terminals and explicitly disconnected output jack), an available global tap reference, and exact same-device output routes for every active included process. Built-in transport or default output alone is insufficient.
- Output evidence uses control-thread reads/listeners and a 250 ms monotonic fallback poll; the real-time callback is unchanged. Route events alone never force capture rebuilds.
- Route metadata contains no PID, app name, output object ID, device UID or name.

- Capture requires an explicit valid `--mode`. CLI validation rejects unknown/repeated/mixed commands, missing values and invalid parent PIDs before permission requests or audio setup. Read-only commands are exclusive.
- The established read-only `--list-devices` command advertises protocol/capabilities; callers verify support before issuing a newer flag to an unknown helper. Legacy handshake fields default to zero/empty.

- Protocol 3 announces a raw-uptime clock origin before permission waits; hardware host timestamps keep their original epoch. Unknown/legacy clock support remains explicit.
- Device discovery uses only `--list-devices`, nonblocking stdout, a 256 KiB cap and a three-second child deadline; timeout kills/reaps the helper. GUI discovery runs off the main thread.

- Meeting hints require a fresh established device-list handshake with protocol >= 4 and exact `meeting_apps` capability before sending `--meeting-apps`; unsupported helpers receive no new flag.
- The handshake and meeting probe share one three-second deadline and 256 KiB stdout budget. Timeout, overflow or unsuccessful exit kill the isolated process group and reap the helper.
- Meeting hints read at most 4,096 Core Audio process objects and active input state only; no stream/tap/aggregate or permission request is created.
- Responsibility attribution has priority when recognized; unavailable/unrecognized responsibility falls back to a public walk capped at 32 steps with cycle detection. Unknown/partial/unstable evidence is incomplete and never proves an app ended.
- Only exact catalog bundle IDs produce canonical app tuples. Helper/caller/own-app activity is excluded; no PID, path, title or process history is emitted. Browser activity remains advisory.
- The C helper and Rust validation consume the same `meeting-apps.def` catalog. Rust rejects unknown/mismatched tuples and normalizes arbitrary native error text.

## Failure and flag rules

| Condition | Severity | Blocks? |
|---|---|---|
| Unknown/repeated/mixed CLI option, missing value or invalid parent PID | Fatal error | Capture permission/setup; no format/audio events |
| Mode other than `remote`/`in_person` | Error | Process creation |
| Unsupported macOS version | Fatal error | Capture |
| Missing/invalid input or implicit Bluetooth default | Fatal error | Capture; select an available input explicitly |
| Microphone access unavailable | Fatal error | Capture; user must change OS permission |
| System process tap creation fails | Nonfatal error | System track only; microphone continues |
| Unexpected channels, rate, byte order or stream type | Fatal error | Capture; never silently mislabel data |
| Ring full, oversized callback or insufficient channels | Overflow gap | That callback's audio range |
| Unchanged property notification | Ignored | No |
| Changed configuration snapshot | Stable 75 ms, then rebuild and gap | Capture during rebuild |
| Confirmed input `alive=false` or wake after suspension | Immediate rebuild | Capture during rebuild |
| Transient uncertain property read that returns to baseline | Pending change cancelled | No |
| Selected input disappears and cannot be reopened | Fatal rebuild failure | Continued capture; automatic hardware fallback is not claimed |
| Missing speaker/jack/data-source/process-route evidence or mixed/nondefault output | Explicit route skip reason | AEC eligibility only; capture continues |
| Headphones, external/wireless/virtual output, absent global reference or no active outputs | Explicit route skip reason | AEC eligibility only; capture continues |
| Device inventory exceeds three seconds or 256 KiB | Discovery error; child killed/reaped | Inventory only; no capture starts |
| Unsupported meeting-app protocol/capability | Incomplete unsupported observation | New flag is never sent; no absence evidence |
| Unknown active process, unresolved ancestry, partial/unstable input inventory | Incomplete observation | No absence evidence |
| Unknown/mismatched app tuple or malformed meeting snapshot | Transport error | No absence evidence |
| Malformed event or event over 4 MiB | Transport error | Further events from that iterator |
| Callback cannot be detached and aggregate destruction also fails | Fatal error | Continued capture; retain callback storage until process exit |
| Helper does not exit after stop | Forced termination | Graceful shutdown after three seconds |

## Ordered side effects

1. The app owns consent and input selection before asking the helper to capture.
2. The helper resolves microphone access, creates an idle-sleep assertion and opens the tap/aggregate configuration.
3. Setup allocates/prefaults the ring, announces route evidence and formats, attaches the IOProc and starts capture.
4. The callback publishes slots; the worker splits tracks, emits audio/levels and carries explicit gaps.
5. A notification/poll compares capture-relevant values. An unchanged snapshot does nothing; stable material changes or confirmed device death initiate rebuilding. A rebuild stops and drains the old producer, emits its gap, increments the epoch and announces the new format.
6. Stop/EOF/signals request teardown. Rust continues reading while the helper detaches the producer and emits its final packet, releases aggregate/tap/assertion, and exits.
7. The caller flushes its downstream recorder only after event EOF, or records the implications of a forced stop.

## Defaults and configuration

| Contract | Current value |
|---|---|
| Platform | Apple Silicon, macOS 14.4+ |
| Capture mode | Explicit `--mode remote` or `--mode in_person`; no implicit capture |
| Helper protocol | 4; advertised through read-only `--list-devices` with supported commands |
| Input selection | Explicit `--mic-device`, otherwise OS default with Bluetooth guard |
| App PID | `--parent-pid`, otherwise helper parent PID |
| Requested IO quantum | 512 frames; actual property is read back |
| Maximum accepted configured quantum | 8,192 frames |
| Ring capacity | `ceil(sample_rate * 10 / actual_quantum) + 2` slots |
| Worker packet target | Approximately 100 ms |
| Process exclusion refresh | Every two seconds; parent walk capped at 64 steps |
| Output-route evidence poll | 250 monotonic milliseconds, plus native notifications; at most 4,096 process objects |
| Fallback configuration poll | Every two monotonic seconds in both capture modes |
| Changed snapshot stabilization | 75 ms; confirmed input-device death bypasses it |
| Rate fingerprint precision | Millihertz; ignores smaller floating-point representation noise |
| Native timestamp origin | Helper startup Mach host time |
| Device inventory deadline/output cap | Three seconds / 256 KiB; meeting handshake+probe share that budget |
| Meeting process/ancestry bound | 4,096 process objects / 32 parent steps, cycle detection |
| Meeting-app catalog | Exact release IDs, shared `crates/vt-capture/resources/meeting-apps.def` |
| Shared source-clock capability | `source_clock_uptime_raw`, announced before microphone permission waits |
| Maximum event line | 4 MiB; consume at most one additional byte to detect oversize |
| Rust stop polling/timeout | 20 ms / three seconds |
| System tap | Private stereo global tap, unmuted, no wait for first remote render |

## Access control and sensitive output

- Capture requires local OS permission; the helper does not grant consent on behalf of participants.
- The bundle provides microphone/system-audio purpose strings. Input enumeration, anonymous `--output-route` and `--meeting-apps` inventory open no input/output stream and create no tap/aggregate. Meeting inventory requests no audio permission.
- PCM travels through a local process pipe with no helper network transmission or helper audio files.
- PCM, transcript content, owner names and hardware identifiers are sensitive and never belong in committed diagnostics or examples.

## Verification limits

Build, deterministic ring/transport/fingerprint tests and device enumeration are established. Short packaged-app checks established both tracks with matching durations and microphone/external-system speech recognition. The observer-churn fix still needs that playback scenario rerun. Full channel/chirp validation, app/helper self-exclusion, hardware drift, lifecycle continuity and end-to-end durability are pending. This baseline does not assert that M0 is complete or that live crash loss is bounded by two seconds.
