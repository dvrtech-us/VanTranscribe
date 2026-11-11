# macOS Capture

The macOS helper captures a selected microphone and, in remote mode, a stereo process tap into separate streams. A Swift control thread owns Core Audio objects; a C ring isolates the real-time callback from JSON publication and Rust consumers. `vt-capture` manages the child process and decodes its event stream. This is the Swift fallback already allowed by the approved design's R2 boundary, not a replacement desktop architecture.

M0 remains incomplete. The packaged app has captured both tracks with matching short-run durations and recognized microphone speech plus an external audio fixture on System. Deterministic ring, transport and configuration-filter checks also pass. Long-call drift, full chirp/self-exclusion and lifecycle gates remain pending; these short checks do not establish a crash-loss bound.

## User flow

1. Open **New recording** and choose remote meeting or in-person capture. Remote mode requests Microphone and System Audio Recording access; in-person mode creates no process tap.
2. Choose an input and acknowledge that participants have been informed. The capture helper itself does not collect consent; the calling app owns this prerequisite.
3. If the default input is Bluetooth, select that input explicitly or choose a non-Bluetooth microphone. The helper never implicitly opens the default Bluetooth microphone.
4. On first capture, allow microphone access when macOS requests it. The app bundle supplies `NSMicrophoneUsageDescription` and `NSAudioCaptureUsageDescription`. Device enumeration alone does not start recording or request access.
5. The helper publishes format, audio, level and lifecycle events while the session worker records and transcribes. If system tap creation fails, remote mode reports a nonfatal error and continues with microphone audio.
6. Stop recording. The caller sends `stop` on stdin and keeps reading events until EOF so the helper can publish its final partial packet before the session flushes its recorder. Forced termination is a timeout fallback.

For direct development commands and native build instructions, see [native/macos/README.md](../../../../native/macos/README.md). Direct capture commands also require the appropriate app privacy context and user permission; a successful build is not a capture test.

## Technical flow

### Setup and the shared aggregate

`validatedCommand` checks every CLI argument before permission queries or audio setup. Capture requires explicit `--mode remote` or `--mode in_person`; unknown/repeated/mixed commands, absent values and invalid parent PIDs fail with a fatal error event. Read-only commands must be used alone. The established `--list-devices` command advertises protocol 4 and supported commands; callers check this safe handshake before invoking a newer flag on an unknown helper version. Old helpers may ignore new flags, so probing them without this handshake is unsafe. `--version`/`--help` are protocol-3 read-only commands.

`runCapture` accepts macOS 14.4 or newer, starts a stdin control reader, requests microphone access and creates an idle-sleep assertion. `CaptureEngine.start` resolves either `--mic-device` or the default input, rejects an implicit Bluetooth default, and requires 1–8 microphone channels.

For remote mode, `excludedAudioProcesses` reads Core Audio process objects, finds their PIDs and follows public parent-process information up to 64 steps to identify descendants of the app PID. The tap receives **process AudioObjectIDs**, not PIDs. It is private, stereo and unmuted. The capture-only helper has no render path and is omitted from that exclusion list to avoid rebuild churn.

The private aggregate uses the selected microphone as its main subdevice and enables drift compensation on the tap. The microphone's input channels precede the tap's two channels in the intended layout. Setup checks the aggregate's total input channel count and every input stream's sample format/rate before starting the IOProc. Unsupported layouts fail rather than being silently mislabeled. The tap's auto-start-wait option is disabled so microphone capture does not wait for a remote app to render sound.

Core Audio must expose little-endian Float32 PCM at 8,000–192,000 Hz. Setup requests a 512-frame IO quantum and reads back the actual quantum. Ring creation accepts a maximum quantum of 8,192 frames. All Core Audio object ownership and teardown remain on the helper's control thread.

### Real-time and worker boundaries

`captureIOProc` accepts valid input hardware host timestamps and calls `vt_ring_write`. The C single-producer/single-consumer ring is preallocated and prefaulted before capture. Its slot count is `ceil(sample_rate * 10 / actual_quantum) + 2`, providing at least ten seconds for the configured quantum. Lock-free atomic indexes publish slots; compile-time assertions require lock-free atomics on the target.

The callback performs bounded copies only: no allocation, locks, JSON encoding or pipe writes. It handles both interleaved and planar buffer lists. A full ring, oversized callback or insufficient channel layout drops that callback, advances frame accounting and carries its overflow range into the next published packet. A final unqueued drop is reported after the producer has stopped. The callback also bumps a lock-free, allocation-free callback counter (`vt_count_callback`) used by heartbeats.

On the worker side `OverflowCoalescer` merges overflow drop runs separated by less than one second of delivered audio into one `overflow` gap from the first dropped frame to the last. The audio between runs is discarded (not published) and counted, and following audio may be held up to one second (bounded to two seconds of frames). The gap length is `dropped_frames + discarded_frames`, both exact at the epoch rate, and `overflow_runs` counts the merged runs. A run followed by a full second of clean audio is closed and the held audio is released in order; `finish` (stop or rebuild) publishes a pending gap. Other gap reasons carry zeros for these fields.

`CaptureEngine.drain` copies each track to worker-owned arrays. `Accumulator` groups approximately 100 ms of audio, flushing early at epoch or frame discontinuities. Nonfinite native samples are replaced with zero before JSON encoding. Level events contain a clamped RMS-derived value; they are signal indicators rather than proof of permission state.

### Timing and lifecycle

Audio events carry epoch-local `first_frame` and the first sample's observed `mHostTime`, converted with the Mach timebase to nanoseconds relative to **that helper process's startup origin**. A helper restart resets its native epoch/time origin. The session must retain these observations and map them to its own continuous timeline across pause/resume; callback-arrival time is not an acceptable replacement hardware timestamp.

Core Audio property notifications request a value comparison, not an unconditional rebuild. `CaptureFingerprint` records the selected input identity/aliveness/nominal rate/channels, aggregate formats/input channels/actual quantum, default output identity/rate/channels, and published process-exclusion IDs. Aggregate stream IDs and reserved ASBD bytes are excluded because they can change without changing the capture format; rates are compared at millihertz precision. Output format changes remain detectable even if a Bluetooth device retains the same object ID.

`ConfigurationChangeGate` ignores a snapshot equal to the active baseline. After a rebuild or start the baseline is provisional: opening an input can itself change the route (for example a Bluetooth headset switching profile), so consequences of the helper's own start are absorbed into the baseline until it has been unchanged for 75 ms and at least one second have passed, or forced after ten seconds (reported `stable=false`). While settling the loop observes on its own cadence. A change to what the running ring was opened for (device, rate, channels, quantum above the ring size) or confirmed device death is never absorbed, and a published exclusion change seen while settling rebuilds once the gate is armed. A failed query keeps the last successfully read value. This yields one rebuild per device change; before, a baseline read mid-transition caused a second rebuild and a second short gap. Material changes must remain stable for 75 ms; returning to baseline cancels a pending change, and a different candidate restarts the stability window. A confirmed input `alive=false` bypasses the window. Failed/uncertain identity or property reads alone do not prove immediate device loss, but a persistently changed snapshot still rebuilds after the short window. Direct input/output format listeners and a monotonic two-second fallback poll catch changes without relying solely on aggregate notifications.

Actual default-route/device/format/exclusion changes and sleep/wake trigger capture rebuilds. `rebuild` stops and drains the old producer, emits its gap, increments the native epoch, and announces the new format. A further rebuild before new audio arrives continues from the earlier gap (`lastEnd` advances) instead of re-reporting the old epoch's end. Each rebuild is timed and reported as a diagnostic (below). An explicit selected device that disappears can make rebuilding fail; uninterrupted fallback across all hardware transitions is not yet verified.

The exclusion list participates in the fingerprint and is checked every two seconds. A stable changed set causes a rebuild. Public parent traversal cannot reliably attribute WebKit processes parented by launchd. The calling app must keep in-app playback disabled while recording, and the helper-process exclusion gate remains pending an actual self-capture test. Capture exclusions continue to use public ancestry; optional responsibility lookup is confined to the independent read-only meeting-app hint below.

### Capture diagnostics

Helper capability `capture_diagnostics` (advertised in `capture_capabilities`; `helper_protocol` stays 4) adds typed diagnostics. Capture sessions publish them as `status` events whose `diagnostic` object has a `kind`; read-only probes never publish them, so their stdout stays one JSON document. A parent that predates them ignores them as plain status. Every field is a duration, count, enumerated reason or public API name.

| `kind` | Fields | Meaning |
|---|---|---|
| `heartbeat` | `epoch`, `callbacks`, `control_ticks`, `queued_packets`, `max_emit_ns`, `at_ns` | About once per second from the control loop. `callbacks` is the process-lifetime IOProc count, `max_emit_ns` the longest stdout write since the last heartbeat (pipe backpressure), `queued_packets` the ring backlog |
| `rebuild` | `reason` (`device_change`, `process_exclusion_change`), `trigger` (`notification`, `poll`, `wake`, `settle_check`), `previous_epoch`, `epoch`, `trigger_age_ns`, `fingerprint_ns`, `stop_ns`, `start_ns`, `result` (`ok`, `failed`, `cancelled`) | One rebuild with its timing; `trigger_age_ns` runs from the first observation of the change to the start of the rebuild |
| `slow_call` | `call`, `duration_ns`, `phase` (`idle`, `stop`, `start`, `fingerprint`, `route`) | A Core Audio call that blocked the control thread for more than 100 ms. `call` is an API name with an optional property selector code; the parent replaces anything else with `other` |
| `epoch` | `epoch`, `sample_rate`, `mic_channels`, `system_channels`, `quantum`, `input_transport` | Stream facts once the epoch's device has started |
| `settled` | `epoch`, `settle_ns`, `absorbed`, `stable` | The baseline fingerprint was fixed after the new stream settled |

**Privacy exception (owner-approved).** The capture baseline forbids persisting a device UID or name. `input_transport` is the single, precisely scoped exception: it carries the input's TRANSPORT TYPE only (`built_in`, `bluetooth`, `usb`, `aggregate`, `virtual`, `other`, `unknown`), is persisted in the recording's health report, and never identifies a device. Device names and UIDs still never leave the helper in diagnostics.

There is no watchdog. A Core Audio call that never returns is visible only as missing heartbeats; the helper is never killed or restarted. Long device-change outages are not fixed by this feature, only measurable. The parent's stall derivation and persistence are in [Recording Health](../recording-health/recording-health.md).

### Conservative output-route evidence

Before the first format/audio of every native epoch, the helper emits `output_route`; it repeats the event when meaningful local route evidence changes. This observation has its own 250 ms monotonic poll plus process/device/stream property notifications. Notifications request control-thread reads, never audio-callback work. Route evidence alone does not alter the capture-configuration fingerprint or force an aggregate rebuild. Actual device/format changes retain the existing rebuild rules; rebuilding discards the previous route observation and emits fresh evidence.

The default output is only a candidate. For each active output process included in the live global tap, the helper reads `kAudioProcessPropertyDevices` with **output scope** and requires the exact device list to contain only that candidate. Missing process lists/running states/device lists, no active outputs, additional devices and app-specific nondefault routes all skip eligibility. Idle input-only processes are ignored; tap exclusions are the actual published exclusion list. Process inventory is capped at 4,096 objects, and unreadable/excessive inventories are ineligible.

Built-in transport alone never establishes speakers. Eligible evidence additionally requires a readable nonempty selected output data-source list, every active output stream reporting the speaker terminal, and a readable output jack state explicitly reporting disconnected. Connected jacks/headphone terminals, USB/Bluetooth/HDMI/DisplayPort, aggregate/virtual/other outputs and unknown evidence are ineligible. SDK terminal types describe general stream functionality; selected data-source IDs are opaque and jack controls are optional. Devices lacking this positive evidence therefore remain unsupported rather than being guessed as speakers. No device-name or localized-name matching is used.

A created live global tap/system track and matching active-process routes are also required. No-reference/in-person operation remains ineligible. `OutputRoute::aec_eligible` independently validates every positive field rather than trusting an `eligible` reason string. This feature publishes eligibility evidence; session DSP owns actual cancellation, reset and derived-audio handling. A timestamp records when the control thread observed evidence, not an exact hardware route-switch time. Deterministic tests establish the policy, not acoustic echo-removal quality or hardware route coverage.

The event contains anonymous typed evidence and a skip reason; no PID, app name, output object ID, UID or device name leaves the helper. `--output-route` performs a read-only anonymous inventory with no reference; it creates no tap/aggregate, starts no input/output and requests no capture permission.

Primary API references: [Apple process devices](https://developer.apple.com/documentation/coreaudio/kaudioprocesspropertydevices), [Apple stream terminal type](https://developer.apple.com/documentation/coreaudio/kaudiostreampropertyterminaltype), [Apple global stereo tap](https://developer.apple.com/documentation/coreaudio/catapdescription/init(stereoglobaltapbutexcludeprocesses:)). The installed SDK's `AudioHardware.h` documents output scoping and jack/data-source controls; `AudioHardwareBase.h` defines speaker/headphone terminal semantics.

### Read-only known meeting-app observation

`vt_capture::meeting_apps(helper)` performs the safe `--list-devices` handshake
before ever issuing `--meeting-apps`. Both protocol >= 4 and exact underscore
capability `meeting_apps` are required. Unsupported old helpers receive only the
established command and return an incomplete observation. The handshake and new
probe share one three-second deadline and 256 KiB output budget. Nonblocking reads
bound retained descendant pipes; failure kills the isolated child process group
and reaps the helper. The public `list_devices` API retains its original behavior.

The new native command reads `kAudioHardwarePropertyProcessObjectList`, bounded
at 4,096 objects, and strict-size `kAudioProcessPropertyIsRunningInput`/PID values.
It creates no stream, aggregate or tap and requests no audio permission. For
active inputs, an optional `dlsym` lookup of the OS responsibility resolver takes
precedence over parentage; a public parent walk provides a fallback with at most
32 steps and cycle detection. Helper/caller PIDs and the exact own bundle are
excluded. Missing/unknown attribution, partial property reads and a changing
process list set `complete=false`; this cannot be used as evidence of absence.

`crates/vt-capture/resources/meeting-apps.def` is the single C/Rust catalog of
exact release bundle IDs for Zoom, current/classic Teams, Slack, Webex/Webex
Meetings, Chrome, Safari, Edge, Firefox and Brave. Prefix/helper-name guesses and
unrecognized app IDs are rejected. Swift emits only canonical `id`, `name`,
`kind` tuples (`meeting` or `browser`), sorted and deduplicated. Rust validates
those tuples against the same catalog, normalizes diagnostics and exposes
`MeetingApp`, `MeetingAppKind`, `MeetingAppSnapshot { complete, apps, error }`
and `known_meeting_apps()` for caller rule validation. No raw PID, executable
path, window/tab title or process history leaves the detector. Browser microphone
activity remains advisory; it does not establish a call. Observation and consent/
recording policy are separate; the session owns any prompts or approved rules.

This adds no database table or SQL artifact to capture. Optional responsibility
SPI is dynamically resolved and confined to hints; capture exclusions, clock,
raw recording, AEC and permissions retain their existing behavior. A missing
symbol falls back to bounded public ancestry and never makes unknown ownership
look known. Deterministic tests establish transport and policy, not real-call
precision or recall.

Primary references: [Apple input-running property](https://developer.apple.com/documentation/coreaudio/kaudioprocesspropertyisrunninginput),
the installed SDK `AudioHardware.h` process property contracts, and
[Chromium responsibility attribution](https://chromium.googlesource.com/chromium/src/+/refs/tags/134.0.6998.28/base/process/process_info_mac.mm).
Catalog examples are corroborated by [Microsoft Teams deployment](https://learn.microsoft.com/microsoftteams/new-teams-bulk-install-client),
[Slack desktop configuration](https://slack.com/help/articles/11906214948755-Manage-desktop-app-configurations),
[Microsoft Edge/Safari identifiers](https://learn.microsoft.com/en-us/azure/active-directory/devices/troubleshoot-mac-sso-extension-plugin),
and [Brave release branding](https://github.com/brave/brave-core/blob/master/app/theme/brave/BRANDING).

### Rust process transport and shutdown

Protocol 3 advertises `capture_capabilities=["source_clock_uptime_raw"]` independently of actual CLI `supported_commands`. `source_clock` announces the helper startup Mach origin in raw-uptime nanoseconds before microphone permission waits. Rust uses the same OS clock; hardware timestamps remain unchanged. See [Recording Health and Source Timing](../recording-health/recording-health.md) for anchor validation, elapsed durations and explicit note provenance.

`list_devices` retains the established read-only argument and API. Its subprocess stdout is nonblocking and capped at 256 KiB, with a three-second monotonic deadline followed by kill/reap on timeout. It creates no audio stream; the Tauri adapter schedules it through a blocking worker rather than the GUI thread. Hung/flooded fake helpers exercise the limits. This is an independent resilience improvement; it does not diagnose the earlier isolated frontend timeout.

`CaptureProcess::spawn_with_device` launches the helper with a piped stdin/stdout and the capture mode/app PID. `take_events` transfers stdout to a blocking worker iterator. Each event line is capped at 4 MiB; malformed events of a known type, or oversized events, terminate that iterator with an error. An event with an unknown `type` (from a newer helper) is decoded as an ignorable status, and an unknown or malformed `diagnostic` decodes as `Unknown`; neither ends the recording. Known types are `source_clock`, `output_route`, `format`, `audio`, `gap`, `level`, `status` and `error`. A record of an oversized line consumes at most the limit plus one byte before failure.

`request_stop` writes the stop command, closes stdin and returns without waiting. The session continues draining audio until EOF. `stop` also waits for exit, checking every 20 ms, and kills/reaps the helper after three seconds. `Drop` invokes this cleanup fallback. In the helper, EOF, SIGTERM and SIGINT request teardown. The producer is detached before ring storage is released; if neither callback removal nor aggregate destruction succeeds, storage is retained until process exit rather than freed under an active callback.

## Event contract

Stdout is reserved for one JSON object per line:

| Event | Fields | Meaning |
|---|---|---|
| `source_clock` | `clock`, `origin_ns`, `observed_ns` | Shared helper startup raw-uptime epoch, announced before permission waits; unknown clocks remain unsynchronized |
| `output_route` | `epoch`, `start_ns`, `route` | Anonymous output evidence, before epoch audio and after meaningful evidence changes |
| `format` | `track`, `sample_rate`, `channels`, `epoch`, `start_ns` | A track configuration is available |
| `audio` | `track`, `epoch`, `first_frame`, `host_time_ns`, `samples` | Interleaved floating-point samples with observed hardware position |
| `gap` | `track`, `epoch`, `start_ns`, `end_ns`, `reason`, `dropped_frames`, `discarded_frames`, `overflow_runs` | Explicit overflow, rebuild or other discontinuity. For `overflow` the range is a coalesced burst and its length is `dropped_frames + discarded_frames`; the three counters are zero for other reasons and for helpers that predate coalescing |
| `level` | `track`, `value` | Worker-computed signal level in 0–1 |
| `status` | `message`, optional `diagnostic` | Capture lifecycle information, or a typed capture diagnostic (above) |
| `error` | `message`, `fatal` | Recoverable degradation or a capture-stopping failure |

`route` contains `kind`, `transport`, `default_output_known`, `data_source_known`, `speaker_terminal`, optional `headphone_jack_connected`, `active_output_processes`, `process_devices_known`, `all_processes_same_output`, `global_tap`, `reference_available` and `reason`. Missing fields deserialize to unknown/ineligible defaults. Kinds are `built_in_speakers`, `headphones`, `external`, `unknown`; reasons distinguish missing reference/output/process evidence, no active output, headphones, external output, mixed process outputs and eligibility. `OutputRouteReason::explanation` supplies concise user-facing text.

Tracks are `mic` and `system`. `DeviceList` contains `devices`, `system_supported`, `helper_protocol`, `supported_commands` and `capture_capabilities` (legacy helpers default the latter fields to zero/empty); each input has `id`, `name`, `is_default` and `bluetooth`. Device IDs and names can identify hardware/owners and must not be copied into committed examples or logs. Diagnostics carry only the input transport type, never an ID or name.

## Key types and entry points

| Type/function | Source | Responsibility |
|---|---|---|
| `validatedCommand`, `testArguments` | `native/macos/CaptureHelper.swift` | Reject ambiguous CLI commands before any capture permission/setup; pure parser regression checks |
| `CaptureEngine`, `runCapture` | `native/macos/CaptureHelper.swift` | Core Audio setup, ownership, lifecycle, teardown and permission handling |
| `captureIOProc` | `native/macos/CaptureHelper.swift` | Hardware callback to the C ring |
| `Accumulator` | `native/macos/CaptureHelper.swift` | Worker packet grouping, sanitization and levels |
| `OutputRoute`, `OutputRouteObservation`, `observeOutputRoute`, `applyProcessEvidence` | `native/macos/CaptureHelper.swift` | Anonymous evidence collection, conservative route policy and independent change detection |
| `OutputRoute`, `OutputRouteKind`, `OutputTransport`, `OutputRouteReason` | `crates/vt-capture/src/lib.rs` | Serializable evidence and independently checked AEC eligibility |
| `StreamFingerprint`, `CaptureFingerprint`, `ConfigurationChangeGate` | `native/macos/CaptureHelper.swift` | Capture-relevant value comparison, short stabilization and post-start settling before rebuilds |
| `OverflowCoalescer` | `native/macos/CaptureHelper.swift` | Merges nearby overflow runs into one gap with exact frame accounting |
| `CaptureDiagnostic`, `InputTransport`, `RebuildReason`, `RebuildTrigger`, `RebuildResult`, `DiagnosticPhase` | `crates/vt-capture/src/lib.rs` | Typed, forward-compatible diagnostics; unknown values decode as `Unknown` |
| `VTAudioRing`, `VTFrameInfo`, `vt_ring_*` | `native/macos/AudioRing.h`, `AudioRing.c` | Preallocated SPSC storage, channel separation and overflow accounting |
| `CaptureProcess`, `CaptureEvents` | `crates/vt-capture/src/lib.rs` | Child lifecycle, bounded decoding and graceful-stop transport |
| `CaptureClock` | `crates/vt-capture/src/lib.rs` | Typed shared-clock identity with unknown/future fallback |
| `CaptureEvent`, `InputDevice`, `DeviceList`, `CaptureError` | `crates/vt-capture/src/lib.rs` | Typed IPC contract and failures |

## Integration points

- `src-tauri/build.rs` compiles/bundles the native executable; the desktop bundle provides privacy purpose strings.
- `crates/vt-session/src/session.rs` consumes helper events, preserves observed source positions, and owns the recorder/inference workers and pause/resume timeline mapping.
- `crates/vt-session/src/lib.rs` exposes device enumeration through the capture adapter. The desktop's `input_devices` and recording commands belong to the session/UI feature, not this crate.
- The recording manifest and durable chunk protocol are documented in [Durable Recording](../../recording/durable-recording/durable-recording.md).

## Routes, access control and privacy

This feature provides a local child-process protocol and CLI arguments, not HTTP routes or an account system. Capture requires the local user's OS permission and the caller's recording-consent flow. System-audio silence is never labeled permission denial; the public API does not provide a reliable system-audio permission preflight.

The helper sends sensitive PCM only over its local stdout pipe. It performs no network requests and does not write audio files. The session's downstream storage policy controls persistence and encryption. Raw events, device-owner names and hardware identifiers must not be copied into committed diagnostics or documentation.

## Database schema and SQL artifacts

None. `vt-capture` and the helper own no database tables or migrations. Session source-position metadata and durable audio manifests are owned by their respective features. No `sql/` artifact is required here.

## Validation and remaining gates

The Swift helper builds for Apple Silicon/macOS 14.4 with warnings treated as errors. C ring tests cover interleaved/planar separation, full-ring overflow, carry-forward and final drops. Four Rust adapter tests cover event positions, invalid modes, bounded malformed-event handling and preservation of final events after `request_stop`. Device enumeration succeeded outside the execution sandbox; sandboxed enumeration can return no devices. `--test-fingerprints` runs pure value-comparison checks without opening devices, covering unchanged notifications, actual input/output/aggregate/exclusion changes, transient reads, candidate stability and immediate confirmed device death.

Live packaged-app checks now establish microphone and System capture, matching short-run track durations, microphone transcription and recognition of an external fixture on System. That external-playback check also exposed repeated epoch transitions without an intentional device switch. The fingerprint filter addresses the unconditional-notification rebuild path; rerunning that scenario is required before asserting the observed churn is eliminated. Full chirp-based channel attribution, app/helper self-exclusion, long-call drift, HFP/device transitions and capture-to-durable loss remain unproven.

Output-route verification compiles the Swift helper with warnings treated as errors, runs no-audio `--test-output-routes` and `--test-fingerprints`, and tests Rust serialization/defaults and every required eligibility fact. Anonymous `--output-route` enumeration is read-only; sandboxed enumeration can expose no devices, which remains unknown/ineligible. No actual capture or playback was started for this increment.

Capture diagnostics add pure Swift self-tests for the settle gate (field sequence reproduced: baseline read mid-transition, then one rebuild) and the overflow coalescer, C ring tests for the new counters, and Rust decode tests for every diagnostic, unknown kinds/values/event types and the exact overflow frame accounting. Not verified: nothing here has been exercised by a real device change or a real recording; the observed long outages are unexplained until a recording carries these diagnostics. See the [2026-11-10 trail entry](trail/2026-11-10-capture-diagnostics-and-gap-coalescing.md).

Strict command validation additionally passes a pure native self-test and six subprocess rejection cases on a known-fresh binary; every rejected invocation emits only a fatal error, never format/audio. Read-only enumeration confirms the protocol-3 capability handshake.

The no-audio source-clock self-test passes 1,000 bracketed Mach/raw-uptime comparisons. Transport tests preserve origin nanoseconds and decode unknown clock identities without inventing synchronization; device tests enforce a deadline and byte cap using synthetic processes only.

Meeting-app validation adds three Rust fake-helper regressions (12 total transport
unit tests) plus the native pure meeting-app self-test. All five native no-audio
self-tests pass on a fresh arm64/macOS-14.4 helper. A safe-handshake-first read-only
host probe outside the sandbox read five input devices and reported a complete
snapshot with zero known active apps and no snapshot error; no actual stream or
permission request was started and no real process identifiers were logged. This
does not claim that meeting lifecycle/accuracy acceptance gates are complete.
