//! Incremental speech decisions, never completed-region batching.
use crate::{
    Result, SttError,
    vad_models::{VAD_MODEL_FILE, verify_vad_file},
};
use sherpa_onnx::{SileroVadModelConfig, VadModelConfig, VoiceActivityDetector};
use std::{collections::VecDeque, path::Path, sync::atomic::AtomicBool};
pub(crate) const WINDOW: usize = 512;
pub(crate) const PRE_ROLL: usize = 5120; //320ms, including onset-decision context.
pub(crate) const HANGOVER: usize = 16000; //real audio, so native endpoints still see silence.
const DETECTOR_EPOCH: usize = 16 * 16000;
/// Explicit detector tuning, independent of the acoustic model and recording.
/// The default preserves existing production behavior; alternatives require
/// separate quality evaluation and do not alter amplitude or gate context.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveVadOptions {
    pub threshold: f32,
}
impl Default for LiveVadOptions {
    fn default() -> Self {
        Self { threshold: 0.5 }
    }
}
impl LiveVadOptions {
    pub fn validate(self) -> Result<()> {
        if !self.threshold.is_finite() || !(0.1..=0.9).contains(&self.threshold) {
            return Err(SttError::InvalidVadOptions(
                "threshold must be finite and within 0.1..=0.9".into(),
            ));
        }
        Ok(())
    }
}
pub(crate) enum GateAction {
    Skip,
    Open(Vec<f32>),
    Feed { close: bool },
}
#[derive(Default)]
pub(crate) struct SpeechGate {
    pub(crate) open: bool,
    quiet: usize,
    pre_roll: VecDeque<f32>,
}
impl SpeechGate {
    pub(crate) fn decide(&mut self, window: &[f32], speech: bool) -> GateAction {
        if !self.open {
            self.pre_roll.extend(window.iter().copied());
            while self.pre_roll.len() > PRE_ROLL {
                self.pre_roll.pop_front();
            }
            if !speech {
                return GateAction::Skip;
            }
            self.open = true;
            self.quiet = 0;
            return GateAction::Open(self.pre_roll.drain(..).collect());
        }
        if speech {
            self.quiet = 0;
        } else {
            self.quiet += window.len();
        }
        let close = self.quiet >= HANGOVER;
        if close {
            self.open = false;
            self.quiet = 0;
            self.pre_roll.clear();
        }
        GateAction::Feed { close }
    }
}
pub(crate) struct LiveVad {
    detector: VoiceActivityDetector,
    detector_frames: usize,
    pub(crate) gate: SpeechGate,
    pub(crate) pending: [f32; WINDOW],
    pub(crate) pending_len: usize,
    pub(crate) processed_frames: u64,
}
impl LiveVad {
    pub(crate) fn config(path: &Path, options: LiveVadOptions) -> Result<VadModelConfig> {
        options.validate()?;
        let file = if path.is_dir() {
            path.join(VAD_MODEL_FILE)
        } else {
            path.to_path_buf()
        };
        verify_vad_file(&file, &AtomicBool::new(false))?;
        let file = file
            .to_str()
            .ok_or_else(|| SttError::InvalidModel("VAD model path is not UTF-8".into()))?;
        Ok(VadModelConfig {
            silero_vad: SileroVadModelConfig {
                model: Some(file.into()),
                threshold: options.threshold,
                min_silence_duration: 0.16,
                min_speech_duration: 0.032,
                window_size: WINDOW as i32,
                max_speech_duration: 18.0,
            },
            sample_rate: 16000,
            num_threads: 1,
            provider: Some("cpu".into()),
            ..Default::default()
        })
    }
    pub(crate) fn new(config: &VadModelConfig) -> Result<Self> {
        Ok(Self {
            detector: VoiceActivityDetector::create(config, 20.0).ok_or(SttError::Engine)?,
            detector_frames: 0,
            gate: SpeechGate::default(),
            pending: [0.0; WINDOW],
            pending_len: 0,
            processed_frames: 0,
        })
    }
    pub(crate) fn reset(&mut self) {
        self.detector.reset();
        self.detector_frames = 0;
        self.gate = SpeechGate::default();
        self.pending_len = 0;
        self.processed_frames = 0;
    }
    #[cfg(test)]
    pub(crate) fn buffered_samples(&self) -> usize {
        self.pending_len + self.gate.pre_roll.len()
    }
    #[cfg(test)]
    pub(crate) fn detector_frames(&self) -> usize {
        self.detector_frames
    }
    pub(crate) fn classify(&mut self, window: &[f32; WINDOW]) -> GateAction {
        // The wrapper's native detector also buffers active speech. Bound that
        // independently of the ASR stream; keep gate hangover through this reset.
        if self.detector_frames >= DETECTOR_EPOCH {
            self.detector.reset();
            self.detector_frames = 0;
        }
        self.detector.accept_waveform(window);
        self.detector_frames += WINDOW;
        let speech = self.detector.detected();
        self.detector.clear();
        self.gate.decide(window, speech)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_thresholds_are_bounded_serializable_and_default_to_existing_policy() {
        assert_eq!(LiveVadOptions::default().threshold, 0.5);
        for threshold in [0.1, 0.2, 0.35, 0.5, 0.9] {
            let options = LiveVadOptions { threshold };
            options.validate().unwrap();
            let encoded = serde_json::to_string(&options).unwrap();
            assert_eq!(
                serde_json::from_str::<LiveVadOptions>(&encoded).unwrap(),
                options
            );
        }
        for threshold in [
            f32::NAN,
            f32::INFINITY,
            f32::NEG_INFINITY,
            -0.5,
            0.0,
            0.099,
            0.901,
            1.0,
        ] {
            assert!(matches!(
                LiveVadOptions { threshold }.validate(),
                Err(SttError::InvalidVadOptions(_))
            ));
            assert!(matches!(
                LiveVad::config(Path::new("uninstalled-vad"), LiveVadOptions { threshold }),
                Err(SttError::InvalidVadOptions(_))
            ));
        }
        assert!(serde_json::from_str::<LiveVadOptions>(r#"{"threshold":0.5,"gain":10}"#).is_err());
    }
    #[test]
    #[ignore = "requires VT_VAD_MODEL_FILE; generated silence/noise only, no audio devices"]
    fn native_options_keep_generated_silence_and_noise_storage_bounded() {
        let path = std::env::var_os("VT_VAD_MODEL_FILE").unwrap();
        for threshold in [0.2, 0.35, 0.5] {
            let config = LiveVad::config(Path::new(&path), LiveVadOptions { threshold }).unwrap();
            let mut detector = LiveVad::new(&config).unwrap();
            for _ in 0..(65 * 16000 / WINDOW) {
                assert!(matches!(
                    detector.classify(&[0.0; WINDOW]),
                    GateAction::Skip
                ));
                assert!(detector.buffered_samples() <= PRE_ROLL + WINDOW);
                assert!(detector.detector_frames() <= DETECTOR_EPOCH);
            }
            let mut seed = 0x1234_5678u32;
            let mut opens = 0;
            for _ in 0..(30 * 16000 / WINDOW) {
                let samples = std::array::from_fn(|_| {
                    seed ^= seed << 13;
                    seed ^= seed >> 17;
                    seed ^= seed << 5;
                    (seed as f64 / u32::MAX as f64 * 2.0 - 1.0) as f32 * 0.003
                });
                if matches!(detector.classify(&samples), GateAction::Open(_)) {
                    opens += 1;
                }
                assert!(detector.buffered_samples() <= PRE_ROLL + WINDOW);
                assert!(detector.detector_frames() <= DETECTOR_EPOCH);
            }
            eprintln!(
                "threshold={threshold} generated-noise gate openings={opens}; boundedness test, not calibration"
            );
            detector.reset();
            assert_eq!(detector.buffered_samples(), 0);
            assert_eq!(detector.detector_frames(), 0);
            assert!(!detector.gate.open);
        }
    }
    #[test]
    fn pre_roll_is_bounded_and_delivered_once_at_onset() {
        let mut gate = SpeechGate::default();
        for n in 0..100 {
            assert!(matches!(
                gate.decide(&[n as f32; WINDOW], false),
                GateAction::Skip
            ));
            assert!(gate.pre_roll.len() <= PRE_ROLL);
        }
        let GateAction::Open(samples) = gate.decide(&[100.0; WINDOW], true) else {
            panic!("speech must open immediately")
        };
        assert_eq!(samples.len(), PRE_ROLL);
        assert_eq!(samples[0], 91.0);
        assert_eq!(*samples.last().unwrap(), 100.0);
        assert!(matches!(
            gate.decide(&[101.0; WINDOW], true),
            GateAction::Feed { close: false }
        ));
        assert!(gate.pre_roll.is_empty());
    }
    #[test]
    fn hangover_feeds_real_silence_then_new_onset_has_no_replayed_audio() {
        let mut gate = SpeechGate::default();
        assert!(matches!(
            gate.decide(&[1.0; WINDOW], true),
            GateAction::Open(_)
        ));
        for _ in 0..31 {
            assert!(matches!(
                gate.decide(&[2.0; WINDOW], false),
                GateAction::Feed { close: false }
            ));
        }
        assert!(matches!(
            gate.decide(&[3.0; WINDOW], false),
            GateAction::Feed { close: true }
        ));
        assert!(matches!(
            gate.decide(&[4.0; WINDOW], false),
            GateAction::Skip
        ));
        let GateAction::Open(samples) = gate.decide(&[5.0; WINDOW], true) else {
            panic!("next speech must reopen")
        };
        assert_eq!(samples.len(), WINDOW * 2);
        assert!(samples.iter().all(|s| *s == 4.0 || *s == 5.0));
    }
}
