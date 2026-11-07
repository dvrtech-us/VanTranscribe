//! Explicit, bounded native reference-style WeSpeaker frontend research.
//!
//! No model download, network, logging, device or storage-writing API is present.
//! Input is normalized mono PCM at exactly 16 kHz; the adapter does not alter it.
//! Native classes use full pinned upstream headers and one matching runtime.
//!
//! The owned native extractor is deliberately not transferable between threads:
//! ```compile_fail
//! fn require_send<T: Send>() {}
//! require_send::<vt_speaker_frontend::ReferenceEmbedder>();
//! ```
#![deny(unsafe_code)]
use std::{marker::PhantomData, path::Path, rc::Rc};
#[cfg(vt_reference_native)]
#[allow(unsafe_code)]
mod native;

pub const SAMPLE_RATE: usize = 16_000;
pub const MIN_SAMPLES: usize = SAMPLE_RATE;
pub const MAX_SAMPLES: usize = SAMPLE_RATE * 5;
pub const FEATURE_BINS: usize = 80;
#[cfg(all(vt_reference_native, any(test, feature = "feature-probe")))]
const MAX_FRAMES: usize = 498;
#[cfg(vt_reference_native)]
const MODEL_BYTES: u64 = 26_534_404;
#[cfg(vt_reference_native)]
const MODEL_SHA256: &str = "6928c40c3b5c695352998c261cb6f2250c4e52a991e324c0f63c2e10fe7c31f8";

#[derive(Clone, Copy, Debug, thiserror::Error, PartialEq, Eq)]
pub enum Error {
    #[error(
        "Reference frontend requires one to five seconds of finite normalized mono 16 kHz samples in [-1,1]."
    )]
    InvalidAudio,
    #[error("Reference model path must identify a readable regular UTF-8 file without NUL bytes.")]
    InvalidModelPath,
    #[error("Reference model does not match the pinned CMN model size and SHA-256.")]
    ModelPin,
    #[error("Reference frontend is unavailable on this platform or build.")]
    Unsupported,
    #[error("Reference frontend native identity does not match its pinned SDK/runtime.")]
    AbiMismatch,
    #[error("Reference frontend native operation failed.")]
    Native,
    #[error("Reference frontend has insufficient native frames for an embedding.")]
    NotReady,
}
pub type Result<T> = std::result::Result<T, Error>;

/// Owns one native extractor. Construction verifies the exact CMN ONNX file;
/// ordinary and derived models cannot be silently substituted. Higher-level
/// code separately verifies its manifest, provenance and license notices.
pub struct ReferenceEmbedder {
    #[cfg(vt_reference_native)]
    handle: native::Handle,
    _thread_bound: PhantomData<Rc<()>>,
}
impl ReferenceEmbedder {
    pub fn new(model_file: &Path) -> Result<Self> {
        #[cfg(vt_reference_native)]
        {
            let path = verified_model(model_file)?;
            native::ensure_abi()?;
            let handle = native::Handle::create(&path)?;
            Ok(Self {
                handle,
                _thread_bound: PhantomData,
            })
        }
        #[cfg(not(vt_reference_native))]
        {
            let _ = model_file;
            Err(Error::Unsupported)
        }
    }
    /// Computes RAW finite 256-dimensional native output. The caller applies its
    /// existing common L2 normalization and never persists unnamed embeddings.
    pub fn extract(&mut self, samples: &[f32]) -> Result<Vec<f32>> {
        validate_audio(samples)?;
        #[cfg(vt_reference_native)]
        {
            self.handle.extract(samples)
        }
        #[cfg(not(vt_reference_native))]
        {
            Err(Error::Unsupported)
        }
    }
}
fn validate_audio(samples: &[f32]) -> Result<()> {
    if !(MIN_SAMPLES..=MAX_SAMPLES).contains(&samples.len())
        || samples
            .iter()
            .any(|v| !v.is_finite() || !(-1.0..=1.0).contains(v))
    {
        return Err(Error::InvalidAudio);
    }
    Ok(())
}
#[cfg(vt_reference_native)]
fn verified_model(path: &Path) -> Result<std::ffi::CString> {
    use sha2::{Digest, Sha256};
    use std::{fs, io::Read};
    let text = path.to_str().ok_or(Error::InvalidModelPath)?;
    if text.is_empty() || text.as_bytes().contains(&0) {
        return Err(Error::InvalidModelPath);
    }
    let meta = fs::symlink_metadata(path).map_err(|_| Error::InvalidModelPath)?;
    if !meta.is_file() || meta.is_symlink() {
        return Err(Error::InvalidModelPath);
    }
    if meta.len() != MODEL_BYTES {
        return Err(Error::ModelPin);
    }
    let path = path.canonicalize().map_err(|_| Error::InvalidModelPath)?;
    let mut file = fs::File::open(&path).map_err(|_| Error::InvalidModelPath)?;
    let mut hash = Sha256::new();
    let mut count = 0u64;
    let mut buffer = [0; 65_536];
    loop {
        let n = file
            .read(&mut buffer)
            .map_err(|_| Error::InvalidModelPath)?;
        if n == 0 {
            break;
        }
        count += n as u64;
        if count > MODEL_BYTES {
            return Err(Error::ModelPin);
        }
        hash.update(&buffer[..n]);
    }
    if count != MODEL_BYTES || format!("{:x}", hash.finalize()) != MODEL_SHA256 {
        return Err(Error::ModelPin);
    }
    std::ffi::CString::new(path.to_str().ok_or(Error::InvalidModelPath)?)
        .map_err(|_| Error::InvalidModelPath)
}

/// Row-major RAW log-mel features before the embedding extractor's CMN. This
/// developer probe is intended only for explicit locally generated synthetic
/// signals; no app route or automatic file export calls it.
#[cfg(any(test, feature = "feature-probe"))]
pub struct FeatureMatrix {
    pub frames: usize,
    pub bins: usize,
    pub values: Vec<f32>,
}
#[cfg(any(test, feature = "feature-probe"))]
pub fn reference_features(samples: &[f32]) -> Result<FeatureMatrix> {
    validate_audio(samples)?;
    #[cfg(vt_reference_native)]
    {
        native::ensure_abi()?;
        native::features(samples)
    }
    #[cfg(not(vt_reference_native))]
    {
        Err(Error::Unsupported)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invalid_geometry_and_amplitude_are_rejected_without_native_objects() {
        assert_eq!(validate_audio(&[]), Err(Error::InvalidAudio));
        assert_eq!(
            validate_audio(&vec![0.; MIN_SAMPLES - 1]),
            Err(Error::InvalidAudio)
        );
        assert_eq!(
            validate_audio(&vec![0.; MAX_SAMPLES + 1]),
            Err(Error::InvalidAudio)
        );
        for bad in [f32::NAN, f32::INFINITY, -1.0001, 1.0001] {
            let mut samples = vec![0.; MIN_SAMPLES];
            samples[0] = bad;
            assert!(matches!(
                reference_features(&samples),
                Err(Error::InvalidAudio)
            ));
        }
    }
    #[cfg(vt_reference_native)]
    #[test]
    fn wrong_or_unsafe_model_paths_fail_before_native_construction() {
        let folder = tempfile::tempdir().unwrap();
        assert!(matches!(
            ReferenceEmbedder::new(folder.path()),
            Err(Error::InvalidModelPath)
        ));
        assert!(matches!(
            ReferenceEmbedder::new(Path::new("invalid\0model")),
            Err(Error::InvalidModelPath)
        ));
        let model = folder.path().join("model.onnx");
        std::fs::File::create(&model)
            .unwrap()
            .set_len(MODEL_BYTES)
            .unwrap();
        assert!(matches!(
            ReferenceEmbedder::new(&model),
            Err(Error::ModelPin)
        ));
        #[cfg(unix)]
        {
            let link = folder.path().join("alias.onnx");
            std::os::unix::fs::symlink(&model, &link).unwrap();
            assert!(matches!(
                ReferenceEmbedder::new(&link),
                Err(Error::InvalidModelPath)
            ));
        }
    }
    #[cfg(vt_reference_native)]
    #[test]
    fn actual_native_identity_and_synthetic_stream_lifecycle_match_snipped_frame_geometry() {
        native::ensure_abi().unwrap();
        for size in [MIN_SAMPLES, MAX_SAMPLES] {
            let samples = (0..size)
                .map(|n| ((n as f32 * 0.17).sin()) * 0.3)
                .collect::<Vec<_>>();
            let a = reference_features(&samples).unwrap();
            let b = reference_features(&samples).unwrap();
            assert_eq!(a.frames, 1 + (size - 400) / 160);
            assert_eq!(a.bins, 80);
            assert_eq!(a.values.len(), a.frames * a.bins);
            assert!(a.values.iter().all(|v| v.is_finite()));
            assert_eq!(a.values, b.values);
        }
    }
    #[cfg(vt_reference_native)]
    #[test]
    #[ignore = "requires VT_REFERENCE_FRONTEND_MODEL exact CMN ONNX file; uses synthetic signals only, no identity score/device"]
    fn pinned_native_extractor_owns_and_releases_custom_streams() {
        let model = std::env::var_os("VT_REFERENCE_FRONTEND_MODEL").unwrap();
        let mut engine = ReferenceEmbedder::new(Path::new(&model)).unwrap();
        let signal = (0..MIN_SAMPLES)
            .map(|n| (n as f32 * 0.1).sin() * 0.2)
            .collect::<Vec<_>>();
        let first = engine.extract(&signal).unwrap();
        let second = engine.extract(&signal).unwrap();
        assert_eq!(first.len(), 256);
        assert!(first.iter().all(|v| v.is_finite()));
        assert_eq!(first, second);
        drop(engine);
        let _fresh = ReferenceEmbedder::new(Path::new(&model)).unwrap();
    }
}
