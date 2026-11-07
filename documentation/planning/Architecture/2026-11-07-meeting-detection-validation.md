# Opt-in meeting detection and local recording reminders

Date: 2026-11-07. Continues the accepted full design and [refined speaker-context checkpoint](2026-11-07-refined-speaker-context-validation.md). The existing independent Anthropic and Codex design reviews remain accepted; no preimplementation approval loop is repeated. The full M0–M5 scope remains active.

## Intended behavior and boundaries

Meeting detection is off until enabled. A read-only macOS Core Audio process snapshot identifies active microphone use by supported apps. Helper processes resolve to a responsible app or a bounded parent chain before matching the shared exact bundle-ID catalog. Unknown ownership and failed property reads remain explicit uncertainty. The application stores no PID/path or process-use log. Local notifications may be retained by the operating system under its notification settings; canonical source metadata belongs only to actual detected recordings. The source snapshot never opens a microphone, tap or output device.

Native meeting apps receive a meeting reminder; browsers receive a microphone-use reminder because microphone use alone does not establish a meeting. Prompt recording goes through the ordinary participant acknowledgment and input selection. Per-app suppression persists. An explicit AlwaysRecord rule additionally requires a participant-notice attestation and saved input choice, and uses the normal capture guards. The recording source app is retained with initial run provenance only when a fresh detector episode actually initiates that recording.

In-person context, open setup/settings, active capture/processing and shutdown block automatic starts. UI startup/reload remains blocked until the frontend supplies its context. A source app releasing the microphone for more than sixty seconds can produce a nonblocking stop reminder for its associated current recording; uncertainty breaks that timer. Nothing stops recording unattended.

Notifications are local and contain only canonical app/reminder text. Notification clicks focus the existing window; recording and stopping remain governed by their own product actions. OS notification settings may suppress delivery; the in-app reminder is also available. The official stable Tauri 2 notification plugin is pinned to 2.8.0 (MSRV 1.90, within the project floor), with no new frontend notification capability grant.

## Verification status

- Native/transport tests: 12 Rust cases pass, including legacy/unadvertised helpers receiving only the established read-only command, malformed/unknown/name-kind spoof rejection, the shared three-second/256 KiB budget and child/pipe bounds. Strict Clippy and native warnings-as-errors build pass. Five pure native selftests and four incompatible/unknown CLI rejection cases pass.
- A fresh protocol-4 helper passed its advertised capability handshake and then a read-only host probe outside the tool sandbox: five input devices, complete snapshot, zero known active apps and no error. No identifiers/names were logged. The earlier sandbox zero-device result is not used as host-availability proof. No permission, capture or output API was invoked.
- Browser final gate: 61 cases passed, including all 43 previous flows plus consent/input/attestation/rule/opt-out, stale episode/snapshot/context, in-person, manual-ended-reminder and both queued old-page authority regressions. Svelte reports zero errors/warnings; the production frontend builds. Six new screenshots at 1280/860 widths were inspected; the existing personal app design fits without horizontal overflow.
- Desktop/all-target compile passes with the stable notification plugin and all five async detector IPC commands. The original 19 focused backend cases and the actual continuous-clock bridge test pass; strict session Clippy passes. The final locked workspace/all-target/all-feature gate passes 334 Rust tests across 22 test binaries with zero failures and 20 intentional opt-in/child cases ignored; formatting and strict all-target Clippy pass. The final Opus implementation review and both focused authority reviews find no blockers. After those gates the idle installed app was gracefully replaced. Its embedded frontend and real detector-context IPC connected, and the updated app remains running. The previous bundle and a pre-update database copy are retained in `/private/tmp/vantranscribe-reviewed-update-b6y__uvr`. Post-install metadata verification confirms zero meetings, no capture/processing, Standard English still selected and meeting detection off.

The signed release passes deep strict code-signature verification, contains 49,327,558 bytes (47.04 MiB), and opens its embedded frontend plus real detector-context IPC in 3.012 seconds using isolated empty data. It creates zero meetings and verifies detection remains off. This single possibly warm-cache startup is not a sustained performance or OS-notification-delivery claim.

No actual private microphone/output or enabled installed detection policy is used in these checks.

Primary API references: [Apple Core Audio input activity](https://developer.apple.com/documentation/coreaudio/kaudioprocesspropertyisrunninginput), [Chromium responsible-process usage](https://raw.githubusercontent.com/chromium/chromium/main/base/process/process_info_mac.mm), [Tauri notification plugin](https://v2.tauri.app/plugin/notification/). The local SDK and pinned package source are implementation evidence; notification preparation is not proof of OS delivery.

## Queued window requests and settings authority

The first full implementation review found no blockers. Root then closed an additional queued-window race: `spawn_blocking` can outlive its WebView, so a previously issued context setter must not release the new page's setup guard. Each page pins its first authoritative `context_generation`; reload increments the server generation under the runtime/controller lock pair, and the production context setter rejects mismatches before memory/SQLite changes. A later snapshot cannot grant the old page a new generation. The focused Opus re-review confirmed this fence and found no regression.

The same fence protects policy saves. Without it, a delayed old AlwaysRecord save could overwrite a fresh page's opt-out/suppression after the fresh page is ready; clearing arming alone would still permit that old policy on a future idle edge. The guarded config setter rejects that save before mutation. Backend threaded and browser regressions preserve byte-identical opt-out/suppression and prove no future automatic candidate/capture. Both production setters require a generation; unguarded fixture helpers are test-only. The focused detection suite passes all 21 cases. A separate final Opus review confirms the policy-save counterexample is closed with no regression. The original implementation review and both focused generation reviews are complete; they do not require repeating the accepted design review.

Prior nonblocking review notes remain explicit: recording start stops saved playback as it does for manual starts; previously enabled observation can run before UI readiness while actions remain blocked; a one-shot OS failure notice may be dropped during setup while the in-app error survives; poisoned-lock handling is not uniformly degraded; budget-consuming helper failures have bounded but no adaptive retry backoff. These do not establish real-call, battery or OS-delivery acceptance.

## Suspend-aware freshness

[The Rust Apple clock implementation](https://doc.rust-lang.org/src/std/sys/time/unix.rs.html#55-67) confirms that `Instant` uses raw uptime, which excludes sleep. Detection uses a separate `ResourceProbe::observation_ns` clock backed by macOS `CLOCK_MONOTONIC_RAW`, including sleep. Source/capture timing and existing maintenance ticks remain unchanged. Missing continuous time cannot authorize capture. A probe is dated before its native read and rejected when its continuous-clock duration exceeds the freshness budget, so a result read across suspend cannot renew stale authorization merely on receipt. Injected clocks verify stale/absent/suspend-crossing observations without suspending the computer or opening audio.

## Final test timing correction

The final broad run exposed a fixture timing assumption: its 100 ms deadline could expire before Python wrote its PID marker under concurrent compilation, so a cleanup assertion failed reading an absent marker. The descendant-pipe fixture now uses the same three-second allowance as the production read-only budget, retaining the timeout and process-reaping assertions. Production code and timeout are unchanged. The focused regression and final full gate pass; the original failed full-run log is retained. The next sandboxed run passes the transport case but cannot bind the eight summary fixtures’ ephemeral loopback servers (`127.0.0.1:0`, permission denied). The final gate therefore runs outside that restriction, offline, using only local mock providers. These fixture ports are unrelated to the embedded packaged frontend.

## Remaining scope

This increment does not establish the physical meeting-app/OS/browser permission matrix, unattended long-call quality, Windows capture/session detection, encrypted speaker memory, gallery calibration or complete M0–M5 acceptance. The broader accepted roadmap remains unchanged.

The next unfinished M4 implementation continues in the [consented voice-memory checkpoint](2026-11-07-consented-voice-memory-validation.md).
