# Advisory Meeting Detection Decision Trail

- Date: 2026-11-07
- Feature: meeting-detection
- Related code: `crates/vt-session/src/meeting_detection.rs`, `meeting_detection_tests.rs`, `lib.rs`; native transport in `vt-capture`
- State: Working draft for this unmerged implementation

## Context

The accepted design §14 calls for opt-in advisory application detection, browser-specific wording, optional explicitly authorized per-app automatic recording and a non-blocking end reminder. It forbids unattended automatic stop. Existing recording consent, source preservation, maintenance/model reservations and native input safeguards must remain authoritative.

## Decisions

- Use complete bounded read-only native observations with two-second polling fallback. Observe with macOS suspend-aware monotonic time, timestamp before the probe and bind input-preflight age separately; missing clocks fail closed without borrowing the capture epoch. Treat unknown/stale/errors as uncertainty, rather than absent activity.
- Keep one in-memory episode per canonical app. Persist policy and selected mode only in existing app settings; do not retain process activity or PIDs.
- Make capture mode/setup readiness backend-owned and reset it on WebView reload. Keep fresh explicit Prompt handoff usable while setup blocks unattended starts.
- Require separate participant-notice attestation and saved input for Always record. Fresh idle absence arms a future active edge; capture/setup/processing consumes observed activity rather than queuing restarts after Stop. Keep the sixty-second end-reminder delay separate from short new-call edges.
- Perform input discovery outside locks, bind the checked input/context revision, and revalidate authority before raw creation. Keep native input/Bluetooth checks as a second boundary.
- Commit canonical detected source, trigger and automatic attestation with initial run provenance. Never guess a source app for generic manual recordings.
- Bind end reminders only to the detected recording's source. Notifications focus the app only and never record/stop. Defer success notice until committed audio; handle async pre-audio failure once and silently accept expected manual Stop.
- Make global opt-out succeed without discovery, disarming incomplete automatic drafts. Repair corrupt optional settings only through explicit settings/mode confirmation.

## Alternatives considered

Frontend-only selected mode could let stale Remote policy bypass in-person intent. Callback-arrival/wall-clock release timing could infer false endings after errors or waits. Treating every browser capture as a meeting would overclaim evidence. Recording immediately for already-observed activity after Settings closes could surprise the user. A sixty-second automatic-recording cooldown would miss short gaps between calls. The chosen state machine uses backend context, monotonic fresh evidence, explicit policy and new observed idle edges instead.

## Consequences and verification

Synthetic fixtures exercise the actual AppService authorization, recording and FLAC decoder without opening devices. Tests cover monotonic/freshness/absence boundaries, suppression/restart, consent/setup/mode/work reservations, concurrent preflight changes, exact raw audio/provenance, future-edge policy, async failures and notification dedupe. Fake helper polling and deadline tests cover disabled no-probe behavior and bounded shutdown. Real application coverage and notification-center delivery remain manual acceptance work; no new calibrated detection or hardware claim is made.

A focused reload remedy adds an ephemeral backend context generation. Each page pins its first snapshot generation, reload advances it, and the guarded context setter rejects old queued IPC before state/persistence mutation. The regression releases an old queued setter after a fresh page opens Settings and verifies that it cannot reopen automatic authority; current-generation confirmation still works. No observation, policy, timer or persistence semantics otherwise change.

The same page-generation fence also covers detection-settings saves. A delayed old-page Always-record save cannot overwrite a fresh page's global opt-out or Suppress rule and arm a future activity edge. The regression verifies unchanged persisted settings and in-memory authority after the stale save, while current-generation save remains usable.
