# Desktop Session

VanTranscribe now contains a Tauri 2 desktop application joining native macOS capture, durable recording, local English streaming recognition, offline Parakeet refinement, versioned transcripts, grounded AI notes, Rust-core saved playback, optional incremental live VAD and a Svelte interface. Independent experimental speaker inference and meeting-local annotation/reconciliation storage are also implemented, with desktop integration and calibrated quality verification continuing. The implementation is undergoing runtime verification. A successful build is not proof of the capture, accuracy, drift or long-session gates in `design.md`; the full app objective and roadmap remain open.

## User flow

1. Build and open the app using the [root instructions](../../../../README.md). The current desktop target is Apple Silicon on macOS 14.4 or newer, with Rust 1.91 or newer and Apple's command-line build tools.
2. In Settings, optionally download the English live model (Standard: about 507 MB archive and 190 MB installed; Compact: about 310 MB archive and 72.5 MB installed) and the Parakeet v3 refinement pack (about 465 MiB download, 640 MiB installed). Both show progress, cancellation and setup errors. Recording audio does not require either model. Model source/license attribution is retained.
3. Choose **New recording**, enter a title and select **Remote meeting** or **In person**. Remote mode requests microphone and system audio; in-person mode requests only the microphone.
4. Select the recording microphone. The interface remembers an explicit choice locally. If the system default is Bluetooth, the user must select that input explicitly or choose another microphone; the helper does not silently open the Bluetooth default.
5. Acknowledge that participants know the conversation will be recorded. This check is required for every recording. It is a recording-awareness acknowledgment, not biometric enrollment or a legal-consent verification service.
6. On first use, grant macOS Microphone and System Audio Recording access when requested. The default process-tap backend does not require Screen Recording permission. If access is missing, open System Settings → Privacy & Security, enable the relevant VanTranscribe microphone/audio entry and retry. Labels vary between macOS releases. Silence alone is not reported as proof of denial.
7. The live view shows elapsed time, input levels, transcript text, notes, bookmarks and capture warnings. With a model installed at recording start, recognition runs on a separate worker. Remote fallback labels are channel labels: microphone **Me**, system **Them**. In-person speech without speaker evidence stays **Unknown**, including after replay or refinement. Experimental independent speaker inference and temporal annotation are separate from STT boundaries, remain uncalibrated/provisional and must not be presented as confirmed human identity.
8. Pause/resume creates a timeline discontinuity instead of joining speech across the pause. Stop requests helper shutdown, drains emitted events through EOF, flushes the track buffers and final recognition results, and saves the meeting. The interface may display `processing` while workers finish.
9. On normal stop the live run is sealed. With the pack installed and automatic refinement enabled, a new offline pass runs against saved audio. Otherwise a configured automatic AI-notes provider can summarize the completed live transcript. Defaults enable the automation preferences but leave the summary provider Off; setup never silently downloads a model or selects a provider.
10. The library lists stored meetings newest first and searches titles, active effective transcript text, notes and fresh summaries through backend FTS. Search is debounced by 250 ms. A saved meeting without text can use live-model replay; failed/recovered/errored transcripts offer retry. **Refine transcript** can process a healthy meeting with the Parakeet pack. Stage/progress and cancellation remain visible while the published transcript stays available.
11. Select a transcript version to inspect history. Historical text is read only. On the current stopped transcript, **Edit text** saves a correction overlay while retaining the original segment. Corrections that no longer uniquely match after refinement stay in a review panel until attached to a selected current segment or explicitly dismissed.
12. The Summary tab is always present. Configure Apple on-device, a numeric-loopback local server, or an explicitly consented external provider in Settings. Then generate/regenerate AI notes for a completed current transcript. Notes expose source links that load the exact historical run/segment; stale and partial outputs are marked and processing gaps remain visible. Provider errors preserve previous notes and transcript status/warnings, and appear separately as `summary_error`. Publishing a replacement transcript clears an older summary failure.
13. Export the current effective transcript as Markdown, TXT, JSON, SRT or WebVTT. Files are written under `<data-root>/Exports/`, and the interface reports the path. Export is disabled while viewing a historical transcript because this command exports the current run. The Audio tab provides Rust-core native playback, separate track waveforms, seek and per-track gain/mute; transcript timestamps also seek/play. There is no WebView audio. Loading does not start sound, and any active/paused/stopping recording blocks playback.

## Technical flow

### Application and interface

`src-tauri/src/main.rs` creates one `Arc<AppService>`, resolves the bundled `vt-capture-helper` and chooses the application data root. The default is `~/Library/Application Support/com.dvrtech.vantranscribe/`. `VANTRANSCRIBE_DATA_DIR` can select an isolated root for diagnostics. `AppService::new` first acquires an OS lock for the canonical data root, then initializes the recording and desktop tables, attempts recovery of interrupted desktop sessions and checks both model installations and loads non-secret processing preferences. Legacy transcript migration and search publication happen transactionally before recovery. This prevents a second app instance from modifying in-progress work. Packaged builds enable Tauri custom-protocol assets; `frontend_ready` confirms interface initialization after device enumeration and the initial snapshot. Local builds use an ad-hoc macOS signature.

The frontend is built from `ui/` using local Svelte 5, TypeScript and Bootstrap 5 dependencies. `AudioPanel.svelte` and `AudioSettings.svelte` consume native audio commands and model/preferences state. `ui/src/lib/api.ts` defines command payloads. `ui/src/App.svelte` polls snapshots and the selected meeting: 500 ms during recording, processing or either model download, and 2 seconds while idle, scheduling the next poll after the previous finishes. A browser without Tauri displays a connection message; there are no seeded meetings or simulated recording data. Theme and selected input UID live in WebView local storage; meeting content stays in the Rust store.

Keyboard shortcuts are `⌥⌘R` for recording setup/current recording, `⌥⌘B` for a bookmark and `⌘F` for library search. Library queries call `search_meetings`; the UI does not substitute a title-only filter. The consent dialog traps focus. Transcript auto-follow pauses when the user scrolls upward and can be resumed explicitly. Light, dark and system appearance are supported.

### Opt-in meeting reminders and explicit recording rules

[Meeting detection](../../capture/meeting-detection/meeting-detection.md) adds `DetectionNotice.svelte` and `MeetingDetectionSettings.svelte`. `Snapshot.meeting_detection` exposes cached app reminders, uncertainty and the associated recording's ended reminder; `Settings.meeting_detection` stores the opt-in toggle and per-app rules. `Settings.capture_mode` is backend-owned and persists the last confirmed capture context. Native process observations run only after detection is enabled, never from a browser-side audio API.

The frontend synchronizes its mode/setup context before permitting detector actions. Startup and WebView reload reset that context to blocked. Each page pins its first snapshot generation; the service rejects older generations under the runtime reservation, so queued context commands or settings saves from a destroyed page cannot restore old context or re-enable an older recording policy. Prompt recording uses the ordinary participant acknowledgment/device form and a fresh detector episode. AlwaysRecord needs a separate per-app attestation and explicit saved input, and applies only to future verified idle activity edges. Manual Stop does not queue another automatic recording for ongoing microphone use. In-person mode, open setup/settings, active work and shutdown block unattended starts. Only a detected recording's associated source can produce a more-than-sixty-second stop reminder; stopping remains explicit.

The desktop registers the pinned Tauri notification plugin and an injected service notifier before starting the detector worker. Its callbacks run outside controller locks; notification clicks only focus the existing window. OS delivery settings can suppress the alert, and the in-app notice remains available. Ordinary launch with detection disabled requests no notification/capture permission and starts no native detection probe. `frontend_ready` and the first successful context IPC emit static readiness markers without app activity data.

| Integration entry | Responsibility |
| --- | --- |
| `AppService::start_meeting_detection`, `reset_detection_context`, `set_detection_notifier` | Bounded worker lifecycle, reload guard and local notice sink |
| `update_meeting_detection(settings, context_generation)` | Validate/persist explicit toggle and per-app policies |
| `set_detection_context(mode, setup_open, context_generation)` | Confirm UI mode and block unattended actions during setup |
| `start_detected_recording(episode_id, title, consent, input_device)` | Fresh-token, ordinary consent/device/runtime gate and atomic source provenance |
| `dismiss_detection(episode_id, suppress)` | Dismiss this activity or persist per-app suppression |
| `dismiss_detection_ended(episode_id)` | Keep recording and dismiss its stop reminder |

These async Tauri commands offload blocking service work. Settings use the existing `app_settings` keys `meeting_detection` and `capture_mode`; the detector adds no usage-history table. Live run provenance contains a canonical source app only when that detector episode actually initiates recording. This module is a macOS increment; Windows detection and real meeting-app accuracy/lifecycle acceptance remain separate.

### Native capture and durable publication

`src-tauri/build.rs` compiles `native/macos/AudioRing.c` and `CaptureHelper.swift`, then packages the helper as an app resource. Swift owns Core Audio objects on its control thread. Remote capture creates a private aggregate device containing the selected microphone and a stereo process tap, with the microphone as the clock source and tap drift compensation enabled. The tap is unmuted. Process exclusions use audio process object IDs for the parent app and its descendants; they are refreshed every two seconds without excluding unrelated WebKit processes.

The Core Audio IOProc only copies into a preallocated C SPSC ring. It performs no allocation, locks, JSON encoding or pipe I/O. Ring capacity covers at least ten seconds at the configured callback quantum. Hardware host ticks are converted into session-relative nanoseconds; dropped frame ranges accompany the next queued packet or the final stop gap. Swift drains the ring and publishes bounded newline-delimited events outside the callback. Device/format changes and sleep/wake request a rebuild, increment the source epoch and emit a gap. The helper holds a prevent-idle-sleep assertion during recording.

`vt-capture::CaptureProcess` validates mode, launches the helper and reads events with a 4 MiB line cap. `vt-session` uses a bounded 128-event capture-reader channel. `TrackBuffer` validates frame geometry and finite values, accumulates at most two seconds per track, and calls the [durable recorder](../../recording/durable-recording/durable-recording.md). Original source epoch/frame/host observations are saved separately in `app_audio_clock`, so the writer's exact adjacent timeline does not erase the measured source clock.

If process-tap creation fails, the helper warns and continues microphone-only capture. Unexpected aggregate channel layouts or incompatible stream formats stop capture rather than risk assigning audio to the wrong track. An unavailable or explicitly refused microphone stops that session. A failed recording leaves an error and allows a new recording attempt. Automatic meeting-app device following, measured drift guarantees and a complete permission/onboarding chirp test are not implemented yet.

### Recognition and stop ordering

If the model is installed when recording starts, `speech-inference` receives mono copies through a separate bounded 600-message channel. Archive audio remains at native rate and channel count. `vt-stt::StreamingRecognizer` owns independent Mic/System streams using the official `sherpa-onnx` crate, the CPU provider, two model threads, greedy decoding and stateful conversion to 16 kHz. Input gaps and rate changes finalize the old stream. Partials remain in memory and render italic/muted; final text is shown at once from the runtime view (`Runtime::live_finals`) and appended only to the active running live run in `app_run_segments` by a session-owned `live-transcript-writer` thread, in IMMEDIATE batches with incremental search rows (see [live transcription](../../transcription/live-english/live-english.md#session-live-transcript-persistence-and-recovery)). The recognizer thread never waits for the database. Partial IDs use the `partial-` prefix and never expose editing controls.

Inference queue saturation warns that live transcription fell behind while recording continues. An engine failure does not intentionally stop the audio path: the speech worker recovers in place from a per-message recognizer error (gap `recognizer_error`, track reset, engine rebuilt only for an engine error; the same cause twice in a row or 20 recoveries per recording is terminal), while the speaker worker is not restarted and sustained-backlog shedding is unchanged. `RecordingView.live_transcription` (`state`, `since_source_ms`, `reason`) and `RecordingView.capture` (`state`, `since_source_ms`, `kind`) report the truthful state; the exact values and the health report's `failures[]` and `capture` sections are in [Recording Health](../../capture/recording-health/recording-health.md). Housekeeping writes on the recording loop (duration, unforced health, source timing, status) wait at most 250 ms and are skipped on a database lock with one warning; the recorder's chunk writer is unchanged, so a lock held over 5 s across a chunk flush still ends the recording. The final echo-info save retries briefly and a lock is a warning. Pause flushes recorder buffers and resets both recognition streams while retaining the loaded ONNX model; a native gap resets only its affected stream. Incoming audio is omitted while paused, and resume writes an explicit pause gap before the next accepted audio. Durations and note timestamps use elapsed session time, including pauses.

Stop first sends `stop` to the helper while the event reader continues draining. Normal completion consumes EOF before final track-buffer publication and inference completion. The service requests a forced helper stop if the graceful drain exceeds five seconds; `CaptureProcess::stop` has its own bounded wait. Closing the app requests shutdown, signals replay cancellation at chunk boundaries and waits for recording, saved transcription and model setup for up to 35 seconds before exiting. These bounds and real device behavior remain subjects of runtime validation.

Saved transcription runs separately from live capture, reads only ready audio chunks, validates FLAC format/size and bounds decoded samples to two seconds per chunk. The service serializes recording and saved processing. Retry finals remain in `app_transcription_staging`; successful publication creates a new replay run, preserves earlier rows and atomically changes the active pointer/search index. Failure/cancellation discards unpublished staging and retains the previous transcript.

### Refinement, revisions and search

`processing::after_recording` reads persisted automation preferences. Automatic refinement requires an installed pack; successful refinement can chain into configured automatic AI notes. A single reserved processing slot prevents overlapping capture/replay/refinement/summary work. Manual actions use `refine_meeting` and `generate_summary`; snapshots expose meeting, kind, stage, progress and optional error. Cancellation is shared across saved-processing workers. Refinement/summary worker panics become visible failures with previous published data retained.

`processing::refinement_job` verifies the recording manifest, creates a run with engine/version/model/hash provenance, then reads each independent track's ready chunks in time order. Gaps reset the affected refiner track. `BatchRefiner` applies bounded Silero VAD, stateful resampling and offline Parakeet inference, returning text and acoustic word timestamps. The session stores those outputs in an unpublished run. Final publication seals the pass, moves `app_meeting_revision.active_run_id`, resolves overlays, marks summaries stale and rebuilds active search within one SQLite transaction. See [batch refinement](../../transcription/batch-refinement/batch-refinement.md) for inference bounds and provenance, and [processing runs](../../storage/processing-runs/processing-runs.md) for persistence details.

`store::initialize` safely copies legacy `app_segments` into a meeting-owned live-kind run while retaining the original archive rows, notes and audio. SQL guards reject original segment updates/deletes and new inserts into sealed passes. Current live finals remain visible during recording; completed and abandoned historical versions remain addressable. Conservative collection of unreferenced obsolete runs is implemented by the separate retention feature; protected history remains addressable.

Text corrections are overlays anchored by exact track, start/end milliseconds and SHA-256 of original text. Automatic reapplication requires a unique match in both directions; changed/ambiguous anchors remain `needs_review`. Explicit review can re-anchor a correction to a selected current segment or dismiss it. Exports and search use effective text. FTS rows contain the active transcript, title, notes and fresh active summaries; user queries are quoted literal tokens rather than raw FTS operators.

### Native saved playback and live speech detection

`audio_service` coordinates `vt_playback::Playback` under the same runtime reservation used to start capture. Playback commands lock runtime before the playback slot; recording start stops/drops output before opening capture. Snapshot releases runtime before reading playback, avoiding reversed lock ordering. Active, paused and stopping capture blocks playback across every meeting. Async Tauri wrappers keep native output setup/control and waveform scanning outside the UI thread.

Loading a player reads only validated manifest metadata. Explicit Play creates native CPAL output after bounded prefill. Native PCM24 FLAC stays at its archived rate/channels; one decoded chunk per track is cached, with a fixed 8,192-frame SPSC queue. The output callback applies current gains and mixes the independent tracks on the shared session timeline. Gaps remain silent. Pause/seek/stop drop old output/read-ahead; position follows consumed frames, including honest buffering on underrun. Source paths, SHA-256, format/frame/time bounds and root-capability resolution are verified without mutating audio or the database. See [saved playback](../../audio/saved-playback/saved-playback.md) for exact limits and physical-latency caveats.

`AudioPreferences` stores `live_vad_enabled=true` and `diarization_enabled=false` defaults in the non-secret `audio_preferences` settings row. Live VAD is used only when enabled and the verified standalone model or refinement-pack VAD is available; missing optional setup leaves ungated recognition available. `StreamingRecognizer::with_vad` owns independent track gates, preserving onset pre-roll and endpoint context while omitting sustained inference silence. Raw archive audio is never gated. Live word times come from native token alignments when valid, otherwise text remains available with empty word timing. See [live English](../../transcription/live-english/live-english.md#optional-live-silero-vad) for gate bounds and timing limits.

The standalone VAD model is an explicit 643,854-byte MIT-licensed download, available independently of the larger packs. Speaker-model setup also requires explicit acquisition-notice acknowledgment; the experimental pyannote/ERes2Net package does not enroll cross-meeting voiceprints or confirm a participant identity. `vt-diarize` uses independent per-track audio windows, clean-turn embeddings, provisional/overlap/unknown states and user temporal constraints. It does not treat a complete STT segment as a single speaker. Core public-fixture verification is separate from full desktop pipeline, overload, latency and calibrated English/channel DER acceptance.

When enabled with an installed pack, `speaker_pipeline::LiveSpeakers` runs independently on the dedicated bounded 600-message speaker worker; it is not an ASR endpoint callback. Queue overload reports that labels fell behind while capture continues. Stop finalizes speaker work before sealing the live run. Offline refinement derives a fresh independent turn set before mapping acoustic word midpoints and splitting segments at speaker changes. A gap resets only that track. If word strings cannot be found in order in the original text, the session retains the complete source text with an honest combined label instead of rewriting speech or inventing alignment. Temporal user constraints are supplied again for final/refined clustering. The real public-audio live and saved AppService pipelines passed independently; long-call backpressure and end-to-end latency acceptance remain open.

Speaker relationship corrections can be replaced or revoked without deleting their audit history. A meeting supports up to 4,096 active relationships; replacements remain possible at capacity, and revocation frees capacity. Removal reverses linked manual aliases/turn overrides; conflicting user names reject a merge transaction. Disabling/missing speaker models retains previous timed labels/names/reviews during refinement with an explicit reuse warning.

`speaker_store` persists run-local immutable timed annotations and meeting-local identities, user names/time constraints, provenance and review cases; no embedding vectors are stored. `DiarizationInfo.segmentation_context` is optional measured inference provenance with `source_window_ms`, `ownership_stride_ms` and `native_window_ms`. Live processing records 5,000/2,500/10,000 ms; refined saved processing records 10,000/2,500/10,000 ms. This describes actual audio context and ownership cadence, not speaker-count or accuracy acceptance. Incremental live results and engine finalization supply their actual phase settings; synthetic results and warning-only rows remain unknown. Retained annotations copy the original context exactly even when the new transcript run has a different kind. `speaker_reconcile::reconcile` compares reliable duration overlap separately per track. Named splits at 30% per child require review; unnamed splits get new IDs. Merges preserve a single named prior identity, while multiple named IDs or intersecting split/merge evidence require review. Remaining pairs use integer Hungarian maximum-weight assignment with at least 50% of the larger cluster duration and no equal-optimum ambiguity. Overlap/unclear/provisional turns supply no identity evidence; unioned intervals prevent duplicate-window inflation. This is label continuity, not human identity verification. See [speaker turns](../../diarization/speaker-turns/speaker-turns.md) for model/core limits; actual live/offline service and synthetic annotation tests now pass; calibrated quality and long-call gates remain open.

### AI notes and processing preferences

`preferences::load/save` persists `auto_refine`, `auto_summarize` and non-secret `SummaryConfig` in `app_settings`. Both automation flags default true, while the summary provider defaults to `none`, template `general`, context 4,096 tokens and cloud consent false. Settings supports General, One-on-one, Standup, Client call and Interview templates. Apple availability is queried from the bundled `vt-summary-helper`; local discovery probes only numeric loopback ports 11434 and 1234. Discovery never selects/enables a provider or downloads a model.

`processing::summary_job` snapshots the active complete run and its correction/input revision. It sends effective transcript segments with their immutable IDs to `vt-summarize`, then stores structured content/Markdown with provider/model/template, exact source IDs and partial/errors metadata. Citation IDs must belong to the source run. Publication rejects a changed active run or revision. Transcript corrections, speaker edits that affect generated transcript inputs and newer transcript publication mark old summaries stale. Title/personal-note edits refresh FTS without changing transcript revision or summary freshness because the current summarizer receives transcript text only. Missing recording intervals or recorded processing warnings also mark generated notes partial. Summary-specific provider failures persist through `MeetingDetail.summary_error` from `app_summary_failures`, independently of recording/transcript warnings and status. Source validation does not prove semantic entailment. See [AI notes](../../summaries/ai-notes/ai-notes.md) for map/reduce, refusal handling, transport bounds and provider controls.

`ProcessingSettings.svelte` keeps draft API keys only in a password field and the explicit IPC request, clearing them after save or leaving Settings. Provider/origin changes revoke draft cloud consent and discard draft credentials. Keys are stored by the backend in provider/origin-scoped macOS Keychain accounts, never returned to the WebView or placed in browser storage/SQLite preferences. An explicit remove-key checkbox targets the selected provider/origin. Cloud adapters require consent, HTTPS, a chosen model and a stored key. No provider fallback occurs silently. Numeric-loopback local inference and Apple inference stay on-device; external provider text is subject to that provider's handling terms.

`SummaryPanel.svelte` renders structured output as escaped text, with an escaped Markdown fallback. It visibly marks stale/partial notes and source gaps. Clicking a citation selects the source run, opens Transcript, clears its filter and focuses the cited segment. Historical text and partial live text have no correction controls.

### Model installation

`ModelManager::install` is invoked only by an explicit setup action. It downloads the pinned official English Zipformer archive over HTTPS, limits redirects to the official release hosts, checks archive size/SHA-256, extracts the selected files, verifies their pinned size/hash and publishes a manifest with source, Apache-2.0 text and attribution. Progress is a `0..1` fraction; the last 5% is reserved for verification/extraction. Cancellation or failure leaves an actionable setup error. Live and refinement speech recognition make no cloud-transcription calls and require no API key. `RefinementModelManager` separately verifies an accepted Parakeet weight catalog and the exact Silero file. The compressed Parakeet archive size is not an acceptance pin. It retains CC BY 4.0/MIT attribution; installation remains an explicit action. Summary credentials are separate from both speech models.

### Live English model choice and overload reporting

Settings exposes independent English acoustic-model choices. Fresh setups select Standard; an existing Compact directory or an explicit saved choice is preserved. Selection is local metadata only and never downloads or removes weights. Changing the model requires capture/processing/model setup to be idle. Downloads, cancellation and verify/repair remain explicit actions. Both models have separately pinned official archives, files and source/licence metadata. Full Lite/Balanced/Accurate application profiles remain separate roadmap work.

The Standard variant uses explicit live Silero threshold 0.2 when VAD is enabled and installed; Compact retains 0.5. Core `StreamingRecognizer::with_vad` keeps its default 0.5, while desktop constructors share `live_models::recognizer`. With VAD disabled/unavailable the selected model runs ungated. This configuration comes from bounded public-meeting/control comparisons; broad calibration and hardware/resource acceptance remain open. Live and replay versions store requested model/file hashes and VAD options without rewriting historical provenance.

Backlog snapshots distinguish partials, speaker work and live STT shedding from raw audio. Saved run-specific processing history remains visible only with the corresponding transcript version. Audit completion describes ledger sealing, not complete speech coverage. Unavailable/incomplete ledger writes are flagged; source audio stays independently retained. See [recording health](../../capture/recording-health/recording-health.md).

### Storage controls and resource health

Settings exposes **Privacy and storage**, with audio kept by default and obsolete, unreferenced transcript versions eligible after seven days. Whole-meeting or audio-only removal requires a fresh preview and explicit confirmation. Automatic retention checks every 60 seconds while idle, and automatic attempts are at least an hour apart, including failed attempts. Playing/buffering saved audio defers automatic cleanup. Saving retention preferences makes the policy eligible for the next idle check; manual preview/cleanup remains available during retry backoff. Exported and backed-up copies remain outside deletion. See [Retention](../../storage/retention/retention.md) for confirmed intents, filesystem capabilities, crash recovery and protected history.

Storage maintenance reserves the shared work slot and drops saved output before confirmed deletion. Recording, processing and saved-audio reads cannot race file removal. Snapshots, meeting detail/search, input inventory and maintenance Tauri commands run on blocking workers to keep the interface responsive. The scheduler is started once by the desktop shell; constructing a service in a test does not start background cleanup. Startup resolves confirmed retention intents before ordinary interrupted-recording recovery.

The interface displays available-space warnings and explicit source timing. Disk pressure sheds inference below 500 MB and requests a clean capture stop below 100 MB; unknown space blocks new capture and processing. Live notes use validated capture-time origins or explicit observed-audio fallback. Saved notes without a matching playback cursor are untimed. See [Recording health](../../capture/recording-health/recording-health.md) for exact thresholds, metadata and verification limits.

## Key types and entry points

| Type or function | Path | Responsibility |
|---|---|---|
| Tauri commands, `main`, `helper_path` | `src-tauri/src/main.rs` | Local command boundary, app lifecycle, data/helper paths |
| Native helper build | `src-tauri/build.rs` | C/Swift compilation and app resource preparation |
| `AppService`, `Runtime`, `RecordingView`, `Snapshot` | `crates/vt-session/src/lib.rs` | Session control, state snapshots, model setup and shutdown |
| `TrackBuffer`, `record_session`, `infer_worker`, `transcribe_saved` | `crates/vt-session/src/session.rs` | Per-track publication, capture/inference workers and saved transcription |
| `Meeting`, `Segment`, `TranscriptWord`, `RunInfo`, `Correction`, `SummaryInfo`, store functions | `crates/vt-session/src/store.rs` | Immutable runs, legacy migration, overlays, search, grounded summaries, notes and exports |
| `CaptureProcess`, `CaptureEvents`, `InputDevice`, `DeviceList` | `crates/vt-capture/src/lib.rs` | Bounded native event transport and input enumeration |
| `CaptureEngine`, `Accumulator` | `native/macos/CaptureHelper.swift` | Aggregate/tap ownership, device transitions and worker publication |
| `VTAudioRing`, `vt_ring_write` | `native/macos/AudioRing.c` | Preallocated lock-free callback transport and gap accounting |
| `StreamingRecognizer`, `TranscriptEvent` | `crates/vt-stt/src/recognize.rs` | CPU streaming recognition and finalization |
| `ModelManager` | `crates/vt-stt/src/models.rs` | Explicit model setup, pinning and verification |
| `ProcessingView`, `processing::refinement_job`, `processing::summary_job` | `crates/vt-session/src/lib.rs`, `crates/vt-session/src/processing.rs` | Reserved/cancellable offline jobs and automatic chaining |
| `Preferences`, `load/save` | `crates/vt-session/src/preferences.rs` | Non-secret automation/provider settings |
| `BatchRefiner`, `RefinementModelManager` | `crates/vt-stt/src/refine.rs`, `crates/vt-stt/src/refinement_models.rs` | Offline word-timestamp inference and verified pack setup |
| `SummaryConfig`, `summarize`, credential helpers | `crates/vt-summarize/src/` | Explicit providers, cited map/reduce and Keychain |
| `Playback`, `AudioRenderer`, `waveform` | `crates/vt-playback/src/` | Bounded native saved output, timeline/gap mixer and waveform peaks |
| `audio_service`, `AudioPreferences`, `model_setup` | `crates/vt-session/src/audio_service.rs`, `audio_preferences.rs`, `model_setup.rs` | Capture/output exclusion, persisted optional gates and explicit pack setup |
| `speaker_reconcile::reconcile`, `speaker_store::save_result/annotate` | `crates/vt-session/src/speaker_reconcile.rs`, `speaker_store.rs` | Pure conservative identity continuity, timed annotations and review persistence |
| `App.svelte`, `AudioPanel.svelte`, `AudioSettings.svelte`, `ProcessingSettings.svelte`, `SummaryPanel.svelte`, typed command API | `ui/src/App.svelte`, `ui/src/lib/`, `ui/src/lib/api.ts` | Live/library/history/corrections/AI-notes/settings UI using real backend state |

## Commands and access control

These are local Tauri commands, not HTTP routes. The app runs as the local OS user. The Tauri capability/CSP boundary and macOS recording permissions apply. There are no application accounts, server roles or unattended auto-recording rules.

| Command | Inputs | Result or side effect |
|---|---|---|
| `app_snapshot` | None | Recording/library, model status, processing view, non-secret settings, source-clock provenance and disk health |
| `maintenance_policy` / `save_maintenance_policy` | None / `policy` | Read or explicitly save future retention rules |
| `preview_deletion` / `commit_deletion` | `id`, `kind` / `token` | Preview selected meeting/audio removal, then confirm the exact durable plan |
| `preview_cleanup` / `apply_cleanup` | None / `preview` | Preview saved policy and apply only while that policy remains current |
| `last_maintenance_report` | None | Last persisted result, pending cleanup and errors |
| `live_model_catalog` / `select_live_model` | None / `model` | Pinned local choices; idle-only selection, no download or current-run switch |
| `input_devices` | None | Device IDs/names, default/Bluetooth flags, system-capture availability |
| `start_recording` | `title`, `mode`, `consent_confirmed`, optional `input_device` | Enforces acknowledgment and single active work; starts workers |
| `pause_recording` / `resume_recording` / `stop_recording` | None | Validated session transition and updated snapshot |
| `meeting_detail` | `id`, optional `run_id` | Active/historical effective segments, notes, run/correction/summary metadata and separate `summary_error`; live partials only for the active view |
| `save_note` | `id`, `text` | Source-clock live note, matching saved playback cursor, or explicit untimed saved note; timing provenance is retained |
| `rename_meeting` | `id`, `title` | Validates and persists a 1–200-character title |
| `export_meeting` | `id`, `format` | New export file; returns its path |
| `install_model` / `cancel_model_download` | None | Explicit asynchronous setup/cancellation |
| `transcribe_meeting` | `id` | Asynchronous saved-audio live-model replay; new run published atomically |
| `install_refinement_model` / `cancel_refinement_model_download` | None | Explicit Parakeet/Silero setup/cancellation |
| `refine_meeting` | `id` | Asynchronous offline refinement, retaining previous transcript until publication |
| `cancel_processing` | None | Signals active saved-processing cancellation |
| `search_meetings` | `query` | Literal-safe FTS over title/current effective transcript/notes/fresh summaries |
| `correct_segment` | `id`, `run_id`, `segment_id`, `replacement` | Current-run text overlay; original segment unchanged |
| `resolve_correction` | `id`, `correction_id`, optional `segment_id` | Attach unmatched overlay to a current segment, or explicitly dismiss |
| `update_preferences` | `auto_refine`, `auto_summarize`, `summary_config`, optional `api_key`, optional `clear_key` | Validates/saves settings, performs explicit scoped Keychain mutation, returns snapshot without secrets |
| `summary_capabilities` | None | Actual Apple availability and read-only local-model discovery |
| `generate_summary` | `id` | Asynchronous grounded AI notes for the complete active run |
| `play_meeting_audio` | `id`, optional `position_ms` | Explicit native playback; capture exclusion enforced |
| `pause_meeting_audio` / `stop_meeting_audio` | None | Pause preserves consumed cursor; Stop releases/resets output |
| `seek_meeting_audio` | `id`, `position_ms` | Clamped native timeline seek, preserving paused/playing state |
| `set_playback_volumes` | `id`, `mic_volume`, `system_volume` | Independent finite 0–1 gains; loads silently if needed |
| `meeting_waveform` | `id`, optional `bins` (512 default) | Offline bounded-memory peaks; no output/cache mutation |
| `install_vad_model` / `cancel_vad_model_download` | None | Explicit standalone Silero setup/cancel |
| `install_diarization_model` / `cancel_diarization_model_download` | `acknowledge_terms` for install | Explicit experimental speaker pack setup with source notice |
| `update_audio_preferences` | `live_vad_enabled`, `diarization_enabled` | Saves optional next-session/processing gates; no capture/model auto-start |
| `frontend_ready` | None | Marks the embedded interface initialized; records no meeting data |
| `open_data_folder` | None | Opens the data root in Finder |

## Database schema

The desktop schema extends the same SQLite file as the durable recorder. The `app_*` schema includes implemented versioned transcript, correction, search and summary storage; meeting-local speaker annotations/constraints are also stored separately, while explicit named voiceprints use a separate encrypted store. Automatic voice recognition and database/audio encryption remain open; retention schema is documented separately.

| Table | Column | Description |
|---|---|---|
| `app_meetings` | `id` | Primary key and foreign key to recorder `meetings.id` |
| | `title` | Editable display title |
| | `status` | `recording`, `paused`, `processing`, `ready`, `recovered` or `failed` |
| | `started_at` | UTC RFC 3339 start timestamp |
| | `duration_ms` | Nonnegative elapsed session duration |
| | `mode` | `remote` or `in_person` |
| | `error` | Nullable session/transcription failure text |
| `app_segments` | `id`, `meeting_id` | Retained legacy archive; copied into run-scoped rows during migration |
| | `track` | `mic` or `system` |
| | `start_ms`, `end_ms` | Session timing; end is at least start |
| | `text`, `speaker` | Legacy final text and Me/Them channel label |
| `app_notes` | `id`, `meeting_id` | Note UUID and meeting foreign key |
| | `t_ms`, `text` | Nonnegative timestamp and note/bookmark text |
| `app_transcription_staging` | `job_id`, `id`, `meeting_id` | Replay job key, segment UUID and meeting foreign key; unpublished replay text |
| | `track`, `start_ms`, `end_ms`, `text`, `speaker` | Same segment representation as `app_segments`, copied into a new replay run only after successful publication |
| `app_audio_clock` | `meeting_id`, `track`, `seq` | Composite key mapping a writer chunk sequence to its source observation |
| | `source_epoch`, `source_first_frame` | Native configuration generation and original first frame |
| | `host_time_ns` | Original source host-clock observation in session nanoseconds |

Additional tables are detailed in the [processing-runs schema](../../storage/processing-runs/processing-runs.md#database-schema):

| Table | Main columns and role |
|---|---|
| `app_processing_runs` | Run UUID, meeting, kind/status, engine JSON, captured user-speaker-constraints version, created time |
| `app_meeting_revision` | Meeting, active-run FK, monotonic transcript/speaker-input revision |
| `app_run_segments` | Composite run/segment key, track, times, immutable text/speaker, word timestamp JSON |
| `app_segment_corrections` | Meeting, track/time/hash anchor, replacement, applied/review/dismissed state, creation time |
| `app_correction_matches` | Correction/run key and unique run/segment target |
| `app_summaries` | Meeting/run FKs, input revision, provider/model/template, JSON/Markdown, stale flag, created time |
| `app_summary_failures` | Meeting PK/FK with delete cascade and non-null summary-only error, independent of recording health |
| `app_summary_sources` | Summary and run-scoped source-segment FKs protecting historical citations |
| `app_search_fts` | FTS5 meeting/source/reference/text rows rebuilt within source-edit/publication transactions |
| `app_speaker_identities`, `app_run_speaker_keys` | Meeting/track identity, optional user name/alias/prior, shared-room flag, nullable person link and run-local cluster mapping |
| `app_speaker_turns`, `app_speaker_run_state` | Run-scoped timed identity/provenance/overlap/uncertainty annotations and embedding-version/calibration warnings plus nullable `app_speaker_run_state.segmentation_context_json`; no vectors |
| `app_speaker_constraints`, `app_speaker_turn_overrides`, `app_speaker_reviews` | Persisted user temporal constraints, explicit turn overrides and unresolved split/merge/constraint review |
| `app_settings` | `key`, `value_json`; non-secret `preferences` value preferences and audio-processing options, created by migration 003 |

Startup checks `PRAGMA table_info(app_speaker_run_state)` before adding nullable `segmentation_context_json` to existing databases. Legacy values stay SQL `NULL` and read back as unknown; the app never reconstructs them from run kind or current defaults. The result UPSERT stores context in the same transaction as timed speaker annotations, and annotation copy preserves its original JSON. Failed result/copy publication rolls back all associated rows. The store APIs reject speaker-result and copy writes to sealed runs; this does not introduce additional direct-SQL guards on the existing run-state table. Retention already owns that table and includes its columns dynamically in deletion fingerprints.

Indexes support meeting/time lookup for segments and notes. SQLite foreign keys are enabled. Final segment and note content is stored unencrypted, along with native-rate audio and exports, under ordinary filesystem permissions. Named voiceprint blobs are encrypted separately; person/consent metadata is plaintext. The [voice-memory feature](../../speakers/voice-memory/voice-memory.md) owns biometric schema and lifecycle.

## SQL artifacts

- [001-desktop.sql](sql/001-desktop.sql): executable desktop metadata, segment, note and clock-observation schema.
- [008-retention.sql](sql/008-retention.sql) and [009-retention-delete-guards.sql](sql/009-retention-delete-guards.sql): durable confirmed deletion intents, audio-purge markers and narrow immutable-history deletion gates.
- Recording health has its [own SQL artifact](../../capture/recording-health/sql/001-resource-health.sql).
- [003-preferences.sql](sql/003-preferences.sql): executable non-secret processing settings schema loaded by `preferences::load`.
- [002-processing-runs.sql](sql/002-processing-runs.sql): run-scoped immutable segments, active revisions, correction matches, cited summaries, separate summary failures and FTS5.
- [003-preferences.sql](sql/003-preferences.sql): non-secret settings table; provider keys remain in Keychain.
- [004-speaker-annotations.sql](sql/004-speaker-annotations.sql): timed speaker metadata, nullable measured segmentation context, stable/run-local identity links, constraints and explicit review records, without embedding vectors; existing-column upgrades are guarded in `store::initialize`.
- [005-replay-word-timing.sql](sql/005-replay-word-timing.sql): guarded legacy staging upgrade preserving acoustic word JSON through replay.
- [Recorder schema](../../recording/durable-recording/sql/001-recording.sql): durable audio manifest used by this feature.

## Verification limits and remaining work

Frontend type checking and UI production bundling pass. Eleven Chrome fixture flows cover recording consent/device choice, recording controls/notes/export, model cancellation, refinement isolation, historical read-only text, explicit overlay review, stale/partial cited notes, historical citation navigation, escaped output, literal search command dispatch, provider consent/origin reset/key removal, and live partial styling. Storage tests cover legacy migration/history, failed/unpublished runs, immutable rows, publication rollback, conservative overlay matching/review and stale-summary/citation guards, metadata edits preserving transcript-only summary freshness, and independent persisted summary-error update/clear. These browser/storage tests use temporary synthetic data and do not make cloud inference calls.

Seven generated-FLAC/mock playback tests and strict crate clippy passed. A separately authorized silent native CPAL test used only temporary synthetic audio with both gains zero: actual callbacks advanced to 32 ms, resumed from 500 to 532 ms, sought while playing to 250 then 282 ms, held pause position, and stopped at zero. Eight pure reconciliation tests cover named/unnamed splits, conflicting merges, per-track/uncertain evidence, percentage gates, ambiguity, capacity and non-greedy/rectangular Hungarian matching. These focused checks do not establish audible fidelity, all device routes or full diarization acceptance.

The prior signed release launched with its embedded frontend connected and a live model installed. Short packaged recordings persisted matching-frame Mic/System audio, microphone transcript lines and an external System speech fixture. Separate feature checks exercise real Parakeet against public audio and Apple notes against fictional text; their evidence and limits are recorded in the linked feature docs. Integration and packaged-runtime verification for this expanded release is still in progress. None of this proves all permission/device transitions, long-session drift, quality budgets or the complete roadmap.

Synthetic storage tests cover exact context save/copy/restart, nullable legacy upgrade/idempotence, warning-only unknown values, atomic result/copy rollback and sealed-run API rejection. The opt-in public saved-refinement tests additionally assert actual Refined context while synthetic historical rows remain unknown.

Calibrated diarization/full desktop live-offline annotation acceptance, AEC/bleed handling, voiceprints, model profiles, automatic meeting detection, retention acceptance, encryption, richer exports and Windows remain open. Native playback device/route/physical-latency and optional VAD quality acceptance remain open. Baseline Mic/System labels and experimental cluster similarities do not verify individual people. Refined word timestamps are acoustic estimates, not forced alignment. AI citations validate source identity, not factual entailment. The full approved design and its remaining acceptance gates are retained.


## Echo preprocessing and bleed review

Audio preferences add `echo_cancellation_enabled=true`. Each live session snapshots it at worker start; a refinement snapshots it once for both its speaker and transcription passes. Changes apply to the next recording/job. The default does not establish route eligibility.

`CaptureEvent::OutputRoute` records anonymous source-time evidence in `app_output_routes` before subsequent audio. `OutputRoute::aec_eligible` requires positive built-in speaker, data-source, disconnected-jack, process-device and live System-reference evidence. A global default device alone is insufficient. Headphones, external/mixed routes and unknown observations skip AEC. No device names, UIDs or process IDs enter this table.

`echo_pipeline::worker` receives raw mono copies on its own bounded channel, converts through `vt-dsp`, then forwards the same processed packets to the independent ASR and speaker workers. Input/output queues never block native capture. Lost consumer work requests a reset before subsequent audio; errors leave original FLAC available. Route/format/gap changes drain old tails and reset adaptive state. The recorder's initial pending buffer uses its first source-clock observation as the inference origin, keeping packet times correct before/across the first chunk flush.

`SavedAudioReader` reuses playback's read-only capability/path/SHA/PCM/frame verification, opens no output stream, and emits chronological <=20 ms mono slices, System first on ties. Saved transcription, refinement and speaker work apply AEC from the captured route history rather than the current machine route. A clean stop seals `app_route_evidence`; missing, interrupted or unsuccessfully persisted histories bypass saved AEC. Per-run `EchoInfo` records actual applied/bypassed Mic durations, engine and warnings. Processed PCM is never saved over source audio or decoded to plaintext temporary files.

The Transcript review panel uses [bleed review](../../transcription/bleed-review/bleed-review.md). Text similarity creates visible suggestions only; automatic speaker/voiceprint evidence is not implemented. Explicit confirmed bleed is hidden by default and excluded from FTS, AI-note input and Markdown/TXT/SRT/VTT exports. JSON/detail retain full speech plus decision metadata. One-click restore changes an annotation and invalidates notes; immutable source segments/audio and historical run snapshots remain intact. Historical citations reveal hidden source lines for review. Earlier uncertain matches remain visible with an explicit keep-visible action.

| Entry point | Location | Responsibility |
| --- | --- | --- |
| `EchoStream`, `worker`, `saved` | `crates/vt-session/src/echo_pipeline.rs` | Shared live/saved echo path, source-time route lookup and consumer reset handling |
| `record_route`, `seal_routes`, `save_info` | `crates/vt-session/src/echo_store.rs` | Capture-time route evidence and run-scoped processing metadata |
| `SavedAudioReader` | `crates/vt-playback/src/reader.rs` | Verified two-track chronological native-audio decoding |
| `set_bleed_decision`, `resolve_bleed_review` | `AppService` and Tauri adapters | Current-run user decisions; processing/historical guards |
| `BleedPanel.svelte` | `ui/src/lib/BleedPanel.svelte` | Candidate comparison, explicit hide/restore, pending-review actions and paginated decisions |

Additional local commands: `update_audio_preferences` accepts `echo_cancellation_enabled` (omitting it preserves the current preference); `set_bleed_decision(id,run_id,segment_id,decision)` accepts `bleed`/`genuine`; `resolve_bleed_review(id,run_id,decision_id)` keeps unmatched speech visible. These are local IPC with the same app/data-root boundary as transcript corrections.

Additional schema artifacts:

- [006-echo-processing.sql](sql/006-echo-processing.sql): `app_output_routes(meeting_id,epoch,start_ns,route_json)` immutable capture observations; `app_route_evidence(meeting_id,complete)` clean-history marker; `app_run_echo_processing(run_id,info_json)` mutable only while the run is running.
- [007-bleed-annotations.sql](sql/007-bleed-annotations.sql): append-only `app_bleed_decisions` anchor/lineage/audit events and per-run `app_bleed_run_matches` snapshots. Fields and reapplication rules are detailed in the bleed feature document.

The desktop now requires stable Rust 1.91 for the pinned Sonora dependency. Distribution includes `licenses/echo/*` notices. Synthetic engine/archive tests and public live/refinement checks prove the connected path; real-route quality, corpus WER/DER/bleed acceptance and long-call gates remain open.

A malformed or unsupported saved English model choice opens with a Settings warning and a supported candidate. The candidate is inactive until the user selects or confirms it; recording remains available, and setup does not download a fallback. Ordinary recording pauses disclose a pause context reset rather than a native source failure. A capture intentionally starting with inference suspended by disk pressure does not claim an inference ledger was expected.

## Consented voice enrollment and People

Settings → People loads metadata through `voice_memory_status`, without querying Keychain. Memory starts off. `update_voice_memory(enabled, retention_months, explanation_acknowledged, context_generation)` accepts 1–24 months (12 default); enabling requires the explanation acknowledgment. `enroll_voice(request, context_generation)` recomputes embeddings from eligible saved refined speech on a blocking worker and commits only after fresh consent/source/context checks. No key or embedding enters IPC.

`forget_person(person_id, remove_names)` and `delete_all_voiceprints(remove_names)` remove local biometric records without decrypting them. The optional name action removes linked speaker-name overlays; it does not rewrite spoken prose, personal notes, stored source snapshots, summaries or exports. `set_speaker_shared_room(id, speaker_id, shared_room)` changes eligibility only while recording/processing are idle and returns refreshed meeting detail. `MeetingDetail.mode` distinguishes in-person room audio from remote system audio.

`voice_memory.rs` coordinates runtime reservations, retained-source reads and final atomic links. `vt-speakers` owns protected blobs, consent/settings schema and metadata DTOs; `vt-diarize::VoiceEmbedder` recomputes bounded clean-source embeddings. The [voice-memory SQL](../../speakers/voice-memory/sql/001-voice-memory.sql) shares the existing app database. `store::initialize` probes legacy speaker columns before adding `shared_room` and `person_id`; existing labels/turns remain intact. Startup and hourly idle maintenance delete expired blobs even while memory is off. Automatic recognition remains unavailable until actual calibration is validated.
