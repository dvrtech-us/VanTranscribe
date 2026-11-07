# VanTranscribe Documentation

VanTranscribe is an MIT-licensed, local-first desktop meeting recorder and transcriber intended to replace Zoom Workplace's "My Notes". The current implementation joins a Tauri 2 app, Rust session/recording core, native macOS microphone/system capture, local English streaming recognition, offline Parakeet refinement, versioned transcript storage, grounded AI notes, Rust-core saved playback, optional incremental Silero VAD and an experimental independent speaker-turn core, with a Svelte + TypeScript + Bootstrap 5 interface. The local macOS release and packaged IPC are verified; full milestone completion is not established by a build or isolated fixture alone.

The approved design retains the full roadmap: the remaining refinement quality gates, calibrated speaker diarization and full annotation acceptance, AEC/bleed handling, calibrated voiceprint matching, playback hardware/latency acceptance, privacy/retention acceptance and Windows support. Refinement and pluggable AI notes are now implemented increments rather than completed milestone claims. Feature docs below distinguish implemented behavior from those planned capabilities.

## Authoritative design

[../design.md](../design.md) is the authoritative design. Feature docs and planning docs must not contradict it; if behavior diverges, update the design or record the decision in a planning doc.

## Documentation structure

```
documentation/
  features/<module>/<feature>/
    <feature>.md          current implemented behavior
    baseline.md           invariants that must not regress
    trail/YYYY-MM-DD-<title>.md   decision trail entries
    sql/*.sql             schema DDL for the feature
  planning/<Module>/YYYY-MM-DD-<title>.md   plans and decision records
```

Rules:

- **Feature docs** (`<feature>.md`) describe the *current implemented* behavior, not intentions. Update them in the same change that alters behavior.
- **Baselines** (`baseline.md`) hold invariants: behaviors, limits and contracts that must hold across changes.
- **Trail entries** record why a change was made. Each feature has one working draft per PR, edited in place until merge. Merged entries are immutable; later PRs record new decisions.
- **Planning docs** capture plans and decision records before or alongside implementation, grouped by module and dated.

## Feature Map

Milestones are from [design.md section 18](../design.md#18-roadmap).

| Module | Scope | Milestone | Status | Design |
|---|---|---|---|---|
| app/desktop-session | Tauri commands, native-helper/session orchestration, live/library/detail/settings UI, refinement, AI notes, model setup and exports | M0 / M1 / M2a / M3 increments | [Connected and installed locally; full hardware/corpus gates pending](features/app/desktop-session/desktop-session.md) | [4. Architecture](../design.md#4-architecture), [13. UI](../design.md#13-ui) |
| recording | Worker-side native-rate FLAC chunks, PCM/timing contracts, SQLite manifest and interrupted-publication recovery | M0 | [Foundation implemented; M0 partial](features/recording/durable-recording/durable-recording.md) | [5.6 Durable recording](../design.md#56-durable-recording-format) |
| capture | Native mic/system aggregate, bounded callback transport and device epochs | M0 (macOS), M5 (Windows 11) | [macOS capture verified; full hardware gates pending](features/capture/macos-capture/macos-capture.md) | [5. Audio capture](../design.md#5-audio-capture-and-real-time-pipeline) |
| dsp/echo-control | Bounded inference-only AEC3, capture-time route eligibility and saved replay | M1 / M2c | [Conservative AEC3 core and live/saved integration implemented](features/dsp/echo-control/echo-control.md); hardware/corpus echo acceptance remains open | [5.3](../design.md#53-pipeline-per-track), [5.4](../design.md#54-echo-and-bleed-handling) |
| stt | Two pinned English CPU streaming models, explicit verified setup and saved-audio transcription | M1 / M2a | [Local English streaming verified; quality gates remain open; batch refinement implemented separately](features/transcription/live-english/live-english.md) | [6. Speech-to-text engines](../design.md#6-speech-to-text-engines) |
| diarization/speaker-turns | Independent segmentation/embedding/clustering, phase-specific source context, meeting-local constraints and conservative duration-overlap reconciliation | M2b | [Live/saved public-fixture runtime verified; calibrated DER/latency gates remain open](features/diarization/speaker-turns/speaker-turns.md) | [7.1](../design.md#71-live-online-diarization), [7.2](../design.md#72-offline-diarization-refinement-pass) |
| audio/saved-playback | Rust-core CPAL output, timeline/gap mixing, seek/pause/track gains and bounded waveforms | M1 | [Implemented; generated-FLAC mocks and silent real CPAL callbacks verified](features/audio/saved-playback/saved-playback.md); hardware/route/latency acceptance remains open | [5.2 Playback exclusion](../design.md#52-capture-backends), [13. UI](../design.md#13-ui) |
| quality/evaluation | Reference-backed WER, DER/JER, bleed, latency, resources and fixed-identity speaker research | M0 / M1 / M2b / M2c / M4 | [Runnable cargo xtask eval and feature-gated voice-eval/calibration-only harnesses implemented](features/quality/evaluation/evaluation.md); fictional fixtures do not establish product acceptance | [19. Testing strategy](../design.md#19-testing-strategy) |
| quality/production-inference | Actual pinned local engines, public-corpus hypotheses, reference scores and measured resources | M1 / M2a / M2b | [Runner and first public benchmark implemented](features/quality/production-inference/production-inference.md); measured quality targets remain unmet | [19. Testing](../design.md#19-testing-strategy) |
| capture/recording-health | Source-clock timing, explicit note provenance, bounded transport and disk-pressure controls | M0 | [Implemented with injected resource and native no-audio clock checks](features/capture/recording-health/recording-health.md); staged source-watermark shedding is implemented; hardware and long-call gates remain open | [5. Capture](../design.md#5-audio-capture-and-real-time-pipeline) |
| storage/retention | Previewed meeting/audio deletion, crash recovery and conservative version retention | M4 increment | [Implemented with temporary-data crash recovery checks](features/storage/retention/retention.md); exported/backed-up copies remain outside deletion | [12. Data model](../design.md#12-data-model) |
| speakers/voice-memory | Explicit biometric consent, encrypted named voiceprints and People/expiry/deletion controls | M4 increment | [Enrollment and storage implemented](features/speakers/voice-memory/voice-memory.md); automatic recognition and gallery calibration remain open | [7.4](../design.md#74-cross-meeting-speaker-memory-opt-in) |
| transcription/bleed-review | Visible duplicate candidates, reversible user decisions and run-scoped history | M2c | [User-confirmed bleed review implemented](features/transcription/bleed-review/bleed-review.md); automatic voiceprint evidence and corpus gates remain open | [5.4](../design.md#54-echo-and-bleed-handling) |
| refinement | Offline Parakeet v3/Silero VAD with word timestamps, explicit model setup and automatic/manual session jobs | M2a | [Implemented and public-fixture verified; full quality gates pending](features/transcription/batch-refinement/batch-refinement.md) | [8. Refinement pass](../design.md#8-post-meeting-refinement-pass) |
| summaries/ai-notes | Apple/local/explicit cloud adapters, bounded map-reduce, cited templates and cancellation | M3 | [Implemented; synthetic, loopback and on-device checks recorded](features/summaries/ai-notes/ai-notes.md) | [9. Summaries](../design.md#9-summaries-and-ai-notes) |
| storage/processing-runs | Immutable live/replay/refinement runs, correction overlays, active FTS, stale summaries and safe legacy migration | M2a / M3 | [Implemented; synthetic storage regressions passed](features/storage/processing-runs/processing-runs.md); database/audio encryption remains planned; encrypted voiceprints, retention and conservative GC implemented separately | [12. Data model](../design.md#12-data-model) |
| capture/meeting-detection | Opt-in app reminders, explicit per-app recording rules and source-bound stop suggestions | M4 increment | [Implemented with native read-only, injected backend and browser checks](features/capture/meeting-detection/meeting-detection.md); real app/OS and notification-delivery acceptance remains open | [14. Meeting detection](../design.md#14-meeting-detection) |
| ui | Live transcript, FTS search, run history, correction review, grounded AI notes, native audio controls/waveforms, model/provider/input settings and appearance | M1 onward | [Connected interface implemented; browser flows and packaged IPC verified; full native hardware matrix pending](features/app/desktop-session/desktop-session.md#application-and-interface) | [13. UI](../design.md#13-ui) |
| export | Markdown/TXT/SRT/VTT/JSON; DOCX/Confluence later | M1 / v1.x | [Five local text formats implemented](features/app/desktop-session/desktop-session.md#commands-and-access-control); DOCX/Confluence planned | [13. UI (Export)](../design.md#13-ui) |
| onboarding/permissions | Per-recording acknowledgment, explicit mic choice, model setup and OS permission handling; external chirp walkthrough later | M0 onward | [Initial flows implemented; permission/capture gates pending](features/app/desktop-session/desktop-session.md#user-flow) | [3.4](../design.md#34-first-run--onboarding), [10. Permissions](../design.md#10-permissions-and-platform-specifics) |
