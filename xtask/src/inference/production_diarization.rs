//! Explicit diarization-only phase comparison on pinned public inputs.
//! Does not instantiate ASR, acquire models, or activate application settings.
use super::*;
use vt_diarize::{Phase, SpeakerTurn};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Comparison {
    schema_version: u32,
    mode: Mode,
    clips: Vec<Clip>,
    model_directory: PathBuf,
    model_manifest_sha256: String,
    control: Control,
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Mode {
    ProductionDiarizationContext,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Clip {
    legacy_manifest: PinnedFile,
    word_only_manifest: PinnedFile,
    baseline_five_turns: PinnedFile,
    experimental_ten_turns: PinnedFile,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Control {
    audio: PinnedFile,
    attribution: String,
    source_urls: Vec<String>,
    license_note: String,
}
fn safe_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
}
fn check_config(config: &Comparison) -> EvalResult<()> {
    let _ = &config.mode;
    if config.schema_version != 1
        || config.clips.is_empty()
        || config.clips.len() > 2
        || config.model_manifest_sha256.len() != 64
        || config.control.attribution.trim().is_empty()
        || config.control.license_note.trim().is_empty()
        || config.control.source_urls.is_empty()
        || config
            .control
            .source_urls
            .iter()
            .any(|url| !url.starts_with("https://"))
    {
        return Err("diarization context comparison requires schema1,1–2 pinned public clips, model manifest hash and attributed public control".into());
    }
    Ok(())
}
fn wave(path: &Path, seconds: usize) -> EvalResult<sherpa_onnx::Wave> {
    let wave = sherpa_onnx::Wave::read(path.to_str().ok_or("WAV path must be UTF8")?)
        .ok_or("cannot decode explicit mono WAV")?;
    let rate = u32::try_from(wave.sample_rate()).map_err(error)?;
    if !(8000..=192000).contains(&rate)
        || wave.samples().is_empty()
        || wave.samples().len() > rate as usize * seconds
        || wave.samples().iter().any(|s| !s.is_finite())
    {
        return Err("diarization WAV exceeds finite mono rate/duration bounds".into());
    }
    Ok(wave)
}
fn input(path: &Path, sources: &mut Vec<Value>) -> EvalResult<Input> {
    let value: Input = serde_json::from_slice(&fs::read(path).map_err(error)?).map_err(error)?;
    let corpus = &value.public_corpus;
    if value.schema_version != 1
        || !safe_id(&corpus.recording_id)
        || corpus.id.trim().is_empty()
        || corpus.license.trim().is_empty()
        || corpus.attribution.trim().is_empty()
        || corpus.derivation.trim().is_empty()
        || value.annotation_sources.len() > 32
        || corpus.source_urls.is_empty()
        || corpus
            .source_urls
            .iter()
            .any(|s| !s.starts_with("https://"))
    {
        return Err("diarization comparison requires explicit public source/licence/scope".into());
    }
    let base = path.parent().ok_or("input needs parent directory")?;
    for pin in [&value.audio, &value.reference_rttm, &value.uem] {
        sources.push(pinned(base, pin, 64 * 1024 * 1024)?);
    }
    for pin in &value.annotation_sources {
        sources.push(pinned(base, pin, 64 * 1024 * 1024)?);
    }
    Ok(value)
}
fn previous_turns(base: &Path, pin: &PinnedFile) -> EvalResult<Vec<SpeakerTurn>> {
    let value: Value =
        serde_json::from_slice(&fs::read(base.join(&pin.path)).map_err(error)?).map_err(error)?;
    let turns: Vec<SpeakerTurn> = serde_json::from_value(
        value
            .get("turns")
            .ok_or("prior result missing turns")?
            .clone(),
    )
    .map_err(error)?;
    if turns.len() > 32768 {
        return Err("prior artifact exceeds production turn capacity".into());
    }
    Ok(turns)
}
// Phase names/provenance intentionally differ for the finalized Live5s control.
// Every scored assignment, timing, score and uncertainty bit must still match.
fn projection(turns: &[SpeakerTurn]) -> EvalResult<Vec<Value>> {
    turns
        .iter()
        .map(|turn| {
            let mut value = serde_json::to_value(turn).map_err(error)?;
            value
                .as_object_mut()
                .ok_or("invalid turn")?
                .remove("provenance");
            Ok(value)
        })
        .collect()
}
fn counts(turns: &[SpeakerTurn]) -> Value {
    let unknown =
        |turn: &&SpeakerTurn| turn.uncertain || turn.provisional || turn.speaker_ids.is_empty();
    json!({"turns":turns.len(),"unknown_or_uncertain_turns":turns.iter().filter(unknown).count(),"unknown_or_uncertain_ms":turns.iter().filter(unknown).map(|t|t.end_ms-t.start_ms).sum::<u64>(),"overlap_turns":turns.iter().filter(|t|t.overlap).count(),"overlap_ms":turns.iter().filter(|t|t.overlap).map(|t|t.end_ms-t.start_ms).sum::<u64>(),"assigned_speakers":turns.iter().flat_map(|t|t.speaker_ids.iter()).collect::<std::collections::BTreeSet<_>>().len()})
}
fn scores(input: &Input, base: &Path, hypothesis: &str, duration_ms: u64) -> EvalResult<Value> {
    let id = &input.public_corpus.recording_id;
    let reference = crate::diarization::parse_rttm(
        &fs::read_to_string(base.join(&input.reference_rttm.path)).map_err(error)?,
        id,
        "1",
    )?;
    let regions = crate::diarization::parse_uem(
        &fs::read_to_string(base.join(&input.uem.path)).map_err(error)?,
        id,
        "1",
    )?;
    let hyp = crate::diarization::parse_hypothesis_rttm(hypothesis, id, "1")?;
    let duration_seconds = duration_ms as f64 / 1000.;
    if regions.iter().any(|span| span.end > duration_seconds)
        || reference.iter().any(|turn| turn.end > duration_seconds)
    {
        return Err("reference protocol extends beyond selected audio".into());
    }
    serde_json::to_value(crate::diarization::score(&reference, &hyp, &regions)?).map_err(error)
}
fn execute(
    samples: &[f32],
    rate: u32,
    dir: &Path,
    phase: Phase,
) -> EvalResult<(stages::DiarRun, Value)> {
    let mut telemetry = telemetry::Telemetry::new();
    let run = stages::diarize(samples, rate, dir, phase, false, &mut telemetry)?;
    let observations = telemetry.observations();
    let duration = samples.len() as u64 * 1000 / u64::from(rate);
    if run
        .result
        .turns
        .iter()
        .chain(&run.turns)
        .any(|turn| turn.track != "mic" || turn.start_ms >= turn.end_ms || turn.end_ms > duration)
    {
        return Err("production diarization turn exceeds source bounds".into());
    }
    if run.result.segmentation_context != Some(phase.segmentation_context()) {
        return Err("production result context does not match requested phase".into());
    }
    Ok((run, observations))
}
pub(super) fn run(manifest: &Path, out: &Path) -> EvalResult<()> {
    let bytes = fs::read(manifest).map_err(error)?;
    if bytes.len() > 1024 * 1024 {
        return Err("diarization comparison manifest exceeds1MiB".into());
    }
    let config: Comparison = serde_json::from_slice(&bytes).map_err(error)?;
    check_config(&config)?;
    let base = manifest.parent().unwrap_or(Path::new("."));
    let mut sources = vec![
        fingerprint(manifest, 1024 * 1024)?,
        pinned(base, &config.control.audio, 16 * 1024 * 1024)?,
    ];
    let directory = base.join(&config.model_directory);
    let model = model_provenance(&directory)?;
    if model["files"][0]["sha256"].as_str() != Some(config.model_manifest_sha256.as_str()) {
        return Err("diarization model manifest pin mismatch".into());
    }
    let control = wave(&base.join(&config.control.audio.path), 30)?;
    fs::create_dir(out).map_err(error)?;
    json_file(
        &out.join("INCOMPLETE.json"),
        &json!({"status":"incomplete"}),
    )?;
    let mut rows = vec![];
    let mut ids = std::collections::BTreeSet::new();
    for clip in &config.clips {
        for pin in [&clip.legacy_manifest, &clip.word_only_manifest] {
            sources.push(pinned(base, pin, 1024 * 1024)?);
        }
        for pin in [&clip.baseline_five_turns, &clip.experimental_ten_turns] {
            sources.push(pinned(base, pin, 64 * 1024 * 1024)?);
        }
        let legacy_path = base.join(&clip.legacy_manifest.path);
        let word_path = base.join(&clip.word_only_manifest.path);
        let legacy = input(&legacy_path, &mut sources)?;
        let word = input(&word_path, &mut sources)?;
        let id = &legacy.public_corpus.recording_id;
        if !ids.insert(id.clone())
            || word.public_corpus.recording_id != *id
            || legacy.audio.sha256 != word.audio.sha256
        {
            return Err(
                "clip IDs must be unique and reference protocols must share exact audio/recording"
                    .into(),
            );
        }
        let legacy_base = legacy_path.parent().ok_or("input needs parent")?;
        let word_base = word_path.parent().ok_or("input needs parent")?;
        let wav = wave(&legacy_base.join(&legacy.audio.path), 600)?;
        let rate = u32::try_from(wav.sample_rate()).map_err(error)?;
        let duration_ms = wav.samples().len() as u64 * 1000 / u64::from(rate);
        // Validate both reference protocols before constructing a model.
        scores(&legacy, legacy_base, "", duration_ms)?;
        scores(&word, word_base, "", duration_ms)?;
        let folder = out.join(id);
        fs::create_dir(&folder).map_err(error)?;
        let baseline = previous_turns(base, &clip.baseline_five_turns)?;
        let experimental = previous_turns(base, &clip.experimental_ten_turns)?;
        let mut phases = vec![];
        for phase in [Phase::Live, Phase::Refined] {
            let (run, observations) = execute(wav.samples(), rate, &directory, phase)?;
            let name = if phase == Phase::Live {
                "live5"
            } else {
                "refined10"
            };
            json_file(&folder.join(format!("{name}-result.json")), &run.result)?;
            json_file(
                &folder.join(format!("{name}-emissions.json")),
                &run.emissions,
            )?;
            let mut hypotheses = vec![(name, &run.turns)];
            if phase == Phase::Live {
                hypotheses.push(("live5-finalized-control", &run.result.turns));
            }
            let mut reports = vec![];
            for (label, turns) in hypotheses {
                let text = rttm(id, turns);
                fs::write(folder.join(format!("{label}.rttm")), &text).map_err(error)?;
                reports.push(json!({"assignment_view":label,"counts":counts(turns),"legacy_segments":scores(&legacy,legacy_base,&text,duration_ms)?,"secondary_only_words":scores(&word,word_base,&text,duration_ms)?}));
            }
            let prior = if phase == Phase::Live {
                &baseline
            } else {
                &experimental
            };
            phases.push(json!({"phase":phase,"settings":run.phase,"observations":observations,"scores":reports,"prior_artifact_assignment_projection_equal":projection(&run.result.turns)?==projection(prior)?,"projection_excludes_only":"provenance string (live/refined phase name)","calibrated":run.result.calibrated,"warnings":run.result.warnings,"constraint_reviews":run.result.constraint_reviews}));
        }
        rows.push(json!({"legacy_corpus":legacy.public_corpus,"secondary_corpus":word.public_corpus,"raw_audio_sha256":legacy.audio.sha256,"audio_ms":wav.samples().len() as f64*1000./f64::from(rate),"phases":phases}));
    }
    let mut controls = vec![];
    for phase in [Phase::Live, Phase::Refined] {
        let (run, observations) = execute(
            control.samples(),
            u32::try_from(control.sample_rate()).map_err(error)?,
            &directory,
            phase,
        )?;
        let name = if phase == Phase::Live {
            "control-live5"
        } else {
            "control-refined10"
        };
        json_file(&out.join(format!("{name}-result.json")), &run.result)?;
        controls.push(json!({"phase":phase,"settings":run.phase,"observations":observations,"counts":counts(&run.result.turns),"source_bounds_valid":true,"reference_scores":null,"reason":"Clean public control has no supplied speaker reference; no DER/JER invented"}));
    }
    for pin in sources.iter().chain(
        model["files"]
            .as_array()
            .ok_or("model fingerprints missing")?,
    ) {
        if fingerprint(
            Path::new(pin["path"].as_str().ok_or("source path missing")?),
            1024 * 1024 * 1024,
        )?["sha256"]
            != pin["sha256"]
        {
            return Err("source/model changed during diarization comparison".into());
        }
    }
    let result = json!({"schema_version":1,"acceptance":"not_claimed","mode":"production_diarization_context","runner":fingerprint(&std::env::current_exe().map_err(error)?,256*1024*1024)?,"sources":sources,"models":model,"clips":rows,"controls":controls,"settings":{"meeting_mode":"in_person","track":"mic","raw_gain":1,"packet_ms":20,"segmentation_threads":1,"embedding_threads":1,"pair_constraints":[],"temporal_constraints":[],"speaker_count_prior":null,"thresholds":"unchanged model-version experimental uncalibrated defaults","asr_constructed":false},"limitations":["Adjacent windows of one public meeting, not independent meetings or in-house4speaker/gallery50 acceptance.","Live uses actual emitted assignments; finalizedLive5s is explicitly labelled a retrospective comparison control.","Legacy segments and secondary standard only-words protocols remain separate; holdout only-words contains3lexical speakers.","Unknown/uncertain/provisional turns do not gain invented RTTM identities; missed reference speech remains scored.","Sampled CPU/RSS and warm-cache possible constructor times are observations, not peak/sustained hardware qualifications.","Unpaced direct-core throughput is not GUI/live capture latency; no devices,network,private meetings,ASR or app activation."]});
    json_file(&out.join("results.json"), &result)?;
    json_file(
        &out.join("COMPLETE.json"),
        &json!({"status":"complete","acceptance":"not_claimed","results":fingerprint(&out.join("results.json"),64*1024*1024)?}),
    )?;
    fs::remove_file(out.join("INCOMPLETE.json")).map_err(error)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn projection_ignores_phase_name_but_preserves_uncertainty_and_geometry() {
        let mut turn = SpeakerTurn {
            id: "mic:turn-1".into(),
            track: "mic".into(),
            start_ms: 1000,
            end_ms: 2000,
            speaker_ids: vec!["mic:speaker-1".into()],
            label: "Speaker1".into(),
            provenance: "refined-cluster".into(),
            score: Some(0.7),
            overlap: false,
            uncertain: false,
            provisional: false,
        };
        let original = projection(&[turn.clone()]).unwrap();
        turn.provenance = "live-cluster".into();
        assert_eq!(original, projection(&[turn.clone()]).unwrap());
        turn.uncertain = true;
        assert_ne!(original, projection(&[turn.clone()]).unwrap());
        turn.uncertain = false;
        turn.start_ms += 1;
        assert_ne!(original, projection(&[turn]).unwrap());
    }
    #[test]
    fn schema_and_public_bounds_fail_closed_before_native_inference() {
        assert!(!safe_id("../outside"));
        assert!(safe_id("ES2002a-0-300"));
        let mut value = json!({"schema_version":1,"mode":"production_diarization_context","clips":[],"model_directory":"unused","model_manifest_sha256":"0".repeat(64),"control":{"audio":{"path":"unused","sha256":"0".repeat(64)},"attribution":"public synthetic test","source_urls":["https://example.invalid"],"license_note":"synthetic"}});
        assert!(
            check_config(&serde_json::from_value::<Comparison>(value.clone()).unwrap()).is_err()
        );
        value["private_meeting_id"] = json!("never");
        assert!(serde_json::from_value::<Comparison>(value).is_err());
    }
}
