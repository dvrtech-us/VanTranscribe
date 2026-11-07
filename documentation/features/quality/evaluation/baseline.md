# Evaluation behavior baseline

- Evaluation reads explicit local reference/hypothesis evidence and never captures audio, downloads models/datasets, or invents missing ground truth.
- Optional production `infer` orchestration is documented separately; `eval` remains a model-free scorer, and generated public hypotheses follow the same metric/provenance rules.
- Manifest schema version is 1. Corpus ID/kind/description/provenance and recording/channel scopes are explicit.
- DER/JER use `collar=0.25` with pyannote's total-width convention: ±0.125 s per reference boundary. Overlap is scored.
- Reference/hypothesis support is clipped to the supplied UEM after collar extrusion. Hypothesis outside UEM cannot influence mapping or false alarms.
- Hungarian mapping maximizes global co-occurrence duration independently per recording/channel. Zero-weight pairs remain unmapped.
- DER is speaker-time-weighted, including overlap. JER uses the same duration-optimal assignment and averages per-reference-speaker union-normalized error.
- Source labels may be permuted. Labels/galleries from separate meetings or channels are never pooled into one assignment.
- RTTM times are canonicalized to 1 ns, and intervals are half-open. Adjacent same-speaker intervals remain active independent of source line order.
- Missing reference files/recordings, malformed JSON/RTTM/UEM, nonfinite/negative/invalid intervals, same-speaker self-overlap and unknown schema fields fail.
- An explicitly empty reference file over UEM represents known non-speech; a missing hypothesis recording represents all missed speech.
- WER normalization is mandatory and recorded. No-reference-word WER is unavailable while raw insertions remain visible.
- Live/batch WER target comparison uses only exact transcript units supplying both roles.
- Bleed annotations match reference segment IDs one-to-one; duplicate/extra/missing IDs fail. Genuine short-reply loss is counted from supplied reference labels, never text similarity.
- Latency uses actual reference/emission event pairs on a declared monotonic session clock. Quantiles use nearest rank, not interpolation.
- CPU is percent of the total machine. Resource samples with different machine/OS/profile/provider contexts cannot be averaged in one manifest.
- Omitted evidence is null/unavailable; zero examples do not establish a quality target.
- Every report states `acceptance: not_claimed`, retains input hashes/provenance and distinguishes synthetic evidence from corpus/hardware acceptance.
- Report publication cannot replace an input file. Compact JSON is synchronized and renamed from a temporary file.

| Condition | Severity | Blocks? |
|---|---|---|
| Missing/invalid supplied reference or scope | Error | Yes, report generation |
| Invalid interval, duplicate self-overlap or annotation-ID mismatch | Error | Yes |
| File changed between repeated reads | Error | Yes |
| Missing optional metric family | Unavailable | No; no comparison for that family |
| Input/gallery/alignment/dense-overlap limit exceeded | Capacity error | Yes; split reference-paired units |
| Synthetic target comparison within limits | Diagnostic only | No milestone acceptance |

Defaults/limits: 250 ms total collar, overlap retained, 256 scored identities per unit, 64 MiB per input, 200,000 RTTM rows, 100,000 words per transcript side, 100 million WER alignment cells, 20 million dense overlap pair operations. No account, HTTP route, DB schema, SQL artifact, secret or network side effect is introduced.

## Speaker identification research

- `voice-eval` and `voice-version` require explicit `inference` builds. The plain `eval` scorer stays model-free.
- Each phase compares exactly fifty unique people; identities and projects cannot cross calibration/held-out boundaries, and known enrollment/probe meetings differ. Named truth is fixed, never permutation-mapped.
- Source windows are hash-verified, one-to-five-second PCM16 mono WAVs with exact original frame geometry. Enrollment/probe minima are twenty/fifteen seconds. No source frame is reused or overlaps globally.
- Calibration fitting uses only calibration scores, a predeclared fixed grid and separate known-wrong/unknown-false constraints. Its complete candidate policy and containing directory are synchronized, and the policy is hashed, before held-out extraction. If calibration recognition is below 90%, held-out remains unscored.
- No native embedding is serialized or cached. App storage, Keychain, capture, output, network and release-calibration installation are absent from the research command.
- Reports separate observed targets from statistical acceptance, include abstentions in the known denominator and disclose correlated trials. Missing evidence never becomes success.
- A failed publication retains incomplete/error evidence. `COMPLETE` is published atomically only after the report and earlier cleanup succeed; publication errors attempt to remove any success marker.
- Research results do not generate a release pin, change the current enrollment version or enable application matching. Native and injected test engines are visibly distinguished.

- Explicit calibration-only mode never extracts/ranks held-out vectors or publishes a held-out candidate-policy file, even when calibration passes. Omitted mode retains full behavior. Reports distinguish selected mode from calibration-policy availability.
- Extractor profile, raw contract and vector dimensions must agree. Default ERes2Net enrollment/model identity is unchanged. Alternate English WeSpeaker research uses a distinct pinned 256-dimensional contract and cannot authorize old-print reuse or runtime recognition.

- The separately versioned CMN research candidate adds only native metadata activating external per-feature time mean subtraction required by the original WeSpeaker graph. Original publisher bytes and both earlier failed results remain preserved. Derived model bytes, transformation identity, profile and raw contract must match; the original profile is never silently corrected in place.
- `wespeaker-resnet34-reference-frontend-v1` is a distinct contract over those same CMN bytes. It uses the pinned Hamming / snipped-edge / Nyquist frontend. `VoiceEmbedder::new_with_profile` rejects it. A calibration-only pass does not extract held-out identities, emit a release pin, or enable application matching.
