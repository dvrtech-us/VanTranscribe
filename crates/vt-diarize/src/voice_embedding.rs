//! Explicit, bounded recomputation for consented enrollment. No identity lookup,
//! calibration claim, persistence or network access occurs here.
use crate::{DiarizeError, EMBEDDING, Result, VoiceEmbeddingProfile, cluster::normalize, models};
use sha2::{Digest, Sha256};
use sherpa_onnx::{LinearResampler, SpeakerEmbeddingExtractor, SpeakerEmbeddingExtractorConfig};
use std::path::Path;
use zeroize::Zeroizing;

/// The identity includes pinned weights, feature/preprocessing contract, input
/// resampler, normalization and the explicitly absent identification calibration.
/// A future calibrated profile must have a different identity.
pub fn voice_embedding_version() -> String {
    format!(
        "{:x}",
        Sha256::digest(format!("{}calibration=unavailable-v1\n", embedding_contract()).as_bytes())
    )
}

/// Raw extraction identity for research calibration. It is not an identification
/// version or authority to reuse an existing, differently calibrated voiceprint.
pub fn voice_embedding_contract_sha256() -> String {
    format!("{:x}", Sha256::digest(embedding_contract().as_bytes()))
}

fn embedding_contract() -> String {
    format!(
        "vantranscribe-voice-enrollment-v1\nweights={}\nsherpa-onnx=2.0.1;eres2net;native-features;mono-mean\nrate=16000;resampler=sherpa-linear-per-span\nnormalization=individual-l2-f32;mean-duration-weighted-f64-l2\n",
        models::embedding_weight_hash()
    )
}

pub struct VoiceEmbedder {
    extractor: SpeakerEmbeddingExtractor,
}
impl VoiceEmbedder {
    /// Verifies the installed pinned model pack before initializing native code.
    pub fn new(model_dir: &Path) -> Result<Self> {
        Self::new_with_profile(model_dir, VoiceEmbeddingProfile::Eres2NetV1)
    }
    /// Explicit research selection only. Existing callers keep the original
    /// profile; no model setting or stored voiceprint is migrated by this API.
    pub fn new_with_profile(model_dir: &Path, profile: VoiceEmbeddingProfile) -> Result<Self> {
        if profile == VoiceEmbeddingProfile::WeSpeakerResNet34ReferenceFrontendV1 {
            return Err(DiarizeError::Invalid(
                "reference frontend profile is extracted only by the explicit research adapter",
            ));
        }
        profile.verify_model(model_dir)?;
        let config = SpeakerEmbeddingExtractorConfig {
            model: Some(
                model_dir
                    .join(EMBEDDING)
                    .to_str()
                    .ok_or(DiarizeError::Model)?
                    .to_owned(),
            ),
            num_threads: 1,
            debug: false,
            provider: Some("cpu".into()),
        };
        let extractor = SpeakerEmbeddingExtractor::create(&config).ok_or(DiarizeError::Engine)?;
        if extractor.dim() as usize != profile.dimensions() {
            return Err(DiarizeError::Model);
        }
        Ok(Self { extractor })
    }
    /// One clean contiguous source span, one to five seconds. Resampling state
    /// cannot leak across a gap, device epoch, or another selected speech span.
    pub fn extract(&mut self, samples: &[f32], rate: u32) -> Result<Vec<f32>> {
        validate_span(samples, rate)?;
        let resampled = Zeroizing::new(if rate == 16_000 {
            Vec::new()
        } else {
            LinearResampler::create(rate as i32, 16_000)
                .ok_or(DiarizeError::Engine)?
                .resample(samples, true)
        });
        let audio = if rate == 16_000 { samples } else { &resampled };
        let stream = self.extractor.create_stream().ok_or(DiarizeError::Engine)?;
        stream.accept_waveform(16_000, audio);
        stream.input_finished();
        if self.extractor.is_ready(&stream) {
            normalize(
                self.extractor
                    .compute(&stream)
                    .ok_or(DiarizeError::Engine)?,
            )
        } else {
            Err(DiarizeError::Engine)
        }
    }
}
fn validate_span(samples: &[f32], rate: u32) -> Result<()> {
    if !(8_000..=192_000).contains(&rate)
        || samples.len() < rate as usize
        || samples.len() > rate as usize * 5
        || samples.iter().any(|v| !v.is_finite())
    {
        return Err(DiarizeError::Invalid(
            "enrollment requires one to five seconds of finite mono source audio",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn voiceprint_version_is_stable_and_distinct_from_uncalibrated_turn_version() {
        let first = voice_embedding_version();
        assert_eq!(first.len(), 64);
        assert!(first.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(first, voice_embedding_version());
        assert_ne!(first, crate::EMBEDDING_VERSION);
        // A new raw-contract accessor must not silently stale enrolled prints.
        assert_eq!(
            first,
            "64e49963b30d2f237d53cfd2dabc5cafbd7eaed9eb2ed52bc1e63dd757c2d62e"
        );
        assert_eq!(
            voice_embedding_contract_sha256(),
            "d0e9574d06f367f35d6ad1ab4e8b2a6c13c2e586ed56ff159c95f1ceb6925928"
        );
    }
    #[test]
    fn reference_frontend_profile_never_opens_the_default_sherpa_stream() {
        let result = VoiceEmbedder::new_with_profile(
            Path::new("."),
            VoiceEmbeddingProfile::WeSpeakerResNet34ReferenceFrontendV1,
        );
        assert!(matches!(result, Err(crate::DiarizeError::Invalid(_))));
    }
    #[test]
    fn source_geometry_is_bounded_before_native_inference() {
        assert!(validate_span(&vec![0.0; 16_000], 16_000).is_ok());
        assert!(validate_span(&vec![0.0; 80_000], 16_000).is_ok());
        assert!(validate_span(&vec![0.0; 15_999], 16_000).is_err());
        assert!(validate_span(&vec![0.0; 80_001], 16_000).is_err());
        assert!(validate_span(&vec![0.0; 16_000], 0).is_err());
        let mut invalid = vec![0.0; 16_000];
        invalid[100] = f32::NAN;
        assert!(validate_span(&invalid, 16_000).is_err());
    }
    #[test]
    #[ignore = "requires pinned public VT_DIARIZE_CONTEXT_WAV and VT_DIARIZE_MODEL_DIR; opens no devices"]
    fn pinned_public_audio_recomputes_finite_normalized_embeddings() {
        let path = std::env::var("VT_DIARIZE_CONTEXT_WAV").unwrap();
        assert_eq!(
            format!("{:x}", Sha256::digest(std::fs::read(&path).unwrap())),
            "6bc58a4efdf20daac252b6b1502632601a71efe0308f6757dc1eda34891a7e4f"
        );
        let wave = sherpa_onnx::Wave::read(&path).unwrap();
        let mut engine = VoiceEmbedder::new(&std::path::PathBuf::from(
            std::env::var_os("VT_DIARIZE_MODEL_DIR").unwrap(),
        ))
        .unwrap();
        let first = engine
            .extract(
                &wave.samples()[..wave.sample_rate() as usize * 3],
                wave.sample_rate() as u32,
            )
            .unwrap();
        let second = engine
            .extract(
                &wave.samples()[..wave.sample_rate() as usize * 3],
                wave.sample_rate() as u32,
            )
            .unwrap();
        assert_eq!(first.len(), 512);
        assert!(first.iter().all(|v| v.is_finite()));
        assert!((first.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() - 1.0).abs() < 1e-5);
        assert!(first.iter().zip(second).all(|(a, b)| (a - b).abs() < 1e-5));
    }
}
