# Put Native Capture Behind the Approved Swift Boundary

- Date: 2026-11-06
- Feature: macos-capture
- Related code: `native/macos/CaptureHelper.swift`, `native/macos/AudioRing.h`, `native/macos/AudioRing.c`, `native/macos/AudioRingTests.c`, `crates/vt-capture/src/lib.rs`
- State: Working draft for this unmerged implementation change

## Context

The approved architecture uses a Rust desktop core while allowing capture to move to a Swift helper at the platform boundary if direct Core Audio bindings are impractical (design risk R2). The existing adversarial reviews and owner decisions remain authoritative. The next implementation step needed a real native capture path connected to the recorder without putting database, encoding or transport work inside a real-time callback.

## Decisions

- Use the sanctioned Swift helper boundary for Core Audio ownership. Preserve the Rust core and Tauri architecture; the helper is a platform adapter, not a separate application architecture.
- Capture the selected microphone and private stereo process tap through one aggregate clock, with tap drift compensation and explicit stream-format/channel checks.
- Put callback storage in a small C11 SPSC implementation with preallocated/prefaulted capacity, lock-free atomics and explicit overflow ranges. Encode JSON only on the worker/control side.
- Send bounded NDJSON events through a local child-process pipe. Preserve observed native hardware times and epoch-local frame indexes independently of the session's normalized recorder timeline.
- Make stop a request followed by event draining. Retain callback storage until process exit if safe callback detachment cannot be established, rather than risking use-after-free during a device failure.
- Refuse implicit opening of a Bluetooth default microphone. Continue microphone-only with a nonfatal error when system tap creation fails.
- Use public parent-process traversal for app/child process-object exclusions and refresh it during capture. Retain the playback prohibition and explicitly leave launchd-parented WebKit attribution/chirp verification open.
- Mark native runtime gates as pending. Compiling and enumerating devices do not prove actual capture, self-exclusion, drift or end-to-end crash durability.
- Treat observer notifications as requests to compare values. Ignore unchanged fingerprints and require a material snapshot to remain stable for 75 ms before rebuilding, while confirmed device death bypasses the window. Include input/output/aggregate formats and exclusion IDs, ignore volatile stream IDs/reserved bytes and sub-millihertz numerical noise, and poll periodically to catch missed notifications. This avoids turning observer bursts into recording gaps while preserving genuine route/format changes.

- Add anonymous `output_route` events independently of capture rebuilds. Require positive speaker/data-source/disconnected-jack evidence and output-scoped device equality for every active included process, plus a live global tap reference. Keep unknown/mixed/nondefault/external routes ineligible instead of assuming the meeting app follows the default device. Poll every 250 ms and supplement with native listeners; preserve the allocation-free IOProc. Missing optional driver evidence remains an explicit unsupported case.

- Require explicit capture mode and validate the complete command line before any permission/audio setup. Unknown probes must fail, rather than fall through to capture. Advertise protocol/capabilities through the already-established read-only device-list command so callers can safely distinguish old helpers before using a new flag.

- Advertise protocol-3 raw-uptime source-clock capability and announce the helper startup Mach origin before permission waits. Parent note/duration timing shares that OS epoch; raw hardware timestamps remain untouched and older helpers keep explicit fallback provenance.
- Bound the existing read-only device-discovery child with nonblocking reads, a three-second deadline and 256 KiB stdout. This protects GUI responsiveness against hung discovery without claiming it caused the earlier isolated frontend timeout.

- Extend the existing safe handshake to protocol 4 with `meeting_apps`. Require both version and exact capability before the new flag; share a three-second/256-KiB budget across both subprocesses, and kill/reap failed bounded probes including retained descendant pipes.
- Keep meeting observation read-only: inspect active Core Audio input processes, prefer optional dynamically resolved responsibility attribution, then use a 32-step cycle-checked public parent fallback. Confine the optional SPI to hints; capture exclusions remain unchanged. Failed/unknown/partial/unstable evidence is incomplete, never absence.
- Compile canonical known release app IDs/names/kinds from one C/Rust catalog. Report only normalized active known apps, exclude own app/helpers, and retain browser activity as advisory. Do not emit raw process metadata or infer unknown helpers by prefix.

## Alternatives Considered

- **Direct Rust Core Audio bindings:** still consistent with the design, but Swift reduces the initial FFI/object-ownership surface while keeping the existing sanctioned platform boundary.
- **JSON or pipe IO in the callback:** rejected because allocation and backpressure could stall capture. The ring bounds callback work while the worker handles transport.
- **Exclude all WebKit processes:** rejected because Safari may be the meeting source. The helper uses public ancestry where it can establish ownership, and documents the unresolved case.
- **Wait for helper exit before draining stdout:** rejected because pipe backpressure can block the final audio publication. `request_stop` lets the caller read through EOF first.

## Consequences

- Positive: native capture and Rust recording are connected through an explicit typed process boundary, and the callback path can be tested without opening microphones.
- Positive: two tracks, hardware positions, rebuild epochs and overflow evidence survive transport for session-level accounting.
- Negative: the helper adds Swift/C build tooling and a native executable to the app bundle.
- Negative: public process ancestry cannot establish every WebKit ownership relationship; playback restrictions and real self-capture validation remain necessary.
- Negative: device disappearance can stop capture if the selected input cannot reopen; broad lifecycle continuity is not yet proven.

## Validation

- Swift helper compiled for Apple Silicon/macOS 14.4 with warnings treated as errors.
- C ring tests passed for interleaved/planar track separation, full-ring overflow, carry-forward and final-drop accounting.
- Four Rust transport tests and Clippy passed, including bounded oversized-event reading and final-event preservation after a stop request.
- Read-only input enumeration succeeded outside the execution sandbox. No device-owner names or hardware identifiers are recorded in these artifacts.
- Short packaged-app checks established matching Mic/System track durations, microphone transcription and recognition of an external audio fixture on System. No fixture transcript or personal device metadata is recorded here.
- The external-playback check showed repeated epochs without an intentional device switch. The previous observers rebuilt unconditionally, so the implementation now compares/stabilizes capture fingerprints. The no-audio `--test-fingerprints` self-test passes; the native playback scenario must be rerun to verify the observed churn is eliminated.
- Full chirp/self-exclusion, long-call drift, HFP/lifecycle and capture-to-durable gates remain pending. M0 is not complete.

- Output-route policy tests cover speaker proof, connected/unknown jack, missing source/terminal/process evidence, all external transport classes, same/default versus mixed/multiple/nondefault per-process devices, idle processes, no active output, reference reset and local process-route change detection. Rust tests cover serialization and independent rejection when any required fact is absent. These tests open no audio streams and make no acoustic-quality claim.

- Strict CLI validation passed the pure argument self-test and six subprocess rejection cases on a freshly compiled helper: fatal error only, no format/audio events. All three native no-audio self-tests pass, as does the known-safe protocol-2 device-list handshake. No capture or playback was started for these checks.

- Protocol-3 transport, clock identity fallback and hung/flooded fake inventory tests pass. The known-fresh native helper compiles with warnings-as-errors and passes all four no-audio self-tests, including 1,000 Mach/raw-uptime brackets. No new hardware capture or output was started for this increment.

- Protocol-4 native build and all five no-audio self-tests pass, including synthetic responsibility priority, ancestor fallback, loops, unknown parents/prefixes, exclusions and deduplication. Twelve Rust tests pass, including legacy accidental-capture prevention, exact tuple validation/privacy, combined deadline/byte caps and kill/reap behavior. A fresh-helper handshake followed by authorized read-only host inventory outside the sandbox reported five input devices, a complete snapshot, zero known active apps and no snapshot error; only counts were logged. Sandboxed zero-device inventory was not treated as host availability evidence. This is not real-call detection accuracy evidence; no audio capture/output or permission request ran.
