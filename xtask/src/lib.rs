//! Offline scoring of supplied, provenance-backed reference/hypothesis files.
pub mod diarization;
#[cfg(feature = "inference")]
pub mod inference;
pub mod observations;
#[cfg(feature = "inference")]
pub mod speaker_calibration;
pub mod words;
use diarization::DiarizationScore;
use observations::{BleedScore, MeasurementReport, Measurements};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use words::{Normalization, WordScore};
pub type EvalResult<T> = Result<T, String>;
pub(crate) fn fail(message: impl Into<String>) -> String {
    message.into()
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CorpusKind {
    Synthetic,
    Public,
    Consented,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Corpus {
    pub id: String,
    pub kind: CorpusKind,
    pub description: String,
    pub provenance: Vec<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema_version: u32,
    corpus: Corpus,
    normalization: Normalization,
    recordings: Vec<Recording>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Recording {
    id: String,
    channel: String,
    diarization: Option<DiarizationInput>,
    #[serde(default)]
    transcripts: Vec<TranscriptInput>,
    bleed: Option<BleedInput>,
    observations: Option<PathBuf>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DiarizationInput {
    reference_rttm: PathBuf,
    hypothesis_rttm: PathBuf,
    uem: PathBuf,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TranscriptInput {
    id: String,
    reference: PathBuf,
    hypotheses: BTreeMap<String, PathBuf>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BleedInput {
    reference: PathBuf,
    hypothesis: PathBuf,
}
#[derive(Clone, Debug, Serialize)]
pub struct SourceFile {
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
    #[serde(skip)]
    canonical: PathBuf,
}
#[derive(Default)]
struct Inputs {
    files: BTreeMap<PathBuf, SourceFile>,
}
impl Inputs {
    fn read(&mut self, path: &Path) -> EvalResult<String> {
        let canonical = path
            .canonicalize()
            .map_err(|e| fail(format!("cannot read required input{}: {e}", path.display())))?;
        let file = File::open(&canonical)
            .map_err(|e| fail(format!("cannot open{}: {e}", path.display())))?;
        let mut bytes = vec![];
        file.take(64 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| fail(e.to_string()))?;
        if bytes.len() > 64 * 1024 * 1024 {
            return Err(fail("input exceeds64MiB; use smaller evaluation units"));
        }
        let sha256 = format!("{:x}", Sha256::digest(&bytes));
        if let Some(previous) = self.files.get(&canonical)
            && previous.sha256 != sha256
        {
            return Err(fail("an input changed during evaluation"));
        }
        self.files.entry(canonical.clone()).or_insert(SourceFile {
            path: path.display().to_string(),
            bytes: bytes.len() as u64,
            sha256,
            canonical,
        });
        String::from_utf8(bytes).map_err(|_| fail(format!("input{} is not UTF-8", path.display())))
    }
    fn json<T: serde::de::DeserializeOwned>(&mut self, path: &Path) -> EvalResult<T> {
        serde_json::from_str(&self.read(path)?)
            .map_err(|e| fail(format!("invalid JSON{}: {e}", path.display())))
    }
}
#[derive(Serialize)]
pub struct TranscriptScore {
    pub id: String,
    pub hypotheses: BTreeMap<String, WordScore>,
}
#[derive(Serialize)]
pub struct RecordingScore {
    pub id: String,
    pub channel: String,
    pub diarization: Option<DiarizationScore>,
    pub transcripts: Vec<TranscriptScore>,
    pub bleed: Option<BleedScore>,
}
#[derive(Default, Serialize)]
pub struct PairedWer {
    pub units: u64,
    pub live: WordScore,
    pub batch: WordScore,
    pub live_to_batch_wer_ratio: Option<f64>,
}
#[derive(Serialize)]
pub struct Target {
    pub id: String,
    pub description: String,
    pub observed: Option<f64>,
    pub limit: f64,
    pub comparison: String,
    pub within_target: Option<bool>,
}
#[derive(Serialize)]
pub struct Report {
    pub schema_version: u32,
    pub harness_version: String,
    pub generated_at_unix_seconds: u64,
    pub corpus: Corpus,
    pub acceptance: String,
    pub definition: Definition,
    pub sources: Vec<SourceFile>,
    pub recordings: Vec<RecordingScore>,
    pub diarization: Option<DiarizationScore>,
    pub wer: BTreeMap<String, WordScore>,
    pub paired_live_batch_wer: PairedWer,
    pub bleed: Option<BleedScore>,
    pub measurements: MeasurementReport,
    pub targets: Vec<Target>,
    pub notes: Vec<String>,
}
#[derive(Serialize)]
pub struct Definition {
    pub collar_seconds: f64,
    pub collar_half_width_seconds: f64,
    pub timestamp_resolution_seconds: f64,
    pub collar_convention: String,
    pub overlap_scored: bool,
    pub gallery_scope: String,
    pub speaker_assignment: String,
    pub jer_mapping: String,
    pub normalization: Normalization,
    pub percentile_method: String,
    pub rates_are_fractions: bool,
}
fn named(value: &str, what: &str) -> EvalResult<()> {
    if value.trim().is_empty() {
        Err(fail(format!("{what} must not be empty")))
    } else {
        Ok(())
    }
}
fn target(id: &str, description: &str, observed: Option<f64>, limit: f64, strict: bool) -> Target {
    Target {
        id: id.into(),
        description: description.into(),
        observed,
        limit,
        comparison: if strict { "<" } else { "<=" }.into(),
        within_target: observed.map(|value| {
            if strict {
                value < limit
            } else {
                value <= limit
            }
        }),
    }
}
fn targets(
    der: &Option<DiarizationScore>,
    paired: &PairedWer,
    bleed: &Option<BleedScore>,
    m: &MeasurementReport,
) -> Vec<Target> {
    vec![
        target(
            "m2b_der",
            "Refined four-speaker DER design target; corpus qualification remains manual",
            der.as_ref()
                .filter(|s| s.reference_speaker_seconds > 0.0)
                .map(|s| s.der),
            0.20,
            false,
        ),
        target(
            "m1_live_wer",
            "Live WER on identical paired reference units <=1.5x batch WER",
            paired.live.wer.zip(paired.batch.wer).map(|(live, _)| live),
            paired.batch.wer.unwrap_or(0.0) * 1.5,
            false,
        ),
        target(
            "m2c_probable_bleed_annotation_rate",
            "Probable-bleed annotations / all annotated final lines (literal design target)",
            bleed.as_ref().and_then(|s| s.annotation_rate),
            0.02,
            true,
        ),
        target(
            "m2c_visible_duplicate_lines",
            "Visible ground-truth bleed duplicates / all annotated final lines",
            bleed.as_ref().and_then(|s| s.visible_duplicate_rate),
            0.02,
            true,
        ),
        target(
            "m2c_lost_short_replies",
            "Zero ground-truth genuine short replies hidden as bleed",
            bleed
                .as_ref()
                .filter(|s| s.genuine_short_replies > 0)
                .map(|s| s.lost_genuine_short_replies as f64),
            0.0,
            false,
        ),
        target(
            "m1_partial_latency_max_ms",
            "Speech onset to first visible partial, maximum supplied observation",
            m.partial_ms.as_ref().map(|p| p.max),
            1500.0,
            false,
        ),
        target(
            "m1_final_latency_max_ms",
            "Utterance end to final text, maximum supplied observation",
            m.final_text_ms.as_ref().map(|p| p.max),
            4000.0,
            false,
        ),
        target(
            "m1_speaker_label_latency_max_ms",
            "Final text to live speaker label, maximum supplied observation",
            m.speaker_label_ms.as_ref().map(|p| p.max),
            1000.0,
            false,
        ),
        target(
            "m0_live_cpu_sample_mean_percent",
            "Reference-machine live CPU target; unweighted mean of supplied total-machine-percent samples",
            m.cpu_sample_mean_percent,
            25.0,
            false,
        ),
        target(
            "m0_rss_max_bytes",
            "Reference-machine RSS design target (decimal1.2GB)",
            m.rss_bytes.as_ref().map(|p| p.max),
            1_200_000_000.0,
            false,
        ),
    ]
}
pub fn evaluate_manifest(path: &Path) -> EvalResult<Report> {
    let mut inputs = Inputs::default();
    let manifest: Manifest = inputs.json(path)?;
    if manifest.schema_version != 1 {
        return Err(fail("unsupported manifest schema_version"));
    }
    named(&manifest.corpus.id, "corpus ID")?;
    named(&manifest.corpus.description, "corpus description")?;
    if manifest.corpus.provenance.is_empty()
        || manifest
            .corpus
            .provenance
            .iter()
            .any(|source| source.trim().is_empty())
    {
        return Err(fail("corpus requires nonempty provenance entries"));
    }
    if manifest.recordings.is_empty() {
        return Err(fail("manifest requires at least one recording"));
    }
    let root = path.parent().unwrap_or(Path::new("."));
    let mut seen = BTreeSet::new();
    let mut records = vec![];
    let mut aggregate_der = None::<DiarizationScore>;
    let mut aggregate_bleed = None::<BleedScore>;
    let mut aggregate_wer = BTreeMap::<String, WordScore>::new();
    let mut paired = PairedWer::default();
    let mut measurements = Measurements::default();
    let mut notes=vec!["Target comparisons are evidence summaries, not milestone acceptance. Corpus coverage, consent, calibration and machine qualification require review.".into(),"Speaker labels are mapped independently for each recording/channel; identities are never pooled across meetings.".into()];
    if matches!(manifest.corpus.kind, CorpusKind::Synthetic) {
        notes.push("Synthetic fixtures validate the scorer only; their scores do not establish product quality.".into());
    }
    for recording in manifest.recordings {
        named(&recording.id, "recording ID")?;
        named(&recording.channel, "channel")?;
        if !seen.insert((recording.id.clone(), recording.channel.clone())) {
            return Err(fail("duplicate recording/channel evaluation unit"));
        }
        if recording.diarization.is_none()
            && recording.transcripts.is_empty()
            && recording.bleed.is_none()
            && recording.observations.is_none()
        {
            return Err(fail("recording contains no evaluation evidence"));
        }
        let der = if let Some(input) = recording.diarization {
            let r = diarization::parse_rttm(
                &inputs.read(&root.join(input.reference_rttm))?,
                &recording.id,
                &recording.channel,
            )?;
            let h = diarization::parse_hypothesis_rttm(
                &inputs.read(&root.join(input.hypothesis_rttm))?,
                &recording.id,
                &recording.channel,
            )?;
            let uem = diarization::parse_uem(
                &inputs.read(&root.join(input.uem))?,
                &recording.id,
                &recording.channel,
            )?;
            let score = diarization::score(&r, &h, &uem)?;
            if score.reference_speaker_seconds == 0.0 {
                notes.push(format!("{}/{} has no scored reference speaker time; DER/JER edge values are reported but do not meet a target by themselves.",recording.id,recording.channel));
            }
            aggregate_der
                .get_or_insert_with(DiarizationScore::default)
                .accumulate(&score);
            Some(score)
        } else {
            None
        };
        let mut transcript_scores = vec![];
        let mut transcript_ids = BTreeSet::new();
        for transcript in recording.transcripts {
            named(&transcript.id, "transcript unit ID")?;
            if !transcript_ids.insert(transcript.id.clone()) || transcript.hypotheses.is_empty() {
                return Err(fail(
                    "transcript units require unique IDs and at least one hypothesis",
                ));
            }
            let reference = inputs.read(&root.join(transcript.reference))?;
            let mut scores = BTreeMap::new();
            for (name, path) in transcript.hypotheses {
                named(&name, "hypothesis role")?;
                let score = words::score(
                    &reference,
                    &inputs.read(&root.join(path))?,
                    manifest.normalization,
                )?;
                aggregate_wer
                    .entry(name.clone())
                    .or_default()
                    .accumulate(&score);
                scores.insert(name, score);
            }
            if let (Some(live), Some(batch)) = (scores.get("live"), scores.get("batch")) {
                paired.units += 1;
                paired.live.accumulate(live);
                paired.batch.accumulate(batch);
            }
            transcript_scores.push(TranscriptScore {
                id: transcript.id,
                hypotheses: scores,
            });
        }
        let bleed = if let Some(input) = recording.bleed {
            let r = inputs.json(&root.join(input.reference))?;
            let h = inputs.json(&root.join(input.hypothesis))?;
            let score = observations::bleed(r, h)?;
            aggregate_bleed
                .get_or_insert_with(BleedScore::default)
                .accumulate(&score);
            Some(score)
        } else {
            None
        };
        if let Some(path) = recording.observations {
            measurements.add(inputs.json(&root.join(path))?)?;
        }
        records.push(RecordingScore {
            id: recording.id,
            channel: recording.channel,
            diarization: der,
            transcripts: transcript_scores,
            bleed,
        });
    }
    paired.live_to_batch_wer_ratio = paired
        .live
        .wer
        .zip(paired.batch.wer)
        .and_then(|(live, batch)| (batch > 0.0).then_some(live / batch));
    let measurements = measurements.report();
    let targets = targets(&aggregate_der, &paired, &aggregate_bleed, &measurements);
    Ok(Report {
        schema_version: 1,
        harness_version: env!("CARGO_PKG_VERSION").into(),
        generated_at_unix_seconds: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| fail(e.to_string()))?
            .as_secs(),
        corpus: manifest.corpus,
        acceptance: "not_claimed".into(),
        definition: Definition {
            collar_seconds: diarization::COLLAR_SECONDS,
            collar_half_width_seconds: diarization::COLLAR_SECONDS / 2.0,
            timestamp_resolution_seconds: 0.000_000_001,
            collar_convention: "pyannote total centered width; +/-0.125seconds".into(),
            overlap_scored: true,
            gallery_scope: "recording_id + channel".into(),
            speaker_assignment:
                "global maximum co-occurrence duration; Hungarian; zero weights remain unmatched"
                    .into(),
            jer_mapping: "same duration-optimal assignment, not a separate IoU-optimal assignment"
                .into(),
            normalization: manifest.normalization,
            percentile_method: "nearest rank ceil(q*n), p50/p95/p99; maximum also reported".into(),
            rates_are_fractions: true,
        },
        sources: inputs.files.into_values().collect(),
        recordings: records,
        diarization: aggregate_der,
        wer: aggregate_wer,
        paired_live_batch_wer: paired,
        bleed: aggregate_bleed,
        measurements,
        targets,
        notes,
    })
}
pub fn write_report(report: &Report, output: &Path) -> EvalResult<()> {
    if let Ok(existing) = output.canonicalize()
        && report
            .sources
            .iter()
            .any(|source| source.canonical == existing)
    {
        return Err(fail(
            "output must not replace any reference, hypothesis, observation or manifest input",
        ));
    }
    let parent = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent).map_err(|e| fail(e.to_string()))?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(|e| fail(e.to_string()))?;
    serde_json::to_writer(temporary.as_file_mut(), report).map_err(|e| fail(e.to_string()))?;
    temporary
        .write_all(b"\n")
        .map_err(|e| fail(e.to_string()))?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|e| fail(e.to_string()))?;
    temporary.persist(output).map_err(|e| fail(e.to_string()))?;
    Ok(())
}
#[cfg(test)]
mod tests;
