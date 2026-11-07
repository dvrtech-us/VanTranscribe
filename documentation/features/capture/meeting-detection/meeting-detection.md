# Advisory Meeting Detection

Meeting detection is an explicit opt-in convenience. Native app microphone activity can suggest a recording; browser microphone use is presented only as microphone use. Detection never decides that a person is in a meeting and never stops a recording. Global detection defaults off, and the disabled worker does not launch the helper.

## User flow and behavior

1. Enable meeting detection in Settings and choose rules for canonical supported applications. Prompt is the default policy, Suppress persists an app-specific opt-out, and Always record requires a separate per-app participant-notice attestation and saved input choice.
2. The desktop registers its current capture mode and setup-dialog context. Startup/reload begins blocked until this explicit handshake. Snapshot exposes an ephemeral `context_generation`; each WebView pins its first authoritative generation and sends it with context changes. Reload increments the generation, and queued old-page context commands and settings saves are rejected before any state or settings mutation, including after the fresh page has opened Settings or saved a new opt-out/suppression policy. An old page cannot replace that fresh policy with Always record authority. In-person mode never displays detection prompts or starts a detected recording.
3. A bounded read-only helper observation is polled every two seconds. Complete, recent observations create ephemeral app activity episodes. A native meeting app produces “Meeting detected in …”; a browser produces “… is using the microphone. Record?” Unknown, incomplete, invalid or failed probes are uncertainty, never confirmed absence.
4. Prompt recording opens the ordinary consent/input flow. Its explicit start action validates the still-fresh episode, enabled policy, backend mode, consent, and idle runtime immediately before creating the recording. Opening setup preserves a current fresh prompt for consent submission but blocks unattended starts.
5. Always record acts on a future observed idle-to-active edge: fresh confirmed absence while idle arms the app, and the next fresh active observation consumes that authority. It does not arm an app already using the microphone when detection/rules/setup are changed or the UI reconnects. A short confirmed idle gap can qualify a new call; the sixty-second reminder delay is not an automatic-recording cooldown.
6. Activity observed while recording, paused, processing, performing maintenance/model setup or interacting with setup consumes automatic eligibility. Simultaneous apps and a source app resuming during recording do not queue another unattended recording after Stop. A later fresh idle-to-active edge can qualify again. Manual prompts remain available where appropriate.
7. Only a recording actually started through a detected action is associated with its canonical source app. More than sixty seconds of fresh continuously confirmed source-app absence can display “Meeting may have ended. … Stop.” Recording continues until explicit user action. Unknown/stale observations reset the release timer; dismissing the reminder never stops capture.

Always-record preflight performs bounded read-only input inventory outside runtime/detection locks. A default input must exist and be non-Bluetooth; an explicit saved device must still exist. There is no substitute input fallback. The native capture helper independently rechecks its existing input/Bluetooth policy. The checked input and ephemeral rule/context revision are revalidated under the same runtime reservation as episode/policy/mode authorization, so a concurrent rule, dismissal or context change cancels the attempt before raw recording creation.

An unsafe or unavailable input produces an actionable error/manual prompt without creating a recording. An asynchronous permission/helper failure before saved PCM produces a single generic failure notice, without a successful-start notice or repeated automatic retries for that episode. Expected manual Stop/shutdown before audio is silent. A successful automatic-start notification waits for a committed nonempty raw audio chunk for that same still-active recording; a clock/permission status or elapsed duration alone is insufficient, and silent PCM qualifies.

Freshness and absence use the dedicated suspend-aware observation clock (`CLOCK_MONOTONIC_RAW` on macOS), rather than the capture clock or app `Instant`. The native probe timestamp is taken before its read-only work, and a late receipt after sleep cannot refresh old evidence. Input preflight carries its own checked timestamp as well. Unknown observation clocks disable starts and clear unattended arming; the implementation never substitutes capture ticks or a sentinel timestamp. The raw hardware timeline and existing capture/disk cadence are unchanged. See [Rust's macOS `Instant` clock implementation](https://doc.rust-lang.org/src/std/sys/time/unix.rs.html) for why the raw-uptime clock used by `Instant` is insufficient across sleep.

## State and limits

| Contract | Value |
|---|---|
| Global default | Disabled |
| Per-app default | Prompt; Always record requires separate explicit authority |
| Poll fallback | Two monotonic seconds between starts, with bounded helper work |
| Complete observation freshness | At most six monotonic seconds |
| End reminder | Strictly more than sixty seconds continuously confirmed absence |
| Dismissed Prompt episode | Deduplicated until more than sixty seconds confirmed absence |
| Always record rearming | Fresh complete absence → active edge while idle; no sixty-second minimum |
| State bound | One episode per canonical catalog app, one recording association |
| Child protocol | Safe existing `--list-devices` handshake; protocol 4 `meeting_apps` capability before `--meeting-apps` |
| Native probe bound | Combined three-second deadline and 256 KiB output cap |

No PID, process activity history, inferred participants or event log is persisted. Episode IDs, observations, timers, notification dedupe and authorization revisions live only in memory. Unknown activity can make detection conservative; polling can miss very short changes. This feature does not establish call quality, app-specific end accuracy or hardware permission reliability.

## Key types and entry points

| Type/function | Source | Responsibility |
|---|---|---|
| `DetectionSettings`, `DetectionRule`, `DetectionInput`, `DetectionPolicy` | `crates/vt-session/src/meeting_detection.rs` | Validated opt-in policies and explicit input/notice authority |
| `DetectionState` | Same module | Monotonic episodes, freshness, uncertainty, source-bound reminders and dedupe |
| `AppService::set_detection_context`, `reset_detection_context` | Same module | Backend capture mode and setup/reload authority |
| `AppService::start_detected_recording`, `start_with_source` | Detection module and `lib.rs` | Fresh action authorization and atomic initial recording provenance |
| `DetectionWorker` | Detection module | Optional read-only polling, stop signal and bounded probe join |
| `MeetingApp`, `MeetingAppSnapshot`, `meeting_apps`, `known_meeting_apps` | `crates/vt-capture/src/lib.rs` | Typed bounded native evidence and shared canonical catalog |

## Integration API

`Settings.meeting_detection` contains `enabled` and `rules`. Each rule has `app_id`, `policy` (`prompt`, `suppress`, `always_record`), `participant_notice_attested` and nullable `input`: `{kind:"default"}` or `{kind:"device",id}`. `Settings.capture_mode` is the backend-owned selection, not a frontend-only detection bypass.

`Snapshot.meeting_detection` contains `context_generation`, `enabled`, `status` (`disabled`, `watching`, `unknown`, `blocked`), optional `error`, `catalog`, `prompts` and nullable `ended`. A prompt contains `episode_id`, canonical `app` and advisory `message`; an ended reminder contains `episode_id`, `meeting_id`, `app_id` and its message. Setup may block automation while retaining a fresh manually usable prompt. Active recording blocks new starts while still permitting its correctly bound ended reminder.

The IPC surface is `update_meeting_detection({settings,context_generation})`, `set_detection_context({mode,setup_open,context_generation})`, `start_detected_recording({episode_id,title,consent,input_device})`, `dismiss_detection({episode_id,suppress})` and `dismiss_detection_ended({episode_id})`. Explicit detected start reuses normal recording consent and capture safeguards. Detection start also rejects model-setup/read-lease/maintenance reservations. Detection never calls Stop itself.

Desktop lifecycle installs `set_detection_notifier` before `start_meeting_detection`, resets interaction context on WebView reload and signals/joins the worker at shutdown. Notification callbacks run outside both locks, are deduplicated and cannot start/stop capture. Native OS notifications are best effort and subject to system notification settings; clicking one only focuses the existing app. In-app notices remain available if OS delivery is blocked. Test sinks never deliver real OS notifications.

## Persistence and recovery

Existing `app_settings` holds only the `meeting_detection` JSON setting and selected `capture_mode`. No additional schema/SQL artifact is required. Global-off save succeeds without device discovery and normalizes incomplete Always-record drafts to Prompt with no attestation/input authority; unknown/duplicate app IDs are discarded while disabling. Enabling rejects unarmed Always-record rules. A valid disconnected saved UID can remain stored disabled; actual automatic startup always revalidates its availability.

Malformed persisted detection/mode settings load as disabled with an explicit warning. Explicit valid settings save repairs the configuration atomically; explicit mode confirmation can repair an invalid mode. Unchanged setup-only context updates stay in memory and do not write SQLite during capture. No prior corrupt auto policy is silently reactivated.

Detected starts atomically add canonical `detected_source_app` and `recording_trigger` (`detected_prompt` or `always_record`) to the initial run engine JSON. Always-record provenance includes the validated participant-notice attestation boolean. This records why that meeting began, not process activity history, participant identity or an invented consent timestamp. Generic manual recording has no inferred source app. Existing [durable recording](../../recording/durable-recording/durable-recording.md), [source/health protection](../recording-health/recording-health.md) and [desktop session](../../app/desktop-session/desktop-session.md) contracts remain in force.

## Verification

Tests use injected monotonic time, temporary SQLite/FLAC, canonical synthetic observations, fake protocol helpers and injected notification sinks. They cover freshness, unknown/stale release interruption, exact sixty-second boundaries, browser wording, dismissal/suppression/restart, in-person and every runtime work gate, short new idle edges, simultaneous/resumed apps without queued restarts, setup handoff, opt-out normalization, bad-mode repair and unchanged-mode RAM updates. End-to-end fake capture decodes exact generated PCM with manifest verification and checks atomic source/trigger provenance. Input-preflight race tests keep snapshots responsive while policy/dismissal/context changes cancel authority. Separate injected observation/capture clocks cover suspension jumps, unavailable clocks and probes/input checks spanning a simulated sleep while capture ticks remain unchanged. No test uses OS audio, private recordings, network or real notifications. Hardware/manual app and notification-delivery acceptance remains open.
