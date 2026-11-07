//! Comparative validation through the actual pinned production recognizer API.
//! No native recognizer reimplementation, gain, activation or model acquisition.
use super::*;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Comparison {
    schema_version: u32,
    mode: Mode,
    clips: Vec<PathBuf>,
    models: Vec<Model>,
    vad: PinnedFile,
    control: Control,
    #[serde(default)]
    paced_control: bool,
    #[serde(default)]
    vad_thresholds: Vec<f32>,
    #[serde(default)]
    default_baseline: Option<PinnedFile>,
    #[serde(default)]
    generated_non_speech_control: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Mode {
    ProductionLiveComparison,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Model {
    id: String,
    directory: PathBuf,
    manifest_sha256: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Control {
    audio: PinnedFile,
    attribution: String,
    source_urls: Vec<String>,
    license_note: String,
}

fn safe_id(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
}
fn check_config(c: &Comparison) -> EvalResult<()> {
    let _ = &c.mode;
    if c.schema_version != 1
        || c.clips.is_empty()
        || c.clips.len() > 2
        || c.models.is_empty()
        || c.models.len() > 2
        || c.control.attribution.trim().is_empty()
        || c.control.license_note.trim().is_empty()
        || c.control.source_urls.is_empty()
        || c.control
            .source_urls
            .iter()
            .any(|s| !s.starts_with("https://"))
    {
        return Err("production live comparison requires schema1,1–2 public clips/models and attributed explicit public control".into());
    }
    let unique = c
        .models
        .iter()
        .map(|m| &m.id)
        .collect::<std::collections::BTreeSet<_>>();
    if unique.len() != c.models.len()
        || c.models
            .iter()
            .any(|m| !safe_id(&m.id) || m.manifest_sha256.len() != 64)
    {
        return Err("model IDs must be unique safe names with exact manifest SHA256".into());
    }
    if c.vad_thresholds.len() > 3
        || c.vad_thresholds
            .iter()
            .any(|v| !v.is_finite() || !(0.1..=0.9).contains(v))
        || c.vad_thresholds
            .iter()
            .map(|v| v.to_bits())
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            != c.vad_thresholds.len()
        || !c.vad_thresholds.is_empty() && c.models.len() != 1
        || c.default_baseline.is_some() && !c.vad_thresholds.contains(&0.5)
    {
        return Err("threshold study requires one model, at most3unique finite .1–.9 thresholds and an explicit .5 case for default parity".into());
    }
    Ok(())
}
fn cases(config: &Comparison) -> Vec<(bool, Option<f32>)> {
    if config.vad_thresholds.is_empty() {
        vec![(true, None), (false, None)]
    } else {
        config
            .vad_thresholds
            .iter()
            .map(|t| (true, Some(*t)))
            .collect()
    }
}
fn projection(events: &[Value]) -> Vec<Value> {
    events.iter().map(|e|json!({"track":e["track"],"start_ms":e["start_ms"],"end_ms":e["end_ms"],"text":e["text"],"is_final":e["is_final"],"words":e["words"]})).collect()
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
        return Err("comparison WAV exceeds finite mono rate/duration bounds".into());
    }
    Ok(wave)
}
fn verify_events(run: &stages::TextRun, duration_ms: u64) -> EvalResult<Value> {
    let mut final_events = 0;
    let mut words = 0;
    for event in &run.events {
        let a = event["start_ms"]
            .as_u64()
            .ok_or("event missing native start")?;
        let b = event["end_ms"].as_u64().ok_or("event missing native end")?;
        if a > b || b > duration_ms {
            return Err("production event timing exceeds source clock".into());
        }
        if event["is_final"] == true {
            final_events += 1;
        }
        for word in event["words"]
            .as_array()
            .ok_or("event missing word timings")?
        {
            let x = word["start_ms"]
                .as_u64()
                .ok_or("word missing native start")?;
            let y = word["end_ms"].as_u64().ok_or("word missing native end")?;
            if x > y || x < a || y > b {
                return Err("production word timing exceeds its source event".into());
            }
            words += 1;
        }
    }
    Ok(
        json!({"events":run.events.len(),"final_events":final_events,"word_timings":words,"native_timing_bounds_validated":true}),
    )
}
#[allow(clippy::too_many_arguments)]
fn execute(
    samples: &[f32],
    rate: u32,
    model: &Model,
    base: &Path,
    vad: &Path,
    gated: bool,
    paced: bool,
    out: &Path,
    reference: Option<&str>,
    threshold: Option<f32>,
    parity_file: Option<&Path>,
) -> EvalResult<Value> {
    let mut telemetry = telemetry::Telemetry::new();
    let run = stages::live_options(
        samples,
        rate,
        &base.join(&model.directory),
        gated.then_some(vad),
        threshold.map(|threshold| vt_stt::LiveVadOptions { threshold }),
        paced,
        &mut telemetry,
    )?;
    let suffix = threshold
        .map(|t| format!("vad-threshold-{t:.2}-{:08x}", t.to_bits()))
        .unwrap_or_else(|| {
            if gated {
                "vad".into()
            } else {
                "ungated".into()
            }
        });
    let key = format!("{}-{suffix}", model.id);
    fs::write(out.join(format!("{key}.txt")), &run.text).map_err(error)?;
    json_file(&out.join(format!("{key}-events.json")), &run.events)?;
    let score = reference
        .map(|r| {
            crate::words::score(
                r,
                &run.text,
                crate::words::Normalization::UnicodeLowercasePunctuationToSpace,
            )
        })
        .transpose()?;
    let parity = if let Some(path) = parity_file {
        let baseline: Vec<Value> =
            serde_json::from_slice(&fs::read(path).map_err(error)?).map_err(error)?;
        Some(
            json!({"native_events_text_words_equal":projection(&run.events)==projection(&baseline),"baseline_events":fingerprint(path,64*1024*1024)?,"scope":"Exact native text/finality/source timings/word timings; wall clock and resource metadata intentionally excluded."}),
        )
    } else {
        None
    };
    let record = json!({"run_key":key,"model":model.id,"vad_enabled":gated,"vad_threshold":gated.then_some(threshold.unwrap_or(0.5)),"gain":1,"source_samples_unchanged":true,"api":if threshold.is_some(){"StreamingRecognizer::with_vad_options(verified_model_dir,pinned_vad,LiveVadOptions{threshold})"}else if gated{"StreamingRecognizer::with_vad(verified_model_dir,Some(pinned_vad))"}else{"StreamingRecognizer::new(verified_model_dir)"},"timing":run.phase,"event_counts":verify_events(&run,samples.len()as u64*1000/u64::from(rate))?,"final_word_count":crate::words::tokens(&run.text,crate::words::Normalization::UnicodeLowercasePunctuationToSpace).len(),"default_threshold_parity":parity,"wer":score,"observations":telemetry.observations()});
    json_file(&out.join(format!("{key}-metrics.json")), &record)?;
    if record["default_threshold_parity"]["native_events_text_words_equal"] == false {
        return Err(
            "explicit threshold .5 differs from the preserved production default baseline".into(),
        );
    }
    Ok(record)
}
pub(super) fn run(manifest: &Path, out: &Path) -> EvalResult<()> {
    let raw = fs::read(manifest).map_err(error)?;
    if raw.len() > 1024 * 1024 {
        return Err("comparison manifest exceeds1MiB".into());
    }
    let config: Comparison = serde_json::from_slice(&raw).map_err(error)?;
    check_config(&config)?;
    let base = manifest.parent().unwrap_or(Path::new("."));
    let mut sources = vec![
        fingerprint(manifest, 1024 * 1024)?,
        pinned(base, &config.vad, 1024 * 1024)?,
        pinned(base, &config.control.audio, 16 * 1024 * 1024)?,
    ];
    let baseline_dir = if let Some(baseline) = &config.default_baseline {
        sources.push(pinned(base, baseline, 64 * 1024 * 1024)?);
        Some(
            base.join(&baseline.path)
                .parent()
                .ok_or("baseline needs parent")?
                .to_path_buf(),
        )
    } else {
        None
    };
    let mut models = vec![];
    for model in &config.models {
        let dir = base.join(&model.directory);
        let file = dir.join("manifest.json");
        let pin = fingerprint(&file, 1024 * 1024)?;
        if pin["sha256"].as_str() != Some(model.manifest_sha256.as_str()) {
            return Err("production model manifest pin mismatch".into());
        }
        let value: Value =
            serde_json::from_slice(&fs::read(file).map_err(error)?).map_err(error)?;
        if value["id"].as_str() != Some(model.id.as_str()) {
            return Err("production model ID mismatch".into());
        }
        models.push(model_provenance(&dir)?);
    }
    let control = wave(&base.join(&config.control.audio.path), 30)?;
    fs::create_dir(out).map_err(error)?;
    json_file(
        &out.join("INCOMPLETE.json"),
        &json!({"status":"incomplete"}),
    )?;
    let runner = json!({"binary":fingerprint(&std::env::current_exe().map_err(error)?,256*1024*1024)?,"settings":"actualproductionAPI;sourceclock20mspackets;Mictrack;rawgain1;productionendpoints.8s/15s;greedy2STTthreads;VAD1thread;sequentialfreshengines;noOSdevices/network/defaultchange"});
    let mut rows = vec![];
    for path in &config.clips {
        let path = base.join(path);
        sources.push(fingerprint(&path, 1024 * 1024)?);
        let input: Input =
            serde_json::from_slice(&fs::read(&path).map_err(error)?).map_err(error)?;
        let clip_base = path.parent().ok_or("clip has no parent")?;
        if input.schema_version != 1
            || !safe_id(&input.public_corpus.recording_id)
            || input.public_corpus.id.trim().is_empty()
            || input.public_corpus.attribution.trim().is_empty()
            || input.public_corpus.derivation.trim().is_empty()
            || input.public_corpus.license.trim().is_empty()
            || input.public_corpus.source_urls.is_empty()
            || input
                .public_corpus
                .source_urls
                .iter()
                .any(|s| !s.starts_with("https://"))
        {
            return Err("comparison requires explicit public corpus licence/scope".into());
        }
        for file in [
            &input.audio,
            &input.reference_text,
            &input.reference_rttm,
            &input.uem,
        ] {
            sources.push(pinned(clip_base, file, 64 * 1024 * 1024)?);
        }
        let wave = wave(&clip_base.join(&input.audio.path), 600)?;
        let rate = u32::try_from(wave.sample_rate()).map_err(error)?;
        let reference =
            fs::read_to_string(clip_base.join(&input.reference_text.path)).map_err(error)?;
        let directory = out.join(&input.public_corpus.recording_id);
        fs::create_dir(&directory).map_err(error)?;
        let mut records = vec![];
        for model in &config.models {
            for (gated, threshold) in cases(&config) {
                eprintln!(
                    "Production catalogue comparison {} model={} VAD={gated}",
                    input.public_corpus.recording_id, model.id
                );
                let parity_file = if threshold == Some(0.5) {
                    baseline_dir.as_ref().map(|d| {
                        d.join(&input.public_corpus.recording_id)
                            .join(format!("{}-vad-events.json", model.id))
                    })
                } else {
                    None
                };
                if let Some(path) = &parity_file {
                    sources.push(fingerprint(path, 64 * 1024 * 1024)?);
                }
                records.push(execute(
                    wave.samples(),
                    rate,
                    model,
                    base,
                    &base.join(&config.vad.path),
                    gated,
                    false,
                    &directory,
                    Some(&reference),
                    threshold,
                    parity_file.as_deref(),
                )?);
            }
        }
        rows.push(json!({"corpus":input.public_corpus,"duration_ms":wave.samples().len()as u64*1000/u64::from(rate),"records":records}));
        json_file(
            &out.join("results.json"),
            &json!({"acceptance":"not_claimed","runner":runner,"models":models,"sources":sources,"clips":rows}),
        )?;
    }
    let control_dir = out.join("public-control");
    fs::create_dir(&control_dir).map_err(error)?;
    let mut control_records = vec![];
    for model in &config.models {
        for (gated, threshold) in cases(&config) {
            let parity_file = if threshold == Some(0.5) {
                baseline_dir.as_ref().map(|d| {
                    d.join("public-control")
                        .join(format!("{}-vad-events.json", model.id))
                })
            } else {
                None
            };
            if let Some(path) = &parity_file {
                sources.push(fingerprint(path, 64 * 1024 * 1024)?);
            }
            control_records.push(execute(
                control.samples(),
                u32::try_from(control.sample_rate()).map_err(error)?,
                model,
                base,
                &base.join(&config.vad.path),
                gated,
                config.paced_control,
                &control_dir,
                None,
                threshold,
                parity_file.as_deref(),
            )?);
        }
    }
    let generated = if config.generated_non_speech_control {
        let (audio, description) = non_speech();
        let directory = out.join("generated-non-speech");
        fs::create_dir(&directory).map_err(error)?;
        let mut records = vec![];
        for model in &config.models {
            for (gated, threshold) in cases(&config) {
                let mut record = execute(
                    &audio,
                    16000,
                    model,
                    base,
                    &base.join(&config.vad.path),
                    gated,
                    false,
                    &directory,
                    None,
                    threshold,
                    None,
                )?;
                record["false_positive_final_words"] = record["final_word_count"].clone();
                records.push(record);
            }
        }
        Some(
            json!({"source":description,"records":records,"reference":"Known generated non-speech; every emitted final word is a false positive. No WER denominator is invented."}),
        )
    } else {
        None
    };
    for source in &sources {
        let p = Path::new(source["path"].as_str().ok_or("missing sourcepath")?);
        if fingerprint(p, 64 * 1024 * 1024)?["sha256"] != source["sha256"] {
            return Err("source changed duringproductioncomparison".into());
        }
    }
    for model in &models {
        for source in model["files"].as_array().ok_or("missing model files")? {
            let p = Path::new(source["path"].as_str().ok_or("missing modelpath")?);
            if fingerprint(p, 1024 * 1024 * 1024)?["sha256"] != source["sha256"] {
                return Err("model changed duringproductioncomparison".into());
            }
        }
    }
    json_file(
        &out.join("results.json"),
        &json!({"schema_version":1,"acceptance":"not_claimed","runner":runner,"models":models,"sources":sources,"clips":rows,"control":{"source":config.control.audio,"attribution":config.control.attribution,"source_urls":config.control.source_urls,"license_note":config.control.license_note,"paced":config.paced_control,"records":control_records,"reference_scoring":"No control transcript reference supplied; no WER invented."},"generated_non_speech":generated,"limitations":["Native packetemissiondelay is not productUI/capture latency.","No in-house four-speaker/gallery50 or sustainedhardwareacceptance.","No candidate selection/default activation or input normalization is performed.","A bounded threshold study is not global VAD calibration."]}),
    )?;
    json_file(
        &out.join("COMPLETE.json"),
        &json!({"status":"complete","acceptance":"not_claimed","results":fingerprint(&out.join("results.json"),64*1024*1024)?}),
    )?;
    fs::remove_file(out.join("INCOMPLETE.json")).map_err(error)?;
    Ok(())
}

fn non_speech() -> (Vec<f32>, Value) {
    let mut audio = vec![0.; 60 * 16000];
    let mut seed = 0x4b52d69768a31f05_u64;
    let mut low = 0_f32;
    for sample in &mut audio[30 * 16000..] {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        let white = (seed >> 40) as f32 / (1_u32 << 23) as f32 - 1.;
        low = 0.85 * low + 0.15 * white;
        *sample = 0.03 * low;
    }
    let mut hash = Sha256::new();
    for value in &audio {
        hash.update(value.to_le_bytes());
    }
    (
        audio,
        json!({"kind":"generated_known_non_speech","rate":16000,"frames":960000,"silence_ms":30000,"colored_noise_ms":30000,"generator":"xorshift64 seed4b52d69768a31f05;white=(state>>40)/2^23-1;low=.85low+.15white;sample=.03low;allf32;no speech synthesis","sha256_f32le":format!("{:x}",hash.finalize())}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn comparison_rejects_duplicate_model_ids_and_missing_public_control_metadata() {
        let base = r#"{"schema_version":1,"mode":"production_live_comparison","clips":["clip.json"],"models":[{"id":"june21","directory":"model","manifest_sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}],"vad":{"path":"vad","sha256":"a"},"control":{"audio":{"path":"control","sha256":"a"},"attribution":"Official public control","source_urls":["https://example.invalid/control"],"license_note":"Public source attribution"}}"#;
        let valid: Comparison = serde_json::from_str(base).unwrap();
        assert!(check_config(&valid).is_ok());
        let mut value: Value = serde_json::from_str(base).unwrap();
        let model = value["models"][0].clone();
        value["models"].as_array_mut().unwrap().push(model);
        assert!(check_config(&serde_json::from_value(value).unwrap()).is_err());
        let mut value: Value = serde_json::from_str(base).unwrap();
        value["control"]["source_urls"] = json!([]);
        assert!(check_config(&serde_json::from_value(value).unwrap()).is_err());
    }

    #[test]
    fn parity_ignores_wall_clock_but_rejects_changed_native_text_or_word_timing() {
        let mut first = json!({"track":"mic","start_ms":10,"end_ms":50,"text":"public","is_final":true,"words":[{"text":"public","start_ms":12,"end_ms":42}],"emitted_wall_ms":100});
        let second = first.clone();
        first["emitted_wall_ms"] = json!(999);
        assert_eq!(
            projection(&[first.clone()]),
            projection(std::slice::from_ref(&second))
        );
        first["words"][0]["start_ms"] = json!(13);
        assert_ne!(projection(&[first]), projection(&[second]));
    }

    #[test]
    fn generated_non_speech_is_bounded_finite_and_starts_with_exact_silence() {
        let (audio, meta) = non_speech();
        assert_eq!(audio.len(), 960000);
        assert!(audio[..480000].iter().all(|v| *v == 0.0));
        assert!(audio.iter().all(|v| v.is_finite() && v.abs() < 0.03));
        let rms = (audio[480000..]
            .iter()
            .map(|v| f64::from(*v).powi(2))
            .sum::<f64>()
            / 480000.0)
            .sqrt();
        assert!((0.003..0.006).contains(&rms));
        assert_eq!(meta["sha256_f32le"].as_str().unwrap().len(), 64);
    }
}
