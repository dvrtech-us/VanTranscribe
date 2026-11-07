use crate::{
    Result, SttError,
    refinement_models::{DECODER, ENCODER, JOINER, TOKENS, VAD, verify_refinement_manifest},
};
use sherpa_onnx::{
    LinearResampler, OfflineRecognizer, OfflineRecognizerConfig, OfflineRecognizerResult,
    SileroVadModelConfig, VadModelConfig, VoiceActivityDetector,
};
use std::path::Path;
use vt_core::TrackKind;

const RATE: u32 = 16_000;
// Upstream max_speech_duration adjusts VAD thresholds; it is not a hard cap.
// Explicit epochs therefore cap even uninterrupted speech and native counters.
const EPOCH_FRAMES: usize = 29 * RATE as usize;
const WINDOW: usize = 512;
const TAIL: usize = 3200;
const TOKEN_FRAME_MS: u64 = 80;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WordTiming {
    pub text: String,
    pub start_ms: u64,
    pub end_ms: u64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefinedSegment {
    pub track: TrackKind,
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
    pub words: Vec<WordTiming>,
}
struct TrackState {
    vad: VoiceActivityDetector,
    resampler: Option<LinearResampler>,
    rate: Option<u32>,
    origin: Option<u64>,
    input_frames: u64,
    last_end: u64,
    output_frames: u64,
    epoch_start: u64,
    epoch_frames: usize,
}
impl TrackState {
    fn new(config: &VadModelConfig) -> Result<Self> {
        Ok(Self {
            vad: VoiceActivityDetector::create(config, 32.0).ok_or(SttError::Engine)?,
            resampler: None,
            rate: None,
            origin: None,
            input_frames: 0,
            last_end: 0,
            output_frames: 0,
            epoch_start: 0,
            epoch_frames: 0,
        })
    }
}
/// Offline Parakeet refinement over bounded Silero VAD utterances. Call only on
/// a worker. No network access occurs here. Track clocks and resamplers persist
/// across chunks; explicit gaps flush the old track before starting a new clock.
pub struct BatchRefiner {
    tracks: [TrackState; 2],
    recognizer: OfflineRecognizer,
    vad_config: VadModelConfig,
    finished: bool,
}
impl BatchRefiner {
    pub fn new(model_dir: &Path) -> Result<Self> {
        verify_refinement_manifest(model_dir, true)?;
        let path = |name: &str| {
            model_dir
                .join(name)
                .to_str()
                .map(str::to_owned)
                .ok_or_else(|| SttError::InvalidModel("model path is not UTF-8".into()))
        };
        let mut config = OfflineRecognizerConfig::default();
        config.model_config.transducer.encoder = Some(path(ENCODER)?);
        config.model_config.transducer.decoder = Some(path(DECODER)?);
        config.model_config.transducer.joiner = Some(path(JOINER)?);
        config.model_config.tokens = Some(path(TOKENS)?);
        config.model_config.model_type = Some("nemo_transducer".into());
        config.model_config.provider = Some("cpu".into());
        config.model_config.num_threads = 2;
        config.decoding_method = Some("greedy_search".into());
        let recognizer = OfflineRecognizer::create(&config).ok_or(SttError::Engine)?;
        let vad_config = VadModelConfig {
            silero_vad: SileroVadModelConfig {
                model: Some(path(VAD)?),
                threshold: 0.5,
                min_silence_duration: 0.35,
                min_speech_duration: 0.2,
                window_size: WINDOW as i32,
                max_speech_duration: 28.0,
            },
            sample_rate: RATE as i32,
            num_threads: 1,
            provider: Some("cpu".into()),
            ..Default::default()
        };
        let tracks = [TrackState::new(&vad_config)?, TrackState::new(&vad_config)?];
        Ok(Self {
            tracks,
            recognizer,
            vad_config,
            finished: false,
        })
    }
    pub fn accept(
        &mut self,
        track: TrackKind,
        mono: &[f32],
        sample_rate: u32,
        start_ms: u64,
    ) -> Result<Vec<RefinedSegment>> {
        self.check_active()?;
        crate::recognize::validate_input(mono, sample_rate, start_ms)?;
        if mono.is_empty() {
            return Ok(vec![]);
        }
        let state = &mut self.tracks[index(track)];
        let mut results = vec![];
        if state.rate.is_some_and(|r| r != sample_rate)
            || state.origin.is_some() && state.last_end.abs_diff(start_ms) > 5
        {
            results.extend(finalize(&self.recognizer, state, track)?);
            *state = TrackState::new(&self.vad_config)?;
        }
        if state.origin.is_none() {
            state.origin = Some(start_ms);
            state.rate = Some(sample_rate);
            if sample_rate != RATE {
                state.resampler = Some(
                    LinearResampler::create(sample_rate as i32, RATE as i32)
                        .ok_or(SttError::Engine)?,
                );
            }
        }
        state.input_frames = state
            .input_frames
            .checked_add(mono.len() as u64)
            .ok_or_else(|| SttError::InvalidAudio("frame counter overflow".into()))?;
        state.last_end = state
            .origin
            .unwrap()
            .checked_add(
                state
                    .input_frames
                    .checked_mul(1000)
                    .ok_or_else(|| SttError::InvalidAudio("duration overflow".into()))?
                    / u64::from(sample_rate),
            )
            .ok_or_else(|| SttError::InvalidAudio("timestamp overflow".into()))?;
        if let Some(resampler) = &state.resampler {
            let samples = resampler.resample(mono, false);
            results.extend(feed(&self.recognizer, state, track, &samples)?);
        } else {
            results.extend(feed(&self.recognizer, state, track, mono)?);
        }
        Ok(results)
    }
    pub fn reset_track(&mut self, track: TrackKind) -> Result<Vec<RefinedSegment>> {
        self.check_active()?;
        let state = &mut self.tracks[index(track)];
        let results = finalize(&self.recognizer, state, track)?;
        *state = TrackState::new(&self.vad_config)?;
        Ok(results)
    }
    pub fn finish(&mut self) -> Result<Vec<RefinedSegment>> {
        if self.finished {
            return Ok(vec![]);
        }
        let mut results = vec![];
        for track in [TrackKind::Mic, TrackKind::System] {
            results.extend(finalize(
                &self.recognizer,
                &mut self.tracks[index(track)],
                track,
            )?);
        }
        self.finished = true;
        Ok(results)
    }
    fn check_active(&self) -> Result<()> {
        if self.finished {
            Err(SttError::InvalidAudio(
                "refiner has already finished".into(),
            ))
        } else {
            Ok(())
        }
    }
}
fn index(track: TrackKind) -> usize {
    if track == TrackKind::Mic { 0 } else { 1 }
}
fn feed(
    recognizer: &OfflineRecognizer,
    state: &mut TrackState,
    track: TrackKind,
    mut samples: &[f32],
) -> Result<Vec<RefinedSegment>> {
    let mut output = vec![];
    while !samples.is_empty() {
        let n = samples
            .len()
            .min(WINDOW)
            .min(EPOCH_FRAMES - state.epoch_frames);
        state.vad.accept_waveform(&samples[..n]);
        state.output_frames += n as u64;
        state.epoch_frames += n;
        samples = &samples[n..];
        output.extend(drain(recognizer, state, track)?);
        if state.epoch_frames == EPOCH_FRAMES {
            state.vad.flush();
            output.extend(drain(recognizer, state, track)?);
            state.vad.reset();
            state.epoch_start = state.output_frames;
            state.epoch_frames = 0;
        }
    }
    Ok(output)
}
fn finalize(
    recognizer: &OfflineRecognizer,
    state: &mut TrackState,
    track: TrackKind,
) -> Result<Vec<RefinedSegment>> {
    if state.origin.is_none() {
        return Ok(vec![]);
    }
    let mut results = vec![];
    if let Some(resampler) = &state.resampler {
        let tail = resampler.resample(&[], true);
        results.extend(feed(recognizer, state, track, &tail)?);
    }
    state.vad.flush();
    results.extend(drain(recognizer, state, track)?);
    Ok(results)
}
fn drain(
    recognizer: &OfflineRecognizer,
    state: &mut TrackState,
    track: TrackKind,
) -> Result<Vec<RefinedSegment>> {
    let mut output = vec![];
    while let Some(speech) = state.vad.front() {
        if speech.start() < 0
            || speech.samples().len() > EPOCH_FRAMES
            || speech.start() as u64 + speech.samples().len() as u64 > state.epoch_frames as u64
        {
            return Err(SttError::InvalidAudio(
                "VAD returned an invalid region".into(),
            ));
        }
        let start_frame = state.epoch_start + speech.start() as u64;
        let start_ms = state
            .origin
            .unwrap()
            .saturating_add(start_frame * 1000 / u64::from(RATE))
            .min(state.last_end);
        let end_ms = state
            .origin
            .unwrap()
            .saturating_add((start_frame + speech.samples().len() as u64) * 1000 / u64::from(RATE))
            .min(state.last_end);
        if end_ms > start_ms && !speech.samples().is_empty() {
            let stream = recognizer.create_stream();
            stream.accept_waveform(RATE as i32, speech.samples());
            stream.accept_waveform(RATE as i32, &[0.0; TAIL]);
            recognizer.decode(&stream);
            let result = stream.get_result().ok_or(SttError::Engine)?;
            if !result.text.trim().is_empty() {
                let words = merge_words(&result, start_ms, end_ms)?;
                output.push(RefinedSegment {
                    track,
                    start_ms,
                    end_ms,
                    text: result.text.trim().to_owned(),
                    words,
                });
            }
        }
        drop(speech);
        state.vad.pop();
    }
    Ok(output)
}
/// Merge the model's space-prefixed BPE pieces. End times use native durations
/// where supplied, otherwise the final token's 80ms encoder-frame extent. These
/// are acoustic alignment estimates, not forced-alignment phoneme boundaries.
fn merge_words(result: &OfflineRecognizerResult, start: u64, end: u64) -> Result<Vec<WordTiming>> {
    merge_token_words(
        result,
        start,
        end,
        (TOKEN_FRAME_MS, TAIL as u64 * 1000 / u64::from(RATE)),
    )
}
pub(crate) fn merge_token_words(
    result: &OfflineRecognizerResult,
    start: u64,
    end: u64,
    timing: (u64, u64),
) -> Result<Vec<WordTiming>> {
    let (token_frame_ms, padding_ms) = timing;
    let times = result
        .timestamps
        .as_ref()
        .filter(|t| t.len() == result.tokens.len())
        .ok_or_else(|| SttError::InvalidAudio("refinement returned no token alignment".into()))?;
    if times.iter().any(|t| !t.is_finite() || *t < 0.0) || times.windows(2).any(|t| t[1] < t[0]) {
        return Err(SttError::InvalidAudio(
            "refinement returned invalid token alignment".into(),
        ));
    }
    let context_ms = end.saturating_sub(start).saturating_add(padding_ms);
    if times
        .iter()
        .any(|t| f64::from(*t) * 1000.0 > context_ms as f64 + token_frame_ms as f64)
    {
        return Err(SttError::InvalidAudio(
            "token alignment exceeds decoder context".into(),
        ));
    }
    if result.durations.as_ref().is_some_and(|d| {
        !d.is_empty() && (d.len() != times.len() || d.iter().any(|v| !v.is_finite() || *v < 0.0))
    }) {
        return Err(SttError::InvalidAudio(
            "invalid token duration alignment".into(),
        ));
    }
    let mut words: Vec<WordTiming> = vec![];
    let mut boundary = true;
    for (i, token) in result.tokens.iter().enumerate() {
        let token = token.replace(['▁', 'Ġ'], " ");
        let trimmed = token.trim();
        if trimmed.is_empty() {
            boundary = true;
            continue;
        }
        let token_start = start
            .saturating_add((times[i] as f64 * 1000.0).round() as u64)
            .min(end);
        let duration = result
            .durations
            .as_ref()
            .and_then(|d| d.get(i))
            .copied()
            .filter(|d| d.is_finite() && *d > 0.0)
            .map(|d| (d as f64 * 1000.0).round() as u64)
            .unwrap_or(token_frame_ms);
        let token_end = token_start.saturating_add(duration).min(end);
        if boundary || token.starts_with(char::is_whitespace) || words.is_empty() {
            words.push(WordTiming {
                text: trimmed.into(),
                start_ms: token_start,
                end_ms: token_end,
            });
        } else {
            let word = words.last_mut().unwrap();
            word.text.push_str(trimmed);
            word.end_ms = word.end_ms.max(token_end);
        }
        boundary = false;
    }
    for i in 0..words.len().saturating_sub(1) {
        words[i].end_ms = words[i]
            .end_ms
            .min(words[i + 1].start_ms)
            .max(words[i].start_ms);
    }
    if words.is_empty() && !result.text.trim().is_empty() {
        return Err(SttError::InvalidAudio(
            "refinement returned text without aligned tokens".into(),
        ));
    }
    Ok(words)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn token_alignment_merges_subwords_and_attaches_punctuation() {
        let r = OfflineRecognizerResult {
            text: "Hello, world!".into(),
            tokens: vec![
                " He".into(),
                "llo".into(),
                ",".into(),
                " world".into(),
                "!".into(),
            ],
            timestamps: Some(vec![0.1, 0.18, 0.18, 0.7, 0.78]),
            durations: None,
        };
        let words = merge_words(&r, 1000, 1900).unwrap();
        assert_eq!(
            words[0],
            WordTiming {
                text: "Hello,".into(),
                start_ms: 1100,
                end_ms: 1260
            }
        );
        assert_eq!(
            words[1],
            WordTiming {
                text: "world!".into(),
                start_ms: 1700,
                end_ms: 1860
            }
        );
    }
    #[test]
    fn token_alignment_never_invents_uniform_word_times() {
        let mut r = OfflineRecognizerResult {
            text: "hi".into(),
            tokens: vec![" hi".into()],
            timestamps: None,
            durations: None,
        };
        assert!(merge_words(&r, 0, 1000).is_err());
        r.timestamps = Some(vec![f32::NAN]);
        assert!(merge_words(&r, 0, 1000).is_err());
        r.timestamps = Some(vec![0.99]);
        let w = merge_words(&r, 1000, 2000).unwrap();
        assert_eq!(w[0].end_ms, 2000);
        r.timestamps = Some(vec![9.0]);
        assert!(merge_words(&r, 1000, 2000).is_err());
        r.timestamps = Some(vec![0.99]);
        r.durations = Some(vec![f32::NAN]);
        assert!(merge_words(&r, 1000, 2000).is_err());
    }
    #[test]
    #[ignore = "requires VT_REFINEMENT_MODEL_DIR and VT_STT_FIXTURE_WAV; never downloads"]
    fn real_parakeet_two_tracks_gaps_timestamps_and_bounded_epochs() {
        let model = std::env::var_os("VT_REFINEMENT_MODEL_DIR").unwrap();
        let fixture = std::env::var("VT_STT_FIXTURE_WAV").unwrap();
        let wave = sherpa_onnx::Wave::read(&fixture).unwrap();
        let rate = wave.sample_rate() as u32;
        let mut engine = BatchRefiner::new(Path::new(&model)).unwrap();
        let mut results = vec![];
        let high_rate = LinearResampler::create(rate as i32, 48_000)
            .unwrap()
            .resample(wave.samples(), true);
        for (track, origin, samples, input_rate) in [
            (TrackKind::Mic, 1000, wave.samples(), rate),
            (TrackKind::System, 3000, high_rate.as_slice(), 48_000),
        ] {
            for (i, packet) in samples.chunks((input_rate / 10) as usize).enumerate() {
                results.extend(
                    engine
                        .accept(track, packet, input_rate, origin + i as u64 * 100)
                        .unwrap(),
                );
            }
            results.extend(engine.reset_track(track).unwrap());
        }
        // A distant second utterance must never inherit the first track clock.
        for (i, packet) in wave.samples().chunks((rate / 10) as usize).enumerate() {
            results.extend(
                engine
                    .accept(TrackKind::Mic, packet, rate, 60000 + i as u64 * 100)
                    .unwrap(),
            );
        }
        results.extend(engine.finish().unwrap());
        assert!(engine.finish().unwrap().is_empty());
        for (track, origin) in [
            (TrackKind::Mic, 1000),
            (TrackKind::System, 3000),
            (TrackKind::Mic, 60000),
        ] {
            let events: Vec<_> = results
                .iter()
                .filter(|e| e.track == track && e.start_ms >= origin && e.start_ms < origin + 20000)
                .collect();
            let text = events
                .iter()
                .map(|e| e.text.as_str())
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase();
            assert!(text.contains("yellow lamps"), "unexpected Parakeet: {text}");
            assert!(events.iter().all(|e| e.end_ms
                <= origin + wave.samples().len() as u64 * 1000 / u64::from(rate)
                && !e.words.is_empty()
                && e.words.iter().all(|w| w.start_ms >= e.start_ms
                    && w.end_ms <= e.end_ms
                    && w.end_ms >= w.start_ms)));
        }
        assert!(engine.accept(TrackKind::Mic, &[0.0], rate, 0).is_err());
        drop(engine);
        // Force several VAD epochs using silence without retaining whole tracks.
        let mut silence = BatchRefiner::new(Path::new(&model)).unwrap();
        for n in 0..65 {
            assert!(
                silence
                    .accept(TrackKind::System, &[0.0; 16000], 16000, n * 1000)
                    .unwrap()
                    .is_empty()
            );
            assert!(silence.tracks[1].epoch_frames < EPOCH_FRAMES);
        }
        assert!(silence.finish().unwrap().is_empty());
    }
}
