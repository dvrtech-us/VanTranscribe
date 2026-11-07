use crate::{
    cluster::{Clusters, normalize},
    *,
};
use sherpa_onnx::{
    LinearResampler, OfflineSpeakerDiarization, OfflineSpeakerDiarizationConfig,
    SpeakerEmbeddingExtractor, SpeakerEmbeddingExtractorConfig,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};
use vt_core::TrackKind;
const RATE: u32 = 16000;
const SHIFT: usize =
    Phase::Live.segmentation_context().ownership_stride_ms as usize * RATE as usize / 1000;
fn source_window(phase: Phase) -> usize {
    phase.segmentation_context().source_window_ms as usize * RATE as usize / 1000
}
const MAX_TURNS: usize = 32768;
const MAX_EMBEDDINGS: usize = 8192;
use crate::finalize::Record;
struct Track {
    audio: Vec<f32>,
    resampler: Option<LinearResampler>,
    rate: Option<u32>,
    origin: Option<u64>,
    input_frames: u64,
    last_end: u64,
    window_frame: u64,
    serial: u64,
    clusters: Clusters,
}
impl Track {
    fn new(name: &str, config: &DiarizerConfig) -> Self {
        Self {
            audio: Vec::with_capacity(source_window(config.phase) + 32000),
            resampler: None,
            rate: None,
            origin: None,
            input_frames: 0,
            last_end: 0,
            window_frame: 0,
            serial: 0,
            clusters: Clusters::new(name, &[]),
        }
    }
}
pub struct Diarizer {
    segmenter: OfflineSpeakerDiarization,
    extractor: SpeakerEmbeddingExtractor,
    config: DiarizerConfig,
    tracks: [Track; 2],
    turns: Vec<SpeakerTurn>,
    finished: bool,
    failed: bool,
    records: Vec<Record>,
    overlap_links: Vec<(String, Vec<String>)>,
    mic_clustered: bool,
}
impl Diarizer {
    pub fn new(model_dir: &Path, config: DiarizerConfig) -> Result<Self> {
        models::verify(model_dir, true)?;
        if config.constraints.len() > 4096 || config.temporal_constraints.len() > 4096 {
            return Err(DiarizeError::Capacity);
        }
        if config.live_speaker_prior.is_some_and(|n| n == 0 || n > 64) {
            return Err(DiarizeError::Invalid("speaker prior must be1–64"));
        }
        let mic_clustered =
            config.mode == MeetingMode::InPerson || crate::finalize::mic_cannot_link(&config);
        let path = |file: &str| {
            model_dir
                .join(file)
                .to_str()
                .map(str::to_owned)
                .ok_or(DiarizeError::Model)
        };
        let embedding = SpeakerEmbeddingExtractorConfig {
            model: Some(path(EMBEDDING)?),
            num_threads: 1,
            debug: false,
            provider: Some("cpu".into()),
        };
        let mut native = OfflineSpeakerDiarizationConfig {
            embedding: embedding.clone(),
            min_duration_on: 0.0,
            min_duration_off: 0.0,
            ..Default::default()
        };
        native.segmentation.pyannote.model = Some(path(SEGMENTATION)?);
        native.segmentation.pyannote.window_shift_ratio = 0.5;
        native.segmentation.num_threads = 1;
        let segmenter = OfflineSpeakerDiarization::create(&native).ok_or(DiarizeError::Engine)?;
        if segmenter.sample_rate() != RATE as i32 {
            return Err(DiarizeError::Model);
        }
        let extractor =
            SpeakerEmbeddingExtractor::create(&embedding).ok_or(DiarizeError::Engine)?;
        if extractor.dim() != 512 {
            return Err(DiarizeError::Model);
        }
        let tracks = [Track::new("mic", &config), Track::new("system", &config)];
        Ok(Self {
            segmenter,
            extractor,
            config,
            tracks,
            turns: Vec::new(),
            finished: false,
            failed: false,
            records: Vec::new(),
            overlap_links: Vec::new(),
            mic_clustered,
        })
    }
    /// Feed independently clocked mono chunks of at most two seconds. No STT
    /// segment, word, text, identity gallery or network is consulted here.
    pub fn accept(
        &mut self,
        track: TrackKind,
        mono: &[f32],
        rate: u32,
        start_ms: u64,
    ) -> Result<Vec<SpeakerTurn>> {
        if self.failed {
            return Err(DiarizeError::Invalid(
                "previous failure requires a new diarizer",
            ));
        }
        let result = self.accept_inner(track, mono, rate, start_ms);
        if result.is_err() {
            self.failed = true;
            for state in &mut self.tracks {
                state.audio.clear();
            }
            self.records.clear();
        }
        result
    }
    fn accept_inner(
        &mut self,
        track: TrackKind,
        mono: &[f32],
        rate: u32,
        start_ms: u64,
    ) -> Result<Vec<SpeakerTurn>> {
        if self.finished {
            return Err(DiarizeError::Invalid("already finished"));
        }
        if !(8000..=192000).contains(&rate)
            || mono.len() > rate as usize * 2
            || mono.iter().any(|x| !x.is_finite())
        {
            return Err(DiarizeError::Invalid(
                "finite mono audio, 8–192kHz, at most2seconds required",
            ));
        }
        if mono.is_empty() {
            return Ok(vec![]);
        }
        let index = index(track);
        let mut updates = Vec::new();
        if self.tracks[index].rate.is_some_and(|r| r != rate)
            || self.tracks[index].origin.is_some()
                && self.tracks[index].last_end.abs_diff(start_ms) > 5
        {
            updates.extend(self.reset_track(track)?);
        }
        let state = &mut self.tracks[index];
        if state.origin.is_none() {
            state.origin = Some(start_ms);
            state.rate = Some(rate);
            if rate != RATE {
                state.resampler = Some(
                    LinearResampler::create(rate as i32, RATE as i32)
                        .ok_or(DiarizeError::Engine)?,
                );
            }
        }
        state.input_frames = state
            .input_frames
            .checked_add(mono.len() as u64)
            .ok_or(DiarizeError::Invalid("clock overflow"))?;
        state.last_end = state
            .origin
            .unwrap()
            .checked_add(
                state
                    .input_frames
                    .checked_mul(1000)
                    .ok_or(DiarizeError::Invalid("clock overflow"))?
                    / u64::from(rate),
            )
            .ok_or(DiarizeError::Invalid("clock overflow"))?;
        if let Some(resampler) = &state.resampler {
            state.audio.extend(resampler.resample(mono, false));
        } else {
            state.audio.extend_from_slice(mono);
        }
        while self.tracks[index].audio.len() >= source_window(self.config.phase) {
            updates.extend(self.window(track, false)?);
        }
        Ok(updates)
    }
    /// Gaps reset clocks/context but retain meeting-local speaker exemplars.
    pub fn reset_track(&mut self, track: TrackKind) -> Result<Vec<SpeakerTurn>> {
        if self.failed {
            return Err(DiarizeError::Invalid(
                "previous failure requires a new diarizer",
            ));
        }
        if self.finished {
            return Err(DiarizeError::Invalid("already finished"));
        }
        let i = index(track);
        if let Some(resampler) = &self.tracks[i].resampler {
            let tail = resampler.resample(&[], true);
            self.tracks[i].audio.extend(tail);
        }
        let mut out = Vec::new();
        while !self.tracks[i].audio.is_empty() {
            out.extend(self.window(track, true)?);
        }
        let state = &mut self.tracks[i];
        state.resampler = None;
        state.rate = None;
        state.origin = None;
        state.input_frames = 0;
        state.window_frame = 0;
        Ok(out)
    }
    fn window(&mut self, track: TrackKind, flushing: bool) -> Result<Vec<SpeakerTurn>> {
        let i = index(track);
        let window = source_window(self.config.phase);
        let state = &mut self.tracks[i];
        let actual = state.audio.len().min(window);
        if actual == 0 {
            return Ok(vec![]);
        }
        let emit = if flushing && actual < window {
            actual
        } else {
            SHIFT
        };
        // Refined windows supply the canonical 10s of real source. Live stays
        // at 5s; sherpa pads it and short final tails. Own only the fixed prefix
        // until flush, and always clip outputs to actual source frames.
        let audio = &state.audio[..actual];
        let native = self
            .segmenter
            .process(audio)
            .ok_or(DiarizeError::Engine)?
            .sort_by_start_time();
        let mut spans = Vec::new();
        let mut bounds = BTreeSet::from([0usize, actual]);
        for value in native {
            if !(0..3).contains(&value.speaker)
                || !value.start.is_finite()
                || !value.end.is_finite()
                || value.start < 0.
                || value.end < value.start
            {
                return Err(DiarizeError::Engine);
            }
            let start = ((value.start as f64 * f64::from(RATE)).round() as usize).min(actual);
            let end = ((value.end as f64 * f64::from(RATE)).round() as usize).min(actual);
            if start < end {
                spans.push((start, end, value.speaker));
                bounds.insert(start);
                bounds.insert(end);
            }
        }
        let bounds = bounds.into_iter().collect::<Vec<_>>();
        let mut output = Vec::new();
        let mut local_clean = BTreeMap::<i32, (String, String)>::new();
        let mut pending_overlap = Vec::<(usize, Vec<i32>)>::new();
        for endpoints in bounds.windows(2) {
            let (a, b) = (endpoints[0], endpoints[1]);
            if a >= emit {
                break;
            }
            let active = spans
                .iter()
                .filter(|(start, end, _)| *start <= a && b <= *end)
                .map(|s| s.2)
                .collect::<BTreeSet<_>>();
            if active.is_empty() {
                continue;
            }
            if self.turns.len() + output.len() >= MAX_TURNS {
                return Err(DiarizeError::Capacity);
            }
            state.serial += 1;
            let id = format!("{}:turn-{}", track.as_str(), state.serial);
            let clip = b.min(emit);
            let origin = state.origin.unwrap();
            let start_ms = origin + (state.window_frame + a as u64) * 1000 / u64::from(RATE);
            let end_ms = (origin + (state.window_frame + clip as u64) * 1000 / u64::from(RATE))
                .min(state.last_end);
            if end_ms <= start_ms {
                continue;
            }
            let overlap = active.len() > 1;
            let mut speaker_ids = Vec::new();
            let mut score = None;
            let mut provisional = false;
            let duration = end_ms - start_ms;
            let mut label = if overlap { "Overlap" } else { "Unknown" }.to_string();
            let mut provenance = "segmentation".to_string();
            if !overlap && duration >= 1000 {
                if self.config.mode == MeetingMode::Remote
                    && track == TrackKind::Mic
                    && !self.mic_clustered
                {
                    speaker_ids.push("mic:me".into());
                    label = "Me".into();
                    provenance = "channel_prior".into();
                } else {
                    let stream = self.extractor.create_stream().ok_or(DiarizeError::Engine)?;
                    stream.accept_waveform(RATE as i32, &audio[a..b]);
                    stream.input_finished();
                    if self.extractor.is_ready(&stream) {
                        let embedding = normalize(
                            self.extractor
                                .compute(&stream)
                                .ok_or(DiarizeError::Engine)?,
                        )?;
                        if self.records.len() >= MAX_EMBEDDINGS {
                            return Err(DiarizeError::Capacity);
                        }
                        self.records.push(Record {
                            turn: id.clone(),
                            embedding: embedding.clone(),
                            duration: end_ms - start_ms,
                        });
                        let (speaker, similarity, visible) =
                            state.clusters.assign(&id, embedding, end_ms - start_ms)?;
                        speaker_ids.push(speaker.clone());
                        score = Some(similarity);
                        provisional = !visible;
                        if visible {
                            label =
                                format!("Speaker {}", speaker.rsplit('-').next().unwrap_or("?"));
                        }
                        provenance = if self.config.phase == Phase::Live {
                            "live-cluster"
                        } else {
                            "refined-cluster"
                        }
                        .into();
                    }
                }
            }
            if overlap {
                pending_overlap.push((output.len(), active.iter().copied().collect()));
            } else if let Some(speaker) = speaker_ids.first() {
                local_clean.insert(*active.first().unwrap(), (id.clone(), speaker.clone()));
            }
            let uncertain = overlap || speaker_ids.is_empty() || provisional;
            output.push(SpeakerTurn {
                id,
                track: track.as_str().into(),
                start_ms,
                end_ms,
                speaker_ids,
                label,
                provenance,
                score,
                overlap,
                uncertain,
                provisional,
            });
        }
        for (position, locals) in pending_overlap {
            let links = locals
                .iter()
                .filter_map(|id| local_clean.get(id))
                .collect::<Vec<_>>();
            if links.len() == locals.len() {
                let ids = links
                    .iter()
                    .map(|(_, speaker)| speaker.clone())
                    .collect::<BTreeSet<_>>();
                if ids.len() == locals.len() {
                    output[position].speaker_ids = ids.into_iter().collect();
                    self.overlap_links.push((
                        output[position].id.clone(),
                        links.iter().map(|(turn, _)| turn.clone()).collect(),
                    ));
                }
            }
        }
        state.audio.drain(..emit);
        state.window_frame += emit as u64;
        self.turns.extend(output.iter().cloned());
        for turn in &mut output {
            crate::finalize::mask_live(turn, &self.config);
        }
        Ok(output)
    }
    pub fn set_temporal_constraints(&mut self, constraints: Vec<TemporalConstraint>) -> Result<()> {
        if self.finished || constraints.len() > 4096 {
            return Err(DiarizeError::Constraint);
        }
        self.config.temporal_constraints = constraints;
        Ok(())
    }
    pub fn finish(&mut self) -> Result<DiarizationResult> {
        if self.failed {
            return Err(DiarizeError::Invalid(
                "previous failure requires a new diarizer",
            ));
        }
        if self.finished {
            return Err(DiarizeError::Invalid("already finished"));
        }
        self.reset_track(TrackKind::Mic)?;
        self.reset_track(TrackKind::System)?;
        let mut result = crate::finalize::finish(
            self.turns.clone(),
            &self.records,
            &self.config,
            self.mic_clustered,
            &self.overlap_links,
        )?;
        result.segmentation_context = Some(self.config.phase.segmentation_context());
        self.finished = true;
        self.records.clear();
        self.turns.clear();
        Ok(result)
    }
}
fn index(track: TrackKind) -> usize {
    if track == TrackKind::Mic { 0 } else { 1 }
}

#[cfg(test)]
mod context_tests {
    use super::*;
    use sha2::{Digest, Sha256};
    fn projection(t: &SpeakerTurn) -> serde_json::Value {
        serde_json::json!({"id":t.id,"track":t.track,"start_ms":t.start_ms,"end_ms":t.end_ms,"speaker_ids":t.speaker_ids,"label":t.label,"provenance":t.provenance,"overlap":t.overlap,"uncertain":t.uncertain,"provisional":t.provisional})
    }
    fn public_control() -> (std::path::PathBuf, sherpa_onnx::Wave) {
        let path = std::env::var("VT_DIARIZE_CONTEXT_WAV")
            .expect("set VT_DIARIZE_CONTEXT_WAV to the pinned public 6.625s control");
        let expected: serde_json::Value =
            serde_json::from_str(include_str!("fixtures/live5-public-control-baseline.json"))
                .unwrap();
        assert_eq!(
            format!("{:x}", Sha256::digest(std::fs::read(&path).unwrap())),
            expected["fixture_sha256"].as_str().unwrap()
        );
        (
            std::env::var_os("VT_DIARIZE_MODEL_DIR").unwrap().into(),
            sherpa_onnx::Wave::read(&path).unwrap(),
        )
    }
    fn feed(
        d: &mut Diarizer,
        track: TrackKind,
        samples: &[f32],
        rate: u32,
        origin: u64,
    ) -> Vec<SpeakerTurn> {
        let mut frame = 0;
        let mut updates = Vec::new();
        for packet in samples.chunks((rate / 10) as usize) {
            updates.extend(
                d.accept(track, packet, rate, origin + frame * 1000 / u64::from(rate))
                    .unwrap(),
            );
            frame += packet.len() as u64;
            assert!(d.tracks[index(track)].audio.len() < source_window(d.config.phase));
            assert!(
                d.tracks[index(track)].audio.capacity() <= source_window(d.config.phase) + 32000
            );
        }
        updates
    }
    #[test]
    fn phase_geometry_drives_source_capacity_without_changing_ownership_or_native_input() {
        for (phase, frames) in [(Phase::Live, 80_000), (Phase::Refined, 160_000)] {
            let config = DiarizerConfig {
                phase,
                ..Default::default()
            };
            let track = Track::new("mic", &config);
            assert_eq!(source_window(phase), frames);
            assert_eq!(track.audio.capacity(), frames + 32_000);
            assert_eq!(SHIFT, 40_000);
            assert_eq!(phase.segmentation_context().ownership_stride_ms, 2_500);
            assert_eq!(phase.segmentation_context().native_window_ms, 10_000);
        }
    }
    #[test]
    #[ignore = "requires VT_DIARIZE_MODEL_DIR and pinned public VT_DIARIZE_CONTEXT_WAV; no downloads/devices"]
    fn real_live_context_matches_recorded_pre_change_public_baseline() {
        let (model, wave) = public_control();
        let rate = wave.sample_rate() as u32;
        let mut d = Diarizer::new(
            &model,
            DiarizerConfig {
                phase: Phase::Live,
                ..Default::default()
            },
        )
        .unwrap();
        let mut updates = Vec::new();
        for (track, origin) in [(TrackKind::Mic, 1000), (TrackKind::System, 3000)] {
            updates.extend(feed(&mut d, track, wave.samples(), rate, origin));
        }
        let result = d.finish().unwrap();
        let expected: serde_json::Value =
            serde_json::from_str(include_str!("fixtures/live5-public-control-baseline.json"))
                .unwrap();
        assert_eq!(
            serde_json::to_value(updates.iter().map(projection).collect::<Vec<_>>()).unwrap(),
            expected["updates"]
        );
        assert_eq!(
            serde_json::to_value(result.turns.iter().map(projection).collect::<Vec<_>>()).unwrap(),
            expected["turns"]
        );
        assert_eq!(
            result.segmentation_context,
            Some(Phase::Live.segmentation_context())
        );
    }
    #[test]
    #[ignore = "requires VT_DIARIZE_MODEL_DIR and pinned public VT_DIARIZE_CONTEXT_WAV; no downloads/devices"]
    fn real_refined_context_owns_prefixes_and_clips_short_flushes_gaps_and_format_changes() {
        let (model, wave) = public_control();
        let rate = wave.sample_rate() as u32;
        assert_eq!(rate, RATE);
        let source = wave
            .samples()
            .iter()
            .copied()
            .cycle()
            .take((rate as usize * 12_625) / 1000)
            .collect::<Vec<_>>();
        let mut d = Diarizer::new(
            &model,
            DiarizerConfig {
                phase: Phase::Refined,
                ..Default::default()
            },
        )
        .unwrap();
        let mut frame = 0usize;
        let mut first = Vec::new();
        for packet in source.chunks((rate / 10) as usize) {
            let updates = d
                .accept(
                    TrackKind::Mic,
                    packet,
                    rate,
                    1000 + frame as u64 * 1000 / u64::from(rate),
                )
                .unwrap();
            frame += packet.len();
            if frame < rate as usize * 10 {
                assert!(
                    updates.is_empty(),
                    "refined mode must wait for real 10s context"
                );
            }
            if frame == rate as usize * 10 {
                assert!(!updates.is_empty());
                assert_eq!(d.tracks[0].window_frame, SHIFT as u64);
                assert_eq!(d.tracks[0].audio.len(), 160_000 - SHIFT);
                first.extend(updates.clone());
            }
            assert!(d.tracks[0].audio.len() < 160_000);
            assert!(d.tracks[0].audio.capacity() <= 192_000);
        }
        assert!(first.iter().all(|t| t.start_ms >= 1000 && t.end_ms <= 3500));
        assert_eq!(d.tracks[0].window_frame, 2 * SHIFT as u64);
        let tail = d.reset_track(TrackKind::Mic).unwrap();
        assert!(!tail.is_empty());
        assert!(
            tail.iter()
                .all(|t| t.start_ms >= 6000 && t.end_ms <= 13_625)
        );
        assert!(d.tracks[0].audio.is_empty());
        assert!(d.tracks[0].origin.is_none());
        assert_eq!(d.tracks[0].window_frame, 0);
        feed(
            &mut d,
            TrackKind::Mic,
            &wave.samples()[..rate as usize * 3 / 4],
            rate,
            60_000,
        );
        let high = LinearResampler::create(rate as i32, 48_000)
            .unwrap()
            .resample(&wave.samples()[..rate as usize * 5 / 4], true);
        assert!(feed(&mut d, TrackKind::System, &high, 48_000, 90_000).is_empty());
        let changed = feed(
            &mut d,
            TrackKind::System,
            &wave.samples()[..rate as usize * 3 / 4],
            rate,
            100_000,
        );
        assert!(!changed.is_empty());
        assert!(
            changed
                .iter()
                .all(|t| t.start_ms >= 90_000 && t.end_ms <= 91_250)
        );
        let result = d.finish().unwrap();
        assert_eq!(
            result.segmentation_context,
            Some(Phase::Refined.segmentation_context())
        );
        assert!(!result.calibrated);
        for (track, start, end) in [
            ("mic", 1000, 13_625),
            ("mic", 60_000, 60_750),
            ("system", 90_000, 91_250),
            ("system", 100_000, 100_750),
        ] {
            let turns = result
                .turns
                .iter()
                .filter(|t| t.track == track && t.start_ms >= start && t.start_ms < end)
                .collect::<Vec<_>>();
            assert!(!turns.is_empty(), "missing {track} source epoch {start}");
            assert!(turns.iter().all(|t| t.end_ms <= end));
            assert!(
                turns
                    .windows(2)
                    .all(|pair| pair[0].end_ms <= pair[1].start_ms)
            );
            if end - start < 1000 {
                assert!(
                    turns
                        .iter()
                        .all(|t| t.speaker_ids.is_empty() && t.label == "Unknown")
                );
            }
        }
        assert_eq!(
            result
                .turns
                .iter()
                .map(|t| t.id.as_str())
                .collect::<BTreeSet<_>>()
                .len(),
            result.turns.len()
        );
        let json = serde_json::to_string(&result).unwrap();
        assert!(!json.contains("embedding\":") && !json.contains("centroid"));
    }
}
