//! Research-only public-corpus open-set evaluation. No app database, network,
//! recording, key access, persisted vectors or release calibration pins.
use crate::EvalResult;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    time::{Instant, SystemTime, UNIX_EPOCH},
};
use vt_diarize::VoiceEmbeddingProfile;
use vt_speakers::{
    ChannelType, EvaluationProtocol, ExpectedIdentity, GalleryPerson, IdentificationReport,
    IdentificationTrial, MatchOutcome, ResearchRanking, UnknownReason, VoiceQuery,
    evaluate_identification, rank_for_evaluation,
};
use zeroize::Zeroizing;
const MAX_MANIFEST: u64 = 16 * 1024 * 1024;
const MAX_SAMPLES: usize = 4096;
const MAX_WINDOWS: usize = 120;
const MAX_SPEECH_FRAMES_SECONDS: u64 = 120;
const FIT_ALGORITHM: &str = "fixed-grid-v1: tau_auto=-1.00..1.00 step0.01; margin=0.01..2.00 step0.01; known-wrong/known<=0.02 and unknown-false/unknown<=0.02; maximize known-correct/all-known; ties higher tau then higher margin";
fn err(value: impl std::fmt::Display) -> String {
    value.to_string()
}
fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 200
        && value.bytes().all(|c| {
            c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.' | b':' | b'/' | b'|')
        })
}
fn entity_id(value: &str) -> bool {
    id(value) && !value.contains('|')
}
fn https(value: &str) -> bool {
    value.starts_with("https://") && value.len() <= 2048 && !value.chars().any(char::is_whitespace)
}
fn read_bounded(path: &Path, limit: u64) -> EvalResult<Vec<u8>> {
    let file = File::open(path).map_err(err)?;
    if !file.metadata().map_err(err)?.is_file() {
        return Err("required input is not a regular file".into());
    }
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes).map_err(err)?;
    if bytes.len() as u64 > limit {
        return Err("required input exceeds its size bound".into());
    }
    Ok(bytes)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema_version: u32,
    protocol_id: String,
    channel_type: ChannelType,
    license_url: String,
    annotation_sha256: String,
    partition_sha256: String,
    #[serde(default)]
    annotation_path: Option<PathBuf>,
    #[serde(default)]
    partition_path: Option<PathBuf>,
    model_dir: PathBuf,
    #[serde(default)]
    embedding_profile: VoiceEmbeddingProfile,
    expected_embedding_contract_sha256: String,
    partitions: Vec<Partition>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Partition {
    id: String,
    gallery_people: Vec<String>,
    enrollments: Vec<Sample>,
    probes: Vec<Sample>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Sample {
    id: String,
    person_id: String,
    meeting_id: String,
    project_id: String,
    windows: Vec<Window>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Window {
    path: PathBuf,
    sha256: String,
    source_url: String,
    source_start_frame: u64,
    source_end_frame: u64,
    source_sha256: Option<String>,
    source_etag: String,
    sample_rate: u32,
}
struct Validated {
    manifest: Manifest,
    root: PathBuf,
    manifest_sha256: String,
    window_count: usize,
    window_bytes: u64,
    annotation_verified: bool,
    partition_verified: bool,
}
fn contained(root: &Path, path: &Path) -> EvalResult<PathBuf> {
    if path.is_absolute()
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err("corpus paths must be relative normal components".into());
    }
    let actual = root.join(path).canonicalize().map_err(err)?;
    if !actual.starts_with(root) {
        return Err("corpus path or symlink escapes the manifest directory".into());
    }
    Ok(actual)
}
fn pinned(root: &Path, path: &Path, expected: &str, limit: u64) -> EvalResult<Vec<u8>> {
    if !hash(expected) {
        return Err("required SHA-256 is malformed".into());
    }
    let bytes = read_bounded(&contained(root, path)?, limit)?;
    if sha(&bytes) != expected {
        return Err("required input SHA-256 mismatch".into());
    }
    Ok(bytes)
}
fn wave(bytes: &[u8], window: &Window) -> EvalResult<Zeroizing<Vec<f32>>> {
    let u16_at = |at: usize| u16::from_le_bytes([bytes[at], bytes[at + 1]]);
    let u32_at =
        |at: usize| u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
    if bytes.len() < 44
        || &bytes[..4] != b"RIFF"
        || &bytes[8..12] != b"WAVE"
        || &bytes[12..16] != b"fmt "
        || &bytes[36..40] != b"data"
        || u32_at(4) as usize != bytes.len() - 8
        || u32_at(16) != 16
        || u16_at(20) != 1
        || u16_at(22) != 1
        || u32_at(24) != window.sample_rate
        || u32_at(28) != window.sample_rate * 2
        || u16_at(32) != 2
        || u16_at(34) != 16
        || u32_at(40) as usize != bytes.len() - 44
        || !(bytes.len() - 44).is_multiple_of(2)
    {
        return Err(
            "window must be canonical PCM16 mono RIFF/WAVE with exact header lengths".into(),
        );
    }
    let frames = (bytes.len() - 44) / 2;
    if window
        .source_end_frame
        .checked_sub(window.source_start_frame)
        != Some(frames as u64)
        || frames < window.sample_rate as usize
        || frames > window.sample_rate as usize * 5
    {
        return Err(
            "window frames must match the declared source interval and one-to-five-second duration"
                .into(),
        );
    }
    Ok(Zeroizing::new(
        bytes[44..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| f32::from(i16::from_le_bytes([b[0], b[1]])) / 32768.0)
            .collect(),
    ))
}
fn validate(path: &Path) -> EvalResult<Validated> {
    let bytes = read_bounded(path, MAX_MANIFEST)?;
    let manifest: Manifest = serde_json::from_slice(&bytes).map_err(err)?;
    if manifest.schema_version != 1
        || !entity_id(&manifest.protocol_id)
        || !https(&manifest.license_url)
        || !hash(&manifest.annotation_sha256)
        || !hash(&manifest.partition_sha256)
        || !hash(&manifest.expected_embedding_contract_sha256)
    {
        return Err(
            "voice evaluation requires schema1, explicit provenance and complete hash pins".into(),
        );
    }
    if manifest.expected_embedding_contract_sha256 != manifest.embedding_profile.contract_sha256() {
        return Err(
            "embedding contract differs from the frozen expected version; no inference was run"
                .into(),
        );
    }
    let root = path
        .canonicalize()
        .map_err(err)?
        .parent()
        .ok_or("manifest has no parent")?
        .to_path_buf();
    let mut annotation_verified = false;
    let mut partition_verified = false;
    if let Some(path) = &manifest.annotation_path {
        pinned(&root, path, &manifest.annotation_sha256, 64 * 1024 * 1024)?;
        annotation_verified = true;
    }
    if let Some(path) = &manifest.partition_path {
        let frozen = pinned(&root, path, &manifest.partition_sha256, MAX_MANIFEST)?;
        bind_partition(&manifest, &frozen)?;
        partition_verified = true;
    }
    if manifest.partitions.len() != 2
        || manifest.partitions[0].id != "calibration"
        || manifest.partitions[1].id != "held_out"
    {
        return Err("partitions must be calibration then held_out exactly once".into());
    }
    let mut all_ids = BTreeSet::new();
    let mut partition_people = Vec::new();
    let mut partition_projects = Vec::new();
    let mut source_intervals = BTreeMap::<&str, Vec<(u64, u64)>>::new();
    let mut source_metadata = BTreeMap::<&str, (u32, &str, Option<&str>)>::new();
    let mut all_paths = BTreeSet::new();
    let mut meeting_projects = BTreeMap::new();
    let mut source_partition = BTreeMap::new();
    let mut window_count = 0;
    let mut window_bytes = 0u64;
    let mut sample_count = 0;
    for part in &manifest.partitions {
        let gallery = part
            .gallery_people
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        if gallery.len() != 50
            || part.gallery_people.len() != 50
            || gallery.iter().any(|v| !entity_id(v))
            || part.enrollments.len() != 50
            || part.probes.is_empty()
        {
            return Err(
                "each partition requires exactly fifty unique enrolled people and nonempty probes"
                    .into(),
            );
        }
        let mut people = BTreeSet::new();
        let mut projects = BTreeSet::new();
        let mut enrolled = BTreeMap::new();
        let mut enrollment_sources = BTreeMap::<&str, BTreeSet<&str>>::new();
        let mut known_probes = BTreeSet::new();
        let mut unknown_probes = 0;
        for (is_enrollment, samples) in [(true, &part.enrollments), (false, &part.probes)] {
            for sample in samples {
                sample_count += 1;
                if sample_count > MAX_SAMPLES
                    || !id(&sample.id)
                    || !entity_id(&sample.person_id)
                    || !entity_id(&sample.meeting_id)
                    || !entity_id(&sample.project_id)
                    || !all_ids.insert(sample.id.as_str())
                    || sample.windows.is_empty()
                    || sample.windows.len() > MAX_WINDOWS
                {
                    return Err(
                        "sample IDs/identity metadata or bounded window count are invalid".into(),
                    );
                }
                if let Some(previous) =
                    meeting_projects.insert(sample.meeting_id.as_str(), sample.project_id.as_str())
                    && previous != sample.project_id
                {
                    return Err("one meeting cannot belong to different projects".into());
                }
                people.insert(sample.person_id.as_str());
                projects.insert(sample.project_id.as_str());
                if is_enrollment {
                    if !gallery.contains(sample.person_id.as_str())
                        || enrolled
                            .insert(sample.person_id.as_str(), sample.meeting_id.as_str())
                            .is_some()
                    {
                        return Err(
                            "enrollment must contain each fixed gallery person exactly once".into(),
                        );
                    }
                } else if gallery.contains(sample.person_id.as_str()) {
                    if enrolled.get(sample.person_id.as_str()) == Some(&sample.meeting_id.as_str())
                    {
                        return Err(
                            "known probes must come from a different meeting than enrollment"
                                .into(),
                        );
                    }
                    known_probes.insert(sample.person_id.as_str());
                } else {
                    unknown_probes += 1;
                }
                let sources = sample
                    .windows
                    .iter()
                    .map(|w| w.source_url.as_str())
                    .collect::<BTreeSet<_>>();
                if is_enrollment {
                    enrollment_sources.insert(sample.person_id.as_str(), sources.clone());
                } else if gallery.contains(sample.person_id.as_str())
                    && enrollment_sources
                        .get(sample.person_id.as_str())
                        .is_some_and(|enrolled| !enrolled.is_disjoint(&sources))
                {
                    return Err(
                        "known probes must use a different source recording than enrollment".into(),
                    );
                }
                let sample_rate = sample.windows[0].sample_rate;
                if sources.len() != 1 || sample.windows.iter().any(|w| w.sample_rate != sample_rate)
                {
                    return Err(
                        "each sample must use one recording with a stable native sample rate"
                            .into(),
                    );
                }
                let mut frames = 0u64;
                for window in &sample.windows {
                    if (manifest.channel_type == ChannelType::RemoteWideband
                        && window.sample_rate < 16_000)
                        || !(8_000..=192_000).contains(&window.sample_rate)
                        || !https(&window.source_url)
                        || !hash(&window.sha256)
                        || window.source_sha256.as_ref().is_some_and(|v| !hash(v))
                        || window.source_etag.is_empty()
                        || window.source_etag.len() > 512
                        || window.source_end_frame <= window.source_start_frame
                    {
                        return Err(
                            "window provenance, frame interval or sample rate is invalid".into(),
                        );
                    }
                    if let Some(previous) =
                        source_partition.insert(window.source_url.as_str(), part.id.as_str())
                        && previous != part.id
                    {
                        return Err(
                            "source recordings cannot cross calibration and held_out partitions"
                                .into(),
                        );
                    }
                    let current = (
                        window.sample_rate,
                        window.source_etag.as_str(),
                        window.source_sha256.as_deref(),
                    );
                    if let Some(previous) = source_metadata.insert(&window.source_url, current)
                        && previous != current
                    {
                        return Err(
                            "one source has inconsistent rate/ETag/hash declarations".into()
                        );
                    }
                    let actual = contained(&root, &window.path)?;
                    if !all_paths.insert(actual) {
                        return Err("an audio artifact is reused by multiple windows".into());
                    }
                    let bytes = pinned(&root, &window.path, &window.sha256, 2_000_044)?;
                    wave(&bytes, window)?;
                    window_count += 1;
                    window_bytes += bytes.len() as u64;
                    frames += window.source_end_frame - window.source_start_frame;
                    source_intervals
                        .entry(&window.source_url)
                        .or_default()
                        .push((window.source_start_frame, window.source_end_frame));
                }
                let minimum = if is_enrollment { 20 } else { 15 };
                if frames < u64::from(sample_rate) * minimum
                    || frames > u64::from(sample_rate) * MAX_SPEECH_FRAMES_SECONDS
                {
                    return Err(
                        "sample lacks required clean source duration or exceeds 120 seconds".into(),
                    );
                }
            }
        }
        if enrolled.len() != 50 || known_probes.len() != 50 || unknown_probes == 0 {
            return Err(
                "each partition needs all fifty known people probed and at least one unknown probe"
                    .into(),
            );
        }
        partition_people.push(people);
        partition_projects.push(projects);
    }
    if !partition_people[0].is_disjoint(&partition_people[1])
        || !partition_projects[0].is_disjoint(&partition_projects[1])
    {
        return Err("calibration and held_out must be both person- and project-disjoint".into());
    }
    for intervals in source_intervals.values_mut() {
        intervals.sort_unstable();
        if intervals.windows(2).any(|v| v[0].1 > v[1].0) {
            return Err("source frame intervals overlap or are reused across samples".into());
        }
    }
    Ok(Validated {
        manifest,
        root,
        manifest_sha256: sha(&bytes),
        window_count,
        window_bytes,
        annotation_verified,
        partition_verified,
    })
}
fn bind_partition(manifest: &Manifest, bytes: &[u8]) -> EvalResult<()> {
    let frozen: Value = serde_json::from_slice(bytes).map_err(err)?;
    if frozen["schema_version"] != 1
        || frozen["protocol_id"] != manifest.protocol_id
        || frozen["annotation_archive_sha256"] != manifest.annotation_sha256
        || frozen["channel_type"] != serde_json::to_value(manifest.channel_type).map_err(err)?
    {
        return Err("frozen partition metadata differs from the exported manifest".into());
    }
    let partitions = frozen["partitions"]
        .as_array()
        .ok_or("frozen partition has no partitions")?;
    if partitions.len() != manifest.partitions.len() {
        return Err("frozen partition count differs".into());
    }
    for part in &manifest.partitions {
        let original = partitions
            .iter()
            .find(|p| p["id"] == part.id)
            .ok_or("partition missing from frozen selection")?;
        let originals = original["samples"]
            .as_array()
            .ok_or("frozen selection lacks samples")?;
        let projects = original["project_ids"]
            .as_array()
            .ok_or("frozen selection lacks projects")?;
        if original["gallery_people"] != serde_json::to_value(&part.gallery_people).map_err(err)?
            || originals.len() != part.enrollments.len() + part.probes.len()
        {
            return Err("gallery or sample count differs from frozen selection".into());
        }
        for (role, samples) in [("enrollment", &part.enrollments), ("probe", &part.probes)] {
            for sample in samples {
                let original = originals
                    .iter()
                    .find(|s| s["id"] == sample.id)
                    .ok_or("sample missing from frozen selection")?;
                if original["person_id"] != sample.person_id
                    || original["meeting_id"] != sample.meeting_id
                    || original["role"] != role
                    || !projects
                        .iter()
                        .any(|p| p.as_str() == Some(sample.project_id.as_str()))
                    || !sample.meeting_id.starts_with(&sample.project_id)
                {
                    return Err(
                        "sample identity, project, meeting or role differs from frozen selection"
                            .into(),
                    );
                }
                let windows = original["source_windows_frames"]
                    .as_array()
                    .ok_or("frozen selection lacks frame windows")?;
                if windows.len() != sample.windows.len() {
                    return Err("window count differs from frozen selection".into());
                }
                let mut frames = 0u64;
                for (window, frozen_window) in sample.windows.iter().zip(windows) {
                    if frozen_window != &json!([window.source_start_frame, window.source_end_frame])
                        || original["source_url"] != window.source_url
                    {
                        return Err(
                            "source URL or source frames differ from frozen selection".into()
                        );
                    }
                    frames = frames
                        .checked_add(
                            window
                                .source_end_frame
                                .checked_sub(window.source_start_frame)
                                .ok_or("invalid frozen source interval")?,
                        )
                        .ok_or("frozen source duration overflow")?;
                }
                if original["selected_frames"].as_u64() != Some(frames) {
                    return Err("selected duration differs from frozen selection".into());
                }
            }
        }
    }
    Ok(())
}
trait EmbeddingEngine {
    fn actual_native(&self) -> bool {
        false
    }
    fn extract(&mut self, audio: &[f32], rate: u32) -> EvalResult<Vec<f32>>;
}
impl EmbeddingEngine for vt_diarize::VoiceEmbedder {
    fn actual_native(&self) -> bool {
        true
    }
    fn extract(&mut self, audio: &[f32], rate: u32) -> EvalResult<Vec<f32>> {
        self.extract(audio, rate).map_err(err)
    }
}
struct ReferenceFrontendEngine {
    embedder: vt_speaker_frontend::ReferenceEmbedder,
}
impl EmbeddingEngine for ReferenceFrontendEngine {
    fn actual_native(&self) -> bool {
        true
    }
    fn extract(&mut self, samples: &[f32], rate: u32) -> EvalResult<Vec<f32>> {
        if !(8_000..=192_000).contains(&rate)
            || samples.len() < rate as usize
            || samples.len() > rate as usize * 5
            || samples.iter().any(|value| !value.is_finite())
        {
            return Err(
                "enrollment requires one to five seconds of finite mono source audio".into(),
            );
        }
        let resampled = if rate == 16_000 {
            None
        } else {
            Some(
                sherpa_onnx::LinearResampler::create(rate as i32, 16_000)
                    .ok_or("reference frontend resampler is unavailable")?
                    .resample(samples, true),
            )
        };
        let audio = resampled.as_deref().unwrap_or(samples);
        let mut value = self.embedder.extract(audio).map_err(err)?;
        if value.len() != 256 || value.iter().any(|item| !item.is_finite()) {
            return Err("reference frontend embedding is not a finite 256-vector".into());
        }
        let norm = value.iter().map(|item| item * item).sum::<f32>().sqrt();
        if !norm.is_finite() || norm < 1e-10 {
            return Err("reference frontend embedding cannot be normalized".into());
        }
        for item in &mut value {
            *item /= norm;
        }
        Ok(value)
    }
}
fn open_embedding_engine(
    path: &Path,
    profile: VoiceEmbeddingProfile,
) -> EvalResult<Box<dyn EmbeddingEngine>> {
    if profile == VoiceEmbeddingProfile::WeSpeakerResNet34ReferenceFrontendV1 {
        profile.verify_research_pack(path).map_err(err)?;
        let embedder =
            vt_speaker_frontend::ReferenceEmbedder::new(&path.join(vt_diarize::EMBEDDING))
                .map_err(err)?;
        return Ok(Box::new(ReferenceFrontendEngine { embedder }));
    }
    Ok(Box::new(
        vt_diarize::VoiceEmbedder::new_with_profile(path, profile).map_err(err)?,
    ))
}
struct VectorSample {
    id: String,
    person_id: String,
    speech_ms: u64,
    embedding: Zeroizing<Vec<f32>>,
}
fn extract(
    root: &Path,
    sample: &Sample,
    dimensions: usize,
    engine: &mut dyn EmbeddingEngine,
) -> EvalResult<VectorSample> {
    let mut mean = Zeroizing::new(vec![0.0f64; dimensions]);
    let mut total_frames = 0u64;
    let sample_rate = sample
        .windows
        .first()
        .ok_or("sample has no source windows")?
        .sample_rate;
    for window in &sample.windows {
        let bytes = Zeroizing::new(pinned(root, &window.path, &window.sha256, 2_000_044)?);
        let audio = wave(&bytes, window)?;
        let vector = Zeroizing::new(engine.extract(&audio, window.sample_rate)?);
        if vector.len() != dimensions || vector.iter().any(|v| !v.is_finite()) {
            return Err(
                "native embedding dimension/geometry differs from the pinned recipe".into(),
            );
        }
        let norm = vector
            .iter()
            .map(|v| f64::from(*v).powi(2))
            .sum::<f64>()
            .sqrt();
        if !norm.is_finite() || (norm - 1.0).abs() > 0.001 {
            return Err("native embedding is not normalized".into());
        }
        let duration = (window.source_end_frame - window.source_start_frame) * 1_000_000_000
            / u64::from(window.sample_rate);
        for (sum, value) in mean.iter_mut().zip(vector.iter()) {
            *sum += f64::from(*value) * duration as f64;
        }
        total_frames += window.source_end_frame - window.source_start_frame;
    }
    let norm = mean.iter().map(|v| v * v).sum::<f64>().sqrt();
    if !norm.is_finite() || norm < 1e-12 {
        return Err("clean sample produced no usable mean embedding".into());
    }
    Ok(VectorSample {
        id: sha(sample.id.as_bytes()),
        person_id: sample.person_id.clone(),
        speech_ms: total_frames * 1000 / u64::from(sample_rate),
        embedding: Zeroizing::new(mean.iter().map(|v| (*v / norm) as f32).collect()),
    })
}
#[derive(Serialize)]
struct RankedTrial {
    query_id: String,
    expected_person: Option<String>,
    best_person: String,
    best_score: f64,
    runner_up_score: Option<f64>,
    margin: Option<f64>,
    clean_speech_ms: u64,
    gallery_people: usize,
}
fn rankings(
    valid: &Validated,
    part: &Partition,
    engine: &mut dyn EmbeddingEngine,
) -> EvalResult<Vec<RankedTrial>> {
    let version = valid.manifest.expected_embedding_contract_sha256.as_str();
    let mut enrollment = Vec::new();
    for (index, sample) in part.enrollments.iter().enumerate() {
        enrollment.push(extract(
            &valid.root,
            sample,
            valid.manifest.embedding_profile.dimensions(),
            engine,
        )?);
        if (index + 1).is_multiple_of(10) {
            eprintln!(
                "voice-eval {}: {}/{} enrollment samples embedded",
                part.id,
                index + 1,
                part.enrollments.len()
            );
        }
    }
    let sets = Zeroizing::new(
        enrollment
            .iter()
            .map(|s| vec![s.embedding.to_vec()])
            .collect::<Vec<_>>(),
    );
    let gallery = enrollment
        .iter()
        .zip(sets.iter())
        .map(|(sample, vecs)| GalleryPerson {
            person_id: &sample.person_id,
            embedding_version: version,
            channel: valid.manifest.channel_type,
            stale: false,
            shared_room: false,
            exemplars: vecs,
        })
        .collect::<Vec<_>>();
    let mut trials = Vec::new();
    for (index, sample) in part.probes.iter().enumerate() {
        let vector = extract(
            &valid.root,
            sample,
            valid.manifest.embedding_profile.dimensions(),
            engine,
        )?;
        let query = VoiceQuery {
            embedding_version: version,
            channel: valid.manifest.channel_type,
            clean_speech_ms: vector.speech_ms,
            shared_room: false,
            embedding: &vector.embedding,
        };
        let ResearchRanking {
            best_person_id,
            best_score,
            runner_up_score,
            margin,
            gallery_people,
            ..
        } = rank_for_evaluation(&query, &gallery)
            .map_err(err)?
            .ok_or("complete gallery produced no research ranking")?;
        if gallery_people != 50 {
            return Err("ranker did not compare the fixed fifty-person gallery".into());
        }
        trials.push(RankedTrial {
            query_id: vector.id,
            expected_person: part
                .gallery_people
                .contains(&sample.person_id)
                .then(|| sample.person_id.clone()),
            best_person: best_person_id,
            best_score,
            runner_up_score,
            margin,
            clean_speech_ms: vector.speech_ms,
            gallery_people,
        });
        if (index + 1).is_multiple_of(25) {
            eprintln!(
                "voice-eval {}: {}/{} probes ranked",
                part.id,
                index + 1,
                part.probes.len()
            );
        }
    }
    // `sets` temporarily duplicates named research means for the borrowed gallery.
    // The nested zeroizing wrapper also wipes on errors; vectors are never serialized or persisted.
    Ok(trials)
}
#[derive(Clone, Copy, Serialize)]
struct Policy {
    tau_auto: f64,
    margin: f64,
}
#[derive(Clone, Copy, Default, Serialize)]
struct Counts {
    known: u64,
    unknown: u64,
    correct: u64,
    wrong_known: u64,
    false_unknown: u64,
}
fn count(trials: &[RankedTrial], policy: Policy) -> Counts {
    let mut c = Counts::default();
    for t in trials {
        if t.expected_person.is_some() {
            c.known += 1;
        } else {
            c.unknown += 1;
        }
        if t.best_score >= policy.tau_auto
            && t.margin.is_some_and(|m| m >= policy.margin)
            && t.clean_speech_ms >= 15_000
        {
            match &t.expected_person {
                Some(expected) if expected == &t.best_person => c.correct += 1,
                Some(_) => c.wrong_known += 1,
                None => c.false_unknown += 1,
            }
        }
    }
    c
}
fn fit(trials: &[RankedTrial]) -> Option<(Policy, Counts)> {
    let mut best = None::<(Policy, Counts)>;
    // This grid and selection order are source constants, frozen before any
    // held-out scoring. No held-out value enters this function.
    for threshold in -100..=100 {
        for margin in 1..=200 {
            let policy = Policy {
                tau_auto: f64::from(threshold) / 100.0,
                margin: f64::from(margin) / 100.0,
            };
            let c = count(trials, policy);
            if c.known == 0
                || c.unknown == 0
                || c.wrong_known * 100 > c.known * 2
                || c.false_unknown * 100 > c.unknown * 2
            {
                continue;
            }
            if best.is_none_or(|(p, prior)| {
                c.correct > prior.correct
                    || c.correct == prior.correct
                        && (policy.tau_auto > p.tau_auto
                            || policy.tau_auto == p.tau_auto && policy.margin > p.margin)
            }) {
                best = Some((policy, c));
            }
        }
    }
    best
}
fn outcomes(trials: &[RankedTrial], policy: Policy) -> Vec<MatchOutcome> {
    trials
        .iter()
        .map(|t| {
            if t.best_score >= policy.tau_auto
                && t.margin.is_some_and(|m| m >= policy.margin)
                && t.clean_speech_ms >= 15_000
            {
                MatchOutcome::Recognized {
                    person_id: t.best_person.clone(),
                    score: t.best_score,
                    margin: t.margin.expect("checked margin"),
                }
            } else {
                MatchOutcome::Unknown {
                    reason: UnknownReason::ResearchPolicyAbstention,
                }
            }
        })
        .collect()
}
fn metrics(
    manifest: &Manifest,
    part: &Partition,
    trials: &[RankedTrial],
    policy: Policy,
) -> EvalResult<Value> {
    let outcomes = outcomes(trials, policy);
    let ids = part
        .gallery_people
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let protocol = EvaluationProtocol {
        protocol_id: &manifest.protocol_id,
        embedding_version: &manifest.expected_embedding_contract_sha256,
        channel: manifest.channel_type,
        gallery_person_ids: &ids,
    };
    let rows = trials
        .iter()
        .zip(&outcomes)
        .map(|(t, outcome)| IdentificationTrial {
            query_id: &t.query_id,
            expected: t
                .expected_person
                .as_deref()
                .map_or(ExpectedIdentity::Unknown, ExpectedIdentity::Known),
            outcome,
        })
        .collect::<Vec<_>>();
    let report: IdentificationReport = evaluate_identification(&protocol, &rows).map_err(err)?;
    let c = count(trials, policy);
    Ok(
        json!({"metrics":report,"known_wrong_rate":ratio(c.wrong_known,c.known),"observed_targets":{"recognition_ge_0_90":c.known>0 && c.correct*100>=c.known*90,"known_wrong_le_0_02":c.known>0 && c.wrong_known*100<=c.known*2,"unknown_false_le_0_02":c.unknown>0 && c.false_unknown*100<=c.unknown*2},"wilson_95":{"recognition":wilson(c.correct,c.known),"known_wrong":wilson(c.wrong_known,c.known),"unknown_false":wilson(c.false_unknown,c.unknown)},"interval_caveat":"Binomial trial intervals are descriptive only: repeated meetings/trials from the same people/projects are correlated, so these bounds do not establish statistical acceptance or independence."}),
    )
}
fn ratio(n: u64, d: u64) -> Option<f64> {
    (d > 0).then(|| n as f64 / d as f64)
}
fn wilson(n: u64, d: u64) -> Option<[f64; 2]> {
    if d == 0 {
        return None;
    }
    let z = 1.959_963_984_540_054;
    let denom = 1.0 + z * z / d as f64;
    let p = n as f64 / d as f64;
    let center = (p + z * z / (2.0 * d as f64)) / denom;
    let radius = z * ((p * (1.0 - p) + z * z / (4.0 * d as f64)) / d as f64).sqrt() / denom;
    Some([
        if n == 0 {
            0.0
        } else {
            (center - radius).max(0.0)
        },
        if n == d {
            1.0
        } else {
            (center + radius).min(1.0)
        },
    ])
}
fn write_new(path: &Path, value: &impl Serialize) -> EvalResult<()> {
    let mut temporary =
        tempfile::NamedTempFile::new_in(path.parent().ok_or("report has no parent")?)
            .map_err(err)?;
    serde_json::to_writer(temporary.as_file_mut(), value).map_err(err)?;
    temporary.write_all(b"\n").map_err(err)?;
    temporary.as_file().sync_all().map_err(err)?;
    temporary.persist_noclobber(path).map_err(err)?;
    Ok(())
}
#[derive(Clone, Copy, Default, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EvaluationMode {
    #[default]
    Full,
    CalibrationOnly,
}
fn run_with(
    manifest: &Path,
    output: &Path,
    mode: EvaluationMode,
    create: impl FnOnce(&Path, VoiceEmbeddingProfile) -> EvalResult<Box<dyn EmbeddingEngine>>,
) -> EvalResult<()> {
    fs::create_dir(output).map_err(|e| format!("output must be a new directory: {e}"))?;
    let mut incomplete = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output.join("INCOMPLETE"))
        .map_err(err)?;
    incomplete
        .write_all(b"No acceptance or completed result is established until COMPLETE exists.\n")
        .map_err(err)?;
    incomplete.sync_all().map_err(err)?;
    let result = (|| {
        let valid = validate(manifest)?;
        let started = Instant::now();
        let model = if valid.manifest.model_dir.is_absolute() {
            valid.manifest.model_dir.clone()
        } else {
            valid.root.join(&valid.manifest.model_dir)
        };
        let mut engine = create(
            &model.canonicalize().map_err(err)?,
            valid.manifest.embedding_profile,
        )?;
        let actual_native = engine.actual_native();
        let model_manifest_sha256 = if actual_native {
            Some(sha(&read_bounded(
                &model.join("manifest.json"),
                1024 * 1024,
            )?))
        } else {
            None
        };
        let calibration = &valid.manifest.partitions[0];
        let calibration_trials = rankings(&valid, calibration, engine.as_mut())?;
        let chosen = fit(&calibration_trials);
        let mut policy_value = Value::Null;
        let mut calibration_result = Value::Null;
        let mut held_result = Value::Null;
        let mut held_trials = Vec::new();
        let mut frozen_policy_sha256 = None;
        let status;
        let calibration_policy_status;
        if let Some((policy, c)) = chosen {
            policy_value = json!(policy);
            calibration_result =
                metrics(&valid.manifest, calibration, &calibration_trials, policy)?;
            calibration_policy_status = if c.correct * 100 >= c.known * 90 {
                "observed_targets_met"
            } else {
                "recognition_below_target"
            };
            if mode == EvaluationMode::CalibrationOnly {
                status = "calibration_only_held_out_not_evaluated";
            } else if c.correct * 100 >= c.known * 90 {
                // Freeze the complete calibration decision before extracting a
                // held-out embedding. Persisting metadata cannot persist vectors.
                write_new(
                    &output.join("frozen-research-policy.json"),
                    &json!({"research_only":true,"algorithm":FIT_ALGORITHM,"policy":policy,"protocol_id":valid.manifest.protocol_id,"manifest_sha256":valid.manifest_sha256,"calibration_metrics":calibration_result}),
                )?;
                // Make the policy's directory entry durable before any held-out
                // inference, not only when the completed report is published.
                File::open(output).and_then(|f| f.sync_all()).map_err(err)?;
                frozen_policy_sha256 = Some(sha(&read_bounded(
                    &output.join("frozen-research-policy.json"),
                    1024 * 1024,
                )?));
                held_trials = rankings(&valid, &valid.manifest.partitions[1], engine.as_mut())?;
                held_result = metrics(
                    &valid.manifest,
                    &valid.manifest.partitions[1],
                    &held_trials,
                    policy,
                )?;
                status = "held_out_evaluated_once";
            } else {
                status = "unavailable_calibration_recognition_below_target";
            }
        } else {
            calibration_policy_status = "no_feasible_policy";
            status = if mode == EvaluationMode::CalibrationOnly {
                "calibration_only_held_out_not_evaluated"
            } else {
                "unavailable_no_feasible_calibration_policy"
            };
        }
        let report = json!({"schema_version":1,"research_only":true,"production_eligible":false,"runtime_pin_generated":false,"actual_native_inference":actual_native,"synthetic_injected_engine":!actual_native,"acceptance":"not_claimed","status":status,"selected_mode":mode,"calibration_policy_status":calibration_policy_status,"embedding_profile":valid.manifest.embedding_profile.id(),"protocol_id":valid.manifest.protocol_id,"channel_type":valid.manifest.channel_type,"algorithm_frozen_in_source":FIT_ALGORITHM,"candidate_research_policy":policy_value,"frozen_research_policy_sha256":frozen_policy_sha256,"calibration":calibration_result,"held_out":held_result,"calibration_trials":calibration_trials,"held_out_trials":held_trials,"provenance":{"manifest_sha256":valid.manifest_sha256,"annotation_sha256":valid.manifest.annotation_sha256,"annotation_file_verified":valid.annotation_verified,"partition_sha256":valid.manifest.partition_sha256,"partition_file_and_export_binding_verified":valid.partition_verified,"license_url":valid.manifest.license_url,"window_sha256_verified_count":valid.window_count,"window_bytes":valid.window_bytes,"original_full_source_hashes":"declared only; clipped windows cannot establish a full original source digest; HTTP ETags are descriptive","embedding_contract_sha256":valid.manifest.expected_embedding_contract_sha256,"legacy_unavailable_embedding_version":vt_diarize::voice_embedding_version(),"selected_profile_unavailable_embedding_version":valid.manifest.embedding_profile.unavailable_version(),"model_pack_manifest_sha256":model_manifest_sha256,"model_pack_verification":if actual_native {"full pinned hashes verified by native VoiceEmbedder before inference"}else {"synthetic test injection; native model verification not claimed"}},"limits":{"gallery_people":50,"minimum_enrollment_speech_ms":20000,"minimum_probe_speech_ms":15000,"maximum_sample_speech_ms":120000,"minimum_window_ms":1000,"maximum_window_ms":5000,"maximum_windows_per_sample":MAX_WINDOWS,"maximum_samples":MAX_SAMPLES,"embedding_dimensions":valid.manifest.embedding_profile.dimensions(),"maximum_manifest_bytes":MAX_MANIFEST,"audio_format":"PCM16 little-endian mono WAV"},"compiled_source_pins":{"research_runner_sha256":sha(include_bytes!("speaker_calibration.rs")),"matching_core_sha256":sha(include_bytes!("../../crates/vt-speakers/src/matching.rs")),"embedding_adapter_sha256":sha(include_bytes!("../../crates/vt-diarize/src/voice_embedding.rs")),"embedding_profile_sha256":sha(include_bytes!("../../crates/vt-diarize/src/voice_profile.rs")),"cargo_lock_sha256":sha(include_bytes!("../../Cargo.lock"))},"duration_seconds":started.elapsed().as_secs_f64(),"generated_at_unix_seconds":SystemTime::now().duration_since(UNIX_EPOCH).map_err(err)?.as_secs(),"limitations":["Operator-supplied public research protocol; corpus representativeness and in-house qualification are not automatically verified.","Calibration and held-out people/projects are disjoint according to the supplied metadata; independent acoustic conditions are not established by that check alone.","Only the declared channel and fixed fifty-person gallery are scored; other channels, conditions and gallery sizes remain unmeasured.","Observed target booleans and descriptive correlated-trial intervals do not authorize automatic identity, enrollment, release calibration or runtime enablement.","Vectors remained in memory and were never written to reports, app storage, logs or caches."]});
        write_new(&output.join("report.json"), &report)?;
        fs::remove_file(output.join("INCOMPLETE")).map_err(err)?;
        File::open(output).and_then(|f| f.sync_all()).map_err(err)?;
        let mut complete = tempfile::NamedTempFile::new_in(output).map_err(err)?;
        complete
            .write_all(b"Research report complete; production acceptance is not claimed.\n")
            .map_err(err)?;
        complete.as_file().sync_all().map_err(err)?;
        complete
            .persist_noclobber(output.join("COMPLETE"))
            .map_err(err)?;
        File::open(output).and_then(|f| f.sync_all()).map_err(err)?;
        Ok(())
    })();
    if let Err(error) = &result {
        // A failed publication must not leave a success marker behind. A final
        // filesystem failure can also prevent cleanup; preserve its error and
        // a best-effort incomplete marker rather than treating it as success.
        let _ = fs::remove_file(output.join("COMPLETE"));
        if let Ok(mut pending) = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(output.join("INCOMPLETE"))
        {
            let _ = pending.write_all(b"Research report publication failed; inspect error.json.\n");
            let _ = pending.sync_all();
        }
        let _ = write_new(
            &output.join("error.json"),
            &json!({"research_only":true,"complete":false,"error":error}),
        );
    }
    result
}
pub fn run(manifest: &Path, output: &Path) -> EvalResult<()> {
    run_in_mode(manifest, output, EvaluationMode::Full)
}
pub fn run_in_mode(manifest: &Path, output: &Path, mode: EvaluationMode) -> EvalResult<()> {
    run_with(manifest, output, mode, open_embedding_engine)
}
pub fn version() -> Value {
    version_for_profile(VoiceEmbeddingProfile::default())
}
pub fn version_for_profile(profile: VoiceEmbeddingProfile) -> Value {
    json!({"embedding_profile":profile.id(),"embedding_dimensions":profile.dimensions(),"raw_contract_sha256":profile.contract_sha256(),"legacy_unavailable_embedding_version":vt_diarize::voice_embedding_version(),"selected_profile_unavailable_embedding_version":profile.unavailable_version(),"research_only":true,"matching_available":false})
}
#[cfg(test)]
mod tests;
