use crate::{DiarizeError, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
pub const MODEL_ID: &str = "pyannote3-eres2net-2024-v1";
pub const SEGMENTATION: &str = "segmentation.int8.onnx";
pub const EMBEDDING: &str = "embedding.onnx";
/// Identity of the clustering profile that produced a run's speaker turns:
/// pinned weights, preprocessing and the ASSIGN/MERGE/MARGIN values (see the
/// `embedding_version_hash_covers_...` test). It changes when thresholds change,
/// so a run refined under other thresholds is recognisably not current. It is
/// not part of model-pack identity (see `verify`) and not the voiceprint
/// version (`voice_embedding_version`, which excludes clustering thresholds).
pub const EMBEDDING_VERSION: &str =
    "ff29acb384a0273fbb07878cab865746d2af4ddd5579d20655ded122328da6c4";
pub const SEGMENTATION_SOURCE: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-segmentation-models/sherpa-onnx-pyannote-segmentation-3-0.tar.bz2";
pub const EMBEDDING_SOURCE: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-recongition-models/3dspeaker_speech_eres2net_base_sv_zh-cn_3dspeaker_16k.onnx";
const ARCHIVE_HASH: &str = "24615ee884c897d9d2ba09bb4d30da6bb1b15e685065962db5b02e76e4996488";
const SEG_HASH: &str = "d582f4b4c6b48205de7e0643c57df0df5615a3c176189be3fc461e9d18827b5d";
pub(crate) fn embedding_weight_hash() -> &'static str {
    EMB_HASH
}
const EMB_HASH: &str = "1a331345f04805badbb495c775a6ddffcdd1a732567d5ec8b3d5749e3c7a5e4b";
// Clustering thresholds for this embedder (ERes2Net, this preprocessing).
// Derived 2026-11-10 by a rule fixed in advance from 6 public AMI calibration
// meetings and checked on 7 speaker-disjoint held-out meetings; see
// documentation/features/diarization/speaker-turns/trail/2026-11-11-ami-derived-clustering-thresholds.md.
// Still experimental: not validated for remote-call audio or accepted for
// production. Cosine similarities, not identification probabilities.
pub(crate) const ASSIGN: f32 = 0.50;
pub(crate) const MERGE: f32 = 0.60;
pub(crate) const MARGIN: f32 = 0.05;
/// Written into a newly installed pack's manifest as a record of the profile
/// at install time. It is not checked by `verify`: retuning thresholds must
/// never make installed model files look broken.
fn calibration_profile() -> String {
    format!(
        "experimental_ami_rule_v1; assign={ASSIGN:.2} merge={MERGE:.2} margin={MARGIN:.2}; derived from 6 public AMI in-person meetings by a fixed rule, checked on 7 held-out meetings; not validated for remote calls or production; NOT identification confidence"
    )
}
const ARCHIVE_BYTES: u64 = 6_958_444;
const SEG_BYTES: u64 = 1_540_506;
const EMB_BYTES: u64 = 39_593_761;
#[derive(Clone, Debug)]
pub struct ModelManager {
    root: PathBuf,
}
#[derive(Serialize, Deserialize)]
struct Manifest {
    id: String,
    embedding_version: String,
    calibration: String,
    segmentation_sha256: String,
    embedding_sha256: String,
    source_urls: Vec<String>,
    licenses: Vec<String>,
    attribution: String,
    acquisition: String,
}
impl ModelManager {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }
    pub fn model_dir(&self) -> PathBuf {
        self.root.join(MODEL_ID)
    }
    pub fn installed(&self) -> bool {
        verify(&self.model_dir(), false).is_ok()
    }
    pub fn verify(&self) -> Result<()> {
        verify(&self.model_dir(), true)
    }
    /// Explicit setup only. Acknowledgement records presentation of the source
    /// license/acquisition notice; it never accepts a gated publisher agreement.
    pub fn install(
        &self,
        progress: impl Fn(u64, u64),
        cancel: Arc<AtomicBool>,
        acknowledge_terms: bool,
    ) -> Result<()> {
        if !acknowledge_terms {
            return Err(DiarizeError::Terms);
        }
        check_cancel(&cancel)?;
        fs::create_dir_all(&self.root)?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.root.join(".diarization-install.lock"))?;
        lock.try_lock()
            .map_err(|_| DiarizeError::Install("another setup is running"))?;
        if self.verify().is_ok() {
            progress(ARCHIVE_BYTES + EMB_BYTES, ARCHIVE_BYTES + EMB_BYTES);
            return Ok(());
        }
        if self.model_dir().exists() {
            return Err(DiarizeError::Install(
                "existing model pack failed verification; remove it explicitly before reinstalling",
            ));
        }
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| DiarizeError::Install("clock"))?
            .as_nanos();
        let stage = self
            .root
            .join(format!(".diarization-{}-{nonce}", std::process::id()));
        fs::create_dir(&stage)?;
        let cleanup = Stage(stage.clone());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&stage, fs::Permissions::from_mode(0o700))?;
        }
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(180))
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                if attempt.previous().len() > 5 {
                    return attempt.error("redirect limit");
                }
                let url = attempt.url();
                if url.scheme() == "https"
                    && matches!(
                        url.host_str(),
                        Some(
                            "github.com"
                                | "release-assets.githubusercontent.com"
                                | "objects.githubusercontent.com"
                        )
                    )
                {
                    attempt.follow()
                } else {
                    attempt.stop()
                }
            }))
            .build()
            .map_err(|_| DiarizeError::Install("HTTP client"))?;
        download(
            &client,
            SEGMENTATION_SOURCE,
            &stage.join("segmentation.tar.bz2"),
            ARCHIVE_BYTES,
            ARCHIVE_HASH,
            &cancel,
            |n| progress(n, ARCHIVE_BYTES + EMB_BYTES),
        )?;
        let decoder = bzip2::read::BzDecoder::new(File::open(stage.join("segmentation.tar.bz2"))?);
        let mut archive = tar::Archive::new(decoder);
        let mut found = false;
        for entry in archive.entries()? {
            check_cancel(&cancel)?;
            let entry = entry?;
            let path = entry.path()?.into_owned();
            if path == Path::new("sherpa-onnx-pyannote-segmentation-3-0/model.int8.onnx") {
                if found || !entry.header().entry_type().is_file() || entry.size() != SEG_BYTES {
                    return Err(DiarizeError::Model);
                }
                let mut file = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(stage.join(SEGMENTATION))?;
                let bytes = std::io::copy(&mut entry.take(SEG_BYTES + 1), &mut file)?;
                if bytes != SEG_BYTES {
                    return Err(DiarizeError::Model);
                }
                file.sync_all()?;
                found = true;
            }
        }
        if !found {
            return Err(DiarizeError::Model);
        }
        fs::remove_file(stage.join("segmentation.tar.bz2"))?;
        download(
            &client,
            EMBEDDING_SOURCE,
            &stage.join(EMBEDDING),
            EMB_BYTES,
            EMB_HASH,
            &cancel,
            |n| progress(ARCHIVE_BYTES + n, ARCHIVE_BYTES + EMB_BYTES),
        )?;
        fs::write(
            stage.join("pyannote-MIT.txt"),
            include_str!("../resources/pyannote-MIT.txt"),
        )?;
        fs::write(
            stage.join("3D-Speaker-Apache-2.0.txt"),
            include_str!("../resources/Apache-2.0.txt"),
        )?;
        let manifest=Manifest{id:MODEL_ID.into(),embedding_version:EMBEDDING_VERSION.into(),calibration:calibration_profile(),segmentation_sha256:SEG_HASH.into(),embedding_sha256:EMB_HASH.into(),source_urls:vec![SEGMENTATION_SOURCE.into(),EMBEDDING_SOURCE.into(),"https://huggingface.co/pyannote/segmentation-3.0".into(),"https://modelscope.cn/models/iic/speech_eres2net_base_sv_zh-cn_3dspeaker_16k".into()],licenses:vec!["MIT (segmentation archive LICENSE, Copyright 2022 CNRS)".into(),"Apache-2.0 (original 3D-Speaker model card)".into()],attribution:"pyannote/Séverin Baroudi, Alexis Plaquet and Hervé Bredin; CNRS. 3D-Speaker/ModelScope ERes2Net; sherpa-onnx conversion by k2-fsa. Source model revisions are not provided by release assets. ERes2Net source model language is Chinese; English/channel calibration is unvalidated.".into(),acquisition:"User explicitly requested official release downloads and acknowledged the source notice. Original pyannote Hugging Face acquisition requires its own publisher conditions; this app neither logs in nor accepts them. No weights are bundled with the application.".into()};
        fs::write(
            stage.join("manifest.json"),
            serde_json::to_vec_pretty(&manifest)?,
        )?;
        verify(&stage, true)?;
        check_cancel(&cancel)?;
        fs::rename(&stage, self.model_dir())?;
        drop(cleanup);
        progress(ARCHIVE_BYTES + EMB_BYTES, ARCHIVE_BYTES + EMB_BYTES);
        Ok(())
    }
}
struct Stage(PathBuf);
impl Drop for Stage {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
pub(crate) fn check_cancel(cancel: &Arc<AtomicBool>) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        Err(DiarizeError::Cancelled)
    } else {
        Ok(())
    }
}
fn download(
    client: &reqwest::Client,
    url: &str,
    path: &Path,
    bytes: u64,
    hash: &str,
    cancel: &Arc<AtomicBool>,
    progress: impl Fn(u64),
) -> Result<()> {
    check_cancel(cancel)?;
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| DiarizeError::Install("runtime"))?;
    runtime.block_on(async {
        let operation=async {
            let mut response=client.get(url).send().await.map_err(|_|DiarizeError::Install("download"))?;
            if !response.status().is_success(){return Err(DiarizeError::Install("HTTP status"));}
            let mut received=0u64;let mut digest=Sha256::new();
            while let Some(chunk)=response.chunk().await.map_err(|_|DiarizeError::Install("response"))? {
                received+=chunk.len()as u64;if received>bytes{return Err(DiarizeError::Model);}file.write_all(&chunk)?;digest.update(&chunk);progress(received);
            }
            file.sync_all()?;if received!=bytes||format!("{:x}",digest.finalize())!=hash{return Err(DiarizeError::Model);}Ok(())
        };
        tokio::pin!(operation);
        loop{tokio::select!{result=&mut operation=>return result,_=tokio::time::sleep(Duration::from_millis(50))=>check_cancel(cancel)?,}}
    })
}
pub(crate) fn verify(dir: &Path, full: bool) -> Result<()> {
    let manifest_path = dir.join("manifest.json");
    let metadata = fs::symlink_metadata(&manifest_path).map_err(|_| DiarizeError::Model)?;
    if !metadata.is_file() || metadata.len() > 32 * 1024 {
        return Err(DiarizeError::Model);
    }
    let m: Manifest =
        serde_json::from_slice(&fs::read(manifest_path)?).map_err(|_| DiarizeError::Model)?;
    // Pack identity is the id plus the pinned weight hashes (and, below, the
    // notices and the weights themselves). `embedding_version` and
    // `calibration` are install-time records of the clustering profile and are
    // deliberately not compared: a threshold change must not invalidate an
    // installed pack (and `install` refuses to replace a pack that fails
    // verification, so comparing them would strand existing installs).
    if m.id != MODEL_ID || m.segmentation_sha256 != SEG_HASH || m.embedding_sha256 != EMB_HASH {
        return Err(DiarizeError::Model);
    }
    for (name, expected) in [
        (
            "pyannote-MIT.txt",
            include_str!("../resources/pyannote-MIT.txt"),
        ),
        (
            "3D-Speaker-Apache-2.0.txt",
            include_str!("../resources/Apache-2.0.txt"),
        ),
    ] {
        let path = dir.join(name);
        let metadata = fs::symlink_metadata(&path).map_err(|_| DiarizeError::Model)?;
        if !metadata.is_file()
            || metadata.len() != expected.len() as u64
            || fs::read_to_string(path)? != expected
        {
            return Err(DiarizeError::Model);
        }
    }
    for (name, size, hash) in [
        (SEGMENTATION, SEG_BYTES, SEG_HASH),
        (EMBEDDING, EMB_BYTES, EMB_HASH),
    ] {
        let path = dir.join(name);
        let metadata = fs::symlink_metadata(&path).map_err(|_| DiarizeError::Model)?;
        if !metadata.is_file() || metadata.len() != size {
            return Err(DiarizeError::Model);
        }
        if full {
            let mut digest = Sha256::new();
            let mut reader = File::open(path)?;
            let mut buffer = [0; 64 * 1024];
            loop {
                let n = reader.read(&mut buffer)?;
                if n == 0 {
                    break;
                }
                digest.update(&buffer[..n]);
            }
            if format!("{:x}", digest.finalize()) != hash {
                return Err(DiarizeError::Model);
            }
        }
    }
    Ok(())
}

pub const MODEL_NOTICE: &str = "Experimental local speaker models: pyannote segmentation 3 ONNX, MIT (Copyright 2022 CNRS), and 3D-Speaker ERes2Net, Apache-2.0 per original ModelScope model card. Public sherpa release downloads are explicit; no models are bundled. Original pyannote Hugging Face acquisition has publisher conditions that this app does not accept. Clustering thresholds are experimental: derived from a small public set of English in-person meetings, not validated for remote calls or accepted for production.";

#[cfg(test)]
mod manifest_tests {
    use super::*;
    /// A pack laid out like a real install (weights sized as pinned; content
    /// is only hashed by the full check), with the given manifest strings.
    fn pack(dir: &Path, embedding_version: &str, calibration: &str, emb_hash: &str) {
        fs::create_dir_all(dir).unwrap();
        File::create(dir.join(SEGMENTATION))
            .unwrap()
            .set_len(SEG_BYTES)
            .unwrap();
        File::create(dir.join(EMBEDDING))
            .unwrap()
            .set_len(EMB_BYTES)
            .unwrap();
        fs::write(
            dir.join("pyannote-MIT.txt"),
            include_str!("../resources/pyannote-MIT.txt"),
        )
        .unwrap();
        fs::write(
            dir.join("3D-Speaker-Apache-2.0.txt"),
            include_str!("../resources/Apache-2.0.txt"),
        )
        .unwrap();
        let manifest = Manifest {
            id: MODEL_ID.into(),
            embedding_version: embedding_version.into(),
            calibration: calibration.into(),
            segmentation_sha256: SEG_HASH.into(),
            embedding_sha256: emb_hash.into(),
            source_urls: vec![],
            licenses: vec![],
            attribution: String::new(),
            acquisition: String::new(),
        };
        fs::write(
            dir.join("manifest.json"),
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();
    }
    #[test]
    fn retuned_thresholds_do_not_invalidate_an_installed_pack() {
        // Manifest written by the 0.55/0.70/0.10 build (verbatim strings).
        let temp = tempfile::TempDir::new().unwrap();
        let manager = ModelManager::new(temp.path().into());
        pack(
            &manager.model_dir(),
            "fb58e351faac141e86cc27aff963f9818a9753dafee0561b381a685fd59c2d52",
            "experimental_uncalibrated; assign=0.55 merge=0.70 margin=0.10; NOT identification confidence",
            EMB_HASH,
        );
        assert_ne!(
            EMBEDDING_VERSION,
            "fb58e351faac141e86cc27aff963f9818a9753dafee0561b381a685fd59c2d52"
        );
        assert!(manager.installed());
    }
    #[test]
    fn pack_identity_still_rejects_other_weights() {
        let temp = tempfile::TempDir::new().unwrap();
        let manager = ModelManager::new(temp.path().into());
        pack(
            &manager.model_dir(),
            EMBEDDING_VERSION,
            &calibration_profile(),
            &"0".repeat(64),
        );
        assert!(!manager.installed());
    }
}
