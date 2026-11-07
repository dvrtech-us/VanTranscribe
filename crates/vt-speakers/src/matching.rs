//! Pure open-set identification. No release calibration is shipped here, and
//! metadata/settings continue to report matching unavailable. A reviewed release
//! must pin a calibration artifact before any caller may authorize recognition.
//! Gallery and query vectors are borrowed processing memory, never DTOs.
use crate::ChannelType;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use zeroize::Zeroizing;

const MAX_GALLERY: usize = 1024;
const MAX_EXEMPLARS: usize = 10;
const MAX_DIMENSIONS: usize = 1024;
const MAX_CALIBRATION_BYTES: usize = 65_536;
const MIN_AUTO_SPEECH_MS: u64 = 15_000;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum MatchError {
    #[error("The calibration artifact does not match its trusted release pin.")]
    CalibrationAuthentication,
    #[error("The calibration artifact is malformed, unsupported or lacks required coverage.")]
    InvalidCalibration,
    #[error("The voice query or gallery has invalid bounded embedding geometry.")]
    InvalidEmbedding,
    #[error("The identification protocol or fixed trial labels are invalid.")]
    InvalidEvaluation,
}
pub type MatchResult<T> = std::result::Result<T, MatchError>;

/// This is a trust boundary, not a deserializable request. Construct only from
/// constants in reviewed release code after the pinned artifact's independent
/// evidence has been assessed. Hash agreement authenticates those bytes; it does
/// not independently prove the evidence's scientific adequacy. Never derive this
/// pin from a user file's own hash or its claims. There are no built-in pins yet.
pub struct CalibrationPin {
    artifact_sha256: String,
    embedding_contract_sha256: String,
    calibration_version: String,
    channel: ChannelType,
    protocol_id: String,
}
impl CalibrationPin {
    pub fn from_reviewed_release(
        artifact_sha256: &'static str,
        embedding_contract_sha256: &'static str,
        calibration_version: &'static str,
        channel: ChannelType,
        protocol_id: &'static str,
    ) -> Self {
        Self {
            artifact_sha256: artifact_sha256.into(),
            embedding_contract_sha256: embedding_contract_sha256.into(),
            calibration_version: calibration_version.into(),
            channel,
            protocol_id: protocol_id.into(),
        }
    }
}

/// Full comparison identity uses a semantic calibration version rather than
/// the artifact hash: embedding_version can then be included inside the pinned
/// artifact without a circular hash. Changing calibration versions changes this
/// identity, even with identical model weights; old prints are not auto-reused.
pub fn identification_version(
    embedding_contract_sha256: &str,
    calibration_version: &str,
) -> MatchResult<String> {
    if !hash(embedding_contract_sha256) || !identifier(calibration_version, 128) {
        return Err(MatchError::InvalidCalibration);
    }
    Ok(format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(&(
                "vt-speakers/identification-version/v1",
                embedding_contract_sha256,
                calibration_version,
            ))
            .map_err(|_| MatchError::InvalidCalibration)?
        )
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CalibrationArtifact {
    format_version: u32,
    calibration_version: String,
    embedding_contract_sha256: String,
    embedding_version: String,
    channel: ChannelType,
    dimensions: usize,
    protocol_id: String,
    /// A report digest is provenance; it gains no authority from a metric claim.
    evidence_sha256: String,
    min_gallery_people: usize,
    min_auto_speech_ms: u64,
    curve: Vec<ThresholdBucket>,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ThresholdBucket {
    max_gallery_people: usize,
    tau_auto: f64,
    tau_suggest: f64,
    margin: f64,
}

/// Fields stay private so no `verified: true` field or IPC object can construct
/// authorization. Binding authenticates a reviewed artifact plus exact semantic
/// version/channel/protocol coverage; missing coverage is never extrapolated.
pub struct Calibration {
    artifact_sha256: String,
    embedding_version: String,
    calibration_version: String,
    channel: ChannelType,
    dimensions: usize,
    protocol_id: String,
    evidence_sha256: String,
    min_gallery_people: usize,
    min_auto_speech_ms: u64,
    curve: Vec<ThresholdBucket>,
}
impl Calibration {
    pub fn bind(bytes: &[u8], pin: &CalibrationPin) -> MatchResult<Self> {
        if bytes.len() > MAX_CALIBRATION_BYTES
            || !hash(&pin.artifact_sha256)
            || format!("{:x}", Sha256::digest(bytes)) != pin.artifact_sha256
        {
            return Err(MatchError::CalibrationAuthentication);
        }
        let artifact: CalibrationArtifact =
            serde_json::from_slice(bytes).map_err(|_| MatchError::InvalidCalibration)?;
        validate_artifact(&artifact, pin)?;
        Ok(Self {
            artifact_sha256: pin.artifact_sha256.clone(),
            embedding_version: artifact.embedding_version,
            calibration_version: artifact.calibration_version,
            channel: artifact.channel,
            dimensions: artifact.dimensions,
            protocol_id: artifact.protocol_id,
            evidence_sha256: artifact.evidence_sha256,
            min_gallery_people: artifact.min_gallery_people,
            min_auto_speech_ms: artifact.min_auto_speech_ms,
            curve: artifact.curve,
        })
    }
    pub fn embedding_version(&self) -> &str {
        &self.embedding_version
    }
    pub fn calibration_version(&self) -> &str {
        &self.calibration_version
    }
    pub fn protocol_id(&self) -> &str {
        &self.protocol_id
    }
    pub fn artifact_sha256(&self) -> &str {
        &self.artifact_sha256
    }
    pub fn evidence_sha256(&self) -> &str {
        &self.evidence_sha256
    }
    fn threshold(&self, people: usize) -> Option<&ThresholdBucket> {
        (people >= self.min_gallery_people)
            .then(|| {
                self.curve
                    .iter()
                    .find(|bucket| people <= bucket.max_gallery_people)
            })
            .flatten()
    }
}
fn hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn identifier(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.' | b':' | b'/'))
}
fn validate_artifact(a: &CalibrationArtifact, pin: &CalibrationPin) -> MatchResult<()> {
    let invalid = || MatchError::InvalidCalibration;
    if a.format_version != 1
        || a.calibration_version != pin.calibration_version
        || a.embedding_contract_sha256 != pin.embedding_contract_sha256
        || a.embedding_version
            != identification_version(&pin.embedding_contract_sha256, &pin.calibration_version)?
        || a.channel != pin.channel
        || a.protocol_id != pin.protocol_id
        || !identifier(&a.protocol_id, 200)
        || !hash(&a.evidence_sha256)
        || !(2..=MAX_DIMENSIONS).contains(&a.dimensions)
        || a.min_gallery_people == 0
        || a.min_gallery_people > MAX_GALLERY
        || !(MIN_AUTO_SPEECH_MS..=120_000).contains(&a.min_auto_speech_ms)
        || a.curve.is_empty()
        || a.curve.len() > 16
    {
        return Err(invalid());
    }
    let mut previous: Option<&ThresholdBucket> = None;
    for bucket in &a.curve {
        if bucket.max_gallery_people < a.min_gallery_people
            || bucket.max_gallery_people > MAX_GALLERY
            || !bucket.tau_auto.is_finite()
            || !bucket.tau_suggest.is_finite()
            || !bucket.margin.is_finite()
            || !(-1.0..=1.0).contains(&bucket.tau_auto)
            || !(-1.0..=1.0).contains(&bucket.tau_suggest)
            || !(0.0..=2.0).contains(&bucket.margin)
            || bucket.margin == 0.0
            || bucket.tau_suggest > bucket.tau_auto
        {
            return Err(invalid());
        }
        if previous.is_some_and(|p| {
            p.max_gallery_people >= bucket.max_gallery_people
                || p.tau_auto > bucket.tau_auto
                || p.tau_suggest > bucket.tau_suggest
                || p.margin > bucket.margin
        }) {
            return Err(invalid());
        }
        previous = Some(bucket);
    }
    if a.curve
        .last()
        .is_some_and(|bucket| bucket.max_gallery_people > 50)
    {
        let at50 = a
            .curve
            .iter()
            .find(|bucket| bucket.max_gallery_people == 50)
            .ok_or_else(invalid)?;
        if a.min_gallery_people > 50
            || a.curve
                .iter()
                .filter(|b| b.max_gallery_people > 50)
                .any(|b| b.tau_auto <= at50.tau_auto)
        {
            return Err(invalid());
        }
    }
    Ok(())
}

/// The caller must supply eligible clean speech and only consented, unexpired
/// gallery material. Stale/shared-room entries are additionally excluded here.
/// These are trusted Rust worker inputs, never WebView request DTOs.
pub struct VoiceQuery<'a> {
    pub embedding_version: &'a str,
    pub channel: ChannelType,
    pub clean_speech_ms: u64,
    pub shared_room: bool,
    pub embedding: &'a [f32],
}
pub struct GalleryPerson<'a> {
    pub person_id: &'a str,
    pub embedding_version: &'a str,
    pub channel: ChannelType,
    pub stale: bool,
    pub shared_room: bool,
    pub exemplars: &'a [Vec<f32>],
}
#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MatchOutcome {
    Unavailable {
        reason: UnavailableReason,
    },
    Unknown {
        reason: UnknownReason,
    },
    Suggest {
        person_id: String,
        score: f64,
        margin: Option<f64>,
    },
    Recognized {
        person_id: String,
        score: f64,
        margin: f64,
    },
}
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UnavailableReason {
    MissingCalibration,
    VersionOrChannelNotCovered,
    GalleryNotCovered,
}
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UnknownReason {
    ResearchPolicyAbstention,
    SharedRoom,
    NoCleanSpeech,
    NoEligiblePeople,
    BelowSuggestionThreshold,
}

pub fn identify(
    query: &VoiceQuery<'_>,
    gallery: &[GalleryPerson<'_>],
    calibration: Option<&Calibration>,
) -> MatchResult<MatchOutcome> {
    let unavailable = |reason| Ok(MatchOutcome::Unavailable { reason });
    let unknown = |reason| Ok(MatchOutcome::Unknown { reason });
    let Some(calibration) = calibration else {
        return unavailable(UnavailableReason::MissingCalibration);
    };
    if query.embedding_version != calibration.embedding_version
        || query.channel != calibration.channel
    {
        return unavailable(UnavailableReason::VersionOrChannelNotCovered);
    }
    if query.shared_room {
        return unknown(UnknownReason::SharedRoom);
    }
    if query.clean_speech_ms == 0 {
        return unknown(UnknownReason::NoCleanSpeech);
    }
    let Some(ranking) = rank(query, gallery, calibration.dimensions)? else {
        return unknown(UnknownReason::NoEligiblePeople);
    };
    let Some(threshold) = calibration.threshold(ranking.gallery_people) else {
        return unavailable(UnavailableReason::GalleryNotCovered);
    };
    let person_id = ranking.best_person_id;
    let score = ranking.best_score;
    let margin = ranking.margin;
    // A singleton has no measured runner-up. Suggest rather than inventing one
    // or treating a missing margin as proof of distinct-person separation.
    if score >= threshold.tau_auto
        && query.clean_speech_ms >= calibration.min_auto_speech_ms
        && margin.is_some_and(|margin| margin >= threshold.margin)
    {
        return Ok(MatchOutcome::Recognized {
            person_id,
            score,
            margin: margin.expect("checked margin"),
        });
    }
    if score >= threshold.tau_suggest {
        return Ok(MatchOutcome::Suggest {
            person_id,
            score,
            margin,
        });
    }
    unknown(UnknownReason::BelowSuggestionThreshold)
}
/// Research scores only. This result never authorizes a label, suggestion,
/// learning, or expiry refresh. It contains no vectors and requires the same
/// exact-version/channel and distinct-person rules as calibrated identification.
#[derive(Debug, Serialize, PartialEq)]
pub struct ResearchRanking {
    pub best_person_id: String,
    pub best_score: f64,
    pub runner_up_person_id: Option<String>,
    pub runner_up_score: Option<f64>,
    pub margin: Option<f64>,
    pub gallery_people: usize,
}
pub fn rank_for_evaluation(
    query: &VoiceQuery<'_>,
    gallery: &[GalleryPerson<'_>],
) -> MatchResult<Option<ResearchRanking>> {
    if !hash(query.embedding_version) {
        return Err(MatchError::InvalidEmbedding);
    }
    if query.shared_room || query.clean_speech_ms == 0 {
        return Ok(None);
    }
    rank(query, gallery, query.embedding.len())
}
fn rank(
    query: &VoiceQuery<'_>,
    gallery: &[GalleryPerson<'_>],
    dimensions: usize,
) -> MatchResult<Option<ResearchRanking>> {
    if gallery.len() > MAX_GALLERY {
        return Err(MatchError::InvalidEmbedding);
    }
    let query_vector = normalized(query.embedding, dimensions)?;
    let mut people = BTreeMap::<&str, Zeroizing<Vec<f64>>>::new();
    for person in gallery {
        if person.embedding_version != query.embedding_version
            || person.channel != query.channel
            || person.stale
            || person.shared_room
        {
            continue;
        }
        if !identifier(person.person_id, 200)
            || person.exemplars.is_empty()
            || person.exemplars.len() > MAX_EXEMPLARS
            || people.contains_key(person.person_id)
        {
            return Err(MatchError::InvalidEmbedding);
        }
        let mut centroid = Zeroizing::new(vec![0.0; dimensions]);
        for exemplar in person.exemplars {
            let vector = normalized(exemplar, dimensions)?;
            for (sum, value) in centroid.iter_mut().zip(vector.iter()) {
                *sum += value;
            }
        }
        normalize_mean(&mut centroid)?;
        people.insert(person.person_id, centroid);
    }
    if people.is_empty() {
        return Ok(None);
    }
    let mut scores = people
        .iter()
        .map(|(id, mean)| {
            (
                *id,
                query_vector
                    .iter()
                    .zip(mean.iter())
                    .map(|(a, b)| a * b)
                    .sum::<f64>()
                    .clamp(-1.0, 1.0),
            )
        })
        .collect::<Vec<_>>();
    scores.sort_by(|(a_id, a), (b_id, b)| b.total_cmp(a).then_with(|| a_id.cmp(b_id)));
    let (best, best_score) = scores[0];
    let runner_up = scores.get(1);
    Ok(Some(ResearchRanking {
        best_person_id: best.into(),
        best_score,
        runner_up_person_id: runner_up.map(|(id, _)| (*id).into()),
        runner_up_score: runner_up.map(|(_, score)| *score),
        margin: runner_up.map(|(_, second)| (best_score - second).max(0.0)),
        gallery_people: people.len(),
    }))
}
fn normalized(value: &[f32], dimensions: usize) -> MatchResult<Zeroizing<Vec<f64>>> {
    if value.len() != dimensions
        || !(2..=MAX_DIMENSIONS).contains(&dimensions)
        || value.iter().any(|v| !v.is_finite())
    {
        return Err(MatchError::InvalidEmbedding);
    }
    let norm = value
        .iter()
        .map(|v| f64::from(*v).powi(2))
        .sum::<f64>()
        .sqrt();
    if !norm.is_finite() || (norm - 1.0).abs() > 0.001 {
        return Err(MatchError::InvalidEmbedding);
    }
    Ok(Zeroizing::new(
        value.iter().map(|v| f64::from(*v) / norm).collect(),
    ))
}
fn normalize_mean(value: &mut [f64]) -> MatchResult<()> {
    let norm = value.iter().map(|v| v * v).sum::<f64>().sqrt();
    if !norm.is_finite() || norm < 1e-12 {
        return Err(MatchError::InvalidEmbedding);
    }
    for v in value {
        *v /= norm;
    }
    Ok(())
}

/// Labels and gallery are fixed before scoring. Known means the exact person is
/// enrolled; unknown means the person is absent. No permutation or post-hoc name
/// mapping can turn a wrong identity into a success.
pub enum ExpectedIdentity<'a> {
    Known(&'a str),
    Unknown,
}
pub struct IdentificationTrial<'a> {
    pub query_id: &'a str,
    pub expected: ExpectedIdentity<'a>,
    pub outcome: &'a MatchOutcome,
}
pub struct EvaluationProtocol<'a> {
    pub protocol_id: &'a str,
    pub embedding_version: &'a str,
    pub channel: ChannelType,
    pub gallery_person_ids: &'a [&'a str],
}
#[derive(Debug, Serialize, PartialEq)]
pub struct IdentificationReport {
    pub protocol_id: String,
    pub embedding_version: String,
    pub channel: ChannelType,
    pub gallery_people: usize,
    pub gallery_is_50: bool,
    pub known_trials: u64,
    pub unknown_trials: u64,
    pub correctly_recognized: u64,
    pub wrongly_recognized_known: u64,
    pub falsely_recognized_unknown: u64,
    pub known_abstentions: u64,
    pub unknown_abstentions: u64,
    pub correct_suggestions: u64,
    pub wrong_suggestions: u64,
    pub unavailable_trials: u64,
    /// Correct automatic recognition divided by all known trials, including abstentions.
    pub recognition_rate: Option<f64>,
    /// Wrong automatic labels divided by all trials, including known and unknown.
    pub false_identification_rate: Option<f64>,
    pub unknown_false_identification_rate: Option<f64>,
}
/// Counts only. It deliberately issues no acceptance flag or confidence bound:
/// corpus independence, leakage and trial correlation require external review.
pub fn evaluate_identification(
    protocol: &EvaluationProtocol<'_>,
    trials: &[IdentificationTrial<'_>],
) -> MatchResult<IdentificationReport> {
    if !identifier(protocol.protocol_id, 200)
        || !hash(protocol.embedding_version)
        || protocol.gallery_person_ids.is_empty()
        || protocol.gallery_person_ids.len() > MAX_GALLERY
        || trials.len() > 100_000
    {
        return Err(MatchError::InvalidEvaluation);
    }
    let gallery = protocol
        .gallery_person_ids
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    if gallery.len() != protocol.gallery_person_ids.len()
        || gallery.iter().any(|id| !identifier(id, 200))
    {
        return Err(MatchError::InvalidEvaluation);
    }
    let mut report = IdentificationReport {
        protocol_id: protocol.protocol_id.into(),
        embedding_version: protocol.embedding_version.into(),
        channel: protocol.channel,
        gallery_people: gallery.len(),
        gallery_is_50: gallery.len() == 50,
        known_trials: 0,
        unknown_trials: 0,
        correctly_recognized: 0,
        wrongly_recognized_known: 0,
        falsely_recognized_unknown: 0,
        known_abstentions: 0,
        unknown_abstentions: 0,
        correct_suggestions: 0,
        wrong_suggestions: 0,
        unavailable_trials: 0,
        recognition_rate: None,
        false_identification_rate: None,
        unknown_false_identification_rate: None,
    };
    let mut queries = BTreeSet::new();
    for trial in trials {
        if !identifier(trial.query_id, 200) || !queries.insert(trial.query_id) {
            return Err(MatchError::InvalidEvaluation);
        }
        let expected = match trial.expected {
            ExpectedIdentity::Known(id) => {
                if !gallery.contains(id) {
                    return Err(MatchError::InvalidEvaluation);
                }
                report.known_trials += 1;
                Some(id)
            }
            ExpectedIdentity::Unknown => {
                report.unknown_trials += 1;
                None
            }
        };
        match trial.outcome {
            MatchOutcome::Recognized {
                person_id,
                score,
                margin,
            } => {
                if !gallery.contains(person_id.as_str())
                    || !valid_score(*score)
                    || !margin.is_finite()
                    || !(0.0..=2.0).contains(margin)
                {
                    return Err(MatchError::InvalidEvaluation);
                }
                match expected {
                    Some(id) if id == person_id => report.correctly_recognized += 1,
                    Some(_) => report.wrongly_recognized_known += 1,
                    None => report.falsely_recognized_unknown += 1,
                }
            }
            other => {
                if expected.is_some() {
                    report.known_abstentions += 1;
                } else {
                    report.unknown_abstentions += 1;
                }
                if let MatchOutcome::Suggest {
                    person_id,
                    score,
                    margin,
                } = other
                {
                    if !gallery.contains(person_id.as_str())
                        || !valid_score(*score)
                        || margin.is_some_and(|v| !v.is_finite() || !(0.0..=2.0).contains(&v))
                    {
                        return Err(MatchError::InvalidEvaluation);
                    }
                    if expected == Some(person_id.as_str()) {
                        report.correct_suggestions += 1;
                    } else {
                        report.wrong_suggestions += 1;
                    }
                }
                if matches!(other, MatchOutcome::Unavailable { .. }) {
                    report.unavailable_trials += 1;
                }
            }
        }
    }
    let ratio = |numerator: u64, denominator: u64| {
        (denominator > 0).then(|| numerator as f64 / denominator as f64)
    };
    report.recognition_rate = ratio(report.correctly_recognized, report.known_trials);
    report.false_identification_rate = ratio(
        report.wrongly_recognized_known + report.falsely_recognized_unknown,
        report.known_trials + report.unknown_trials,
    );
    report.unknown_false_identification_rate =
        ratio(report.falsely_recognized_unknown, report.unknown_trials);
    Ok(report)
}
fn valid_score(value: f64) -> bool {
    value.is_finite() && (-1.0..=1.0).contains(&value)
}

#[cfg(test)]
mod tests;
