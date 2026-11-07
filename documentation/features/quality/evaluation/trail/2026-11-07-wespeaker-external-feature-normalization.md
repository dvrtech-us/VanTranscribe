# WeSpeaker external feature normalization

- Date: 2026-11-07
- Feature: evaluation
- Related code: `crates/vt-diarize/src/voice_profile.rs`, `xtask/scripts/voice_model/prepare_wespeaker_cmn.py`

## Context

The ordinary publisher WeSpeaker model yielded 13/150 known recognition under the frozen error limits. Physical graph/metadata inspection and official inference source identify missing external feature mean normalization: the graph begins convolution without input CMN, the reference subtracts each feature's time mean, and sherpa only does so when an absent metadata flag requests it.

## Decisions

Preserve the original source artifact and failed result. Create a distinct `wespeaker-resnet34-cmn-v1` candidate by appending only `feature_normalize_type=global-mean` to ONNX metadata; every original byte, including graph and neural parameters, remains unchanged. Pin the derived container hash, transformation ID, raw extraction version and modified-artifact attribution separately. Use calibration-only mode and keep all corpus identities/windows, fitting rules and held-out non-use unchanged.

## Alternatives Considered

- Silently modify the already measured profile: would violate comparison/version provenance.
- Treat the failed score as evidence that English models are intrinsically poor: ignores a concrete integration mismatch.
- Change a threshold, corpus identity or waveform to improve the result: does not repair the missing upstream operation.

## Consequences

- Positive: reproducible correction for a verified preprocessing mismatch without parameter or corpus tuning.
- Negative: other native/reference feature differences remain; actual quality still requires measurement and this research never itself enables app recognition.
