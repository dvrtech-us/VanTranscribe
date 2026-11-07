# First Desktop Runtime Checkpoint

- Date: 2026-11-06 (Central)
- Objective: have a working app
- Goal status: active; the full approved roadmap remains the target
- Related: `design.md`, existing architecture decisions and feature documentation

This records the first installed-app checkpoint. Subsequent refinement, immutable-run/search and AI-notes implementation evidence is recorded in [the next checkpoint](2026-11-06-refinement-and-ai-notes-validation.md).

## Current artifact

The Apple Silicon release application is built at `target/release/bundle/macos/VanTranscribe.app` and installed at `~/Applications/VanTranscribe.app`. The installed application has been launched. Its embedded Svelte frontend completed native IPC initialization without a development server. The application uses the normal local data directory and a verified local English model; it did not start a recording on ordinary launch.

The app supports microphone/system recording, Me/Them live English transcription, pause/resume, durable audio, a persistent meeting library, notes/bookmarks, title editing, five text export formats and model setup. Failed or interrupted saved transcription can be retried without discarding the previous partial transcript before replacement succeeds.

## Inspected evidence

| Requirement | Evidence | Limit |
|---|---|---|
| Build a real desktop app | `npm run build` created the optimized macOS bundle | Other platforms are not implemented |
| Package an independent frontend | Explicit custom-protocol feature; installed app logged interface initialization through native IPC | Native click automation was unavailable; browser flow tests substitute IPC only in the test harness |
| Local speech setup | Pinned cached archive installed and verified in the normal model directory | English streaming pack only |
| Native capture and storage | Short packaged checks persisted matching Mic/System frame totals and source-clock observations | No 30-minute meeting, 60-minute drift or long soak gate claimed |
| Real speech recognition | Offline public fixture and two-track offsets were exercised; packaged System track recognized external fixture speech | Recognition across a configuration transition was not exact; full WER/latency budgets remain unproven |
| Discontinuities | A real source-epoch change generated explicit gaps and continued recording; unchanged notification filtering reduced repeated rebuilds | Full unplug/HFP/sleep/permission matrix remains open |
| Durability | Real SIGKILL tests at four publication boundaries; independent FLAC decode; repeated recovery and corruption tests | Synthetic/process-death evidence does not establish all hardware/storage/power-loss bounds |
| Session correctness | Tests cover pause/resume, persistence, failures, staged replay, shutdown cancellation, epoch transitions and instance exclusion | Additional native UI/manual QA remains useful |
| UI flows | Three Chrome tests cover consent/device choice, controls/notes/export, setup cancellation and narrow layout; Svelte check is clean | Browser tests do not claim native capture behavior |
| Release safety | Strict recursive signature verification passed; release executable contains no capture-check diagnostic path | Local ad-hoc signing; no distribution notarization |
| Review | Claude Opus reviewed code, reported six issues and verified their targeted remedies | The follow-up was focused, not a new review of the entire roadmap |

The final Rust suite passed 41 tests, with the subprocess helper and explicit real-model test ignored in the ordinary suite. The helper is invoked by the SIGKILL test, and the real-model check was run separately. Swift/C builds and native ring/fingerprint checks passed. Clippy, formatting, browser tests and production frontend build passed.

Raw microphone content, transcript text, device-owner names and machine-specific test paths are not included in documentation. Runtime artifacts stay outside the repository.

## Completion audit against the approved roadmap

| Milestone | State | Remaining acceptance evidence/work |
|---|---|---|
| M0 reliable capture and benchmarks | Partial, real capture works | Long-call drift, full device/permission/sleep matrix, loss envelope, model benchmarks and self-exclusion/chirp gates |
| M1 live transcript | Usable first implementation | VAD, quality/latency gates, complete processing-run store and Rust-core playback |
| M2a refinement | Planned; failure-safe replay repair exists | Batch Parakeet, immutable runs, correction overlays and atomic active revision/FTS publication |
| M2b diarization | Planned | Online/offline speaker turns, calibrated clustering and reconciliation |
| M2c echo | Planned | AEC, route handling, reversible bleed annotations and double-talk evaluation |
| M3 summaries | Planned | Provider settings, Apple/local/cloud implementations, map-reduce and grounded source links |
| M4 memory/search/privacy | Planned | Voiceprints/consent, FTS, encryption/retention and advisory meeting detection |
| M5 Windows | Planned | Native Windows backend, packaging and actual Windows parity validation |

The presence of a runnable macOS app is concrete progress toward the objective; it is not proof that every numbered design goal or milestone is complete. Continue with refinement/persistence and summary integration while retaining the independent capture, diarization and echo gates.
