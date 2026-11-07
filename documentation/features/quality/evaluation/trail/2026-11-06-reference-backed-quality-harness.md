# Reference-backed quality harness

- Date: 2026-11-06
- Feature: evaluation
- Related code: `xtask/`, `.cargo/config.toml`

## Context

The approved design requires WER, DER/JER, bleed/short-reply preservation and latency/resource evidence. Native smoke tests prove execution but do not establish corpus quality or reference-machine acceptance.

## Decisions

Provide an offline Rust `cargo xtask eval --manifest --output` command with explicit corpus provenance, required references for supplied metric families, source hashes and compact JSON. Keep dependencies to existing workspace packages. Include a fictional corpus with hand-computed scores and an actual CLI subprocess test.

Pin pyannote-compatible metric semantics rather than relying on an ambiguous collar label: `collar=0.25` has total width 250 ms, excluding ±125 ms. Score overlap and apply one duration-maximizing Hungarian assignment per recording/channel. Use pyannote's duration mapping for JER, not a separate IoU mapping. Document empty-reference behavior and deterministic mapping/timing conventions.

Report actual annotation rates, reference-backed visible duplicates and lost genuine short replies separately. Require measured event clocks for latency and comparable metadata for resource aggregation. Missing evidence remains unavailable. Every report explicitly declines automatic milestone acceptance.

## Alternatives Considered

- Treating a 0.25 parameter as ±250 ms: rejected because that does not match pyannote's parameter semantics.
- Mapping each time slice or pooling identities across meetings: rejected because it hides confusion or creates false cross-meeting identity requirements.
- Inferring reference labels/bleed from hypothesis text: rejected because it invents ground truth.
- Claiming corpus acceptance from synthetic fixtures: rejected; checked-in data validates arithmetic only.
- Requiring Python/scipy/pyannote installation at runtime: avoided to keep the developer command offline and lightweight; primary upstream algorithms are cited and own Rust logic is tested against hand-computable cases.

## Consequences

- Positive: reproducible evidence artifacts with fixed definitions, strict input validation, audit mappings/counts and no audio/device/network side effects.
- Negative: real consented corpus/hardware measurements and CI scheduling remain necessary; exact SciPy tie-breaking is not claimed; explicit gallery/input/work limits may require corpus units to be split without changing their reference correspondence.

### Open-set speaker-memory evaluation

A separate feature-gated research command reuses the pinned production embedding extractor and the pure matching ranker without invoking app enrollment or creating a release calibration. It fixes people/projects and source-frame provenance before scoring, fits only on calibration, freezes the candidate before held-out extraction, counts identity errors without Hungarian relabeling and includes abstentions. Data acquisition is an explicit bounded official-source companion script. Availability/format failures are recorded before a deterministic final protocol, without observing identity scores or reducing the fifty-person gallery. Native vectors remain transient; all reports are research-only and cannot authorize app recognition.

A fault-injected final-publication regression reproduced an erroneous success marker when incomplete-marker cleanup failed. The final order now clears earlier state before atomic success publication and retains error/incomplete evidence on failure. Candidate-policy fitting and scores are unchanged.
