# Parakeet v3 offline refinement

- Date: 2026-11-06
- Feature: batch-refinement
- Related code: `crates/vt-stt/src/refine.rs`, `crates/vt-stt/src/refinement_models.rs`, `crates/vt-stt/src/models.rs`

## Context

Post-recording refinement requires a real high-accuracy offline model with track isolation, bounded native memory and acoustic word alignments. Reusing live Zipformer and renaming its output would not implement the approved design.

## Decisions

Use the canonical sherpa-onnx Parakeet-TDT 0.6B v3 INT8 NeMo transducer and Silero VAD with the statically linked 2.0.1 runtime. Pin official release archive/VAD hashes and independently verified extracted weight hashes. Preserve NVIDIA CC-BY-4.0 attribution, Silero MIT and native notices in the bundle and install manifest. Share the existing safe archive extractor without weakening its checks.

Maintain per-track persistent sinc resampling and independent native VAD. Explicitly flush/reset VAD every 29 seconds because upstream's max-duration field adjusts its threshold rather than guaranteeing a hard region bound. Submit each audio frame once; use 200ms decoder-only zero context. Merge native BPE acoustic timestamps into words, recording a frame-level end estimate when native durations are unavailable.

## Alternatives Considered

- Reusing streaming Zipformer: rejected because it is not the offline refinement model.
- Full-track decoding: rejected because multi-hour inputs violate memory bounds.
- Uniformly dividing text into word times: rejected because these are not acoustic alignments.
- Overlapping forced epochs: avoided to prevent duplicated ownership of boundary audio and repeated output words; hard-cut boundary quality is a documented tradeoff.

## Consequences

- Positive: actual local Parakeet inference, explicit reproducible setup and licensing, bounded input/model waveform work and independent clocks.
- Negative: roughly 465MiB setup download and 640MiB installed weights; CPU inference/model initialization costs; words crossing a forced epoch can lose context; native token timing has 80ms frame granularity and does not constitute forced alignment.
