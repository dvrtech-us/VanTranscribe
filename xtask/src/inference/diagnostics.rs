//! Explicit corpus-only ablations; never change production settings or baseline.
use super::*;
use sherpa_onnx::{LinearResampler, SileroVadModelConfig, VadModelConfig, VoiceActivityDetector};
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Diagnostics {
    #[serde(default)]
    pub ungated: bool,
    #[serde(default)]
    pub gains: Vec<u8>,
    pub control_wav: Option<PinnedFile>,
}
impl Diagnostics {
    pub fn validate(&self) -> EvalResult<()> {
        let unique = self.gains.iter().collect::<std::collections::BTreeSet<_>>();
        if self.gains.len() > 4
            || unique.len() != self.gains.len()
            || self.gains.iter().any(|g| ![2, 4, 8, 16].contains(g))
        {
            return Err("diagnostic gains must be a unique bounded subset of2,4,8,16".into());
        }
        Ok(())
    }
}
fn acoustic(samples: &[f32]) -> Value {
    json!({"rms":(samples.iter().map(|s|f64::from(*s).powi(2)).sum::<f64>()/samples.len() as f64).sqrt(),"peak":samples.iter().map(|s|s.abs()).fold(0.0,f32::max)})
}
fn detector(samples: &[f32], rate: u32, vad: &Path) -> EvalResult<Value> {
    let input = if rate == 16000 {
        samples.to_vec()
    } else {
        LinearResampler::create(rate as i32, 16000)
            .ok_or("diagnostic resampler failed")?
            .resample(samples, true)
    };
    let config = VadModelConfig {
        silero_vad: SileroVadModelConfig {
            model: Some(vad.to_str().ok_or("VAD path must be UTF8")?.into()),
            threshold: 0.5,
            min_silence_duration: 0.16,
            min_speech_duration: 0.032,
            window_size: 512,
            max_speech_duration: 18.0,
        },
        sample_rate: 16000,
        num_threads: 1,
        provider: Some("cpu".into()),
        ..Default::default()
    };
    let detector = VoiceActivityDetector::create(&config, 20.0).ok_or("diagnostic VAD failed")?;
    let (mut speech, mut count, mut frames) = (0, 0, 0);
    for window in input.as_chunks::<512>().0 {
        if frames >= 16 * 16000 {
            detector.reset();
            frames = 0;
        }
        detector.accept_waveform(window);
        frames += 512;
        count += 1;
        if detector.detected() {
            speech += 1
        }
        detector.clear();
    }
    Ok(
        json!({"windows":count,"detected_speech_windows":speech,"window_ms":32,"settings":"same native detector fields/reset/clear sequence as production LiveVad; does not measure ASR gate-open windows","discarded_partial_samples":input.len()%512}),
    )
}
fn save_text(out: &Path, name: &str, run: &stages::TextRun) -> EvalResult<()> {
    fs::write(out.join(format!("{name}.txt")), &run.text).map_err(error)?;
    json_file(&out.join(format!("{name}-events.json")), &run.events)
}
#[allow(clippy::too_many_arguments)]
pub(super) fn run(
    config: &Diagnostics,
    samples: &[f32],
    rate: u32,
    streaming: &Path,
    refinement: &Path,
    diarization: &Path,
    id: &str,
    reference_text: &Path,
    reference_turns: &[crate::diarization::Turn],
    regions: &[crate::diarization::Span],
    base: &Path,
    output: &Path,
) -> EvalResult<Value> {
    config.validate()?;
    let out = output.join("diagnostics");
    fs::create_dir(&out).map_err(error)?;
    let reference = fs::read_to_string(reference_text).map_err(error)?;
    let normalize = crate::words::Normalization::UnicodeLowercasePunctuationToSpace;
    let vad = refinement.join("silero_vad.onnx");
    let mut telemetry = telemetry::Telemetry::new();
    let mut result = json!({"config":config,"acceptance":"not_claimed","baseline_acoustics":acoustic(samples),"raw_native_vad":detector(samples,rate,&vad)?,"gain_transform":"corpus memory only; multiply gain then clamp[-1,1]; original WAV/production configuration unchanged","gains":[]});
    if config.ungated {
        let run = stages::live(samples, rate, streaming, None, false, &mut telemetry)?;
        save_text(&out, "raw-ungated", &run)?;
        result["raw_ungated"] = json!({"wer":crate::words::score(&reference,&run.text,normalize)?,"phase":run.phase,"events":run.events.len()});
    }
    if let Some(control) = &config.control_wav {
        let path = base.join(&control.path);
        let wave = sherpa_onnx::Wave::read(path.to_str().ok_or("control path must be UTF8")?)
            .ok_or("cannot decode explicit public control")?;
        let control_rate = u32::try_from(wave.sample_rate()).map_err(error)?;
        if !(8000..=192000).contains(&control_rate)
            || wave.samples().is_empty()
            || wave.samples().len() > control_rate as usize * 30
            || wave.samples().iter().any(|s| !s.is_finite())
        {
            return Err("control must be nonempty and at most30seconds".into());
        }
        let mut records = vec![];
        for gated in [true, false] {
            let run = stages::live(
                wave.samples(),
                control_rate,
                streaming,
                gated.then_some(vad.as_path()),
                false,
                &mut telemetry,
            )?;
            save_text(
                &out,
                if gated {
                    "control-vad"
                } else {
                    "control-ungated"
                },
                &run,
            )?;
            records.push(json!({"vad":gated,"phase":run.phase,"events":run.events.len(),"final_text":run.text}));
        }
        result["control"] = json!({"source":control,"acoustics":acoustic(wave.samples()),"native_vad":detector(wave.samples(),control_rate,&vad)?,"runs":records,"reference_scoring":"No control transcript reference was supplied; no WER invented."});
    }
    for gain in &config.gains {
        eprintln!("Explicit corpus-only diagnostic gain={gain}");
        let clipped = samples
            .iter()
            .filter(|s| s.abs() * f32::from(*gain) > 1.0)
            .count();
        let amplified = samples
            .iter()
            .map(|s| (s * f32::from(*gain)).clamp(-1.0, 1.0))
            .collect::<Vec<_>>();
        let live = stages::live(
            &amplified,
            rate,
            streaming,
            Some(&vad),
            false,
            &mut telemetry,
        )?;
        save_text(&out, &format!("gain{gain}-live"), &live)?;
        let batch = stages::batch(&amplified, rate, refinement, &mut telemetry)?;
        save_text(&out, &format!("gain{gain}-batch"), &batch)?;
        let diar = stages::diarize(
            &amplified,
            rate,
            diarization,
            vt_diarize::Phase::Refined,
            false,
            &mut telemetry,
        )?;
        let hypothesis = rttm(id, &diar.turns);
        fs::write(out.join(format!("gain{gain}-refined.rttm")), &hypothesis).map_err(error)?;
        json_file(
            &out.join(format!("gain{gain}-refined-turns.json")),
            &diar.result,
        )?;
        let turns = crate::diarization::parse_hypothesis_rttm(&hypothesis, id, "1")?;
        result["gains"].as_array_mut().ok_or("invalid diagnostic array")?.push(json!({"gain":gain,"clipped_samples":clipped,"samples":samples.len(),"acoustics":acoustic(&amplified),"native_vad":detector(&amplified,rate,&vad)?,"live_wer":crate::words::score(&reference,&live.text,normalize)?,"batch_wer":crate::words::score(&reference,&batch.text,normalize)?,"refined_diarization":crate::diarization::score(reference_turns,&turns,regions)?,"phases":[live.phase,batch.phase,diar.phase]}));
    }
    json_file(&out.join("observations.json"), &telemetry.observations())?;
    json_file(&out.join("results.json"), &result)?;
    Ok(result)
}
