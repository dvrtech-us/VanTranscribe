# Reference WeSpeaker frontend calibration

- Date: 2026-11-07
- Feature: evaluation
- Related code: `crates/vt-speaker-frontend`, `crates/vt-diarize/src/voice_profile.rs`, `xtask/src/speaker_calibration.rs`, `xtask/scripts/voice_frontend/compare_reference_features.py`

## Context

The CMN WeSpeaker candidate reached 130/150 known recognition. Three frontend differences remained versus reference ONNX inference: Povey versus Hamming, `snip_edges=false` versus true, and a 7,600 Hz cutoff versus 8 kHz. The public sherpa C API cannot set those options.

## Decisions

Add a pinned C++ adapter that builds `OnlineStream` from an explicit `FeatureExtractorConfig` and passes it to `SpeakerEmbeddingExtractor::Compute`. Compare raw log-mel frames with `torchaudio.compliance.kaldi.fbank` before any identity score, with the maximum absolute error limit fixed at 0.001. Give the passing frontend its own profile, `wespeaker-resnet34-reference-frontend-v1`, on the same CMN weights. Refuse that profile in `VoiceEmbedder::new_with_profile`. Run `voice-eval --calibration-only` on the frozen windows. Leave held-out unscored and leave application matching off.

## Alternatives Considered

- Score another model before measuring the known frontend mismatch.
- Change the already published CMN profile in place.
- Loosen the feature-error limit or the recognition grid after seeing scores.
- Open the held-out set because calibration recognition passed 90%.

## Consequences

- Positive: raw features match the reference recipe within the predeclared limit, and calibration recognition is 138/150 with no wrong-known and no unknown false identification.
- Negative: held-out performance is still unknown, and this research profile does not enable application recognition.
