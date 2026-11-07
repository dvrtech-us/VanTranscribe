# VanTranscribe

A local-first desktop meeting recorder and transcriber. The repository now includes a Tauri 2 app with a Rust session core, native macOS microphone/system-audio capture, durable FLAC recording, local English speech recognition, Parakeet offline refinement, versioned transcripts with correction overlays, grounded AI notes, Rust-core saved playback, optional live voice activity detection, inference-only AEC3 echo control and a Svelte interface. Experimental live/refined speaker turns and meeting-local name/correction workflows are included; calibrated speaker-label acceptance remains open.

The app is in development and runtime verification is still underway. The full [approved design](design.md) remains the target; a successful build does not establish the complete capture, accuracy or long-session requirements. See the [documentation map](documentation/README.md) for implemented behavior and open milestones.

## Build and open the desktop app

Current target: **macOS 14.4 or newer on Apple Silicon**. Install stable Rust **1.91+**, Node.js/npm and Xcode command-line tools with the macOS 26 SDK or newer (needed to compile the availability-guarded Apple summary helper). The build compiles and bundles the native capture and summary helpers and applies an ad-hoc signature for local use. Distribution signing/notarization remains separate.

```sh
npm install
npm --prefix ui install
npm run build
open target/release/bundle/macos/VanTranscribe.app
```

For development with the connected desktop interface:

```sh
npm run dev
```

Running Vite alone in a browser does not provide the desktop capture backend. The interface reports a missing desktop connection instead of displaying fake meetings.

## Record a meeting

1. Optionally open **Settings** and download the local English Zipformer model. Standard is recommended for new setups (about 507 MB download and 190 MB selected files); Compact uses about 310 MB download and 72.5 MB selected files. Existing selections are preserved, and switching never downloads automatically. No API key is required.
2. Choose **New recording**, enter a title, and select **Remote meeting** for microphone plus system audio or **In person** for microphone only.
3. Choose a microphone. If your system default is Bluetooth, explicitly choose that device or a built-in/USB input; VanTranscribe avoids silently changing a headset into call mode.
4. Let participants know, acknowledge the recording reminder and start.
5. Grant macOS **Microphone** and **System Audio Recording** access when prompted. The default backend does not need Screen Recording permission. If access is unavailable, check the VanTranscribe entries in **System Settings → Privacy & Security**, then retry. Permission labels differ between macOS versions; silence alone does not establish denial.
6. Use the live view to read recognized speech, add notes/bookmarks, pause/resume and stop. Recording audio works before model setup; install the model and use **Transcribe** on a saved meeting without transcript text afterward.
7. Optionally download the Parakeet refinement pack in Settings (about 465 MiB download, 640 MiB installed). Automatic refinement runs after recording when enabled and installed; **Refine transcript** also processes saved meetings. Progress/cancellation stay visible while the previous transcript remains available.
8. Search titles, transcript text and notes from the library. Open a meeting to inspect current or historical transcript versions. Current stopped text supports correction overlays; unmatched edits remain available for explicit review after reprocessing.
9. Set up AI notes in Settings: Apple on-device, a numeric-loopback local model server, or an explicitly consented external provider. Availability checks do not enable a provider. Choose a model/template as applicable; cloud keys are stored in macOS Keychain. The always-visible Summary tab generates cited key points, decisions, actions and open questions for completed transcripts. Old/partial notes are marked, and source links load the cited transcript version.
10. Open **Audio** for native playback, separate Mic/System waveforms, timeline seeking and independent track volumes; zero mutes a track. Transcript timestamps can play from that moment. Loading a meeting never starts output, and playback is blocked while any recording is active, paused or stopping.
11. Audio processing preferences enable echo reduction only when the capture-time built-in speaker route and System reference can be verified. Headphones, mixed/unknown output routes and incomplete saved route histories bypass AEC. Raw recordings and playback keep the original audio.
12. Settings optionally installs the small standalone Silero VAD model (about 629 KiB), also included in the refinement pack. Enabled VAD skips sustained inference silence while preserving context; it never removes raw recording audio. Experimental speaker models have a separate explicit source/license acknowledgment and default-off preference.
13. Review possible microphone echo in Transcript. Similar text stays visible until you confirm bleed. Hidden lines can be restored; short replies remain visible. The JSON export retains the full text and annotations.
14. Rename meetings, add notes and export the current effective transcript as Markdown, TXT, JSON, SRT or WebVTT. Historical views are read only; return to the current version to export.
15. Optionally enable **Voice memory** in Settings → People after reading its separate biometric explanation. With the speaker pack enabled, refine a saved meeting and choose **Remember this voice** for an eligible individual speaker. Supply that person’s separate consent and source channel; at least twenty seconds of retained clean refined speech is required. People controls forget saved voices. Automatic voice recognition remains unavailable pending calibration.

Remote fallback labels are **Me** for the microphone and **Them** for system audio; in-person speech without speaker evidence remains **Unknown**. Optional audio-derived labels are experimental, provisional and uncalibrated; neither channel labels nor cluster similarity verify an individual’s identity. Actual public-audio live/offline service integration is verified; calibrated speaker quality and long-call acceptance remain open. Playback runs in Rust, never through WebView audio.

## Local data and privacy

The default data folder is:

```text
~/Library/Application Support/com.dvrtech.vantranscribe/
  vantranscribe.db
  meetings/<id>/audio/{mic,system}/<epoch>-<seq>.flac
  models/
  Exports/
```

Audio, transcripts, notes and exports are currently **unencrypted** and governed by ordinary local filesystem permissions. Live/refinement speech recognition runs on-device, and the recording path does not upload audio. Model setup downloads files only when requested. AI notes are Off by default. Apple and numeric-loopback local providers keep inference on this Mac; an explicitly configured and consented external provider receives transcript text under its own data-handling terms. Keys are stored in provider/origin-scoped macOS Keychain accounts, never returned to the interface or placed in WebView storage. Local stored transcripts and AI notes are not encrypted by this app.

Speaker memory is a separate explicit opt-in. Consented, named voiceprints are encrypted with a separate macOS Keychain key; person names and consent metadata remain ordinary local database fields. Settings → People explains the retention schedule and supports forgetting people. Automatic voice recognition remains unavailable until version-specific calibration is validated. See [voice memory](documentation/features/speakers/voice-memory/voice-memory.md) and [privacy and retention](PRIVACY.md).

Settings can open the data folder in Finder. For isolated diagnostics, set `VANTRANSCRIBE_DATA_DIR` before launching the executable. Theme and explicit microphone selection are remembered in local WebView storage.

## Develop and verify

```sh
npm --prefix ui run check
npm --prefix ui run build
cargo fmt --all -- --check
cargo test --workspace --all-targets --features vantranscribe/custom-protocol
cargo clippy --workspace --all-targets --features vantranscribe/custom-protocol -- -D warnings
npm run test:ui
```

The durable recorder also has a synthetic harness that does not open audio devices:

```sh
cargo run -p vt-record --example synthetic -- /tmp/vantranscribe-demo demo-001 6
cargo run -p vt-record --example synthetic -- --recover /tmp/vantranscribe-demo demo-001
```

Choose a new meeting ID for each synthetic recording. Duration accepts 1–60 seconds and defaults to four seconds.

Implementation references:

- [Meeting detection](documentation/features/capture/meeting-detection/meeting-detection.md): opt-in microphone-use reminders, explicit per-app rules, suspend-aware freshness and manual stop suggestions.
- [Retention and deletion](documentation/features/storage/retention/retention.md): previewed local deletion, audio-only purge and conservative history cleanup; exports/backups remain outside removal.
- [Recording health](documentation/features/capture/recording-health/recording-health.md): source-clock note provenance, disk-pressure controls and bounded audio relay.
- [Production inference](documentation/features/quality/production-inference/production-inference.md): actual public-corpus runs and measured failures; product quality targets remain open.
- [Desktop session](documentation/features/app/desktop-session/desktop-session.md) and [baseline](documentation/features/app/desktop-session/baseline.md): native helper, session threads, commands, UI, model setup and desktop schema.
- [Saved playback](documentation/features/audio/saved-playback/saved-playback.md): bounded read-only FLAC decoding, shared track timeline, native output and waveform peaks.
- [Bleed review](documentation/features/transcription/bleed-review/bleed-review.md): visible suggestions, explicit reversible choices, conservative reapplication and preserved history.
- [Echo control](documentation/features/dsp/echo-control/echo-control.md): conservative route-gated AEC3, bounded timestamp pairing and source-preserving live/saved processing.
- [Quality evaluation](documentation/features/quality/evaluation/evaluation.md): `cargo xtask eval --manifest PATH --output PATH`, explicit references, metric conventions and honest missing-evidence reports.
- [Speaker turns](documentation/features/diarization/speaker-turns/speaker-turns.md): independent local segmentation/embedding/clustering, five-second live and ten-second refined source context, per-run provenance and experimental quality limits.
- [Batch refinement](documentation/features/transcription/batch-refinement/batch-refinement.md): verified Parakeet/Silero pack, bounded local inference and acoustic word times.
- [Processing runs](documentation/features/storage/processing-runs/processing-runs.md): safe legacy migration, immutable revisions, overlays, transactional search and stale-summary guards.
- [AI notes](documentation/features/summaries/ai-notes/ai-notes.md): explicit provider adapters, bounded cited map/reduce, cancellation and Keychain controls.
- [Durable recording](documentation/features/recording/durable-recording/durable-recording.md) and [baseline](documentation/features/recording/durable-recording/baseline.md): PCM conversion, chunk publication, manifest recovery and crash boundaries.

## Remaining roadmap

Short packaged-app checks have recorded real microphone/system tracks with matching frame totals, produced microphone transcripts and recognized an external speech fixture on the System track. Seventy-one browser fixture flows cover recorder consent/device choice, downloads, refinement isolation, history/correction review, grounded stale/partial notes, citation navigation, provider consent/credentials, search, live partials, storage previews/confirmation, audio retention and source timing. Storage tests protect migrations, publication rollback, immutable text and run-scoped citations; separate public-audio Parakeet and fictional-text Apple checks are recorded in feature docs. Generated-FLAC playback mocks and a silent native CPAL smoke check verified callback-driven position, pause, resume, seek and stop without playing private audio. The updated signed local macOS release is installed and its packaged frontend initialization passes; broader hardware acceptance remains open. These checks do not establish long-call drift, every device/permission transition, transcript quality budgets or complete M0/M2a/M3 acceptance.

Calibrated diarization/full live-offline annotation acceptance, AEC hardware/corpus acceptance and automatic bleed identity evidence, calibrated voice recognition, model profiles, retention acceptance and database/audio encryption, meeting-detection app/OS acceptance, richer exports and Windows remain open. Playback still needs broad device/route/latency validation; VAD needs quality/corpus acceptance. Current channel labels do not identify individual speakers; acoustic word timestamps are not forced alignment; citations identify transcript sources without proving AI claims. The full [approved roadmap](design.md#18-roadmap) remains open and has not been reduced to the currently implemented increments.
