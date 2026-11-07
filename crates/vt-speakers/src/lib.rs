//! Explicit, consented local biometric memory. No matching is authorized until
//! version-specific, gallery-aware calibration has been validated and shipped.
//! This crate never logs or serializes embedding material through a public DTO.
mod crypto;
mod keys;
mod matching;
pub use matching::{
    Calibration, CalibrationPin, EvaluationProtocol, ExpectedIdentity, GalleryPerson,
    IdentificationReport, IdentificationTrial, MatchError, MatchOutcome, MatchResult,
    ResearchRanking, UnavailableReason, UnknownReason, VoiceQuery, evaluate_identification,
    identification_version, identify, rank_for_evaluation,
};
mod store;

pub use keys::{KeyProvider, KeychainProvider, SecretKey};
use serde::{Deserialize, Serialize};
pub use store::{
    delete_all, enroll, expire, forget_person, initialize, settings, status, update_settings,
};

pub type Result<T> = std::result::Result<T, MemoryError>;
#[derive(Debug, thiserror::Error)]
pub enum MemoryError {
    #[error("Speaker memory database operation failed.")]
    Database(#[from] rusqlite::Error),
    #[error("{0}")]
    Invalid(&'static str),
    #[error("Speaker memory is off. Enable it and acknowledge the biometric explanation first.")]
    Disabled,
    #[error(
        "The voiceprint key is unavailable. Unlock Keychain and retry; existing voiceprints were not changed."
    )]
    KeyUnavailable,
    #[error("Voiceprint authentication failed. Existing voiceprints were not changed.")]
    Authentication,
    #[error("Secure random generation failed. No voiceprint was saved.")]
    Random,
    #[error("The selected person no longer exists. Refresh People and retry.")]
    MissingPerson,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MemorySettings {
    pub enabled: bool,
    pub retention_months: u32,
}
impl Default for MemorySettings {
    fn default() -> Self {
        Self {
            enabled: false,
            retention_months: 12,
        }
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChannelType {
    RemoteWideband,
    RemoteNarrowband,
    InPerson,
}
impl ChannelType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RemoteWideband => "remote_wideband",
            Self::RemoteNarrowband => "remote_narrowband",
            Self::InPerson => "in_person",
        }
    }
}
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct MemoryStatus {
    pub enabled: bool,
    pub retention_months: u32,
    pub matching_available: bool,
    /// Metadata only: protected does not claim the Keychain is currently unlocked.
    pub key_status: String,
    pub people: Vec<PersonInfo>,
}
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct PersonInfo {
    pub id: String,
    pub display_name: String,
    pub is_self: bool,
    pub voiceprints: Vec<VoiceprintInfo>,
    pub consent: ConsentInfo,
}
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct VoiceprintInfo {
    pub id: String,
    pub channel_type: String,
    pub channel_provenance: String,
    pub embedding_version: String,
    pub n_exemplars: u32,
    pub last_matched_at: String,
    pub expires_at: String,
    pub stale: bool,
}
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct ConsentInfo {
    pub attested_at: String,
    pub method: String,
    pub attested_by: String,
}

/// Trusted enrollment boundary, never a WebView DTO. The session computes the
/// mean from eligible refined speech and supplies its actual source duration.
/// This type deliberately implements neither Debug nor Serialize.
pub struct Enrollment {
    pub display_name: String,
    pub person_id: Option<String>,
    pub is_self: bool,
    pub written_consent: bool,
    pub consent_method: String,
    pub attested_by: String,
    pub channel_type: ChannelType,
    pub channel_provenance: String,
    pub embedding_version: String,
    pub clean_speech_ms: u64,
    pub shared_room: bool,
    pub embedding: Vec<f32>,
}
impl Drop for Enrollment {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.embedding.zeroize();
    }
}
