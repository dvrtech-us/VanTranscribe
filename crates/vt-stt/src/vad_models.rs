//! Explicit standalone setup for the pinned upstream 16kHz Silero v4 export.
use crate::{
    Result, SttError,
    models::{cancelled, hash_file, install_lock, sync_dir},
    refinement_models::download,
};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};

pub const VAD_MODEL_ID: &str = "sherpa-onnx-silero-vad-v4";
pub const VAD_MODEL_FILE: &str = "silero_vad.onnx";
pub const VAD_MODEL_SOURCE: &str = crate::refinement_models::VAD_SOURCE;
pub const VAD_MODEL_BYTES: u64 = crate::refinement_models::VAD_BYTES;
pub const VAD_MODEL_SHA256: &str = crate::refinement_models::REFINEMENT_FILES[4].2;
pub const VAD_MODEL_LICENSE: &str = "MIT";
#[derive(Serialize, Deserialize)]
struct Manifest {
    id: String,
    source: String,
    bytes: u64,
    sha256: String,
    license: String,
    license_text: String,
    attribution: String,
    conversion: String,
    acquisition_terms: String,
}
#[derive(Clone, Debug)]
pub struct VadModelManager {
    root: PathBuf,
}
impl VadModelManager {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }
    pub fn model_dir(&self) -> PathBuf {
        self.root.join(VAD_MODEL_ID)
    }
    pub fn model_path(&self) -> PathBuf {
        self.model_dir().join(VAD_MODEL_FILE)
    }
    /// Cheap catalog/file metadata check; construction and explicit verify hash.
    pub fn installed(&self) -> bool {
        self.verify_inner(false, &AtomicBool::new(false)).is_ok()
    }
    pub fn verify(&self) -> Result<()> {
        self.verify_inner(true, &AtomicBool::new(false))
    }
    pub fn install(
        &self,
        progress: impl Fn(u64, u64) + Send,
        cancel: Arc<AtomicBool>,
    ) -> Result<()> {
        fs::create_dir_all(&self.root)?;
        let _lock = install_lock(&self.root)?;
        cancelled(&cancel)?;
        if self.verify_inner(true, &cancel).is_ok() {
            progress(VAD_MODEL_BYTES, VAD_MODEL_BYTES);
            return Ok(());
        }
        cancelled(&cancel)?;
        let work = tempfile::Builder::new()
            .prefix(".vad-install-")
            .tempdir_in(&self.root)?;
        let source = work.path().join("download.onnx");
        let client = reqwest::Client::builder()
            .https_only(true)
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(120))
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                if attempt.previous().len() >= 5 {
                    return attempt.error("too many model redirects");
                }
                if attempt.url().scheme() == "https"
                    && matches!(
                        attempt.url().host_str().unwrap_or(""),
                        "github.com"
                            | "release-assets.githubusercontent.com"
                            | "objects.githubusercontent.com"
                    )
                {
                    attempt.follow()
                } else {
                    attempt.error("unexpected model download host")
                }
            }))
            .build()?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        runtime.block_on(download(
            &client,
            VAD_MODEL_SOURCE,
            &source,
            VAD_MODEL_BYTES,
            VAD_MODEL_SHA256,
            &cancel,
            |n| progress(n * 95 / 100, VAD_MODEL_BYTES),
        ))?;
        self.publish(&source, work.path(), &cancel)?;
        progress(VAD_MODEL_BYTES, VAD_MODEL_BYTES);
        Ok(())
    }
    /// Explicit offline setup from this exact pinned file, including a previously
    /// installed refinement pack's VAD. No network request is made.
    pub fn install_from_file(&self, source: &Path, cancel: Arc<AtomicBool>) -> Result<()> {
        fs::create_dir_all(&self.root)?;
        let _lock = install_lock(&self.root)?;
        cancelled(&cancel)?;
        if self.verify_inner(true, &cancel).is_ok() {
            return Ok(());
        }
        cancelled(&cancel)?;
        verify_vad_file(source, &cancel)?;
        let work = tempfile::Builder::new()
            .prefix(".vad-install-")
            .tempdir_in(&self.root)?;
        self.publish(source, work.path(), &cancel)
    }
    fn publish(&self, source: &Path, work: &Path, cancel: &AtomicBool) -> Result<()> {
        let candidate = work.join("candidate");
        fs::create_dir(&candidate)?;
        let mut input = File::open(source)?;
        let path = candidate.join(VAD_MODEL_FILE);
        let mut output = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)?;
        let mut buffer = [0; 65536];
        let mut total = 0_u64;
        loop {
            cancelled(cancel)?;
            let n = input.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            total += n as u64;
            if total > VAD_MODEL_BYTES {
                return Err(SttError::InvalidModel(
                    "VAD file exceeds catalog size".into(),
                ));
            }
            output.write_all(&buffer[..n])?;
        }
        output.sync_all()?;
        drop(output);
        verify_vad_file(&path, cancel)?;
        let manifest=Manifest {id:VAD_MODEL_ID.into(),source:VAD_MODEL_SOURCE.into(),bytes:VAD_MODEL_BYTES,sha256:VAD_MODEL_SHA256.into(),license:VAD_MODEL_LICENSE.into(),license_text:include_str!("../resources/silero-vad-LICENSE.txt").into(),attribution:"Silero Team (https://github.com/snakers4/silero-vad), MIT. ONNX export maintained by k2-fsa / sherpa-onnx contributors.".into(),conversion:"Canonical k2-fsa maintained Silero v4 16kHz ONNX export; no local weight modifications or conversion. https://k2-fsa.github.io/sherpa/onnx/vad/silero-vad.html".into(),acquisition_terms:"Public official release; no account, API key or acceptance form required.".into()};
        let mut output = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(candidate.join("manifest.json"))?;
        output.write_all(&serde_json::to_vec_pretty(&manifest)?)?;
        output.sync_all()?;
        cancelled(cancel)?;
        sync_dir(&candidate)?;
        let final_path = self.model_dir();
        let previous = work.join("previous-install");
        let replaced = final_path.try_exists()?;
        if replaced {
            fs::rename(&final_path, &previous)?
        }
        if let Err(error) = fs::rename(&candidate, &final_path) {
            if replaced {
                let _ = fs::rename(&previous, &final_path);
            }
            return Err(error.into());
        }
        sync_dir(&self.root)?;
        Ok(())
    }
    fn verify_inner(&self, full: bool, cancel: &AtomicBool) -> Result<()> {
        cancelled(cancel)?;
        let input = File::open(self.model_dir().join("manifest.json"))?;
        if input.metadata()?.len() > 32 * 1024 {
            return Err(SttError::InvalidModel("VAD manifest is oversized".into()));
        }
        let m: Manifest = serde_json::from_reader(input)?;
        if m.id != VAD_MODEL_ID
            || m.source != VAD_MODEL_SOURCE
            || m.bytes != VAD_MODEL_BYTES
            || m.sha256 != VAD_MODEL_SHA256
            || m.license != VAD_MODEL_LICENSE
        {
            return Err(SttError::InvalidModel(
                "VAD manifest differs from catalog".into(),
            ));
        }
        let path = self.model_path();
        let metadata = fs::symlink_metadata(&path)?;
        if !metadata.is_file() || metadata.len() != VAD_MODEL_BYTES {
            return Err(SttError::InvalidModel(
                "VAD model is missing or incomplete".into(),
            ));
        }
        if full {
            verify_vad_file(&path, cancel)?
        }
        Ok(())
    }
}
pub(crate) fn verify_vad_file(path: &Path, cancel: &AtomicBool) -> Result<()> {
    cancelled(cancel)?;
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file()
        || metadata.len() != VAD_MODEL_BYTES
        || hash_file(path, cancel)? != VAD_MODEL_SHA256
    {
        return Err(SttError::InvalidModel(
            "VAD file differs from pinned official export".into(),
        ));
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancellation_and_untrusted_files_never_publish() {
        let root = tempfile::tempdir().unwrap();
        let manager = VadModelManager::new(root.path().into());
        let bad = root.path().join("bad.onnx");
        fs::write(&bad, b"untrusted").unwrap();
        assert!(matches!(
            manager.install_from_file(&bad, Arc::new(AtomicBool::new(true))),
            Err(SttError::Cancelled)
        ));
        assert!(
            manager
                .install_from_file(&bad, Arc::new(AtomicBool::new(false)))
                .is_err()
        );
        assert!(!manager.installed());
        assert!(!manager.model_dir().exists());
    }
    #[cfg(unix)]
    #[test]
    fn native_initialization_rejects_symlinked_vad_files() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("file");
        File::create(&file)
            .unwrap()
            .set_len(VAD_MODEL_BYTES)
            .unwrap();
        let link = root.path().join("link");
        std::os::unix::fs::symlink(file, &link).unwrap();
        assert!(verify_vad_file(&link, &AtomicBool::new(false)).is_err());
    }
}
