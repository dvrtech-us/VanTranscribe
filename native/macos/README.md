# macOS capture helper

`CaptureHelper.swift` owns the Core Audio tap, private aggregate device and IOProc.
The chosen microphone is the aggregate clock; the stereo process tap uses drift
compensation. Microphone channels precede the tap's two channels. The helper checks
the total input channel count and every input stream's floating-point format.
Actual channel ordering and self-exclusion must additionally be checked with
external/app-generated chirps on supported hardware before declaring M0 complete.

The IOProc calls `AudioRing.c`: it copies Float32 input into preallocated SPSC
storage with at least ten seconds of capacity for the configured buffer quantum.
It never allocates, locks, encodes JSON or writes to a pipe. A control/worker loop
drains the ring, splits Mic/System, and publishes roughly 100 ms NDJSON packets.
Queue overflow advances hardware frame indexes and emits explicit gap ranges;
an oversized callback is dropped instead of overrunning storage.

## Build

With Xcode command-line tools, from the repository root:

```sh
mkdir -p target/native
clang -std=c11 -O2 -mmacosx-version-min=14.4 -c native/macos/AudioRing.c -o target/native/AudioRing.o
swiftc -O -swift-version 5 -target arm64-apple-macosx14.4 \
  -import-objc-header native/macos/AudioRing.h native/macos/CaptureHelper.swift \
  target/native/AudioRing.o -o target/native/vt-capture-helper \
  -framework CoreAudio -framework AVFoundation -framework Foundation \
  -framework AppKit -framework IOKit
```

The app bundle supplies `NSMicrophoneUsageDescription` and
`NSAudioCaptureUsageDescription`; privacy prompts are initiated only when capture
starts. `--list-devices` enumerates inputs without recording and advertises `helper_protocol: 4` plus `supported_commands`. Callers must check this established read-only handshake before trying newer commands on a helper whose version is unknown. A sandboxed shell may
see no devices, so runtime capture verification must use the actual app.

```sh
target/native/vt-capture-helper --list-devices
target/native/vt-capture-helper --mode remote --parent-pid <app-pid> --mic-device <chosen-uid>
```

Capture requires explicit `--mode remote` or `--mode in_person`. No arguments, unknown/repeated/mixed options, missing values and invalid parent PIDs fail before permission checks or audio setup. Read-only commands must be used alone; `--help` and `--version` are supported since protocol 3.

Send `stop` followed by a newline on stdin to stop. EOF, SIGTERM and SIGINT also
release IOProc, aggregate, tap and idle-sleep assertion. `--mode in_person` opens
only the microphone. Opening a Bluetooth microphone requires explicit UID
selection; the default Bluetooth input is never opened implicitly.

## Events and lifecycle

Stdout is reserved for JSON events (`source_clock`, `output_route`, `format`, `audio`, `gap`, `level`, `status`,
`error`). Audio frames use epoch-local indexes and hardware `mHostTime` converted
to monotonic session nanoseconds. Callers must retain these observed positions
separately from any normalized worker timeline. Native nonfinite samples are
replaced by zero on the worker before JSON publication.

Protocol 3 announces `source_clock` with `clock=uptime_raw`, `origin_ns` and
`observed_ns` before microphone permission waits. Mach hardware timestamps and
Rust's raw-uptime clock share that epoch; the parent never estimates the origin
from callback/pipe arrival. `--list-devices` and `--version` advertise
`capture_capabilities: ["source_clock_uptime_raw"]`; unsupported helpers retain
explicit observed-audio fallback timing. Clock event names are capabilities,
not CLI probe commands.

Rust device discovery invokes only `--list-devices`, caps stdout at 256 KiB and
kills/reaps a helper that exceeds its three-second deadline. Reads are nonblocking,
so a hung driver or retained descendant pipe cannot stall the discovery worker.
The GUI adapter schedules discovery off its main thread.

Property notifications request a capture-configuration comparison; they never
rebuild capture by themselves. The fingerprint contains selected-input identity,
aliveness/rate/channels, aggregate formats/channels/quantum, default-output
identity/rate/channels and process-exclusion IDs. Unchanged notifications are
ignored. A changed fingerprint must remain stable for 75 ms before rebuilding;
confirmed input-device death bypasses that window. Stream object IDs, reserved
ASBD bytes and sub-millihertz numerical noise are excluded from comparison.
Rate/channel changes on an unchanged Bluetooth output ID are still meaningful.
A monotonic two-second configuration poll catches missed property notifications.

Actual changes and sleep/wake rebuild capture and increment epochs. The old
epoch's gap is emitted before the new format. If the tap cannot be created,
remote mode continues microphone-only with an explicit nonfatal error. Silence
does not establish that permission was denied.

The exclusion list contains Core Audio process object IDs found by public parent
process traversal from the app PID, and it refreshes during capture. A WebKit
process parented by launchd cannot be attributed by this public parent walk alone;
in-app playback must stay disabled while recording. This implementation does not
claim the pending chirp-based helper-process self-exclusion gate is complete.

## Output-route evidence

Every epoch emits anonymous `output_route` evidence before audio. The control
thread polls every 250 ms and observes device/stream/process changes separately
from the capture rebuild fingerprint. Eligibility requires a selected output data
source, active speaker terminals, an explicitly disconnected jack, built-in
transport, a live global reference and exact default-device equality for every
active included process's output-scoped devices. Headphones, mixed/app-specific
routes, external outputs and missing driver evidence skip cancellation; transport
or default device alone is never proof. Identifiers stay local to the helper.

`--output-route` reads anonymous evidence without creating a tap/aggregate,
starting input/output or requesting microphone permission. It reports no reference
because it does not capture. Sandboxed inventories may be empty; this remains
unknown, never inferred as speakers.

Only invoke `--output-route` on a freshly built helper or after its `--list-devices` response advertises `output_route`. Older helpers can ignore unknown options; never probe an unverified binary with a new flag. Input enumeration remains the compatibility-safe handshake.

See the [capture feature contract](../../documentation/features/capture/macos-capture/macos-capture.md)
for typed fields, limitations and primary Apple API references.

## Tests

`AudioRingTests.c` checks interleaved and planar channel separation, a full ring,
overflow carry-forward and a final unqueued gap without opening audio devices.

```sh
clang -std=c11 -Wall -Wextra -Werror -O2 -mmacosx-version-min=14.4 \
  native/macos/AudioRing.c native/macos/AudioRingTests.c \
  -o target/native/ring-tests -framework CoreAudio
target/native/ring-tests
target/native/vt-capture-helper --test-fingerprints
target/native/vt-capture-helper --test-output-routes
target/native/vt-capture-helper --test-arguments
target/native/vt-capture-helper --test-source-clock
```

The fingerprint self-test opens no audio devices. It covers unchanged observer
bursts, actual route/format/exclusion changes, transient failed reads, the
stability window and immediate handling of a confirmed dead input. A packaged
app has captured both tracks and recognized microphone/external-system speech in
short checks; long-call drift, full self-exclusion and lifecycle soak gates remain
pending. The changed fingerprint filter still needs the external-playback
scenario rerun before claiming unnecessary epoch transitions are eliminated.

The output-route self-test also opens no audio devices. It verifies conservative
classification, unknown/mixed/nondefault per-process evidence and reference reset.
It does not establish AEC quality or support for every physical device driver.

The argument self-test is pure. Separate subprocess tests on the known-fresh
helper confirm invalid/empty commands emit only a fatal error, with no format or
audio events. The read-only device handshake exposes protocol capabilities; legacy
helpers deserialize with protocol zero and an empty capability list.

The source-clock self-test performs 1,000 bracketed comparisons of Mach ticks
converted to nanoseconds and `CLOCK_UPTIME_RAW`. It reads clocks only, without
opening capture or output. Parent-side health/timing behavior is documented in
[Recording Health](../../documentation/features/capture/recording-health/recording-health.md).

## Read-only meeting-app hints

Protocol 4 advertises `meeting_apps` (capability tokens use underscores; the CLI
flag is `--meeting-apps`). `vt_capture::meeting_apps` first runs the established
`--list-devices` handshake and sends the new flag only when both protocol >= 4
and the exact capability are present. The two subprocesses share one three-second
deadline and 256 KiB stdout budget. Each runs in an isolated process group;
timeout, overflow and unsuccessful exit kill the group and reap the helper.
Legacy helpers return an incomplete unsupported observation without receiving a
new flag. This handshake is mandatory even for manual development probes on a
helper of unknown provenance.

`--meeting-apps` reads the Core Audio process list and `IsRunningInput` only; it
opens no stream, creates no tap/aggregate and requests no microphone or system
recording permission. At most 4,096 process objects are inspected. Active input
PIDs remain local: an optional dynamically looked-up OS responsibility function
identifies a responsible app before a public parent walk capped at 32 steps.
The helper/caller and exact VanTranscribe bundle are excluded. Missing or looping
ancestry, unknown active apps, failed/partial properties and an unstable process
list produce `complete=false`. Failure therefore cannot establish that an app
stopped using its microphone. Unknown app IDs and guessed bundle prefixes are
never reported as recognized apps.

The one canonical catalog lives in
`crates/vt-capture/resources/meeting-apps.def`, included by C and Rust. It lists
release Zoom, current/classic Teams, Slack, Webex/Webex Meetings, Chrome, Safari,
Edge, Firefox and Brave bundle IDs. Swift returns canonical `id`, `name` and
`kind` (`meeting` or `browser`), deduplicated and sorted. Rust independently
validates every tuple, discards arbitrary native diagnostic text, and exposes
`MeetingAppSnapshot { complete, apps, error }` plus `known_meeting_apps()`.
The snapshot contains no raw PID, executable path, window/tab title or history.
A browser using its microphone is only an advisory hint, not proof of a meeting.
Responsibility attribution is optional platform SPI and is confined to this
read-only hint; capture exclusions and raw audio remain unchanged.

After checking the safe handshake, `--test-meeting-apps` runs pure synthetic
catalog/ancestry/responsibility/loop/self-exclusion/completeness tests without
reading actual process activity. `--test-arguments` rejects mixed meeting probes
before device or permission work. Rust fake-helper tests cover legacy safety,
malformed/spoofed results, privacy normalization, shared bounds and child cleanup.
A safe-handshake-first host probe outside the sandbox read five input devices and
returned a complete snapshot with zero known active apps and no snapshot error.
No permission request, capture or output ran; this does not establish real-call
detection accuracy or lifecycle gates. Sandboxed inventory had reported zero
devices and was not treated as host availability evidence.
