//! Explicit, pinned installation of the offline refinement model and VAD.
use crate::{
    Result, SttError,
    models::{
        FileRecord, cancelled, extract_catalog, hash_file, install_lock, sync_dir, wait_cancel,
    },
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};

pub const REFINEMENT_MODEL_ID: &str = "sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8";
pub const REFINEMENT_MODEL_SOURCE: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8.tar.bz2";
pub const REFINEMENT_MODEL_LICENSE: &str = "CC-BY-4.0";
/// Progress estimate for the compressed official archive. GitHub has replaced
/// this asset without changing its URL, so this length is not an acceptance pin.
pub const REFINEMENT_ARCHIVE_BYTES: u64 = 487_170_055;
pub const REFINEMENT_ARCHIVE_SHA256: &str =
    "5793d0fd397c5778d2cf2126994d58e9d56b1be7c04d13c7a15bb1b4eafb16bf";
/// Safety cap for one compressed download. A larger response is rejected.
const REFINEMENT_ARCHIVE_MAX_BYTES: u64 = 1024 * 1024 * 1024;
pub const VAD_SOURCE: &str =
    "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/silero_vad.onnx";
pub const VAD_BYTES: u64 = 643_854;
pub const REFINEMENT_SETUP_BYTES: u64 = REFINEMENT_ARCHIVE_BYTES + VAD_BYTES;
pub(crate) const ENCODER: &str = "encoder.int8.onnx";
pub(crate) const DECODER: &str = "decoder.int8.onnx";
pub(crate) const JOINER: &str = "joiner.int8.onnx";
pub(crate) const TOKENS: &str = "tokens.txt";
pub(crate) const VAD: &str = "silero_vad.onnx";
/// Current official weight identity. The compressed archive around these files is not pinned.
pub const REFINEMENT_FILES: [(&str, u64, &str); 5] = [
    (
        ENCODER,
        652_282_294,
        "1afbe69c1ffdabee94755f42e4daf1d93549131a3da11356bda84fcef0f2c479",
    ),
    (
        DECODER,
        11_845_274,
        "0222a86147299d4dee90edf95b6d13827edb42ad8b7c9cf0b2582f030937c0b5",
    ),
    (
        JOINER,
        6_355_277,
        "cbd02f3696e9944970111fb439d9c3fd365a80374565f13f2b9c7007d253fc98",
    ),
    (
        TOKENS,
        93_939,
        "d58544679ea4bc6ac563d1f545eb7d474bd6cfa467f0a6e2c1dc1c7d37e3c35d",
    ),
    (
        VAD,
        VAD_BYTES,
        "9e2449e1087496d8d4caba907f23e0bd3f78d91fa552479bb9c23ac09cbb1fd6",
    ),
];
/// Weights already installed from the archive GitHub published before 2026-11-07.
const PREVIOUS_REFINEMENT_FILES: [(&str, u64, &str); 5] = [
    (
        ENCODER,
        652_184_281,
        "acfc2b4456377e15d04f0243af540b7fe7c992f8d898d751cf134c3a55fd2247",
    ),
    (
        DECODER,
        11_845_275,
        "179e50c43d1a9de79c8a24149a2f9bac6eb5981823f2a2ed88d655b24248db4e",
    ),
    (
        JOINER,
        6_355_277,
        "3164c13fc2821009440d20fcb5fdc78bff28b4db2f8d0f0b329101719c0948b3",
    ),
    (
        TOKENS,
        93_939,
        "d58544679ea4bc6ac563d1f545eb7d474bd6cfa467f0a6e2c1dc1c7d37e3c35d",
    ),
    (
        VAD,
        VAD_BYTES,
        "9e2449e1087496d8d4caba907f23e0bd3f78d91fa552479bb9c23ac09cbb1fd6",
    ),
];
const ACCEPTED_REFINEMENT_FILES: [[(&str, u64, &str); 5]; 2] =
    [REFINEMENT_FILES, PREVIOUS_REFINEMENT_FILES];
const REQUIRED: [&str; 4] = [ENCODER, DECODER, JOINER, TOKENS];
#[derive(Debug, Serialize, Deserialize)]
struct Manifest {
    id: String,
    source: String,
    archive_sha256: String,
    license: String,
    license_text: String,
    attribution: String,
    conversion: String,
    acquisition_terms: String,
    vad_source: String,
    vad_license: String,
    vad_license_text: String,
    files: BTreeMap<String, FileRecord>,
}
#[derive(Clone, Debug)]
pub struct RefinementModelManager {
    root: PathBuf,
}
impl RefinementModelManager {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }
    pub fn model_dir(&self) -> PathBuf {
        self.root.join(REFINEMENT_MODEL_ID)
    }
    pub fn installed(&self) -> bool {
        verify_refinement_manifest(&self.model_dir(), false).is_ok()
    }
    pub fn verify(&self) -> Result<()> {
        verify_refinement_manifest(&self.model_dir(), true)
    }
    /// Weight identity of the pack on disk, in catalog order.
    pub fn recorded_weights(&self) -> Result<Vec<(String, u64, String)>> {
        verify_refinement_manifest(&self.model_dir(), false)?;
        let manifest: Manifest =
            serde_json::from_reader(File::open(self.model_dir().join("manifest.json"))?)?;
        let catalog = matching_catalog(&manifest.files).ok_or_else(|| {
            SttError::InvalidModel(
                "installed refinement weights do not match an accepted catalog".into(),
            )
        })?;
        Ok(catalog
            .iter()
            .map(|&(name, bytes, hash)| (name.to_owned(), bytes, hash.to_owned()))
            .collect())
    }
    pub fn install(
        &self,
        progress: impl Fn(u64, u64) + Send,
        cancel: Arc<AtomicBool>,
    ) -> Result<()> {
        fs::create_dir_all(&self.root)?;
        let _lock = install_lock(&self.root)?;
        cancelled(&cancel)?;
        if verify_refinement_manifest_cancel(&self.model_dir(), true, &cancel).is_ok() {
            progress(REFINEMENT_SETUP_BYTES, REFINEMENT_SETUP_BYTES);
            return Ok(());
        }
        cancelled(&cancel)?;
        let work = tempfile::Builder::new()
            .prefix(".parakeet-install-")
            .tempdir_in(&self.root)?;
        let archive = work.path().join("model.tar.bz2");
        let vad = work.path().join(VAD);
        let client = reqwest::Client::builder()
            .https_only(true)
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(1200))
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
        let fetched = runtime.block_on(async {
            let fetched = download_archive(
                &client,
                REFINEMENT_MODEL_SOURCE,
                &archive,
                REFINEMENT_ARCHIVE_MAX_BYTES,
                &cancel,
                |n| progress(n * 95 / 100, REFINEMENT_SETUP_BYTES),
            )
            .await?;
            download(
                &client,
                VAD_SOURCE,
                &vad,
                VAD_BYTES,
                REFINEMENT_FILES[4].2,
                &cancel,
                |n| {
                    progress(
                        (REFINEMENT_ARCHIVE_BYTES + n) * 95 / 100,
                        REFINEMENT_SETUP_BYTES,
                    )
                },
            )
            .await?;
            Ok::<_, SttError>(fetched)
        })?;
        self.publish(&archive, &vad, work.path(), &cancel, &fetched.sha256)?;
        progress(REFINEMENT_SETUP_BYTES, REFINEMENT_SETUP_BYTES);
        Ok(())
    }
    /// Import an official archive and a canonical `silero_vad.onnx` beside it.
    pub fn install_from_archive(&self, archive: &Path, cancel: Arc<AtomicBool>) -> Result<()> {
        self.install_from_archives(archive, &archive.with_file_name(VAD), cancel)
    }
    /// Offline cache import. Extracted weights must match an accepted catalog.
    /// The archive wrapper's size and digest are recorded and are not required.
    pub fn install_from_archives(
        &self,
        archive: &Path,
        vad: &Path,
        cancel: Arc<AtomicBool>,
    ) -> Result<()> {
        fs::create_dir_all(&self.root)?;
        let _lock = install_lock(&self.root)?;
        cancelled(&cancel)?;
        if verify_refinement_manifest_cancel(&self.model_dir(), true, &cancel).is_ok() {
            return Ok(());
        }
        cancelled(&cancel)?;
        let archive_sha256 = hash_file(archive, &cancel)?;
        verify_artifact(vad, VAD_BYTES, REFINEMENT_FILES[4].2, &cancel)?;
        cancelled(&cancel)?;
        let work = tempfile::Builder::new()
            .prefix(".parakeet-install-")
            .tempdir_in(&self.root)?;
        self.publish(archive, vad, work.path(), &cancel, &archive_sha256)
    }
    fn publish(
        &self,
        archive: &Path,
        vad: &Path,
        work: &Path,
        cancel: &AtomicBool,
        archive_sha256: &str,
    ) -> Result<()> {
        let candidate = work.join("candidate");
        fs::create_dir(&candidate)?;
        let mut files = extract_catalog(
            File::open(archive)?,
            &candidate,
            cancel,
            REFINEMENT_MODEL_ID,
            &REQUIRED,
        )?;
        cancelled(cancel)?;
        let mut input = File::open(vad)?;
        let mut output = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(candidate.join(VAD))?;
        // VAD is a small fixed-size pinned asset; copy in bounded cancellation intervals.
        let mut buffer = [0; 65536];
        let mut total = 0;
        loop {
            use std::io::Read;
            cancelled(cancel)?;
            let n = input.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            total += n as u64;
            if total > VAD_BYTES {
                return Err(SttError::InvalidModel("VAD exceeds catalog size".into()));
            }
            output.write_all(&buffer[..n])?;
        }
        output.sync_all()?;
        drop(output);
        files.insert(
            VAD.into(),
            FileRecord {
                bytes: total,
                sha256: hash_file(&candidate.join(VAD), cancel)?,
            },
        );
        let manifest=Manifest {id:REFINEMENT_MODEL_ID.into(),source:REFINEMENT_MODEL_SOURCE.into(),archive_sha256:archive_sha256.into(),license:REFINEMENT_MODEL_LICENSE.into(),license_text:include_str!("../resources/CC-BY-4.0.txt").into(),attribution:"NVIDIA Corporation: parakeet-tdt-0.6b-v3 (https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3), CC BY 4.0. ONNX export and INT8 quantization: Fangjun Kuang / sherpa-onnx contributors. Silero VAD: Silero Team, MIT.".into(),conversion:"Canonical sherpa-onnx NeMo transducer ONNX export, INT8 encoder/decoder/joiner. https://github.com/k2-fsa/sherpa-onnx/tree/master/scripts/nemo/parakeet-tdt-0.6b-v3. No local model conversion or training.".into(),acquisition_terms:"Public ungated official release downloads; no account, API key or acceptance form.".into(),vad_source:VAD_SOURCE.into(),vad_license:"MIT".into(),vad_license_text:include_str!("../resources/silero-vad-LICENSE.txt").into(),files};
        let mut output = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(candidate.join("manifest.json"))?;
        output.write_all(&serde_json::to_vec_pretty(&manifest)?)?;
        output.sync_all()?;
        verify_refinement_manifest_cancel(&candidate, true, cancel)?;
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
}
fn verify_artifact(path: &Path, bytes: u64, hash: &str, cancel: &AtomicBool) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.len() != bytes || hash_file(path, cancel)? != hash {
        return Err(SttError::InvalidModel(
            "cached model artifact differs from pinned catalog".into(),
        ));
    }
    Ok(())
}
pub(crate) fn verify_refinement_manifest(directory: &Path, full: bool) -> Result<()> {
    verify_refinement_manifest_cancel(directory, full, &AtomicBool::new(false))
}
fn verify_refinement_manifest_cancel(
    directory: &Path,
    full: bool,
    cancel: &AtomicBool,
) -> Result<()> {
    let file = File::open(directory.join("manifest.json"))?;
    if file.metadata()?.len() > 128 * 1024 {
        return Err(SttError::InvalidModel(
            "oversized refinement manifest".into(),
        ));
    }
    let m: Manifest = serde_json::from_reader(file)?;
    if m.id != REFINEMENT_MODEL_ID
        || m.source != REFINEMENT_MODEL_SOURCE
        || !is_sha256_hex(&m.archive_sha256)
        || m.license != REFINEMENT_MODEL_LICENSE
        || m.vad_source != VAD_SOURCE
        || m.vad_license != "MIT"
        || matching_catalog(&m.files).is_none()
    {
        return Err(SttError::InvalidModel(
            "refinement manifest differs from catalog".into(),
        ));
    }
    let catalog = matching_catalog(&m.files).expect("catalog matched above");
    for &(name, bytes, hash) in catalog {
        cancelled(cancel)?;
        let r = m
            .files
            .get(name)
            .ok_or_else(|| SttError::InvalidModel(format!("missing {name}")))?;
        let path = directory.join(name);
        let metadata = fs::symlink_metadata(&path)?;
        if r.bytes != bytes || r.sha256 != hash || !metadata.is_file() || metadata.len() != bytes {
            return Err(SttError::InvalidModel(format!(
                "invalid refinement file: {name}"
            )));
        }
        if full && hash_file(&path, cancel)? != hash {
            return Err(SttError::InvalidModel(format!(
                "refinement checksum failed: {name}"
            )));
        }
    }
    Ok(())
}
fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}
fn matching_catalog(
    files: &BTreeMap<String, FileRecord>,
) -> Option<&'static [(&'static str, u64, &'static str)]> {
    ACCEPTED_REFINEMENT_FILES
        .iter()
        .find(|catalog| {
            catalog.len() == files.len()
                && catalog.iter().all(|(name, bytes, hash)| {
                    files
                        .get(*name)
                        .is_some_and(|record| record.bytes == *bytes && record.sha256 == *hash)
                })
        })
        .map(|catalog| catalog.as_slice())
}
struct DownloadedArchive {
    sha256: String,
}
async fn download_archive(
    client: &reqwest::Client,
    source: &str,
    path: &Path,
    max_bytes: u64,
    cancel: &AtomicBool,
    progress: impl Fn(u64),
) -> Result<DownloadedArchive> {
    let mut response = wait_cancel(client.get(source).send(), cancel)
        .await?
        .error_for_status()?;
    if response
        .content_length()
        .is_some_and(|length| length > max_bytes)
    {
        return Err(SttError::InvalidModel(
            "model download exceeds safety limit".into(),
        ));
    }
    let mut file = OpenOptions::new().create_new(true).write(true).open(path)?;
    let mut total = 0;
    let mut digest = Sha256::new();
    progress(0);
    loop {
        cancelled(cancel)?;
        let Some(buffer) = wait_cancel(response.chunk(), cancel).await? else {
            break;
        };
        total += buffer.len() as u64;
        if total > max_bytes {
            return Err(SttError::InvalidModel(
                "model download exceeds safety limit".into(),
            ));
        }
        file.write_all(&buffer)?;
        digest.update(&buffer);
        progress(total);
    }
    file.sync_all()?;
    Ok(DownloadedArchive {
        sha256: format!("{:x}", digest.finalize()),
    })
}
pub(crate) async fn download(
    client: &reqwest::Client,
    source: &str,
    path: &Path,
    bytes: u64,
    hash: &str,
    cancel: &AtomicBool,
    progress: impl Fn(u64),
) -> Result<()> {
    let mut response = wait_cancel(client.get(source).send(), cancel)
        .await?
        .error_for_status()?;
    if response.content_length().is_some_and(|n| n != bytes) {
        return Err(SttError::InvalidModel(
            "official model asset size changed".into(),
        ));
    }
    let mut file = OpenOptions::new().create_new(true).write(true).open(path)?;
    let mut total = 0;
    let mut digest = Sha256::new();
    progress(0);
    loop {
        cancelled(cancel)?;
        let Some(buffer) = wait_cancel(response.chunk(), cancel).await? else {
            break;
        };
        total += buffer.len() as u64;
        if total > bytes {
            return Err(SttError::InvalidModel(
                "model download exceeds catalog size".into(),
            ));
        }
        file.write_all(&buffer)?;
        digest.update(&buffer);
        progress(total);
    }
    file.sync_all()?;
    if total != bytes || format!("{:x}", digest.finalize()) != hash {
        return Err(SttError::InvalidModel(
            "official model download checksum failed".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancelled_or_untrusted_cache_never_publishes() {
        let root = tempfile::tempdir().unwrap();
        let manager = RefinementModelManager::new(root.path().into());
        let archive = root.path().join("archive");
        let vad = root.path().join(VAD);
        fs::write(&archive, b"not an official model").unwrap();
        fs::write(&vad, b"not VAD").unwrap();
        assert!(matches!(
            manager.install_from_archives(&archive, &vad, Arc::new(AtomicBool::new(true))),
            Err(SttError::Cancelled)
        ));
        assert!(
            manager
                .install_from_archives(&archive, &vad, Arc::new(AtomicBool::new(false)))
                .is_err()
        );
        assert!(!manager.model_dir().exists());
        assert!(!manager.installed());
    }
    #[test]
    fn manifest_metadata_is_bound_to_each_weight_and_vad_hash() {
        let root = tempfile::tempdir().unwrap();
        let manager = RefinementModelManager::new(root.path().into());
        let dir = manager.model_dir();
        fs::create_dir(&dir).unwrap();
        let files = REFINEMENT_FILES
            .into_iter()
            .map(|(name, bytes, hash)| {
                File::create(dir.join(name))
                    .unwrap()
                    .set_len(bytes)
                    .unwrap();
                (
                    name.into(),
                    FileRecord {
                        bytes,
                        sha256: hash.into(),
                    },
                )
            })
            .collect();
        let mut m = Manifest {
            id: REFINEMENT_MODEL_ID.into(),
            source: REFINEMENT_MODEL_SOURCE.into(),
            archive_sha256: REFINEMENT_ARCHIVE_SHA256.into(),
            license: REFINEMENT_MODEL_LICENSE.into(),
            license_text: String::new(),
            attribution: String::new(),
            conversion: String::new(),
            acquisition_terms: String::new(),
            vad_source: VAD_SOURCE.into(),
            vad_license: "MIT".into(),
            vad_license_text: String::new(),
            files,
        };
        fs::write(dir.join("manifest.json"), serde_json::to_vec(&m).unwrap()).unwrap();
        assert!(manager.installed());
        assert!(manager.verify().is_err());
        m.archive_sha256 =
            "1495e91b59d7fc53ddca91f3431758ec99f8fa253fdb0e1e851be0f72b14b0f9".into();
        fs::write(dir.join("manifest.json"), serde_json::to_vec(&m).unwrap()).unwrap();
        assert!(
            manager.installed(),
            "archive wrapper digest is provenance, not an acceptance pin"
        );
        m.files.get_mut(VAD).unwrap().sha256 = "arbitrary".into();
        fs::write(dir.join("manifest.json"), serde_json::to_vec(&m).unwrap()).unwrap();
        assert!(!manager.installed());
    }
    #[test]
    fn previously_published_weights_remain_installed() {
        let root = tempfile::tempdir().unwrap();
        let manager = RefinementModelManager::new(root.path().into());
        let dir = manager.model_dir();
        fs::create_dir(&dir).unwrap();
        let files = PREVIOUS_REFINEMENT_FILES
            .into_iter()
            .map(|(name, bytes, hash)| {
                File::create(dir.join(name))
                    .unwrap()
                    .set_len(bytes)
                    .unwrap();
                (
                    name.into(),
                    FileRecord {
                        bytes,
                        sha256: hash.into(),
                    },
                )
            })
            .collect();
        let manifest = Manifest {
            id: REFINEMENT_MODEL_ID.into(),
            source: REFINEMENT_MODEL_SOURCE.into(),
            archive_sha256: REFINEMENT_ARCHIVE_SHA256.into(),
            license: REFINEMENT_MODEL_LICENSE.into(),
            license_text: String::new(),
            attribution: String::new(),
            conversion: String::new(),
            acquisition_terms: String::new(),
            vad_source: VAD_SOURCE.into(),
            vad_license: "MIT".into(),
            vad_license_text: String::new(),
            files,
        };
        fs::write(
            dir.join("manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        assert!(manager.installed());
        let recorded = manager.recorded_weights().unwrap();
        assert_eq!(recorded[0].1, PREVIOUS_REFINEMENT_FILES[0].1);
    }
    #[test]
    fn refinement_archive_uses_the_same_safe_extractor() {
        let root = tempfile::tempdir().unwrap();
        let mut tar = tar::Builder::new(bzip2::write::BzEncoder::new(
            vec![],
            bzip2::Compression::fast(),
        ));
        let mut header = tar::Header::new_gnu();
        header.set_size(0);
        header.set_mode(0o600);
        header.set_entry_type(tar::EntryType::Symlink);
        header.set_link_name("../../outside").unwrap();
        header.set_cksum();
        tar.append_data(
            &mut header,
            format!("{REFINEMENT_MODEL_ID}/{ENCODER}"),
            std::io::empty(),
        )
        .unwrap();
        let archive = tar.into_inner().unwrap().finish().unwrap();
        assert!(
            extract_catalog(
                archive.as_slice(),
                root.path(),
                &AtomicBool::new(false),
                REFINEMENT_MODEL_ID,
                &REQUIRED
            )
            .is_err()
        );
        assert!(!root.path().join(ENCODER).exists());
    }
}
