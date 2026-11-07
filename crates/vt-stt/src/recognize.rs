use crate::{
    LiveVadOptions, Result, SttError, WordTiming,
    live_vad::{GateAction, LiveVad, WINDOW},
    models::verify_manifest,
};
use sherpa_onnx::{
    LinearResampler, OfflineRecognizerResult, OnlineRecognizer, OnlineRecognizerConfig,
    OnlineStream,
};
use std::path::Path;
use vt_core::TrackKind;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TranscriptEvent {
    pub track: TrackKind,
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
    pub is_final: bool,
    pub words: Vec<WordTiming>,
}
struct TrackState {
    stream: OnlineStream,
    resampler: Option<LinearResampler>,
    sample_rate: Option<u32>,
    origin_ms: Option<u64>,
    frames_received: u64,
    last_end_ms: u64,
    previous_text: String,
    stream_origin_ms: Option<u64>,
    stream_end_ms: u64,
    vad: Option<LiveVad>,
}
impl TrackState {
    fn new(
        recognizer: &OnlineRecognizer,
        vad: Option<&sherpa_onnx::VadModelConfig>,
    ) -> Result<Self> {
        Ok(Self {
            stream: recognizer.create_stream(),
            resampler: None,
            sample_rate: None,
            origin_ms: None,
            frames_received: 0,
            last_end_ms: 0,
            previous_text: String::new(),
            stream_origin_ms: None,
            stream_end_ms: 0,
            vad: vad.map(LiveVad::new).transpose()?,
        })
    }
    fn reset_stream(&mut self, recognizer: &OnlineRecognizer) {
        self.stream = recognizer.create_stream();
        self.stream_origin_ms = None;
        self.stream_end_ms = 0;
        self.previous_text.clear();
    }
    fn reset(&mut self, recognizer: &OnlineRecognizer) {
        self.reset_stream(recognizer);
        self.resampler = None;
        self.sample_rate = None;
        self.origin_ms = None;
        self.frames_received = 0;
        self.last_end_ms = 0;
        if let Some(vad) = &mut self.vad {
            vad.reset();
        }
    }
}
/// Independent per-track streaming clocks, resamplers and optional live VAD.
/// Feed 20–100ms mono packets on a worker. Only inference is gated; callers must
/// preserve native raw recording separately. Legacy `new` does not use VAD.
pub struct StreamingRecognizer {
    tracks: [TrackState; 2],
    recognizer: OnlineRecognizer,
    finished: bool,
}
impl StreamingRecognizer {
    pub fn new(model_dir: &Path) -> Result<Self> {
        Self::with_vad(model_dir, None)
    }
    /// The optional VAD may be its standalone model directory or the exact
    /// pinned `silero_vad.onnx` file (including one from the refinement pack).
    /// An invalid requested VAD returns an error rather than silently disabling it.
    pub fn with_vad(model_dir: &Path, vad_path: Option<&Path>) -> Result<Self> {
        match vad_path {
            Some(path) => Self::with_vad_options(model_dir, path, LiveVadOptions::default()),
            None => Self::build(model_dir, None, LiveVadOptions::default()),
        }
    }
    /// Explicit threshold tuning requires a verified VAD file or model directory.
    /// Options are validated before model I/O or native initialization.
    pub fn with_vad_options(
        model_dir: &Path,
        vad_path: &Path,
        options: LiveVadOptions,
    ) -> Result<Self> {
        options.validate()?;
        Self::build(model_dir, Some(vad_path), options)
    }
    fn build(model_dir: &Path, vad_path: Option<&Path>, options: LiveVadOptions) -> Result<Self> {
        let model = verify_manifest(model_dir, true)?;
        let [encoder, decoder, joiner, tokens] = model.files();
        let vad = vad_path
            .map(|path| LiveVad::config(path, options))
            .transpose()?;
        let path = |name: &str| {
            model_dir
                .join(name)
                .to_str()
                .map(str::to_owned)
                .ok_or_else(|| SttError::InvalidModel("model path is not valid UTF-8".into()))
        };
        let mut config = OnlineRecognizerConfig::default();
        config.model_config.transducer.encoder = Some(path(encoder.name)?);
        config.model_config.transducer.decoder = Some(path(decoder.name)?);
        config.model_config.transducer.joiner = Some(path(joiner.name)?);
        config.model_config.tokens = Some(path(tokens.name)?);
        config.model_config.num_threads = 2;
        config.model_config.provider = Some("cpu".into());
        config.model_config.debug = false;
        config.enable_endpoint = true;
        config.rule1_min_trailing_silence = 2.4;
        config.rule2_min_trailing_silence = 0.8;
        config.rule3_min_utterance_length = 15.0;
        config.decoding_method = Some("greedy_search".into());
        config.max_active_paths = 4;
        let recognizer = OnlineRecognizer::create(&config).ok_or(SttError::Engine)?;
        let tracks = [
            TrackState::new(&recognizer, vad.as_ref())?,
            TrackState::new(&recognizer, vad.as_ref())?,
        ];
        Ok(Self {
            tracks,
            recognizer,
            finished: false,
        })
    }
    pub fn accept(
        &mut self,
        track: TrackKind,
        samples: &[f32],
        sample_rate: u32,
        start_ms: u64,
    ) -> Result<Vec<TranscriptEvent>> {
        self.check_active()?;
        validate_input(samples, sample_rate, start_ms)?;
        if samples.is_empty() {
            return Ok(vec![]);
        }
        let state = &mut self.tracks[index(track)];
        let mut events = vec![];
        if state.sample_rate.is_some_and(|rate| rate != sample_rate)
            || state.origin_ms.is_some() && state.last_end_ms.abs_diff(start_ms) > 5
        {
            events.extend(finalize(&self.recognizer, state, track));
            state.reset(&self.recognizer);
        }
        if state.origin_ms.is_none() {
            state.origin_ms = Some(start_ms);
            state.sample_rate = Some(sample_rate);
            if sample_rate != 16000 {
                state.resampler = Some(
                    LinearResampler::create(sample_rate as i32, 16000).ok_or(SttError::Engine)?,
                );
            }
        }
        let frames = state
            .frames_received
            .checked_add(samples.len() as u64)
            .ok_or_else(|| SttError::InvalidAudio("frame counter overflow".into()))?;
        let elapsed = frames
            .checked_mul(1000)
            .ok_or_else(|| SttError::InvalidAudio("duration overflow".into()))?
            / u64::from(sample_rate);
        let end = state
            .origin_ms
            .unwrap()
            .checked_add(elapsed)
            .ok_or_else(|| SttError::InvalidAudio("timestamp range overflow".into()))?;
        state.frames_received = frames;
        state.last_end_ms = end;
        if let Some(resampler) = &state.resampler {
            let mono = resampler.resample(samples, false);
            events.extend(feed(&self.recognizer, state, track, &mono));
        } else {
            events.extend(feed(&self.recognizer, state, track, samples));
        }
        Ok(events)
    }
    pub fn reset_track(&mut self, track: TrackKind) -> Result<Vec<TranscriptEvent>> {
        self.check_active()?;
        let state = &mut self.tracks[index(track)];
        let events = finalize(&self.recognizer, state, track);
        state.reset(&self.recognizer);
        Ok(events)
    }
    pub fn finish(&mut self) -> Result<Vec<TranscriptEvent>> {
        if self.finished {
            return Ok(vec![]);
        }
        let mut events = vec![];
        for track in [TrackKind::Mic, TrackKind::System] {
            events.extend(finalize(
                &self.recognizer,
                &mut self.tracks[index(track)],
                track,
            ));
        }
        self.finished = true;
        Ok(events)
    }
    fn check_active(&self) -> Result<()> {
        if self.finished {
            Err(SttError::InvalidAudio(
                "recognizer has already finished".into(),
            ))
        } else {
            Ok(())
        }
    }
}
fn index(track: TrackKind) -> usize {
    if track == TrackKind::Mic { 0 } else { 1 }
}
pub(crate) fn validate_input(samples: &[f32], rate: u32, start_ms: u64) -> Result<()> {
    if !(8000..=192000).contains(&rate) {
        return Err(SttError::InvalidAudio(
            "sample rate must be 8000..192000 Hz".into(),
        ));
    }
    if samples.len() > rate as usize * 2 || samples.iter().any(|s| !s.is_finite()) {
        return Err(SttError::InvalidAudio(
            "input must contain at most two seconds of finite mono samples".into(),
        ));
    }
    start_ms
        .checked_add(samples.len() as u64 * 1000 / u64::from(rate))
        .ok_or_else(|| SttError::InvalidAudio("timestamp range overflow".into()))?;
    Ok(())
}
fn time_at(state: &TrackState, frame: u64) -> u64 {
    state
        .origin_ms
        .unwrap_or(0)
        .saturating_add(frame.saturating_mul(1000) / 16000)
        .min(state.last_end_ms)
}
fn feed(
    recognizer: &OnlineRecognizer,
    state: &mut TrackState,
    track: TrackKind,
    mut samples: &[f32],
) -> Vec<TranscriptEvent> {
    let Some(mut vad) = state.vad.take() else {
        let origin = state.origin_ms.unwrap_or(0);
        return feed_online(recognizer, state, track, samples, origin, state.last_end_ms);
    };
    let mut events = vec![];
    while !samples.is_empty() {
        let n = samples.len().min(WINDOW - vad.pending_len);
        vad.pending[vad.pending_len..vad.pending_len + n].copy_from_slice(&samples[..n]);
        vad.pending_len += n;
        samples = &samples[n..];
        if vad.pending_len < WINDOW {
            continue;
        }
        let window = vad.pending;
        vad.pending_len = 0;
        let frame = vad.processed_frames;
        vad.processed_frames += WINDOW as u64;
        match vad.classify(&window) {
            GateAction::Skip => {}
            GateAction::Open(pre_roll) => {
                let start = time_at(state, vad.processed_frames - pre_roll.len() as u64);
                let end = time_at(state, vad.processed_frames);
                events.extend(feed_online(recognizer, state, track, &pre_roll, start, end));
            }
            GateAction::Feed { close } => {
                events.extend(feed_online(
                    recognizer,
                    state,
                    track,
                    &window,
                    time_at(state, frame),
                    time_at(state, vad.processed_frames),
                ));
                if close {
                    events.extend(flush_online(recognizer, state, track));
                    state.reset_stream(recognizer);
                }
            }
        }
    }
    state.vad = Some(vad);
    events
}
fn feed_online(
    recognizer: &OnlineRecognizer,
    state: &mut TrackState,
    track: TrackKind,
    samples: &[f32],
    start: u64,
    end: u64,
) -> Vec<TranscriptEvent> {
    if samples.is_empty() {
        return vec![];
    }
    if state.stream_origin_ms.is_none() {
        state.stream_origin_ms = Some(start)
    }
    state.stream_end_ms = end;
    state.stream.accept_waveform(16000, samples);
    let mut events = vec![];
    while recognizer.is_ready(&state.stream) {
        recognizer.decode(&state.stream);
        if recognizer.is_endpoint(&state.stream) {
            if let Some(event) = result_event(recognizer, state, track, true) {
                events.push(event)
            }
            recognizer.reset(&state.stream);
            state.previous_text.clear();
        }
    }
    if let Some(event) = result_event(recognizer, state, track, false) {
        events.push(event)
    }
    events
}
fn finalize(
    recognizer: &OnlineRecognizer,
    state: &mut TrackState,
    track: TrackKind,
) -> Vec<TranscriptEvent> {
    if state.origin_ms.is_none() {
        return vec![];
    }
    let mut events = vec![];
    if let Some(resampler) = &state.resampler {
        let tail = resampler.resample(&[], true);
        events.extend(feed(recognizer, state, track, &tail));
    }
    if let Some(mut vad) = state.vad.take() {
        if vad.gate.open && vad.pending_len > 0 {
            let start = time_at(state, vad.processed_frames);
            let end = time_at(state, vad.processed_frames + vad.pending_len as u64);
            events.extend(feed_online(
                recognizer,
                state,
                track,
                &vad.pending[..vad.pending_len],
                start,
                end,
            ));
            vad.processed_frames += vad.pending_len as u64;
        }
        vad.pending_len = 0;
        state.vad = Some(vad);
    }
    events.extend(flush_online(recognizer, state, track));
    events
}
fn flush_online(
    recognizer: &OnlineRecognizer,
    state: &mut TrackState,
    track: TrackKind,
) -> Vec<TranscriptEvent> {
    if state.stream_origin_ms.is_none() {
        return vec![];
    }
    state.stream.accept_waveform(16000, &[0.0; 4800]);
    state.stream.input_finished();
    while recognizer.is_ready(&state.stream) {
        recognizer.decode(&state.stream)
    }
    result_event(recognizer, state, track, true)
        .into_iter()
        .collect()
}
fn result_event(
    recognizer: &OnlineRecognizer,
    state: &mut TrackState,
    track: TrackKind,
    is_final: bool,
) -> Option<TranscriptEvent> {
    let result = recognizer.get_result(&state.stream)?;
    let text = result.text.trim().to_owned();
    if text.is_empty() || !is_final && text == state.previous_text {
        return None;
    }
    state.previous_text = text.clone();
    let origin = state.stream_origin_ms?;
    let offset = (f64::from(result.start_time.unwrap_or(0.0)).max(0.0) * 1000.0) as u64;
    let end_ms = state.stream_end_ms.min(state.last_end_ms);
    let start_ms = origin.saturating_add(offset).min(end_ms);
    let alignment = OfflineRecognizerResult {
        text: text.clone(),
        tokens: result.tokens,
        timestamps: result.timestamps,
        durations: None,
    };
    let words = crate::refine::merge_token_words(&alignment, start_ms, end_ms, (40, 300))
        .unwrap_or_default();
    Some(TranscriptEvent {
        track,
        start_ms,
        end_ms,
        text,
        is_final,
        words,
    })
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_validation_prevents_native_fatal_errors() {
        assert!(validate_input(&[f32::NAN], 16_000, 0).is_err());
        assert!(validate_input(&[0.0], 0, 0).is_err());
        assert!(validate_input(&[0.0; 16_001], 8_000, 0).is_err());
        assert!(validate_input(&[0.0; 16], 16_000, u64::MAX).is_err());
        assert!(validate_input(&[0.0; 160], 16_000, 10).is_ok());
    }

    #[test]
    fn sinc_resampling_is_equivalent_across_chunk_boundaries() {
        let signal: Vec<_> = (0..48_000)
            .map(|n| (n as f32 * std::f32::consts::TAU * 440.0 / 48_000.0).sin())
            .collect();
        let whole = LinearResampler::create(48_000, 16_000)
            .unwrap()
            .resample(&signal, true);
        let streaming = LinearResampler::create(48_000, 16_000).unwrap();
        let mut chunked = Vec::new();
        for packet in signal.chunks(997) {
            chunked.extend(streaming.resample(packet, false));
        }
        chunked.extend(streaming.resample(&[], true));
        assert_eq!(whole.len(), 16_000);
        assert_eq!(whole.len(), chunked.len());
        let max_error = whole
            .iter()
            .zip(&chunked)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0_f32, f32::max);
        assert!(
            max_error < 0.000_01,
            "streaming resampler boundary error: {max_error}"
        );
    }

    #[test]
    fn resampling_rejects_above_nyquist_energy() {
        let rms = |frequency: f32| {
            let signal: Vec<_> = (0..48_000)
                .map(|n| (n as f32 * std::f32::consts::TAU * frequency / 48_000.0).sin())
                .collect();
            let output = LinearResampler::create(48_000, 16_000)
                .unwrap()
                .resample(&signal, true);
            (output[100..15_900].iter().map(|s| s * s).sum::<f32>() / 15_800.0).sqrt()
        };
        assert!(rms(440.0) > 0.65);
        assert!(rms(12_000.0) < 0.05);
    }

    #[test]
    #[ignore = "requires VT_STT_MODEL_DIR and VT_STT_FIXTURE_WAV; never downloads models"]
    fn real_model_keeps_two_tracks_and_session_offsets_independent() {
        let model = std::env::var_os("VT_STT_MODEL_DIR")
            .expect("set VT_STT_MODEL_DIR to an installed model directory");
        let fixture = std::env::var("VT_STT_FIXTURE_WAV")
            .expect("set VT_STT_FIXTURE_WAV to upstream test_wavs/0.wav");
        let wave = sherpa_onnx::Wave::read(&fixture).expect("read fixture");
        let rate = wave.sample_rate() as u32;
        let mut recognizer = StreamingRecognizer::new(Path::new(&model)).unwrap();
        let mut events = Vec::new();
        let mut frame = 0_u64;
        for packet in wave.samples().chunks((rate / 10) as usize) {
            for (track, origin) in [(TrackKind::Mic, 1_000), (TrackKind::System, 3_000)] {
                events.extend(
                    recognizer
                        .accept(
                            track,
                            packet,
                            rate,
                            origin + frame * 1_000 / u64::from(rate),
                        )
                        .unwrap(),
                );
            }
            frame += packet.len() as u64;
        }
        events.extend(recognizer.finish().unwrap());
        for (track, origin) in [(TrackKind::Mic, 1_000), (TrackKind::System, 3_000)] {
            let final_events: Vec<_> = events
                .iter()
                .filter(|e| e.track == track && e.is_final)
                .collect();
            let text = final_events
                .iter()
                .map(|e| e.text.as_str())
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase();
            assert!(
                text.contains("yellow lamps"),
                "unexpected real transcription: {text}"
            );
            assert!(
                final_events.iter().all(|e| e.start_ms >= origin
                    && e.end_ms <= origin + frame * 1_000 / u64::from(rate))
            );
        }
    }
}

#[cfg(test)]
mod vad_tests {
    use super::*;
    #[test]
    fn malformed_threshold_options_fail_before_any_model_or_native_initialization() {
        for threshold in [f32::NAN, f32::INFINITY, -0.1, 0.099, 0.901, 2.0] {
            assert!(matches!(
                StreamingRecognizer::with_vad_options(
                    Path::new("missing-live-model"),
                    Path::new("missing-pinned-vad"),
                    LiveVadOptions { threshold }
                ),
                Err(SttError::InvalidVadOptions(_))
            ));
        }
    }
    fn push(
        engine: &mut StreamingRecognizer,
        track: TrackKind,
        samples: &[f32],
        rate: u32,
        origin: u64,
        packet_ms: u32,
    ) -> Vec<TranscriptEvent> {
        let mut frame = 0_u64;
        let mut events = vec![];
        for packet in samples.chunks((rate * packet_ms / 1000) as usize) {
            events.extend(
                engine
                    .accept(track, packet, rate, origin + frame * 1000 / u64::from(rate))
                    .unwrap(),
            );
            frame += packet.len() as u64;
            let vad = engine.tracks[index(track)].vad.as_ref().unwrap();
            assert!(vad.buffered_samples() <= crate::live_vad::PRE_ROLL + WINDOW);
            assert!(vad.pending_len < WINDOW);
            assert!(vad.detector_frames() <= 16 * 16000);
        }
        events
    }
    #[test]
    #[ignore = "requires VT_STT_MODEL_DIR, VT_VAD_MODEL_FILE and VT_STT_FIXTURE_WAV; no downloads/devices"]
    fn real_threshold_options_preserve_legacy_default_onsets_tracks_gaps_and_word_bounds() {
        let model = std::env::var_os("VT_STT_MODEL_DIR").unwrap();
        let vad = std::env::var_os("VT_VAD_MODEL_FILE").unwrap();
        let fixture = std::env::var("VT_STT_FIXTURE_WAV").unwrap();
        let wave = sherpa_onnx::Wave::read(&fixture).unwrap();
        let rate = wave.sample_rate() as u32;
        let run = |options: Option<LiveVadOptions>| {
            let mut engine = match options {
                None => {
                    StreamingRecognizer::with_vad(Path::new(&model), Some(Path::new(&vad))).unwrap()
                }
                Some(options) => StreamingRecognizer::with_vad_options(
                    Path::new(&model),
                    Path::new(&vad),
                    options,
                )
                .unwrap(),
            };
            let mut events = Vec::new();
            for (track, origin) in [(TrackKind::Mic, 1000), (TrackKind::System, 3000)] {
                assert!(
                    push(
                        &mut engine,
                        track,
                        &vec![0.0; rate as usize],
                        rate,
                        origin,
                        20
                    )
                    .is_empty()
                );
                let speech = push(&mut engine, track, wave.samples(), rate, origin + 1000, 20);
                assert!(speech.iter().any(|e| !e.is_final && !e.text.is_empty()));
                events.extend(speech);
                let end = origin + 1000 + wave.samples().len() as u64 * 1000 / u64::from(rate);
                events.extend(push(
                    &mut engine,
                    track,
                    &vec![0.0; rate as usize * 4],
                    rate,
                    end,
                    20,
                ));
                events.extend(engine.reset_track(track).unwrap());
                let gate = engine.tracks[index(track)].vad.as_ref().unwrap();
                assert_eq!(gate.buffered_samples(), 0);
                assert_eq!(gate.detector_frames(), 0);
                assert_eq!(gate.processed_frames, 0);
            }
            events.extend(push(
                &mut engine,
                TrackKind::Mic,
                wave.samples(),
                rate,
                60_000,
                20,
            ));
            events.extend(engine.finish().unwrap());
            for (track, origin) in [(TrackKind::Mic, 1000), (TrackKind::System, 3000)] {
                let finals: Vec<_> = events
                    .iter()
                    .filter(|e| e.is_final && e.track == track && e.end_ms < 30_000)
                    .collect();
                let text = finals
                    .iter()
                    .map(|e| e.text.as_str())
                    .collect::<Vec<_>>()
                    .join(" ")
                    .to_lowercase();
                assert!(
                    text.starts_with("after early"),
                    "leading speech at {options:?}: {text}"
                );
                assert_eq!(
                    text.matches("yellow lamps").count(),
                    1,
                    "{options:?}: {text}"
                );
                assert!(finals.iter().all(|e| e.start_ms >= origin));
            }
            assert!(
                events
                    .iter()
                    .any(|e| e.is_final && e.track == TrackKind::Mic && e.start_ms >= 60_000)
            );
            for event in events.iter().filter(|e| e.is_final) {
                assert!(!event.words.is_empty());
                assert!(event.words.iter().all(|w| w.start_ms >= event.start_ms
                    && w.end_ms <= event.end_ms
                    && w.start_ms <= w.end_ms));
            }
            events
        };
        let legacy = run(None);
        assert_eq!(legacy, run(Some(LiveVadOptions::default())));
        for threshold in [0.2, 0.35] {
            run(Some(LiveVadOptions { threshold }));
        }
    }
    #[test]
    #[ignore = "requires VT_STT_MODEL_DIR, VT_VAD_MODEL_FILE and VT_STT_FIXTURE_WAV; never downloads"]
    fn real_live_vad_is_incremental_retains_onsets_tracks_gaps_words_and_bounded_silence() {
        let model = std::env::var_os("VT_STT_MODEL_DIR").unwrap();
        let vad = std::env::var_os("VT_VAD_MODEL_FILE").unwrap();
        let fixture = std::env::var("VT_STT_FIXTURE_WAV").unwrap();
        let wave = sherpa_onnx::Wave::read(&fixture).unwrap();
        let rate = wave.sample_rate() as u32;
        let temp = tempfile::tempdir().unwrap();
        let manager = crate::VadModelManager::new(temp.path().into());
        manager
            .install_from_file(
                Path::new(&vad),
                std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            )
            .unwrap();
        assert!(manager.installed());
        manager.verify().unwrap();
        let mut engine =
            StreamingRecognizer::with_vad(Path::new(&model), Some(&manager.model_dir())).unwrap();
        let crop = (rate as usize * 350 / 1000).min(wave.samples().len());
        let speech = &wave.samples()[crop..];
        let high = sherpa_onnx::LinearResampler::create(rate as i32, 48000)
            .unwrap()
            .resample(speech, true);
        let mut all = vec![];
        for (track, origin, samples, input_rate, packet_ms) in [
            (TrackKind::Mic, 1000, speech, rate, 20),
            (TrackKind::System, 3000, high.as_slice(), 48000, 100),
        ] {
            assert!(
                push(
                    &mut engine,
                    track,
                    &vec![0.0; input_rate as usize],
                    input_rate,
                    origin,
                    packet_ms
                )
                .is_empty()
            );
            let events = push(
                &mut engine,
                track,
                samples,
                input_rate,
                origin + 1000,
                packet_ms,
            );
            assert!(
                events
                    .iter()
                    .any(|event| !event.is_final && !event.text.is_empty()),
                "partials must arrive during speech, before a completed VAD region"
            );
            if let Some(first) = events
                .iter()
                .find(|event| !event.is_final && !event.words.is_empty())
            {
                eprintln!(
                    "{track:?} first partial input endpoint={}ms, first aligned word={}ms",
                    first.end_ms, first.words[0].start_ms
                );
            }
            all.extend(events);
            let end = origin + 1000 + samples.len() as u64 * 1000 / u64::from(input_rate);
            let mut tail_events = vec![];
            for n in 0..40 {
                tail_events.extend(
                    engine
                        .accept(
                            track,
                            &vec![0.0; (input_rate / 10) as usize],
                            input_rate,
                            end + n * 100,
                        )
                        .unwrap(),
                );
                if tail_events.iter().any(|event| event.is_final) {
                    break;
                }
            }
            assert!(
                tail_events.iter().any(|event| event.is_final),
                "speech must finalize within4seconds of real silence"
            );
            all.extend(tail_events);
            all.extend(engine.reset_track(track).unwrap());
        }
        for (track, origin) in [(TrackKind::Mic, 1000), (TrackKind::System, 3000)] {
            let finals = all
                .iter()
                .filter(|event| event.track == track && event.is_final)
                .collect::<Vec<_>>();
            let text = finals
                .iter()
                .map(|event| event.text.as_str())
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase();
            eprintln!("{track:?} gated final: {text}");
            assert_eq!(
                text.matches("yellow lamps").count(),
                1,
                "duplicate boundary words: {text}"
            );
            assert!(
                text.starts_with("after early"),
                "leading phonemes lost: {text}"
            );
            assert!(text.contains("yellow lamps"), "{text}");
            assert!(finals.iter().all(|event| {
                event.start_ms >= origin
                    && event.end_ms < origin + 12000
                    && !event.words.is_empty()
                    && event.words.iter().all(|word| {
                        word.start_ms >= event.start_ms
                            && word.end_ms <= event.end_ms
                            && word.end_ms >= word.start_ms
                    })
            }));
        }
        let mut gap = push(&mut engine, TrackKind::Mic, speech, rate, 60000, 20);
        gap.extend(engine.reset_track(TrackKind::Mic).unwrap());
        assert!(
            gap.iter()
                .filter(|event| event.is_final)
                .all(|event| event.start_ms >= 60000)
        );
        assert!(
            gap.iter()
                .any(|event| event.is_final && event.text.to_lowercase().contains("yellow lamps"))
        );
        // Continuous real speech crosses the native detector reset and the
        // unchanged15s ASR limit without building completed VAD utterances.
        let continuous =
            &wave.samples()[crop..(rate as usize * 6100 / 1000).min(wave.samples().len())];
        let mut frames = 0_u64;
        let mut first_final_at = None;
        for _ in 0..4 {
            for packet in continuous.chunks((rate / 10) as usize) {
                let events = engine
                    .accept(
                        TrackKind::Mic,
                        packet,
                        rate,
                        90000 + frames * 1000 / u64::from(rate),
                    )
                    .unwrap();
                frames += packet.len() as u64;
                if first_final_at.is_none() && events.iter().any(|event| event.is_final) {
                    first_final_at = Some(frames * 1000 / u64::from(rate));
                }
                let vad = engine.tracks[0].vad.as_ref().unwrap();
                assert!(vad.buffered_samples() <= crate::live_vad::PRE_ROLL + WINDOW);
                assert!(vad.detector_frames() <= 16 * 16000);
            }
        }
        assert!(
            first_final_at.is_some_and(|time| time <= 17000),
            "15s forced final did not occur: {first_final_at:?}"
        );
        engine.reset_track(TrackKind::Mic).unwrap();
        for n in 0..65 {
            assert!(
                engine
                    .accept(TrackKind::System, &[0.0; 16000], 16000, 130000 + n * 1000)
                    .unwrap()
                    .is_empty()
            );
            let vad = engine.tracks[1].vad.as_ref().unwrap();
            assert!(vad.buffered_samples() <= crate::live_vad::PRE_ROLL + WINDOW);
            assert!(vad.detector_frames() <= 16 * 16000);
        }
        assert!(engine.finish().unwrap().is_empty());
        assert!(engine.finish().unwrap().is_empty());
    }
}
