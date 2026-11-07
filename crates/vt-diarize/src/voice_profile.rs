//! Explicit extractor identities. Research profiles do not change the installed
//! diarizer, existing enrollment versions, or authorize identification.
use crate::{DiarizeError, Result, models};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{fs, fs::File, io::Read, path::Path, str::FromStr};

const WESPEAKER_BYTES: u64 = 26_534_365;
const WESPEAKER_SHA256: &str = "5ef208a9da1453335308a6b6f4e6dfbd7e183a38b604de0a57664f45d257fe94";
const WESPEAKER_CMN_BYTES: u64 = 26_534_404;
const WESPEAKER_CMN_SHA256: &str =
    "6928c40c3b5c695352998c261cb6f2250c4e52a991e324c0f63c2e10fe7c31f8";
const CMN_TRANSFORMATION: &str = "onnx-metadata-feature-normalize-global-mean-v1";
const WESPEAKER_SOURCE: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-recongition-models/wespeaker_en_voxceleb_resnet34.onnx";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum VoiceEmbeddingProfile {
    #[default]
    #[serde(rename = "eres2net-v1")]
    Eres2NetV1,
    #[serde(rename = "wespeaker-resnet34-v1")]
    WeSpeakerResNet34V1,
    #[serde(rename = "wespeaker-resnet34-cmn-v1")]
    WeSpeakerResNet34CmnV1,
    #[serde(rename = "wespeaker-resnet34-reference-frontend-v1")]
    WeSpeakerResNet34ReferenceFrontendV1,
}
impl VoiceEmbeddingProfile {
    pub const fn id(self) -> &'static str {
        match self {
            Self::Eres2NetV1 => "eres2net-v1",
            Self::WeSpeakerResNet34V1 => "wespeaker-resnet34-v1",
            Self::WeSpeakerResNet34CmnV1 => "wespeaker-resnet34-cmn-v1",
            Self::WeSpeakerResNet34ReferenceFrontendV1 => {
                "wespeaker-resnet34-reference-frontend-v1"
            }
        }
    }
    pub const fn dimensions(self) -> usize {
        match self {
            Self::Eres2NetV1 => 512,
            Self::WeSpeakerResNet34V1
            | Self::WeSpeakerResNet34CmnV1
            | Self::WeSpeakerResNet34ReferenceFrontendV1 => 256,
        }
    }
    fn research_weight(self) -> (u64, &'static str) {
        match self {
            Self::WeSpeakerResNet34CmnV1 | Self::WeSpeakerResNet34ReferenceFrontendV1 => {
                (WESPEAKER_CMN_BYTES, WESPEAKER_CMN_SHA256)
            }
            Self::WeSpeakerResNet34V1 => (WESPEAKER_BYTES, WESPEAKER_SHA256),
            Self::Eres2NetV1 => (39_593_761, models::embedding_weight_hash()),
        }
    }
    fn uses_cmn_metadata(self) -> bool {
        matches!(
            self,
            Self::WeSpeakerResNet34CmnV1 | Self::WeSpeakerResNet34ReferenceFrontendV1
        )
    }
    fn transformation(self) -> &'static str {
        if self.uses_cmn_metadata() {
            CMN_TRANSFORMATION
        } else {
            "unmodified"
        }
    }
    fn notice(self) -> &'static [u8] {
        if self.uses_cmn_metadata() {
            include_bytes!("../resources/WeSpeaker-CMN-NOTICE.txt")
        } else {
            include_bytes!("../resources/WeSpeaker-NOTICE.txt")
        }
    }
    fn research_contract(self) -> String {
        if self == Self::WeSpeakerResNet34ReferenceFrontendV1 {
            return format!(
                "vantranscribe-voice-enrollment-v2\nprofile={}\nweights={}\nsherpa-onnx=2.0.1;wespeaker;reference-frontend-v1;mono-f32\nrate=16000;resampler=sherpa-linear-per-span\nfrontend=hamming;snip_edges=true;high_freq=0;low_freq=20;bins=80;frame_length_ms=25;frame_shift_ms=10;dither=0;remove_dc_offset=true;preemphasis=0.97;round_to_power_of_two=true;normalize_samples=false\nfeature-normalization=exported-graph-and-native-metadata\ndimensions={}\nnormalization=individual-l2-f32;mean-duration-weighted-f64-l2\n",
                self.id(),
                self.research_weight().1,
                self.dimensions()
            );
        }
        format!(
            "vantranscribe-voice-enrollment-v2\nprofile={}\nweights={}\nsherpa-onnx=2.0.1;wespeaker;native-metadata-features;mono-f32\nrate=16000;resampler=sherpa-linear-per-span\nfeature-normalization=exported-graph-and-native-metadata\ndimensions={}\nnormalization=individual-l2-f32;mean-duration-weighted-f64-l2\n",
            self.id(),
            self.research_weight().1,
            self.dimensions()
        )
    }
    /// Raw preprocessing/weight identity, without any identification authority.
    pub fn contract_sha256(self) -> String {
        match self {
            Self::Eres2NetV1 => crate::voice_embedding_contract_sha256(),
            Self::WeSpeakerResNet34V1
            | Self::WeSpeakerResNet34CmnV1
            | Self::WeSpeakerResNet34ReferenceFrontendV1 => {
                digest(self.research_contract().as_bytes())
            }
        }
    }
    pub fn unavailable_version(self) -> String {
        match self {
            Self::Eres2NetV1 => crate::voice_embedding_version(),
            Self::WeSpeakerResNet34V1
            | Self::WeSpeakerResNet34CmnV1
            | Self::WeSpeakerResNet34ReferenceFrontendV1 => digest(
                format!("{}calibration=unavailable-v1\n", self.research_contract()).as_bytes(),
            ),
        }
    }
    /// Checks the pinned pack without constructing an extractor.
    pub fn verify_research_pack(self, directory: &Path) -> Result<()> {
        self.verify_model(directory)
    }
    pub(crate) fn verify_model(self, directory: &Path) -> Result<()> {
        if self == Self::Eres2NetV1 {
            return models::verify(directory, true);
        }
        let raw = read_regular(&directory.join("manifest.json"), 32 * 1024)?;
        let manifest: ResearchManifest =
            serde_json::from_slice(&raw).map_err(|_| DiarizeError::Model)?;
        self.validate_manifest(&manifest)?;
        for (name, expected) in [
            (
                "Apache-2.0.txt",
                include_bytes!("../resources/Apache-2.0.txt").as_slice(),
            ),
            ("WeSpeaker-NOTICE.txt", self.notice()),
        ] {
            if read_regular(&directory.join(name), 32 * 1024)? != expected {
                return Err(DiarizeError::Model);
            }
        }
        self.verify_weights(directory)
    }
    fn validate_manifest(self, manifest: &ResearchManifest) -> Result<()> {
        if manifest.embedding_profile != self
            || manifest.embedding_contract_sha256 != self.contract_sha256()
            || manifest.embedding_weight_sha256 != self.research_weight().1
            || manifest.embedding_weight_bytes != self.research_weight().0
            || manifest.dimensions != self.dimensions()
            || manifest.source_url != WESPEAKER_SOURCE
            || manifest.license_notices != ["CC-BY-4.0", "Apache-2.0"]
            || manifest.transformation != self.transformation()
            || !manifest.research_only
        {
            return Err(DiarizeError::Model);
        }
        Ok(())
    }
    fn verify_weights(self, directory: &Path) -> Result<()> {
        let path = directory.join(crate::EMBEDDING);
        let meta = fs::symlink_metadata(&path).map_err(|_| DiarizeError::Model)?;
        if !meta.is_file() || meta.len() != self.research_weight().0 {
            return Err(DiarizeError::Model);
        }
        let mut reader = File::open(path)?;
        let mut hasher = Sha256::new();
        let mut buffer = [0u8; 64 * 1024];
        let mut bytes = 0u64;
        loop {
            let n = reader.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            bytes += n as u64;
            if bytes > self.research_weight().0 {
                return Err(DiarizeError::Model);
            }
            hasher.update(&buffer[..n]);
        }
        if bytes != self.research_weight().0
            || format!("{:x}", hasher.finalize()) != self.research_weight().1
        {
            return Err(DiarizeError::Model);
        }
        Ok(())
    }
}
impl FromStr for VoiceEmbeddingProfile {
    type Err = DiarizeError;
    fn from_str(value: &str) -> Result<Self> {
        match value {
            "eres2net-v1" => Ok(Self::Eres2NetV1),
            "wespeaker-resnet34-v1" => Ok(Self::WeSpeakerResNet34V1),
            "wespeaker-resnet34-cmn-v1" => Ok(Self::WeSpeakerResNet34CmnV1),
            "wespeaker-resnet34-reference-frontend-v1" => {
                Ok(Self::WeSpeakerResNet34ReferenceFrontendV1)
            }
            _ => Err(DiarizeError::Invalid("unknown voice embedding profile")),
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ResearchManifest {
    embedding_profile: VoiceEmbeddingProfile,
    embedding_contract_sha256: String,
    embedding_weight_sha256: String,
    embedding_weight_bytes: u64,
    dimensions: usize,
    source_url: String,
    license_notices: Vec<String>,
    research_only: bool,
    #[serde(default = "unmodified")]
    transformation: String,
}
fn unmodified() -> String {
    "unmodified".into()
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn read_regular(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let meta = fs::symlink_metadata(path).map_err(|_| DiarizeError::Model)?;
    if !meta.is_file() || meta.len() > limit {
        return Err(DiarizeError::Model);
    }
    let mut bytes = Vec::new();
    File::open(path)?.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(DiarizeError::Model);
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn manifest() -> serde_json::Value {
        json!({
            "embedding_profile":"wespeaker-resnet34-v1",
            "embedding_contract_sha256":VoiceEmbeddingProfile::WeSpeakerResNet34V1.contract_sha256(),
            "embedding_weight_sha256":WESPEAKER_SHA256,
            "embedding_weight_bytes":WESPEAKER_BYTES,
            "dimensions":256,"source_url":WESPEAKER_SOURCE,"license_notices":["CC-BY-4.0","Apache-2.0"],
            "research_only":true
        })
    }
    fn write_manifest(directory: &Path, value: &serde_json::Value) {
        fs::write(
            directory.join("manifest.json"),
            serde_json::to_vec(value).unwrap(),
        )
        .unwrap();
    }
    #[test]
    fn explicit_profile_ids_and_versions_do_not_reinterpret_existing_voiceprints() {
        let old = VoiceEmbeddingProfile::default();
        let new = VoiceEmbeddingProfile::WeSpeakerResNet34V1;
        assert_eq!(old, VoiceEmbeddingProfile::Eres2NetV1);
        assert_eq!(old.dimensions(), 512);
        assert_eq!(new.dimensions(), 256);
        assert_eq!(
            old.contract_sha256(),
            crate::voice_embedding_contract_sha256()
        );
        assert_eq!(old.unavailable_version(), crate::voice_embedding_version());
        assert_eq!(
            new.contract_sha256(),
            "638b9f488bb743fb3c09eac905d8e9711469f129e88f57ffcfa19c9abdae1094"
        );
        assert_eq!(
            new.unavailable_version(),
            "4e71553593f13bb96e7d823b43f48451e9f5787bfd881883a0c12b5d242f9522"
        );
        assert_ne!(old.contract_sha256(), new.contract_sha256());
        assert_ne!(new.contract_sha256(), new.unavailable_version());
        for profile in [old, new] {
            assert_eq!(
                profile.id().parse::<VoiceEmbeddingProfile>().unwrap(),
                profile
            );
            assert_eq!(
                serde_json::from_value::<VoiceEmbeddingProfile>(json!(profile.id())).unwrap(),
                profile
            );
        }
        assert!("wespeaker".parse::<VoiceEmbeddingProfile>().is_err());
        assert!(serde_json::from_value::<VoiceEmbeddingProfile>(json!("untrusted-model")).is_err());
    }
    #[test]
    fn research_model_rejects_forged_metadata_or_correct_size_with_wrong_weights() {
        let folder = tempfile::tempdir().unwrap();
        let profile = VoiceEmbeddingProfile::WeSpeakerResNet34V1;
        let model = File::create(folder.path().join(crate::EMBEDDING)).unwrap();
        model.set_len(WESPEAKER_BYTES).unwrap();
        fs::write(
            folder.path().join("Apache-2.0.txt"),
            include_bytes!("../resources/Apache-2.0.txt"),
        )
        .unwrap();
        fs::write(
            folder.path().join("WeSpeaker-NOTICE.txt"),
            include_bytes!("../resources/WeSpeaker-NOTICE.txt"),
        )
        .unwrap();
        profile
            .validate_manifest(&serde_json::from_value(manifest()).unwrap())
            .unwrap();
        for (field, value) in [
            ("embedding_weight_sha256", json!("f".repeat(64))),
            ("embedding_profile", json!("eres2net-v1")),
            ("dimensions", json!(512)),
            ("source_url", json!("https://example.invalid/model")),
            ("research_only", json!(false)),
            ("license_notices", json!(["Apache-2.0"])),
        ] {
            let mut forged = manifest();
            forged[field] = value;
            let forged: ResearchManifest = serde_json::from_value(forged).unwrap();
            assert!(profile.validate_manifest(&forged).is_err(), "{field}");
        }
        write_manifest(folder.path(), &manifest());
        // Filename and byte count cannot substitute for the hard-coded digest.
        assert!(profile.verify_model(folder.path()).is_err());
        #[cfg(unix)]
        {
            fs::remove_file(folder.path().join("manifest.json")).unwrap();
            fs::write(
                folder.path().join("elsewhere.json"),
                serde_json::to_vec(&manifest()).unwrap(),
            )
            .unwrap();
            std::os::unix::fs::symlink("elsewhere.json", folder.path().join("manifest.json"))
                .unwrap();
            assert!(profile.verify_model(folder.path()).is_err());
        }
    }
    fn cmn_manifest() -> serde_json::Value {
        let mut value = manifest();
        value["embedding_profile"] = json!("wespeaker-resnet34-cmn-v1");
        value["embedding_contract_sha256"] =
            json!(VoiceEmbeddingProfile::WeSpeakerResNet34CmnV1.contract_sha256());
        value["embedding_weight_sha256"] = json!(WESPEAKER_CMN_SHA256);
        value["embedding_weight_bytes"] = json!(WESPEAKER_CMN_BYTES);
        value["transformation"] = json!(CMN_TRANSFORMATION);
        value
    }
    #[test]
    fn explicit_cmn_identity_and_transform_do_not_reinterpret_either_existing_profile() {
        let cmn = VoiceEmbeddingProfile::WeSpeakerResNet34CmnV1;
        let ordinary = VoiceEmbeddingProfile::WeSpeakerResNet34V1;
        assert_eq!(cmn.dimensions(), 256);
        assert_eq!(
            cmn.contract_sha256(),
            "1f1d6fbd14574251738805424bcd3f94c66c41f08ab23ef72d3d83e89fe32ba0"
        );
        assert_eq!(
            cmn.unavailable_version(),
            "f08d4bc75e641ecd7a8a42f55acc10df42a1e8e38970679e9b4d8a5caf6f4ff9"
        );
        let reference = VoiceEmbeddingProfile::WeSpeakerResNet34ReferenceFrontendV1;
        assert_eq!(reference.dimensions(), 256);
        assert_eq!(
            reference.contract_sha256(),
            "27d8ed7041366fde7527d485bc6e5b2970f9b500a3cbd8f244d8f2d0e0c3c7bd"
        );
        assert_eq!(
            reference.unavailable_version(),
            "0aba11d81ffddc6845c8f9bcc376c1c55f17168c479e6b6e7c49ce344ef9e8ec"
        );
        assert_ne!(reference.contract_sha256(), cmn.contract_sha256());
        assert_eq!(
            reference.id().parse::<VoiceEmbeddingProfile>().unwrap(),
            reference
        );
        assert_ne!(cmn.contract_sha256(), ordinary.contract_sha256());
        assert_ne!(
            cmn.unavailable_version(),
            VoiceEmbeddingProfile::Eres2NetV1.unavailable_version()
        );
        assert_eq!(cmn.id().parse::<VoiceEmbeddingProfile>().unwrap(), cmn);
        assert_eq!(
            serde_json::from_value::<VoiceEmbeddingProfile>(json!(cmn.id())).unwrap(),
            cmn
        );
        assert_ne!(cmn.notice(), ordinary.notice());
        // Omission remains compatible only with the unchanged ordinary profile.
        let ordinary_manifest: ResearchManifest = serde_json::from_value(manifest()).unwrap();
        assert_eq!(ordinary_manifest.transformation, "unmodified");
        ordinary.validate_manifest(&ordinary_manifest).unwrap();
        assert!(cmn.validate_manifest(&ordinary_manifest).is_err());
        let valid: ResearchManifest = serde_json::from_value(cmn_manifest()).unwrap();
        cmn.validate_manifest(&valid).unwrap();
        assert!(ordinary.validate_manifest(&valid).is_err());
        for (field, value) in [
            ("transformation", json!("unmodified")),
            ("embedding_weight_sha256", json!(WESPEAKER_SHA256)),
            ("embedding_weight_bytes", json!(WESPEAKER_BYTES)),
            (
                "embedding_contract_sha256",
                json!(ordinary.contract_sha256()),
            ),
            ("embedding_profile", json!(ordinary.id())),
        ] {
            let mut forged = cmn_manifest();
            forged[field] = value;
            assert!(
                cmn.validate_manifest(&serde_json::from_value(forged).unwrap())
                    .is_err(),
                "{field}"
            );
        }
        let mut omitted = cmn_manifest();
        omitted.as_object_mut().unwrap().remove("transformation");
        assert!(
            cmn.validate_manifest(&serde_json::from_value(omitted).unwrap())
                .is_err()
        );
    }
    #[test]
    fn derived_model_size_and_manifest_cannot_substitute_for_its_pinned_bytes() {
        let folder = tempfile::tempdir().unwrap();
        let profile = VoiceEmbeddingProfile::WeSpeakerResNet34CmnV1;
        File::create(folder.path().join(crate::EMBEDDING))
            .unwrap()
            .set_len(WESPEAKER_CMN_BYTES)
            .unwrap();
        fs::write(
            folder.path().join("Apache-2.0.txt"),
            include_bytes!("../resources/Apache-2.0.txt"),
        )
        .unwrap();
        fs::write(folder.path().join("WeSpeaker-NOTICE.txt"), profile.notice()).unwrap();
        write_manifest(folder.path(), &cmn_manifest());
        assert!(profile.verify_model(folder.path()).is_err());
    }
    #[test]
    #[ignore = "requires explicit VT_WESPEAKER_CMN_RESEARCH_MODEL_DIR; checks public derived weights/native dimensions only, no audio"]
    fn pinned_public_wespeaker_cmn_profile_initializes_with_declared_dimensions() {
        let directory = std::env::var_os("VT_WESPEAKER_CMN_RESEARCH_MODEL_DIR").unwrap();
        crate::VoiceEmbedder::new_with_profile(
            Path::new(&directory),
            VoiceEmbeddingProfile::WeSpeakerResNet34CmnV1,
        )
        .expect("pinned CMN metadata model and native dimensions must match");
    }
    #[test]
    #[ignore = "requires explicit VT_WESPEAKER_RESEARCH_MODEL_DIR; verifies only public weights and native metadata, opens no devices"]
    fn pinned_public_wespeaker_profile_initializes_with_declared_dimensions() {
        let directory = std::env::var_os("VT_WESPEAKER_RESEARCH_MODEL_DIR").unwrap();
        crate::VoiceEmbedder::new_with_profile(
            Path::new(&directory),
            VoiceEmbeddingProfile::WeSpeakerResNet34V1,
        )
        .expect("pinned model and native dimensions must match the explicit profile");
    }
}
