# Research policy freeze durability

- Date: 2026-11-07
- Feature: evaluation
- Related code: `xtask/src/speaker_calibration.rs`, `xtask/scripts/voice_corpus/`

## Context

The final Opus implementation review found no blocking defects and identified narrow publication/provenance hardening before the first native identity-scoring run. Accepted Anthropic/Codex design reviews remain unchanged.

## Decisions

Synchronize both the frozen candidate policy file and its directory entry before held-out inference. Filter every preparation candidate row to selected projects, and reject unequal frozen/export partition counts before pairing. Preserve the already frozen input and fitting algorithm. Cite real results only when the report verifies both provenance files. Disclose any future invocation that reuses held-out data; the one-shot rule is procedural across runs.

## Alternatives Considered

- Synchronizing the directory only when publishing the final report: leaves a power-loss window after held-out scoring starts.
- Treating observed zero errors as proof of a 2% error target: impossible at 42 unknown and 145 known held-out trials, even before accounting for correlated subjects/projects.

## Consequences

- Positive: durable freeze ordering and stricter preparation checks without changing frozen samples or scoring.
- Negative: this public-corpus experiment remains non-production-eligible; larger independent and representative acceptance evidence is required.
