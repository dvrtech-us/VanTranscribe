# Refinement and AI Notes Runtime Checkpoint

- Date: 2026-11-06 (Central)
- Objective: have a working app
- Goal status: active; the approved design and full roadmap remain the target
- Design review: the previously accepted Claude Opus and Codex reviews remain authoritative; this increment did not restart the design approval process

## Implemented increment

The desktop app now connects saved-audio Parakeet/Silero refinement, immutable transcript runs, historical views, correction overlays, transactional full-text search and grounded AI notes. Summaries support Apple Foundation Models, numeric-loopback compatible servers and explicitly consented cloud providers. Provider `none` remains the default; keys are stored only in provider/origin-scoped macOS Keychain entries.

Automatic refinement runs after recording only when enabled and the separately installed pack is available. Automatic AI notes require an enabled setting and an explicitly configured provider. Cancellation, worker failure and shutdown preserve previously published text and notes. A completed replacement is published atomically with correction matching and the active search index.

## Runtime evidence

| Check | Evidence | Limit |
| --- | --- | --- |
| Real refinement through `AppService` | Public WAV persisted as bounded Mic/System FLAC chunks, processed with the verified Parakeet/Silero pack, then published with word timing, run provenance and updated FTS | Short public fixture; multilingual accuracy and hour-long performance budgets are not established |
| Preservation | Prior run remains immutable; bookmark retained; prior summary becomes stale; audio manifest hashes unchanged | No historical-run garbage collection yet |
| Failure/cancellation/shutdown | Synthetic service tests retain published content, abandon failed unpublished runs, stop iteration and suppress automatic work after shutdown | Native inference can only observe cancellation between bounded calls |
| Correction timing | Effective edited text clears incompatible word alignment; raw completed text and timing JSON stay unchanged | Changed text has segment timing only until a future alignment pass |
| Real Apple summary through `AppService` | Fictional transcript plus correction generated and persisted with the current run, correction version and valid source IDs; result was not partial | Citation validity does not establish semantic accuracy; human source review remains necessary |
| Summary races and failure | Synthetic local HTTP fixtures verify persistence/restart, prior-note preservation, and rejection of output after a concurrent correction | No real external provider request or real credential was used |
| Gates | 87 Rust tests pass with five opt-in/subprocess entries ignored in the ordinary suite; real service refinement/Apple checks pass separately; all-target Clippy and formatting pass | Public fixtures and hardware/provider gates remain separately scoped |
| UI | Eleven Chrome fixture flows pass, including history, correction review, partial/stale notes, source navigation, provider consent, search and live partials | Browser tests substitute IPC in the test harness; native click automation remains unavailable |
| Packaging | Release custom-protocol bundle builds; strict recursive signature verification passes; both helper bytes match built resources; packaged Apple availability reports available; isolated release launch logs frontend IPC initialization | Local ad-hoc signing only; distribution notarization remains open |

The Swift summary helper weak-links FoundationModels and retains macOS 14.4 as its minimum deployment target. Building it requires an SDK containing FoundationModels. Runtime Apple summaries require a supported, enabled and ready Apple Intelligence model; the app does not enable OS features or choose an alternate provider automatically.

No private meeting content, owner/device names or credentials are included in these records. Public/fictional fixtures and runtime artifacts remain outside the repository.

## Remaining acceptance work

This advances M2a and M3 and implements the active-run/correction/search part of storage. It does not complete the approved milestones. Open work includes M0 long-call drift and device/permission/sleep gates, measured speech quality/latency/performance, live VAD, diarization, AEC and reversible bleed handling, Rust-core playback, speaker memory, model profiles, encryption/retention, meeting detection and Windows parity.

Refinement uses bounded 29-second speech epochs with independent track state; a hard nonoverlapping split may affect a boundary word. Native acoustic end timing can have 80 ms granularity. The current implementation does not claim forced alignment, individual speaker identification, complete hardware acceptance or full roadmap completion.

## Final implementation review remedies

Claude Opus reviewed the completed increment and identified summary/transcript health coupling, lost transcript warnings, unnecessary metadata-triggered staleness, and a model-thread spawn cleanup edge case. The remedies persist summary failures separately, restore source status/warnings after summary operations, keep title/personal-note edits outside the transcript revision token, and clear model busy/error state if its worker cannot start. Completed historical runs remain selectable; abandoned unpublished runs are omitted. Markdown and structured JSON share the same final partial flag after recording caveats are added.

Service regressions cover provider failure/cancellation, stale publication after correction, successful summaries preserving transcript warnings, metadata edits during generation, and persisted independent error state. The additional browser flow verifies that a summary failure does not offer transcript repair or abandoned partial revisions. A macOS synthetic HTTP fixture race was traced to inherited nonblocking accepted sockets and corrected without changing product transport behavior.

The same Claude Opus reviewer verified every original remedy in a focused follow-up. Its low-severity consistency suggestions were also applied: new transcript publication clears an older summary failure, summary-start failure preserves observed status, and shutdown suppresses the narrow automatic-refinement-start warning race.

## Installed outcome

The final release replaced the idle copy at `~/Applications/VanTranscribe.app`; its previous bundle was retained outside the repository. Signature verification passed before replacement. The updated installed process remained running and logged embedded frontend IPC initialization. A metadata-only status check confirmed that no recording was active. The separate refinement pack remains an explicit Settings installation, and AI notes remain Off until the user configures a provider. The overall goal remains active for the remaining approved roadmap.

Subsequent native playback, live VAD and experimental speaker integration evidence is recorded in [the next checkpoint](2026-11-06-playback-and-speaker-validation.md).
