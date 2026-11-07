# Calibration-only extractor comparison

- Date: 2026-11-07
- Feature: evaluation
- Related code: `xtask/src/speaker_calibration.rs`, `crates/vt-diarize/src/voice_profile.rs`

## Context

The first pinned ERes2Net experiment achieved 113/150 known recognition, below the frozen 90% calibration gate. Held-out data was not scored. The accepted design already lists WeSpeaker ResNet34 as an alternative; independent Anthropic/Codex design reviews remain the implementation basis.

## Decisions

Add explicit calibration-only mode that skips held-out inference even when calibration passes. Compare the design-listed English VoxCeleb ResNet34 using a distinct extraction/weight identity and dimensions, while preserving original app defaults and stored-print versions. Pin downloaded bytes before scoring and record that individual release API digest fields are null while the separate publisher checksum matches the independent download. Share bounded preprocessing and mean normalization. Do not emit a release pin or enable app matching from research results.

## Alternatives Considered

- Use default full mode for exploratory comparison: it would consume held-out data whenever a candidate passes calibration.
- Relax thresholds or drop difficult identities to make the original result pass: changes the scientific question and does not improve model separation.
- Replace the app's existing extractor/version immediately: would silently stale or reinterpret stored enrollments before quality is known.

## Consequences

- Positive: actual model comparisons can proceed without held-out tuning or app-data migration.
- Negative: calibration gains alone cannot establish production eligibility, channel coverage or full milestone acceptance.
