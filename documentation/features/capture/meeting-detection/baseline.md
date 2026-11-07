# Advisory Meeting Detection Baseline

## Flow invariants

- Detection defaults off and disabled polling never launches the helper. Only canonical supported app IDs can authorize rules.
- Native app activity is advisory; browser microphone use is never called a confirmed meeting. No participant identity is inferred.
- In-person mode, unconfirmed/reloaded UI context and setup block automation. Setup preserves a fresh explicit Prompt handoff and never supplies participant consent itself.
- Every production context setter and detection-settings save checks the WebView-pinned ephemeral generation under Runtime → Detection locks before mutation/persistence. Reload advances it; stale queued commands cannot unblock either reload or a fresh setup dialog, or overwrite the fresh page’s opt-out/suppression policy with automatic authority. Generations are never persisted.
- Prompt starts require normal explicit consent and fresh current episode/policy/mode validation under the same runtime reservation as raw recording creation.
- Always record requires explicit per-app participant-notice attestation plus saved microphone choice. Fresh complete idle absence arms a future activity edge. Unknown/stale evidence clears arming.
- Busy/setup/recording activity consumes automatic eligibility. Continuous or resumed activity during a prior recording does not auto-restart after Stop. A new short confirmed idle edge can qualify again.
- Observation freshness uses a suspend-aware clock distinct from capture ticks. Timestamp native probes before work, expire receipts/preflight after suspension, and reject/clear authority on unavailable clocks without fallback timestamps.
- Native input inventory occurs outside runtime locks; saved input and context/rule revision are rechecked before creation. Default Bluetooth/missing default and absent explicit device reject with zero new raw meetings; no input substitution occurs.
- Automatic success notification requires a committed nonempty chunk for that same still-active recording. Quiet PCM qualifies; clocks/elapsed time do not. Async pre-audio failure produces one generic failure notice and a manual retry prompt; expected Stop/shutdown is silent.
- Only actual detected starts bind source app provenance and reminders. More than sixty seconds of fresh confirmed source-app absence can suggest Stop; detection never stops recording.
- Notification sinks run outside locks, are deduplicated and cannot authorize capture. OS delivery is best effort; notices remain in-app.

## Defaults and bounds

| Setting | Baseline |
|---|---|
| Global / per-app default | Off / Prompt |
| Poll / freshness | Two / six monotonic seconds |
| End reminder / dismissed Prompt episode debounce | Strictly more than sixty confirmed seconds |
| Always future activity | Complete idle absence → active edge; no sixty-second minimum |
| Native probe | Three seconds, 256 KiB, safe capability handshake first |
| Runtime state | Canonical-app-bounded episodes plus one active source association |

## Persistence and side effects

1. Load only settings/mode; malformed values disable detection until explicit repair.
2. Register fresh backend UI mode/setup context, then read bounded native snapshots only while enabled/Remote.
3. Apply only complete fresh observations; unknown/error/stale breaks absence and unattended authority.
4. Validate a detected action and runtime reservation; persist canonical source and trigger with the original run before starting normal capture.
5. Continue raw capture regardless of source-app disappearance. Show an advisory source-bound reminder after the threshold; wait for explicit Stop.

`app_settings` is the only detection-settings persistence. No PID/activity/event history is stored. Disabled save never needs discovery and disarms incomplete Always drafts; setup-only changes with valid unchanged mode are RAM-only. Observations/episodes/authority are cleared on restart/reload. Existing run provenance records canonical detected source and trigger only for actual detected starts.

## Acceptance limits

Synthetic/injected tests do not establish real Zoom/Teams/browser call detection accuracy, every helper-parent mapping, actual OS notification delivery or all hardware permission/input-route behavior. The full roadmap and hardware/manual acceptance gates remain open.
