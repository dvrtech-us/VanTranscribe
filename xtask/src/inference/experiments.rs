//! Explicit experimental native configurations. No app configuration is changed.
use super::*;
use sherpa_onnx::{
    OfflineSpeakerDiarization, OfflineSpeakerDiarizationConfig, OnlineRecognizer,
    OnlineRecognizerConfig, SpeakerEmbeddingExtractorConfig,
};
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Probe {
    schema_version: u32,
    clips: Vec<PathBuf>,
    variants: Vec<Variant>,
    #[serde(default)]
    canonical_diarization: bool,
    #[serde(default = "default_thresholds")]
    canonical_thresholds: Vec<f32>,
}
fn default_thresholds() -> Vec<f32> {
    vec![0.5]
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Variant {
    id: String,
    encoder: PinnedFile,
    decoder: PinnedFile,
    joiner: PinnedFile,
    tokens: PinnedFile,
    license: String,
    source_urls: Vec<String>,
    #[serde(default = "one")]
    gain: f32,
    #[serde(default = "greedy")]
    decoding_method: String,
    #[serde(default)]
    official_endpoints: bool,
}
fn one() -> f32 {
    1.0
}
fn greedy() -> String {
    "greedy_search".into()
}
pub fn run(manifest: &Path, output: &Path) -> EvalResult<()> {
    let source = fs::read(manifest).map_err(error)?;
    if source.len() > 1024 * 1024 {
        return Err("probe config exceeds1MiB".into());
    }
    let mode: Value = serde_json::from_slice(&source).map_err(error)?;
    if mode.get("mode").and_then(Value::as_str) == Some("production_live_comparison") {
        return super::production_live::run(manifest, output);
    }
    if mode.get("mode").and_then(Value::as_str) == Some("production_diarization_context") {
        return super::production_diarization::run(manifest, output);
    }
    let probe: Probe = serde_json::from_slice(&source).map_err(error)?;
    let runner = json!({"binary":fingerprint(&std::env::current_exe().map_err(error)?,256*1024*1024)?,"config":fingerprint(manifest,1024*1024)?});
    if probe.schema_version != 1
        || probe.clips.is_empty()
        || probe.clips.len() > 2
        || probe.variants.is_empty()
        || probe.variants.len() > 12
    {
        return Err("probe requires1–2public clips and1–12explicitvariants".into());
    }
    if probe.canonical_thresholds.is_empty()
        || probe.canonical_thresholds.len() > 3
        || probe
            .canonical_thresholds
            .iter()
            .any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
    {
        return Err("canonical thresholds require1–3finitecosinedistances in0..1".into());
    }
    let base = manifest.parent().unwrap_or(Path::new("."));
    let mut names = std::collections::BTreeSet::new();
    let mut model_sources = vec![];
    for v in &probe.variants {
        if v.id.is_empty()
            || !v
                .id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
            || !names.insert(v.id.clone())
            || !v.gain.is_finite()
            || !(1.0..=16.0).contains(&v.gain)
            || !matches!(
                v.decoding_method.as_str(),
                "greedy_search" | "modified_beam_search"
            )
            || v.license.is_empty()
            || v.source_urls.is_empty()
            || v.source_urls.iter().any(|s| !s.starts_with("https://"))
        {
            return Err(
                "invalid/duplicate experimentalvariant identity/licence/source/settings".into(),
            );
        }
        for file in [&v.encoder, &v.decoder, &v.joiner, &v.tokens] {
            model_sources.push(pinned(base, file, 1024 * 1024 * 1024)?);
        }
    }
    fs::create_dir(output).map_err(error)?;
    json_file(
        &output.join("INCOMPLETE.json"),
        &json!({"status":"incomplete"}),
    )?;
    let mut rows = vec![];
    for path in &probe.clips {
        let path = base.join(path);
        let clip_base = path.parent().ok_or("clip needs parent")?;
        let config: Input =
            serde_json::from_slice(&fs::read(&path).map_err(error)?).map_err(error)?;
        if config.schema_version != 1
            || config.public_corpus.id.trim().is_empty()
            || config.public_corpus.license.is_empty()
            || config.public_corpus.source_urls.is_empty()
            || config
                .public_corpus
                .source_urls
                .iter()
                .any(|s| !s.starts_with("https://"))
            || config.public_corpus.attribution.trim().is_empty()
            || config.public_corpus.derivation.trim().is_empty()
            || !safe_id(&config.public_corpus.recording_id)
        {
            return Err("probe requires explicit public source manifests".into());
        }
        let mut sources = vec![fingerprint(&path, 1024 * 1024)?];
        for file in [
            &config.audio,
            &config.reference_text,
            &config.reference_rttm,
            &config.uem,
        ] {
            sources.push(pinned(clip_base, file, 64 * 1024 * 1024)?);
        }
        let audio = clip_base.join(&config.audio.path);
        let wave = sherpa_onnx::Wave::read(audio.to_str().ok_or("UTF8 audio required")?)
            .ok_or("invalid probe WAV")?;
        let rate = u32::try_from(wave.sample_rate()).map_err(error)?;
        if !(8000..=192000).contains(&rate)
            || wave.samples().is_empty()
            || wave.samples().len() > rate as usize * 600
        {
            return Err("probe requires bounded mono audio <=600s".into());
        }
        let reference =
            fs::read_to_string(clip_base.join(&config.reference_text.path)).map_err(error)?;
        let clip_dir = output.join(&config.public_corpus.recording_id);
        fs::create_dir(&clip_dir).map_err(error)?;
        let mut variants = vec![];
        for v in &probe.variants {
            eprintln!(
                "Native experiment {}: {}",
                config.public_corpus.recording_id, v.id
            );
            let samples = wave
                .samples()
                .iter()
                .map(|s| (s * v.gain).clamp(-1., 1.))
                .collect::<Vec<_>>();
            let clipped = wave
                .samples()
                .iter()
                .filter(|s| s.abs() * v.gain > 1.)
                .count();
            let (text, events, timing) = native(&samples, rate, v, base)?;
            fs::write(clip_dir.join(format!("{}.txt", v.id)), &text).map_err(error)?;
            json_file(&clip_dir.join(format!("{}-events.json", v.id)), &events)?;
            variants.push(json!({"variant":v,"clipped_samples":clipped,"wer":crate::words::score(&reference,&text,crate::words::Normalization::UnicodeLowercasePunctuationToSpace)?,"timing":timing}));
        }
        let diarization = if probe.canonical_diarization {
            let mut results = vec![];
            for threshold in &probe.canonical_thresholds {
                results.push(canonical(
                    wave.samples(),
                    rate,
                    &clip_base.join(&config.models.diarization),
                    &config.public_corpus.recording_id,
                    &clip_base.join(&config.reference_rttm.path),
                    &clip_base.join(&config.uem.path),
                    &clip_dir,
                    *threshold,
                )?);
            }
            Some(results)
        } else {
            None
        };
        for s in &sources {
            if fingerprint(
                Path::new(s["path"].as_str().ok_or("missing sourcepath")?),
                64 * 1024 * 1024,
            )?["sha256"]
                != s["sha256"]
            {
                return Err("clip changed duringprobe".into());
            }
        }
        rows.push(json!({"corpus":config.public_corpus,"sources":sources,"variants":variants,"canonical_diarization":diarization}));
        json_file(
            &output.join("results.json"),
            &json!({"acceptance":"not_claimed","experiments":rows,"model_files":model_sources,"runner":runner}),
        )?;
    }
    for s in &model_sources {
        if fingerprint(
            Path::new(s["path"].as_str().ok_or("missing modelpath")?),
            1024 * 1024 * 1024,
        )?["sha256"]
            != s["sha256"]
        {
            return Err("candidate model changed duringprobe".into());
        }
    }
    json_file(
        &output.join("COMPLETE.json"),
        &json!({"status":"complete","acceptance":"not_claimed","config":fingerprint(manifest,1024*1024)?}),
    )?;
    fs::remove_file(output.join("INCOMPLETE.json")).map_err(error)?;
    Ok(())
}
fn safe_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
}

fn native(
    samples: &[f32],
    rate: u32,
    v: &Variant,
    base: &Path,
) -> EvalResult<(String, Vec<Value>, Value)> {
    let load = Instant::now();
    let mut cfg = OnlineRecognizerConfig::default();
    let path = |p: &PinnedFile| {
        base.join(&p.path)
            .to_str()
            .map(str::to_string)
            .ok_or("model path must beUTF8")
    };
    cfg.model_config.transducer.encoder = Some(path(&v.encoder)?);
    cfg.model_config.transducer.decoder = Some(path(&v.decoder)?);
    cfg.model_config.transducer.joiner = Some(path(&v.joiner)?);
    cfg.model_config.tokens = Some(path(&v.tokens)?);
    cfg.model_config.num_threads = 2;
    cfg.decoding_method = Some(v.decoding_method.clone());
    cfg.max_active_paths = 4;
    cfg.enable_endpoint = true;
    cfg.rule1_min_trailing_silence = 2.4;
    cfg.rule2_min_trailing_silence = if v.official_endpoints { 1.2 } else { 0.8 };
    cfg.rule3_min_utterance_length = if v.official_endpoints { 20.0 } else { 15.0 };
    let recognizer =
        OnlineRecognizer::create(&cfg).ok_or("native experimental recognizer failed")?;
    let load_ms = load.elapsed().as_secs_f64() * 1000.;
    let mut telemetry = telemetry::Telemetry::new();
    telemetry.sample();
    let start = Instant::now();
    let stream = recognizer.create_stream();
    let mut events = vec![];
    let mut finals = vec![];
    let mut offset = 0;
    for packet in samples.chunks((rate / 50) as usize) {
        stream.accept_waveform(rate as i32, packet);
        while recognizer.is_ready(&stream) {
            recognizer.decode(&stream);
            if recognizer.is_endpoint(&stream) {
                if let Some(result) = recognizer.get_result(&stream)
                    && !result.text.trim().is_empty()
                {
                    finals.push(result.text.clone());
                    events.push(json!({"text":result.text,"native_start_time":result.start_time,"native_timestamps":result.timestamps,"input_end_ms":(offset+packet.len()) as f64*1000./f64::from(rate),"emitted_wall_ms":start.elapsed().as_secs_f64()*1000.}));
                }
                recognizer.reset(&stream);
            }
        }
        offset += packet.len();
        telemetry.sample();
    }
    stream.accept_waveform(rate as i32, &vec![0.; (rate as f32 * 0.3) as usize]);
    stream.input_finished();
    while recognizer.is_ready(&stream) {
        recognizer.decode(&stream);
    }
    if let Some(result) = recognizer.get_result(&stream)
        && !result.text.trim().is_empty()
    {
        finals.push(result.text.clone());
        events.push(json!({"text":result.text,"native_start_time":result.start_time,"native_timestamps":result.timestamps,"terminal_flush":true,"emitted_wall_ms":start.elapsed().as_secs_f64()*1000.}));
    }
    let processing_ms = start.elapsed().as_secs_f64() * 1000.;
    Ok((
        finals.join(" "),
        events,
        json!({"constructor_ms":load_ms,"processing_ms":processing_ms,"real_time_factor":processing_ms/(samples.len() as f64*1000./f64::from(rate)),"num_threads":2,"configured_api_sample_rate":16000,"configured_api_feature_dim":80,"native_model_may_override_features":true,"input_contract":"normalized[-1,1] floats; native default normalize_samples=true","vad":false,"packet_ms":20,"observations":telemetry.observations()}),
    ))
}
#[allow(clippy::too_many_arguments)]
fn canonical(
    samples: &[f32],
    rate: u32,
    dir: &Path,
    id: &str,
    reference: &Path,
    uem: &Path,
    out: &Path,
    threshold: f32,
) -> EvalResult<Value> {
    eprintln!("Canonical full-context diarization experiment: {id}");
    let path = |name: &str| {
        dir.join(name)
            .to_str()
            .map(str::to_string)
            .ok_or("model path must beUTF8")
    };
    let mut cfg = OfflineSpeakerDiarizationConfig {
        embedding: SpeakerEmbeddingExtractorConfig {
            model: Some(path(vt_diarize::EMBEDDING)?),
            num_threads: 1,
            debug: false,
            provider: Some("cpu".into()),
        },
        min_duration_on: 0.,
        min_duration_off: 0.,
        ..Default::default()
    };
    cfg.segmentation.pyannote.model = Some(path(vt_diarize::SEGMENTATION)?);
    cfg.segmentation.pyannote.window_shift_ratio = 0.5;
    cfg.segmentation.num_threads = 1;
    cfg.clustering.threshold = threshold;
    let engine =
        OfflineSpeakerDiarization::create(&cfg).ok_or("canonical native diarizer failed")?;
    let audio = if rate == 16000 {
        samples.to_vec()
    } else {
        sherpa_onnx::LinearResampler::create(rate as i32, 16000)
            .ok_or("resamplerfailed")?
            .resample(samples, true)
    };
    let start = Instant::now();
    let segments = engine
        .process(&audio)
        .ok_or("canonical diarizationfailed")?
        .sort_by_start_time();
    let mut rttm = String::new();
    let mut raw = vec![];
    for t in segments {
        if !t.start.is_finite() || !t.end.is_finite() || t.start < 0. || t.end <= t.start {
            return Err("invalid canonical native interval".into());
        }
        rttm.push_str(&format!(
            "SPEAKER {id} 1 {:.6} {:.6} <NA> <NA> native-{} <NA> <NA>\n",
            t.start,
            t.end - t.start,
            t.speaker
        ));
        raw.push(json!({"start":t.start,"end":t.end,"speaker":t.speaker}));
    }
    fs::write(out.join(format!("canonical10s-{threshold:.2}.rttm")), &rttm).map_err(error)?;
    json_file(
        &out.join(format!("canonical10s-{threshold:.2}-segments.json")),
        &raw,
    )?;
    let refturns =
        crate::diarization::parse_rttm(&fs::read_to_string(reference).map_err(error)?, id, "1")?;
    let hyp = crate::diarization::parse_hypothesis_rttm(&rttm, id, "1")?;
    let regions = crate::diarization::parse_uem(&fs::read_to_string(uem).map_err(error)?, id, "1")?;
    Ok(
        json!({"der_jer":crate::diarization::score(&refturns,&hyp,&regions)?,"processing_ms":start.elapsed().as_secs_f64()*1000.,"threshold":threshold,"settings":"sherpa full bounded<=600s offline API; canonical10s segmentation context,50%shift; numclusters=-1; threshold is complete-linkage cosineDISTANCE; no referencecount; native assignments including shortsegments; DOES NOTimplementproductUnknown<1s/userconstraintpolicy","models":model_provenance(dir)?}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn experiment_ids_cannot_escape_the_new_output_directory() {
        for id in ["../escape", "/absolute", "x/y", "x\\y", "", "two words"] {
            assert!(!safe_id(id));
        }
        assert!(safe_id("ES2002a-array1-01-300-600"));
    }
}
