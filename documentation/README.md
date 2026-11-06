# VanTranscribe Documentation

VanTranscribe is a free, open-source (planned MIT) desktop meeting recorder and transcriber intended to replace Zoom Workplace's "My Notes". No bot joins the meeting: the app captures system audio (remote participants) and the local microphone (you) as two separate tracks, produces a live transcript with speaker labels, then runs a post-meeting refinement pass for accuracy. Summaries and action items come from a pluggable model (Apple Intelligence, a local LLM, or a cloud API). It is local-first, built with Tauri 2, a Rust core, and a Svelte + TypeScript + Bootstrap 5 UI, targeting macOS first and Windows later.

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
- **Trail entries** record why a change was made. They are **never edited after creation**; corrections are new entries.
- **Planning docs** capture plans and decision records before or alongside implementation, grouped by module and dated.

## Feature Map

Milestones are from [design.md section 18](../design.md#18-roadmap).

| Module | Scope | Milestone | Status | Design |
|---|---|---|---|---|
| capture | Mic and system audio tracks, OS backends | M0 (macOS, durable recording + device changes), M5 (Windows 11) | Planned | [5. Audio capture](../design.md#5-audio-capture-and-real-time-pipeline) |
| dsp (AEC/VAD) | Resample, VAD (M1), AEC and bleed handling (M2c) | M1 / M2c | Planned | [5.3](../design.md#53-pipeline-per-track), [5.4](../design.md#54-echo-and-bleed-handling) |
| stt | Live streaming STT (M1), batch refinement STT (M2a) | M1 / M2a | Planned | [6. Speech-to-text engines](../design.md#6-speech-to-text-engines) |
| diarization | Online (live) and offline diarization | M2b | Planned | [7.1](../design.md#71-live-online-diarization), [7.2](../design.md#72-offline-diarization-refinement-pass) |
| speakers (voiceprints) | Opt-in cross-meeting speaker memory | M4 | Planned | [7.4](../design.md#74-cross-meeting-speaker-memory-opt-in) |
| refinement | Post-meeting refinement pass, processing runs | M2a | Planned | [8. Refinement pass](../design.md#8-post-meeting-refinement-pass) |
| summarize | Pluggable summarizer, map-reduce, templates | M3 | Planned | [9. Summaries](../design.md#9-summaries-and-ai-notes) |
| storage/search | SQLite store (M1), FTS search (M4), encryption and retention (M4) | M1 / M4 | Planned | [12. Data model](../design.md#12-data-model) |
| meeting-detection | Detect meeting apps using the mic | M4 | Planned | [14. Meeting detection](../design.md#14-meeting-detection) |
| ui | Live window, library, meeting detail, settings | M1 onward | Planned | [13. UI](../design.md#13-ui) |
| export | Markdown (M1); TXT/SRT/VTT/JSON; DOCX/Confluence (v1.x) | M1 / v1.x | Planned | [13. UI (Export)](../design.md#13-ui) |
| onboarding/permissions | First-run flow, permission detection (denied detection in M0) | M0 onward | Planned | [3.4](../design.md#34-first-run--onboarding), [10. Permissions](../design.md#10-permissions-and-platform-specifics) |
