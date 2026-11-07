use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use bzip2::read::BzDecoder;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{Result, SttError};

pub const MODEL_ID: &str = "sherpa-onnx-streaming-zipformer-en-2023-06-26";
pub const MODEL_SOURCE: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-streaming-zipformer-en-2023-06-26.tar.bz2";
pub const MODEL_LICENSE: &str = "Apache-2.0";
pub const ARCHIVE_BYTES: u64 = 310_414_022;
const ARCHIVE_SHA256: &str = "639e25b578e9e997131402199419c13a941f8e4e198e2da1ce57dbf5cf401282";
pub(crate) const ENCODER: &str = "encoder-epoch-99-avg-1-chunk-16-left-128.int8.onnx";
pub(crate) const DECODER: &str = "decoder-epoch-99-avg-1-chunk-16-left-128.onnx";
pub(crate) const JOINER: &str = "joiner-epoch-99-avg-1-chunk-16-left-128.int8.onnx";
pub(crate) const TOKENS: &str = "tokens.txt";
#[cfg(test)]
const REQUIRED_FILES: [&str; 4] = [ENCODER, DECODER, JOINER, TOKENS];
const MAX_EXTRACTED_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const ACQUISITION_TERMS: &str =
    "Public download; no account, API key or access-form acceptance required.";

/// An explicit English live acoustic model, independent of full application profiles.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveModel {
    /// The existing LibriSpeech June 26 model; remains the default.
    #[default]
    EnglishSmall,
    /// The larger LibriSpeech + GigaSpeech June 21 model; explicit opt-in only.
    EnglishStandard,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct LiveModelFile {
    pub name: &'static str,
    pub bytes: u64,
    pub sha256: &'static str,
}
const SMALL_FILES: [LiveModelFile; 4] = [
    LiveModelFile {
        name: ENCODER,
        bytes: 70_108_816,
        sha256: "5022b2eca5b19d1bc104fcf33e26bc32604b7df553cd2e1f62e31dc7b05e9c87",
    },
    LiveModelFile {
        name: DECODER,
        bytes: 2_093_080,
        sha256: "85914f840b71110487ece7a30ebd0bd4b8fb5c2623215bba3aa5ab4c138fc591",
    },
    LiveModelFile {
        name: JOINER,
        bytes: 259_416,
        sha256: "abd5e30f3f16fc510605c6029dba33f10e4386bd75c5bdc30cf94076864db10d",
    },
    LiveModelFile {
        name: TOKENS,
        bytes: 5_048,
        sha256: "49e3c2646595fd907228b3c6787069658f67b17377c60aeb8619c4551b2316fb",
    },
];
const STANDARD_FILES: [LiveModelFile; 4] = [
    LiveModelFile {
        name: "encoder-epoch-99-avg-1.int8.onnx",
        bytes: 187_823_992,
        sha256: "32c98281c7bd8b63e3e142d007251b37f120572e8fdea9a4f5a79ce22b10ec4f",
    },
    LiveModelFile {
        name: "decoder-epoch-99-avg-1.onnx",
        bytes: 2_092_566,
        sha256: "9da02b77cb08826756ec6a88635f35a40374e4164e7c6359121a9145958a6ceb",
    },
    LiveModelFile {
        name: "joiner-epoch-99-avg-1.int8.onnx",
        bytes: 259_335,
        sha256: "831477d390e59a61f1b6a6f763b9903e6c6366ff6034f1ddba613be82637122f",
    },
    SMALL_FILES[3],
];
impl LiveModel {
    pub fn id(self) -> &'static str {
        match self {
            Self::EnglishSmall => MODEL_ID,
            Self::EnglishStandard => "sherpa-onnx-streaming-zipformer-en-2023-06-21",
        }
    }
    pub fn source(self) -> &'static str {
        match self {
            Self::EnglishSmall => MODEL_SOURCE,
            Self::EnglishStandard => {
                "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-streaming-zipformer-en-2023-06-21.tar.bz2"
            }
        }
    }
    pub fn archive_bytes(self) -> u64 {
        match self {
            Self::EnglishSmall => ARCHIVE_BYTES,
            Self::EnglishStandard => 506_956_414,
        }
    }
    pub fn archive_sha256(self) -> &'static str {
        match self {
            Self::EnglishSmall => ARCHIVE_SHA256,
            Self::EnglishStandard => {
                "455f40e556aa2b20ac9d3bffd603b58002075c1193b4070938540c11efe0a4da"
            }
        }
    }
    pub fn license(self) -> &'static str {
        MODEL_LICENSE
    }
    /// Required model artifacts, excluding the small manifest/license overhead.
    pub fn installed_bytes(self) -> u64 {
        self.files().iter().map(|file| file.bytes).sum()
    }
    /// Encoder, FP32 decoder, joiner and tokens, in that order.
    pub fn files(self) -> &'static [LiveModelFile; 4] {
        match self {
            Self::EnglishSmall => &SMALL_FILES,
            Self::EnglishStandard => &STANDARD_FILES,
        }
    }
    fn attribution(self) -> &'static str {
        match self {
            Self::EnglishSmall => {
                "Streaming English Zipformer model: Zengwei Yao / icefall contributors. ONNX export: csukuangfj / sherpa-onnx contributors. Training corpus: LibriSpeech."
            }
            Self::EnglishStandard => {
                "Streaming English Zipformer model: marcoyang / icefall contributors. ONNX export: csukuangfj / sherpa-onnx contributors. Training corpora: LibriSpeech and GigaSpeech."
            }
        }
    }
    fn conversion(self) -> &'static str {
        match self {
            Self::EnglishSmall => {
                "Canonical sherpa-onnx 2023-06-26 ONNX export of Zengwei/icefall-asr-librispeech-streaming-zipformer-2023-05-17; INT8 encoder/joiner and FP32 decoder. Export script is in the pinned upstream archive; no local conversion."
            }
            Self::EnglishStandard => {
                "Canonical sherpa-onnx 2023-06-21 ONNX export of marcoyang/icefall-libri-giga-pruned-transducer-stateless7-streaming-2023-04-04 (https://github.com/k2-fsa/icefall/pull/984); INT8 encoder/joiner and FP32 decoder. Publisher model cards and the pinned archive README identify Apache-2.0; no local conversion."
            }
        }
    }
    fn from_id(id: &str) -> Result<Self> {
        [Self::EnglishSmall, Self::EnglishStandard]
            .into_iter()
            .find(|model| model.id() == id)
            .ok_or_else(|| SttError::InvalidModel("unsupported live model catalog".into()))
    }
}
#[cfg(test)]
fn expected_file(name: &str) -> Option<(u64, &'static str)> {
    LiveModel::EnglishSmall
        .files()
        .iter()
        .find(|f| f.name == name)
        .map(|f| (f.bytes, f.sha256))
}

#[derive(Clone, Debug)]
pub struct ModelManager {
    root: PathBuf,
    model: LiveModel,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FileRecord {
    pub(crate) sha256: String,
    pub(crate) bytes: u64,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ModelManifest {
    id: String,
    source: String,
    archive_sha256: String,
    license: String,
    license_text: String,
    attribution: String,
    conversion: String,
    acquisition_terms: String,
    files: BTreeMap<String, FileRecord>,
}

impl ModelManager {
    /// `root` is the application's models directory, not its data directory.
    pub fn new(root: PathBuf) -> Self {
        Self::for_model(root, LiveModel::EnglishSmall)
    }
    pub fn for_model(root: PathBuf, model: LiveModel) -> Self {
        Self { root, model }
    }
    pub fn model(&self) -> LiveModel {
        self.model
    }
    pub fn model_dir(&self) -> PathBuf {
        self.root.join(self.model.id())
    }

    /// Cheap setup check suitable for UI polling. Full hashes are verified by
    /// recognizer construction and installation, not on each UI snapshot.
    pub fn installed(&self) -> bool {
        self.verify_selected(false).is_ok()
    }

    pub fn verify(&self) -> Result<()> {
        self.verify_selected(true)
    }
    fn verify_selected(&self, full: bool) -> Result<()> {
        if verify_manifest(&self.model_dir(), full)? != self.model {
            return Err(SttError::InvalidModel(
                "installed catalog differs from the explicitly selected live model".into(),
            ));
        }
        Ok(())
    }

    /// Explicit opt-in download. Only official HTTPS sources and their release
    /// asset redirect hosts are accepted. The archive hash is pinned in source.
    pub fn install(
        &self,
        progress: impl Fn(u64, u64) + Send,
        cancel: Arc<AtomicBool>,
    ) -> Result<()> {
        fs::create_dir_all(&self.root)?;
        let _lock = install_lock(&self.root)?;
        cancelled(&cancel)?;
        if self.verify().is_ok() {
            progress(self.model.archive_bytes(), self.model.archive_bytes());
            return Ok(());
        }
        let work = tempfile::Builder::new()
            .prefix(".zipformer-install-")
            .tempdir_in(&self.root)?;
        let archive = work.path().join("model.tar.bz2");
        let client = reqwest::Client::builder()
            .https_only(true)
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(600))
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                if attempt.previous().len() >= 5 {
                    return attempt.error("too many model redirects");
                }
                let host = attempt.url().host_str().unwrap_or("");
                if attempt.url().scheme() == "https"
                    && matches!(
                        host,
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
        runtime.block_on(async {
            let mut response = wait_cancel(client.get(self.model.source()).send(), &cancel)
                .await?
                .error_for_status()?;
            if response
                .content_length()
                .is_some_and(|n| n != self.model.archive_bytes())
            {
                return Err(SttError::InvalidModel(
                    "official archive size changed; update the model catalog".into(),
                ));
            }
            let mut file = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&archive)?;
            let mut downloaded = 0;
            let mut digest = Sha256::new();
            progress(0, self.model.archive_bytes());
            loop {
                cancelled(&cancel)?;
                let Some(buffer) = wait_cancel(response.chunk(), &cancel).await? else {
                    break;
                };
                let n = buffer.len();
                downloaded += n as u64;
                if downloaded > self.model.archive_bytes() {
                    return Err(SttError::InvalidModel(
                        "model archive exceeds expected size".into(),
                    ));
                }
                file.write_all(&buffer)?;
                digest.update(&buffer);
                // Reserve the final 5% of UI progress for verified extraction.
                progress(
                    downloaded.saturating_mul(95) / 100,
                    self.model.archive_bytes(),
                );
            }
            file.sync_all()?;
            if downloaded != self.model.archive_bytes()
                || format!("{:x}", digest.finalize()) != self.model.archive_sha256()
            {
                return Err(SttError::InvalidModel(
                    "official model archive checksum did not match".into(),
                ));
            }
            drop(file);
            self.publish_archive(&archive, work.path(), &cancel)?;
            progress(self.model.archive_bytes(), self.model.archive_bytes());
            Ok(())
        })
    }

    /// Imports a previously downloaded official archive through the same hash,
    /// extraction and publication checks. Useful for offline setup and tests.
    pub fn install_from_archive(&self, archive: &Path, cancel: Arc<AtomicBool>) -> Result<()> {
        fs::create_dir_all(&self.root)?;
        let _lock = install_lock(&self.root)?;
        cancelled(&cancel)?;
        if self.verify().is_ok() {
            return Ok(());
        }
        if fs::metadata(archive)?.len() != self.model.archive_bytes()
            || hash_file(archive, &cancel)? != self.model.archive_sha256()
        {
            return Err(SttError::InvalidModel(
                "cached archive checksum did not match the official catalog".into(),
            ));
        }
        let work = tempfile::Builder::new()
            .prefix(".zipformer-install-")
            .tempdir_in(&self.root)?;
        self.publish_archive(archive, work.path(), &cancel)
    }

    fn publish_archive(&self, archive: &Path, work: &Path, cancel: &AtomicBool) -> Result<()> {
        let candidate = work.join("candidate");
        fs::create_dir(&candidate)?;
        let names = self.model.files().map(|file| file.name);
        let files = extract_catalog(
            File::open(archive)?,
            &candidate,
            cancel,
            self.model.id(),
            &names,
        )?;
        let manifest = ModelManifest {
            id: self.model.id().into(),
            source: self.model.source().into(),
            archive_sha256: self.model.archive_sha256().into(),
            license: self.model.license().into(),
            license_text: include_str!("../resources/Apache-2.0.txt").into(),
            attribution: self.model.attribution().into(),
            conversion: self.model.conversion().into(),
            acquisition_terms: ACQUISITION_TERMS.into(),
            files,
        };
        let mut output = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(candidate.join("manifest.json"))?;
        output.write_all(&serde_json::to_vec_pretty(&manifest)?)?;
        output.sync_all()?;
        verify_manifest(&candidate, true)?;
        cancelled(cancel)?;
        sync_dir(&candidate)?;
        let final_path = self.model_dir();
        let previous = work.join("previous-install");
        let replaced = final_path.try_exists()?;
        if replaced {
            fs::rename(&final_path, &previous)?;
        }
        if let Err(e) = fs::rename(&candidate, &final_path) {
            if replaced {
                let _ = fs::rename(&previous, &final_path);
            }
            return Err(e.into());
        }
        sync_dir(&self.root)?;
        Ok(())
    }
}

pub(crate) fn verify_manifest(directory: &Path, full_hash: bool) -> Result<LiveModel> {
    let file = File::open(directory.join("manifest.json"))?;
    if file.metadata()?.len() > 128 * 1024 {
        return Err(SttError::InvalidModel("manifest is oversized".into()));
    }
    let manifest: ModelManifest = serde_json::from_reader(file)?;
    let model = LiveModel::from_id(&manifest.id)?;
    if manifest.archive_sha256 != model.archive_sha256()
        || manifest.source != model.source()
        || manifest.license != model.license()
        || manifest.license_text != include_str!("../resources/Apache-2.0.txt")
        || manifest.attribution != model.attribution()
        || manifest.conversion != model.conversion()
        || manifest.acquisition_terms != ACQUISITION_TERMS
    {
        return Err(SttError::InvalidModel(
            "model manifest does not match the English model catalog".into(),
        ));
    }
    if manifest.files.len() != model.files().len() {
        return Err(SttError::InvalidModel("unexpected model file list".into()));
    }
    for expected in model.files() {
        let name = expected.name;
        let record = manifest
            .files
            .get(name)
            .ok_or_else(|| SttError::InvalidModel(format!("missing file: {name}")))?;
        if record.bytes != expected.bytes || record.sha256 != expected.sha256 {
            return Err(SttError::InvalidModel(format!(
                "file manifest differs from the pinned catalog: {name}"
            )));
        }
        let path = directory.join(name);
        let metadata = fs::symlink_metadata(&path)?;
        if !metadata.is_file()
            || metadata.len() != record.bytes
            || record.bytes == 0
            || record.bytes > MAX_EXTRACTED_BYTES
        {
            return Err(SttError::InvalidModel(format!(
                "missing or incomplete model file: {name}"
            )));
        }
        if full_hash && hash_file(&path, &AtomicBool::new(false))? != record.sha256 {
            return Err(SttError::InvalidModel(format!(
                "model checksum failed: {name}"
            )));
        }
    }
    Ok(model)
}

#[cfg(test)]
fn extract_selected(
    source: impl Read,
    directory: &Path,
    cancel: &AtomicBool,
) -> Result<BTreeMap<String, FileRecord>> {
    extract_catalog(source, directory, cancel, MODEL_ID, &REQUIRED_FILES)
}

pub(crate) fn extract_catalog(
    source: impl Read,
    directory: &Path,
    cancel: &AtomicBool,
    model_id: &str,
    required: &[&str],
) -> Result<BTreeMap<String, FileRecord>> {
    let mut archive = tar::Archive::new(BzDecoder::new(source));
    let mut files = BTreeMap::new();
    let mut total = 0_u64;
    for (index, entry) in archive.entries()?.enumerate() {
        cancelled(cancel)?;
        if index > 1_000 {
            return Err(SttError::InvalidModel(
                "archive has too many entries".into(),
            ));
        }
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        let components = path.components().collect::<Vec<_>>();
        if components.is_empty()
            || !components.iter().all(|c| matches!(c, Component::Normal(_)))
            || components[0].as_os_str() != model_id
        {
            return Err(SttError::InvalidModel(
                "archive contains an unexpected or unsafe path".into(),
            ));
        }
        let kind = entry.header().entry_type();
        if kind.is_dir() {
            continue;
        }
        if !kind.is_file() {
            return Err(SttError::InvalidModel(
                "archive contains a link or unsupported entry".into(),
            ));
        }
        if entry.size() > MAX_EXTRACTED_BYTES.saturating_sub(total) {
            return Err(SttError::InvalidModel(
                "archive expands beyond its safety limit".into(),
            ));
        }
        let selected = components.len() == 2
            && components[1]
                .as_os_str()
                .to_str()
                .is_some_and(|s| required.contains(&s));
        let name = components
            .last()
            .and_then(|c| c.as_os_str().to_str())
            .unwrap_or("")
            .to_owned();
        let mut file = if selected {
            Some(
                OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .open(directory.join(&name))?,
            )
        } else {
            None
        };
        let mut digest = Sha256::new();
        let mut bytes = 0;
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            cancelled(cancel)?;
            let n = entry.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            total += n as u64;
            bytes += n as u64;
            if total > MAX_EXTRACTED_BYTES {
                return Err(SttError::InvalidModel(
                    "archive expands beyond its safety limit".into(),
                ));
            }
            if let Some(file) = &mut file {
                file.write_all(&buffer[..n])?;
                digest.update(&buffer[..n]);
            }
        }
        if let Some(file) = file {
            file.sync_all()?;
            files.insert(
                name,
                FileRecord {
                    sha256: format!("{:x}", digest.finalize()),
                    bytes,
                },
            );
        }
    }
    if files.len() != required.len() {
        return Err(SttError::InvalidModel(
            "archive is missing required model files".into(),
        ));
    }
    Ok(files)
}

pub(crate) fn hash_file(path: &Path, cancel: &AtomicBool) -> Result<String> {
    let mut file = File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        cancelled(cancel)?;
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        digest.update(&buffer[..n]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

pub(crate) fn install_lock(root: &Path) -> Result<File> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(root.join(".install.lock"))?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(std::fs::TryLockError::WouldBlock) => Err(SttError::Busy),
        Err(std::fs::TryLockError::Error(e)) => Err(e.into()),
    }
}

pub(crate) fn cancelled(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        Err(SttError::Cancelled)
    } else {
        Ok(())
    }
}

pub(crate) async fn wait_cancel<T>(
    future: impl std::future::Future<Output = reqwest::Result<T>>,
    cancel: &AtomicBool,
) -> Result<T> {
    let mut future = std::pin::pin!(future);
    let mut poll = tokio::time::interval(Duration::from_millis(100));
    let mut timeout = std::pin::pin!(tokio::time::sleep(Duration::from_secs(30)));
    loop {
        tokio::select! {
            result = &mut future => return Ok(result?),
            _ = poll.tick() => cancelled(cancel)?,
            _ = &mut timeout => return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "model download stalled; retry when connected").into()),
        }
    }
}

#[cfg(unix)]
pub(crate) fn sync_dir(path: &Path) -> Result<()> {
    Ok(File::open(path)?.sync_all()?)
}
#[cfg(not(unix))]
pub(crate) fn sync_dir(_: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn archive(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let encoder = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::fast());
        let mut archive = tar::Builder::new(encoder);
        for (name, bytes) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o600);
            header.set_cksum();
            archive.append_data(&mut header, *name, *bytes).unwrap();
        }
        archive.into_inner().unwrap().finish().unwrap()
    }

    #[test]
    fn selected_extraction_keeps_only_catalog_files() {
        let root = tempfile::tempdir().unwrap();
        let names = REQUIRED_FILES.map(|name| format!("{MODEL_ID}/{name}"));
        let entries: Vec<_> = names
            .iter()
            .map(|name| (name.as_str(), b"model".as_slice()))
            .chain(std::iter::once((
                "sherpa-onnx-streaming-zipformer-en-2023-06-26/unused.onnx",
                b"ignored".as_slice(),
            )))
            .collect();
        let files = extract_selected(
            archive(&entries).as_slice(),
            root.path(),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(files.len(), 4);
        assert!(!root.path().join("unused.onnx").exists());
        for name in REQUIRED_FILES {
            assert_eq!(files[name].bytes, 5);
        }
    }

    #[test]
    fn rejects_unexpected_archive_root_and_missing_files() {
        let root = tempfile::tempdir().unwrap();
        assert!(
            extract_selected(
                archive(&[("other/tokens.txt", b"tokens")]).as_slice(),
                root.path(),
                &AtomicBool::new(false)
            )
            .is_err()
        );
        assert!(
            extract_selected(
                archive(&[]).as_slice(),
                root.path(),
                &AtomicBool::new(false)
            )
            .is_err()
        );
    }

    #[test]
    fn archive_links_are_rejected_without_creating_the_target() {
        let root = tempfile::tempdir().unwrap();
        let encoder = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::fast());
        let mut archive = tar::Builder::new(encoder);
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Symlink);
        header.set_size(0);
        header.set_mode(0o600);
        header.set_link_name("../../outside").unwrap();
        header.set_cksum();
        archive
            .append_data(
                &mut header,
                format!("{MODEL_ID}/{TOKENS}"),
                std::io::empty(),
            )
            .unwrap();
        let bytes = archive.into_inner().unwrap().finish().unwrap();
        assert!(extract_selected(bytes.as_slice(), root.path(), &AtomicBool::new(false)).is_err());
        assert!(!root.path().join(TOKENS).exists());
    }

    #[test]
    fn manifest_checks_catalog_and_full_content_hashes() {
        let root = tempfile::tempdir().unwrap();
        let manager = ModelManager::new(root.path().into());
        let directory = manager.model_dir();
        fs::create_dir(&directory).unwrap();
        let mut files = BTreeMap::new();
        for name in REQUIRED_FILES {
            let (bytes, sha256) = expected_file(name).unwrap();
            File::create(directory.join(name))
                .unwrap()
                .set_len(bytes)
                .unwrap();
            files.insert(
                name.to_owned(),
                FileRecord {
                    bytes,
                    sha256: sha256.into(),
                },
            );
        }
        let mut manifest = ModelManifest {
            id: MODEL_ID.into(),
            source: MODEL_SOURCE.into(),
            archive_sha256: ARCHIVE_SHA256.into(),
            license: MODEL_LICENSE.into(),
            license_text: include_str!("../resources/Apache-2.0.txt").into(),
            attribution: LiveModel::EnglishSmall.attribution().into(),
            conversion: LiveModel::EnglishSmall.conversion().into(),
            acquisition_terms: ACQUISITION_TERMS.into(),
            files,
        };
        let path = directory.join("manifest.json");
        fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        assert!(
            manager.installed(),
            "UI setup check uses pinned metadata and file sizes"
        );
        assert!(
            manager.verify().is_err(),
            "engine startup must hash actual contents"
        );
        manifest.files.get_mut(TOKENS).unwrap().sha256 = "0".repeat(64);
        fs::write(path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        assert!(
            !manager.installed(),
            "local manifest cannot replace pinned catalog hashes"
        );
    }

    #[test]
    fn cancelled_network_wait_completes_without_waiting_for_the_response() {
        let cancel = Arc::new(AtomicBool::new(false));
        let trigger = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            trigger.store(true, Ordering::Relaxed);
        });
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let started = std::time::Instant::now();
        let result: Result<()> = runtime.block_on(wait_cancel(std::future::pending(), &cancel));
        assert!(matches!(result, Err(SttError::Cancelled)));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn installation_cancellation_does_not_publish_partial_model() {
        let root = tempfile::tempdir().unwrap();
        let models = ModelManager::new(root.path().into());
        assert!(matches!(
            models.install(|_, _| {}, Arc::new(AtomicBool::new(true))),
            Err(SttError::Cancelled)
        ));
        assert!(!models.installed());
        assert!(!models.model_dir().exists());
    }

    #[test]
    fn cached_archive_requires_the_pinned_checksum() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("bad.tar.bz2");
        fs::write(&path, b"not a model").unwrap();
        let models = ModelManager::new(root.path().join("models"));
        assert!(matches!(
            models.install_from_archive(&path, Arc::new(AtomicBool::new(false))),
            Err(SttError::InvalidModel(_))
        ));
        assert!(!models.installed());
    }
    fn sparse_fixture(root: &Path, model: LiveModel) -> ModelManifest {
        fs::create_dir_all(root).unwrap();
        let files = model
            .files()
            .iter()
            .map(|f| {
                File::create(root.join(f.name))
                    .unwrap()
                    .set_len(f.bytes)
                    .unwrap();
                (
                    f.name.into(),
                    FileRecord {
                        bytes: f.bytes,
                        sha256: f.sha256.into(),
                    },
                )
            })
            .collect();
        ModelManifest {
            id: model.id().into(),
            source: model.source().into(),
            archive_sha256: model.archive_sha256().into(),
            license: model.license().into(),
            license_text: include_str!("../resources/Apache-2.0.txt").into(),
            attribution: model.attribution().into(),
            conversion: model.conversion().into(),
            acquisition_terms: ACQUISITION_TERMS.into(),
            files,
        }
    }
    fn write_manifest(path: &Path, manifest: &ModelManifest) {
        fs::write(
            path.join("manifest.json"),
            serde_json::to_vec(manifest).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn variants_keep_legacy_default_and_bind_every_manifest_field_to_the_selected_catalog() {
        let root = tempfile::tempdir().unwrap();
        let default = ModelManager::new(root.path().into());
        assert_eq!(default.model(), LiveModel::EnglishSmall);
        assert_eq!(default.model_dir(), root.path().join(MODEL_ID));
        assert_eq!(LiveModel::EnglishSmall.installed_bytes(), 72_466_360);
        assert_eq!(LiveModel::EnglishStandard.installed_bytes(), 190_180_941);
        for model in [LiveModel::EnglishSmall, LiveModel::EnglishStandard] {
            let manager = ModelManager::for_model(root.path().into(), model);
            let path = manager.model_dir();
            let mut manifest = sparse_fixture(&path, model);
            write_manifest(&path, &manifest);
            assert!(manager.installed());
            assert_eq!(verify_manifest(&path, false).unwrap(), model);
            assert!(
                manager.verify().is_err(),
                "sparse bytes never satisfy full hashes"
            );
            for field in [
                "id",
                "source",
                "archive_sha256",
                "license",
                "license_text",
                "attribution",
                "conversion",
                "acquisition_terms",
            ] {
                let mut json = serde_json::to_value(&manifest).unwrap();
                json[field] = "untrusted replacement".into();
                fs::write(
                    path.join("manifest.json"),
                    serde_json::to_vec(&json).unwrap(),
                )
                .unwrap();
                assert!(!manager.installed(), "{model:?} {field}");
            }
            manifest.files.get_mut(TOKENS).unwrap().sha256 = "f".repeat(64);
            write_manifest(&path, &manifest);
            assert!(!manager.installed());
            manifest.files.get_mut(TOKENS).unwrap().sha256 = model.files()[3].sha256.into();
            manifest.files.get_mut(TOKENS).unwrap().bytes += 1;
            write_manifest(&path, &manifest);
            assert!(!manager.installed());
            manifest.files.get_mut(TOKENS).unwrap().bytes = model.files()[3].bytes;
            write_manifest(&path, &manifest);
            assert!(manager.installed());
            let mut json = serde_json::to_value(&manifest).unwrap();
            json["files"][TOKENS]["unexpected"] = "untrusted".into();
            fs::write(
                path.join("manifest.json"),
                serde_json::to_vec(&json).unwrap(),
            )
            .unwrap();
            assert!(!manager.installed());
        }
    }
    #[test]
    fn a_valid_other_variant_manifest_cannot_satisfy_an_explicit_manager() {
        let root = tempfile::tempdir().unwrap();
        let manager = ModelManager::new(root.path().into());
        let path = manager.model_dir();
        let manifest = sparse_fixture(&path, LiveModel::EnglishStandard);
        write_manifest(&path, &manifest);
        assert_eq!(
            verify_manifest(&path, false).unwrap(),
            LiveModel::EnglishStandard
        );
        assert!(!manager.installed());
        assert!(manager.verify().is_err());
    }
    #[cfg(unix)]
    #[test]
    fn both_variants_reject_symlinked_weights() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let outside = root.path().join("outside-tokens");
        File::create(&outside).unwrap().set_len(5048).unwrap();
        for model in [LiveModel::EnglishSmall, LiveModel::EnglishStandard] {
            let manager = ModelManager::for_model(root.path().into(), model);
            let path = manager.model_dir();
            let manifest = sparse_fixture(&path, model);
            write_manifest(&path, &manifest);
            fs::remove_file(path.join(TOKENS)).unwrap();
            symlink(&outside, path.join(TOKENS)).unwrap();
            assert!(!manager.installed());
        }
    }
    #[test]
    fn both_variants_use_the_same_root_selection_duplicate_and_unsafe_path_guards() {
        for model in [LiveModel::EnglishSmall, LiveModel::EnglishStandard] {
            let root = tempfile::tempdir().unwrap();
            let required = model.files().map(|f| f.name);
            let names = required.map(|f| format!("{}/{f}", model.id()));
            let mut entries: Vec<_> = names
                .iter()
                .map(|name| (name.as_str(), b"model".as_slice()))
                .collect();
            let valid = archive(&entries);
            let files = extract_catalog(
                valid.as_slice(),
                root.path(),
                &AtomicBool::new(false),
                model.id(),
                &required,
            )
            .unwrap();
            assert_eq!(files.len(), 4);
            let duplicate = tempfile::tempdir().unwrap();
            entries.push((names[0].as_str(), b"duplicate"));
            assert!(
                extract_catalog(
                    archive(&entries).as_slice(),
                    duplicate.path(),
                    &AtomicBool::new(false),
                    model.id(),
                    &required
                )
                .is_err()
            );
            let other = if model == LiveModel::EnglishSmall {
                LiveModel::EnglishStandard
            } else {
                LiveModel::EnglishSmall
            };
            let wrong = tempfile::tempdir().unwrap();
            assert!(
                extract_catalog(
                    valid.as_slice(),
                    wrong.path(),
                    &AtomicBool::new(false),
                    other.id(),
                    &other.files().map(|f| f.name)
                )
                .is_err()
            );
            let encoder = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::fast());
            let mut tar = tar::Builder::new(encoder);
            let mut header = tar::Header::new_gnu();
            header.set_size(4);
            header.set_mode(0o600);
            let bad = format!("{}/../escape", model.id());
            header.as_mut_bytes()[..bad.len()].copy_from_slice(bad.as_bytes());
            header.set_cksum();
            tar.append(&header, b"evil".as_slice()).unwrap();
            let bytes = tar.into_inner().unwrap().finish().unwrap();
            let unsafe_root = tempfile::tempdir().unwrap();
            assert!(
                extract_catalog(
                    bytes.as_slice(),
                    unsafe_root.path(),
                    &AtomicBool::new(false),
                    model.id(),
                    &required
                )
                .is_err()
            );
            assert!(!unsafe_root.path().join("escape").exists());
        }
    }
    #[test]
    fn variant_installation_obeys_cancellation_and_the_shared_install_lock() {
        let root = tempfile::tempdir().unwrap();
        let archive = root.path().join("not-downloaded.tar.bz2");
        fs::write(&archive, b"untrusted").unwrap();
        for model in [LiveModel::EnglishSmall, LiveModel::EnglishStandard] {
            let manager = ModelManager::for_model(root.path().join("models"), model);
            assert!(matches!(
                manager.install_from_archive(&archive, Arc::new(AtomicBool::new(true))),
                Err(SttError::Cancelled)
            ));
            assert!(!manager.model_dir().exists());
            let lock = install_lock(&manager.root).unwrap();
            assert!(matches!(
                manager.install_from_archive(&archive, Arc::new(AtomicBool::new(false))),
                Err(SttError::Busy)
            ));
            drop(lock);
            assert!(matches!(
                manager.install_from_archive(&archive, Arc::new(AtomicBool::new(false))),
                Err(SttError::InvalidModel(_))
            ));
        }
    }
    #[test]
    #[ignore = "explicit cached official archive/native engine test; no network or audio devices"]
    fn cached_standard_archive_installs_and_initializes_the_production_recognizer() {
        let archive = std::env::var_os("VT_STT_STANDARD_ARCHIVE")
            .map(PathBuf::from)
            .expect("set VT_STT_STANDARD_ARCHIVE to the official cached archive");
        let temp = tempfile::tempdir().unwrap();
        let root = std::env::var_os("VT_STT_STANDARD_MODELS_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| temp.path().join("models"));
        let manager = ModelManager::for_model(root.clone(), LiveModel::EnglishStandard);
        manager
            .install_from_archive(&archive, Arc::new(AtomicBool::new(false)))
            .unwrap();
        manager.verify().unwrap();
        assert!(manager.installed());
        let small = ModelManager::new(temp.path().join("wrong-variant-models"));
        assert_eq!(small.model(), LiveModel::EnglishSmall);
        assert!(!small.installed());
        assert!(matches!(
            small.install_from_archive(&archive, Arc::new(AtomicBool::new(false))),
            Err(SttError::InvalidModel(_))
        ));
        let mut recognizer = crate::StreamingRecognizer::new(&manager.model_dir()).unwrap();
        assert!(recognizer.finish().unwrap().is_empty());
        if let Some(vad) = std::env::var_os("VT_STT_TEST_VAD_PATH") {
            let mut recognizer =
                crate::StreamingRecognizer::with_vad(&manager.model_dir(), Some(Path::new(&vad)))
                    .unwrap();
            assert!(recognizer.finish().unwrap().is_empty());
        }
        manager
            .install_from_archive(&archive, Arc::new(AtomicBool::new(false)))
            .unwrap();
        assert!(manager.installed());
    }
}
