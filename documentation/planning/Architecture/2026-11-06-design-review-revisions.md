# Design Review Revisions (design.md v0.1 → v0.2)

- Date: 2026-11-06
- Related: ../../../design.md, 2026-11-06-initial-architecture-decisions.md
- Reviewers: Claude Opus (Anthropic) and OpenAI Codex, independent adversarial reviews

## Context
The project owner's working rules require two adversarial reviews from different model vendors before any work starts. Both reviews were run against design.md v0.1, and their findings were discussed with the owner. The owner decided to:
1. **Keep the full roadmap scope**, against both reviewers' advice to cut v1.
2. **Apply all agreed technical fixes.**

## Decisions

| # | Decision | design.md § |
|---|---|---|
| 1 | Use the **official upstream `sherpa-onnx` Rust crate**. `sherpa-rs` is deprecated and read-only. This **supersedes decision (d)** in 2026-11-06-initial-architecture-decisions.md, which named `sherpa-rs` / own FFI. | §6 |
| 2 | Live STT default is **Streaming Zipformer EN**. Moonshine is excluded from defaults, because sherpa-onnx exposes Moonshine v2 offline only and the license for non-English models is unclear. | §6, §15, §16 |
| 3 | CPU execution provider by default. CoreML and DirectML are opt-in after M0 benchmarks. Borrowed performance figures (FluidAudio CoreML) are not commitments. | §6, §15 |
| 4 | Self-exclusion uses **process AudioObjectIDs, including WebKit helper processes**. Playback runs from the Rust core and is disabled while recording. | §5.2, §13 |
| 5 | **Recording is isolated from inference.** Durable 2 s FLAC chunks with fsync and a manifest, explicit gaps, crash recovery, compaction to Opus. No plaintext temp WAVs. | §5.3, §5.6 |
| 6 | Hardware timestamps (`mHostTime`, sample index, epoch). The mic and tap share one aggregate device with drift compensation. Streaming STT is fed incrementally. ONNX threads are budgeted globally. | §4.3, §5.3 |
| 7 | Lifecycle state machine for device changes, HFP, sleep/wake, permission loss, disk-full, and crash. | §5.5 |
| 8 | The Mic track follows the meeting app's actual input device. Never open a Bluetooth mic unless the user chooses it. | §5.2 |
| 9 | AEC is used only when the meeting renders to the device the mic hears. It is reset on route change and replayed offline in refinement. VPIO is dropped. Bleed is a **reversible annotation** (≥ 4 tokens), not a deletion. | §5.4 |
| 10 | Speaker turns come from separate speaker-change detection, not from STT segments. `unknown` and `overlap` labels. Exemplar-set clusters. Thresholds calibrated per embedding version. | §7.1 |
| 11 | Constrained clustering is implemented in `vt-diarize`, because sherpa-onnx clustering lacks soft priors. Split/merge-aware reconciliation. Identity constraints with provenance. | §7.2, §7.3 |
| 12 | Voiceprints are stored per channel type, with a versioned embedding hash, a second-best margin, gallery-size-aware calibration, a shared-room flag, and 12-month expiry. Unnamed embeddings are never persisted. | §7.4 |
| 13 | **Immutable processing runs** with `active_run_id`, correction overlays, atomic publish including FTS, and stale-summary marking. | §8, §12 |
| 14 | Apple FM: 4,096-token session budget, 1.2–1.5K-token chunks, recursive reduce, a Swift-side job API with callbacks and cancellation, weak-linking, evaluation of `fm-rs`. | §9 |
| 15 | Permission state is reported as "uncertain", never "denied". Onboarding verifies capture with an external-process chirp. | §10.1 |
| 16 | **Recording consent and biometric consent are separate.** Per-person written-consent attestation, a published retention schedule, deletion propagation, and redacted logs. | §11 |
| 17 | Meeting detection is advisory only: helper-to-parent process mapping, browser mic use is not treated as a meeting, and there is no unattended auto-stop. | §14 |
| 18 | Windows target is **Windows 11 only** (process loopback needs build 20348+). Feature probe and a dedicated COM thread. Phi Silica is offered only in the MSIX build. | §5.2, §17 |
| 19 | Roadmap: M0 becomes "reliable capture + benchmarks". M2 is split into independently gated M2a (refinement), M2b (diarization) and M2c (echo). | §18 |
| 20 | Corrections: disk math (24 kbps ≈ 10.8 MB/h/track), Lite pack size, pyannote seg-3.0 is MIT (CNRS), Sortformer license still to confirm. | §15, §16 |

## Alternatives Considered
- **Lean v1** (both reviewers' recommendation): Windows, cloud STT, live diarization, AEC and voiceprints would move out of v1. Rejected by the owner, who kept the full scope. The risk is tracked as R15, mitigated by independent milestone gates.
- **Own FFI over the sherpa-onnx C API.** Unnecessary now that an official Rust crate exists.
- **Opus for the live recording archive.** Rejected because it is lossy and resampled. Opus is used only for post-refinement compaction.
- **Deleting bleed text.** Rejected because it risks destroying genuine short replies.

## Consequences
- **Positive:**
  - Audio durability is designed rather than hoped for.
  - User corrections survive re-processing.
  - The BIPA posture is defensible.
  - Performance targets will be grounded in measurement.
- **Negative:**
  - The data model and the M0 milestone are larger.
  - Full-scope v1 remains a schedule risk for a solo developer.
  - Several claims are still unverified (listed in design.md §20).

## Disagreements between reviewers
- **Moonshine license:** Codex says the current streaming models are MIT. Opus says the non-English models are under a non-commercial community license. Both can be true, since the English models are MIT. Moonshine is kept out of the defaults until this is verified.
