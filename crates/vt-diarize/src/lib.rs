//! Independent audio-derived speaker turns. Unnamed embeddings stay in memory.
//! Explicit enrollment recomputes bounded source embeddings for a separately
//! consented, encrypted store owned by the session; this crate never persists them.
mod cluster;
mod diagnostics;
mod engine;
mod finalize;
mod models;
mod voice_embedding;
mod voice_profile;
pub use voice_embedding::{
    VoiceEmbedder, voice_embedding_contract_sha256, voice_embedding_version,
};
pub use voice_profile::VoiceEmbeddingProfile;
#[cfg(test)]
mod tests;
pub use diagnostics::{CapacityLimit, Diagnostics, TrackDiagnostics};
pub use engine::Diarizer;
pub use models::{
    EMBEDDING, EMBEDDING_SOURCE, EMBEDDING_VERSION, MODEL_ID, MODEL_NOTICE, ModelManager,
    SEGMENTATION, SEGMENTATION_SOURCE,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;
#[derive(Debug, Error)]
pub enum DiarizeError {
    #[error("Invalid diarization input: {0}")]
    Invalid(&'static str),
    #[error("Diarization model setup is missing or does not match pinned files.")]
    Model,
    #[error("Local diarization inference failed.")]
    Engine,
    #[error("Diarization cancelled.")]
    Cancelled,
    #[error(
        "Diarization capacity reached; previous results must not be published as complete. (limit: {0})"
    )]
    Capacity(CapacityLimit),
    #[error("Speaker constraints conflict or refer to unavailable turns.")]
    Constraint,
    #[error("Explicit model acquisition acknowledgement is required.")]
    Terms,
    #[error("Diarization model installation failed: {0}")]
    Install(&'static str),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}
impl DiarizeError {
    pub fn capacity_limit(&self) -> Option<CapacityLimit> {
        match self {
            Self::Capacity(l) => Some(*l),
            _ => None,
        }
    }
}
pub type Result<T> = std::result::Result<T, DiarizeError>;
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MeetingMode {
    #[default]
    Remote,
    InPerson,
}
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Live,
    #[default]
    Refined,
}
/// Source context is distinct from the pinned model's native input and from
/// the nonduplicated prefix owned by each steady-state inference window.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SegmentationContext {
    pub source_window_ms: u32,
    pub ownership_stride_ms: u32,
    pub native_window_ms: u32,
}
impl Phase {
    pub const fn segmentation_context(self) -> SegmentationContext {
        SegmentationContext {
            source_window_ms: match self {
                Self::Live => 5_000,
                Self::Refined => 10_000,
            },
            ownership_stride_ms: 2_500,
            native_window_ms: 10_000,
        }
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConstraintKind {
    MustLink,
    CannotLink,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PairConstraint {
    pub left_turn_id: String,
    pub right_turn_id: String,
    pub kind: ConstraintKind,
    pub provenance: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Anchor {
    pub track: String,
    pub start_ms: u64,
    pub end_ms: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TemporalConstraint {
    pub left: Anchor,
    pub right: Anchor,
    pub kind: ConstraintKind,
    pub provenance: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DiarizerConfig {
    pub mode: MeetingMode,
    pub phase: Phase,
    pub constraints: Vec<PairConstraint>,
    pub temporal_constraints: Vec<TemporalConstraint>,
    pub live_speaker_prior: Option<usize>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SpeakerTurn {
    pub id: String,
    pub track: String,
    pub start_ms: u64,
    pub end_ms: u64,
    pub speaker_ids: Vec<String>,
    pub label: String,
    pub provenance: String,
    pub score: Option<f32>,
    pub overlap: bool,
    pub uncertain: bool,
    pub provisional: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Speaker {
    pub id: String,
    pub track: String,
    pub label: String,
    pub speech_ms: u64,
    pub provisional: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SpeakerMerge {
    pub from: String,
    pub into: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConstraintReview {
    pub provenance: String,
    pub reason: String,
    pub anchors: Vec<Anchor>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DiarizationResult {
    pub turns: Vec<SpeakerTurn>,
    pub speakers: Vec<Speaker>,
    pub merges: Vec<SpeakerMerge>,
    pub embedding_version: String,
    pub calibrated: bool,
    pub warnings: Vec<String>,
    #[serde(default)]
    pub constraint_reviews: Vec<ConstraintReview>,
    #[serde(default)]
    pub segmentation_context: Option<SegmentationContext>,
}
/// Timestamp midpoint mapping; a crossing segment is never treated as one voice.
/// Returns every assignment active at the midpoint, including explicit overlap.
pub fn turns_at_midpoint<'a>(
    turns: &'a [SpeakerTurn],
    track: &str,
    start_ms: u64,
    end_ms: u64,
) -> Vec<&'a SpeakerTurn> {
    if end_ms < start_ms {
        return Vec::new();
    }
    let mid = start_ms.saturating_add(end_ms.saturating_sub(start_ms) / 2);
    turns
        .iter()
        .filter(|turn| turn.track == track && turn.start_ms <= mid && mid < turn.end_ms)
        .collect()
}

/// Resolve persisted time anchors against this run. IDs are never reused across
/// different segmentations. Ambiguous or under-covered anchors require review.
pub fn resolve_temporal_constraints(
    turns: &[SpeakerTurn],
    constraints: &[TemporalConstraint],
) -> Result<Vec<PairConstraint>> {
    fn resolve(turns: &[SpeakerTurn], anchor: &Anchor) -> Result<String> {
        if !matches!(anchor.track.as_str(), "mic" | "system") || anchor.end_ms <= anchor.start_ms {
            return Err(DiarizeError::Constraint);
        }
        let mut candidates = turns
            .iter()
            .filter(|t| {
                t.track == anchor.track
                    && !t.overlap
                    && t.end_ms > anchor.start_ms
                    && t.start_ms < anchor.end_ms
            })
            .map(|t| {
                (
                    t,
                    t.end_ms.min(anchor.end_ms) - t.start_ms.max(anchor.start_ms),
                )
            })
            .collect::<Vec<_>>();
        candidates.sort_by_key(|(_, overlap)| std::cmp::Reverse(*overlap));
        let Some((turn, best)) = candidates.first() else {
            return Err(DiarizeError::Constraint);
        };
        if best.saturating_mul(2) < anchor.end_ms - anchor.start_ms
            || candidates.get(1).is_some_and(|(_, next)| next == best)
        {
            return Err(DiarizeError::Constraint);
        }
        Ok(turn.id.clone())
    }
    constraints
        .iter()
        .map(|c| {
            if c.provenance.trim().is_empty() || c.left.track != c.right.track {
                return Err(DiarizeError::Constraint);
            }
            let left_turn_id = resolve(turns, &c.left)?;
            let right_turn_id = resolve(turns, &c.right)?;
            if left_turn_id == right_turn_id && c.kind == ConstraintKind::CannotLink {
                return Err(DiarizeError::Constraint);
            }
            Ok(PairConstraint {
                left_turn_id,
                right_turn_id,
                kind: c.kind,
                provenance: c.provenance.clone(),
            })
        })
        .collect()
}
