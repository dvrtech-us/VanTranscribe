//! Explicit local public-corpus inference. No acquisition, devices or app DB.
mod diagnostics;
pub mod experiments;
mod production_diarization;
mod production_live;
mod stages;
mod telemetry;
#[cfg(test)]
mod tests;
use crate::{EvalResult, evaluate_manifest, write_report};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
    time::Instant,
};
use vt_core::TrackKind;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    schema_version: u32,
    public_corpus: PublicCorpus,
    audio: PinnedFile,
    reference_text: PinnedFile,
    reference_rttm: PinnedFile,
    uem: PinnedFile,
    #[serde(default)]
    annotation_sources: Vec<PinnedFile>,
    models: Models,
    #[serde(default)]
    paced_live: bool,
    #[serde(default)]
    diagnostics: Option<diagnostics::Diagnostics>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PublicCorpus {
    id: String,
    recording_id: String,
    license: String,
    attribution: String,
    source_urls: Vec<String>,
    derivation: String,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PinnedFile {
    path: PathBuf,
    sha256: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Models {
    streaming: PathBuf,
    refinement: PathBuf,
    diarization: PathBuf,
}

fn error(e: impl std::fmt::Display) -> String {
    e.to_string()
}
fn json_file(path: &Path, value: &impl Serialize) -> EvalResult<()> {
    fs::write(path, serde_json::to_vec_pretty(value).map_err(error)?).map_err(error)
}
fn fingerprint(path: &Path, limit: u64) -> EvalResult<Value> {
    let mut file = File::open(path).map_err(error)?;
    let bytes = file.metadata().map_err(error)?.len();
    if bytes > limit {
        return Err(format!("input exceeds bounded size: {}", path.display()));
    }
    let mut hash = Sha256::new();
    let mut buffer = [0; 65536];
    loop {
        let n = file.read(&mut buffer).map_err(error)?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    Ok(
        json!({"path":path.canonicalize().map_err(error)?,"bytes":bytes,"sha256":format!("{:x}",hash.finalize())}),
    )
}
fn pinned(base: &Path, input: &PinnedFile, limit: u64) -> EvalResult<Value> {
    let value = fingerprint(&base.join(&input.path), limit)?;
    if input.sha256.len() != 64 || value["sha256"].as_str() != Some(input.sha256.as_str()) {
        return Err(format!("SHA256 mismatch for {}", input.path.display()));
    }
    Ok(value)
}
fn model_provenance(dir: &Path) -> EvalResult<Value> {
    let manifest = dir.join("manifest.json");
    let value: Value =
        serde_json::from_slice(&fs::read(&manifest).map_err(error)?).map_err(error)?;
    let mut files = vec![fingerprint(&manifest, 1024 * 1024)?];
    let mut names = fs::read_dir(dir)
        .map_err(error)?
        .map(|e| e.map(|e| e.path()))
        .collect::<std::io::Result<Vec<_>>>()
        .map_err(error)?;
    names.sort();
    for name in names {
        if name.extension().is_some_and(|e| e == "onnx" || e == "txt") {
            files.push(fingerprint(&name, 1024 * 1024 * 1024)?);
        }
    }
    Ok(json!({"manifest":value,"files":files}))
}

pub fn run(manifest: &Path, output: &Path) -> EvalResult<()> {
    let bytes = fs::read(manifest).map_err(error)?;
    if bytes.len() > 1024 * 1024 {
        return Err("inference manifest exceeds1MiB".into());
    }
    let input: Input = serde_json::from_slice(&bytes).map_err(error)?;
    if let Some(config) = &input.diagnostics {
        config.validate()?;
    }
    let runner = json!({"version":env!("CARGO_PKG_VERSION"),"binary":fingerprint(&std::env::current_exe().map_err(error)?,256*1024*1024)?});
    let corpus = &input.public_corpus;
    if input.schema_version != 1
        || corpus.id.trim().is_empty()
        || corpus.recording_id.trim().is_empty()
        || corpus.recording_id.chars().any(char::is_whitespace)
        || corpus.license.trim().is_empty()
        || corpus.attribution.trim().is_empty()
        || corpus.derivation.trim().is_empty()
        || corpus.source_urls.is_empty()
        || corpus
            .source_urls
            .iter()
            .any(|url| !url.starts_with("https://"))
    {
        return Err("public inference requires schema1, explicit public source/licence/attribution/derivation and a whitespace-free recording ID".into());
    }
    let base = manifest.parent().unwrap_or(Path::new("."));
    let mut sources = vec![fingerprint(manifest, 1024 * 1024)?];
    for (file, limit) in [
        (&input.audio, 64 * 1024 * 1024),
        (&input.reference_text, 16 * 1024 * 1024),
        (&input.reference_rttm, 16 * 1024 * 1024),
        (&input.uem, 1024 * 1024),
    ] {
        sources.push(pinned(base, file, limit)?);
    }
    for file in &input.annotation_sources {
        sources.push(pinned(base, file, 64 * 1024 * 1024)?);
    }
    if let Some(control) = input
        .diagnostics
        .as_ref()
        .and_then(|d| d.control_wav.as_ref())
    {
        sources.push(pinned(base, control, 16 * 1024 * 1024)?);
    }
    let streaming = base.join(&input.models.streaming);
    let refinement = base.join(&input.models.refinement);
    let diarization = base.join(&input.models.diarization);
    let models = json!({"streaming":model_provenance(&streaming)?,"refinement":model_provenance(&refinement)?,"diarization":model_provenance(&diarization)?});
    let audio = base.join(&input.audio.path);
    let wave = sherpa_onnx::Wave::read(audio.to_str().ok_or("audio path must be UTF8")?)
        .ok_or("cannot decode mono PCM WAV")?;
    let rate = u32::try_from(wave.sample_rate()).map_err(error)?;
    let samples = wave.samples();
    if !(8000..=192000).contains(&rate)
        || samples.is_empty()
        || samples.len() > rate as usize * 600
        || samples.iter().any(|s| !s.is_finite())
    {
        return Err(
            "benchmark requires finite mono WAV lasting at most600seconds at8–192kHz".into(),
        );
    }
    // Validate reference identity/UEM coverage before expensive inference.
    let reference = fs::read_to_string(base.join(&input.reference_rttm.path)).map_err(error)?;
    let uem = fs::read_to_string(base.join(&input.uem.path)).map_err(error)?;
    let reference_turns = crate::diarization::parse_rttm(&reference, &corpus.recording_id, "1")?;
    let regions = crate::diarization::parse_uem(&uem, &corpus.recording_id, "1")?;
    let duration = samples.len() as f64 / f64::from(rate);
    if regions.iter().any(|r| r.end > duration + 0.000_001) {
        return Err(
            "UEM extends beyond supplied audio; crop references and audio consistently".into(),
        );
    }
    crate::diarization::score(&reference_turns, &[], &regions)?;
    // An existing destination is never merged or overwritten, including inputs.
    fs::create_dir(output).map_err(|e| format!("create new output directory: {e}"))?;
    json_file(
        &output.join("INCOMPLETE.json"),
        &json!({"status":"incomplete","note":"Only COMPLETE.json establishes a finished evidence set; partial stage files are diagnostic."}),
    )?;
    fs::copy(
        base.join(&input.reference_text.path),
        output.join("reference.txt"),
    )
    .map_err(error)?;
    fs::write(output.join("reference.rttm"), reference).map_err(error)?;
    fs::write(output.join("regions.uem"), uem).map_err(error)?;
    let all = Instant::now();
    let mut telemetry = telemetry::Telemetry::new();
    let live = stages::live(
        samples,
        rate,
        &streaming,
        Some(&refinement.join("silero_vad.onnx")),
        input.paced_live,
        &mut telemetry,
    )?;
    fs::write(output.join("live.txt"), &live.text).map_err(error)?;
    json_file(&output.join("live-events.json"), &live.events)?;
    let batch = stages::batch(samples, rate, &refinement, &mut telemetry)?;
    fs::write(output.join("batch.txt"), &batch.text).map_err(error)?;
    json_file(&output.join("batch-events.json"), &batch.events)?;
    let live_diar = stages::diarize(
        samples,
        rate,
        &diarization,
        vt_diarize::Phase::Live,
        input.paced_live,
        &mut telemetry,
    )?;
    let refined = stages::diarize(
        samples,
        rate,
        &diarization,
        vt_diarize::Phase::Refined,
        false,
        &mut telemetry,
    )?;
    for (name, result) in [("live", &live_diar), ("refined", &refined)] {
        json_file(
            &output.join(format!("{name}-turns.json")),
            &json!({"scored_turns":result.turns,"finish_result":result.result,"emissions":result.emissions}),
        )?;
        fs::write(
            output.join(format!("{name}.rttm")),
            rttm(&corpus.recording_id, &result.turns),
        )
        .map_err(error)?;
    }
    let observations = telemetry.observations();
    json_file(&output.join("observations.json"), &observations)?;
    let diagnostic = if let Some(config) = &input.diagnostics {
        Some(diagnostics::run(
            config,
            samples,
            rate,
            &streaming,
            &refinement,
            &diarization,
            &corpus.recording_id,
            &base.join(&input.reference_text.path),
            &reference_turns,
            &regions,
            base,
            output,
        )?)
    } else {
        None
    };
    // Verify sources and model payloads before publishing score reports.
    for source in &sources {
        let path = Path::new(source["path"].as_str().ok_or("missing source path")?);
        if fingerprint(path, 64 * 1024 * 1024)?["sha256"] != source["sha256"] {
            return Err("input changed during inference".into());
        }
    }
    for model in models
        .as_object()
        .ok_or("invalid model provenance")?
        .values()
    {
        for source in model["files"]
            .as_array()
            .ok_or("missing model fingerprints")?
        {
            let path = Path::new(source["path"].as_str().ok_or("missing model path")?);
            if fingerprint(path, 1024 * 1024 * 1024)?["sha256"] != source["sha256"] {
                return Err("model changed during inference".into());
            }
        }
    }
    let settings = json!({"schema_version":1,"acceptance":"not_claimed","corpus":corpus,"sources":sources,"models":models,"diagnostics":diagnostic,
        "runner":runner,
        "timing":{"packet_ms":20,"paced_live":input.paced_live,"latency_scope":"core inference only; no app UI/capture/worker queues measured","constructor_scope":"first constructor including production hash verification; OS disk cache not cleared","total_wall_ms":all.elapsed().as_secs_f64()*1000.0},
        "audio":{"rate":rate,"frames":samples.len(),"duration_ms":samples.len() as f64*1000.0/f64::from(rate),"track":TrackKind::Mic.as_str(),"meeting_mode":"in_person"},
        "threads":{"stt":2,"vad":1,"segmentation":1,"embedding":1,"engines":"sequential phases"},
        "phases":[live.phase,batch.phase,live_diar.phase,refined.phase],"limitations":["Public AMI does not satisfy in-house four-speaker or gallery50 acceptance.","Unknown/uncertain/provisional turns are omitted from RTTM and count as missed reference speech, never synthesized labels.","AMI word/segment annotation derivation and overlap ordering are corpus-specific; see source manifest.","Product latency observations remain unavailable in both modes; paced delivery delays and emission wall times live in event/turn artifacts, with actual call durations retained.","No OS capture, audio output, network, AppService UI or private meeting database is used."]});
    json_file(&output.join("inference.json"), &settings)?;
    for phase in ["live", "refined"] {
        let mut evaluation = json!({"schema_version":1,"corpus":{"id":corpus.id,"kind":"public","description":format!("{} production {phase} diarization + paired ASR",corpus.id),"provenance":[corpus.attribution.clone(),corpus.license.clone(),corpus.derivation.clone(),"Full source/model/settings hashes in inference.json".to_string()]},"normalization":"unicode_lowercase_punctuation_to_space","recordings":[{"id":corpus.recording_id,"channel":"1","diarization":{"reference_rttm":"reference.rttm","hypothesis_rttm":format!("{phase}.rttm"),"uem":"regions.uem"},"transcripts":[{"id":"whole-selected-window-time-ordered","reference":"reference.txt","hypotheses":{"live":"live.txt","batch":"batch.txt"}}],"observations":"observations.json"}]});
        if observations["resources"]
            .as_array()
            .is_none_or(Vec::is_empty)
        {
            evaluation["recordings"][0]
                .as_object_mut()
                .ok_or("invalid evaluation record")?
                .remove("observations");
        }
        let path = output.join(format!("{phase}-manifest.json"));
        json_file(&path, &evaluation)?;
        let report = evaluate_manifest(&path)?;
        write_report(&report, &output.join(format!("{phase}-report.json")))?;
    }
    json_file(
        &output.join("COMPLETE.json"),
        &json!({"status":"complete","acceptance":"not_claimed","inference_sha256":fingerprint(&output.join("inference.json"),64*1024*1024)?["sha256"]}),
    )?;
    fs::remove_file(output.join("INCOMPLETE.json")).map_err(error)?;
    Ok(())
}
fn rttm(id: &str, turns: &[vt_diarize::SpeakerTurn]) -> String {
    let mut lines = String::new();
    for turn in turns
        .iter()
        .filter(|t| !t.uncertain && !t.provisional && t.end_ms > t.start_ms)
    {
        for speaker in &turn.speaker_ids {
            lines.push_str(&format!(
                "SPEAKER {id} 1 {:.3} {:.3} <NA> <NA> {speaker} <NA> <NA>\n",
                turn.start_ms as f64 / 1000.0,
                (turn.end_ms - turn.start_ms) as f64 / 1000.0
            ));
        }
    }
    lines
}
