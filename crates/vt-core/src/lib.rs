//! Shared audio contracts for the recording worker.
//!
//! Positions count **frames**, each containing one sample per channel. Platform
//! adapters convert hardware clock values to monotonic session nanoseconds before
//! constructing these values. Epochs identify a device/format configuration; they
//! do not change the session time origin.
//!
//! [`AudioBlock`] owns an allocated sample buffer. It is a worker-side boundary,
//! not a real-time callback API: allocating or validating blocks in the hardware
//! callback is not part of this contract.

use std::{error::Error, fmt, str::FromStr};

/// Independent recording channels; tracks are never mixed for processing.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum TrackKind {
    Mic,
    System,
}

impl TrackKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Mic => "mic",
            Self::System => "system",
        }
    }
}

impl FromStr for TrackKind {
    type Err = AudioError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "mic" => Ok(Self::Mic),
            "system" => Ok(Self::System),
            _ => Err(AudioError::UnknownTrack),
        }
    }
}

/// Interleaved native-rate audio format supported by the recording foundation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StreamFormat {
    pub sample_rate: u32,
    pub channels: u16,
}

impl StreamFormat {
    pub fn new(sample_rate: u32, channels: u16) -> Result<Self, AudioError> {
        let format = Self {
            sample_rate,
            channels,
        };
        format.validate()?;
        Ok(format)
    }

    /// Revalidate values received through public fields or persistence.
    pub fn validate(self) -> Result<(), AudioError> {
        if !(8_000..=192_000).contains(&self.sample_rate) {
            return Err(AudioError::InvalidSampleRate(self.sample_rate));
        }
        if !(1..=8).contains(&self.channels) {
            return Err(AudioError::InvalidChannels(self.channels));
        }
        Ok(())
    }
}

/// First frame's location within an epoch and on the shared session timeline.
///
/// `first_frame` counts frames, not scalar interleaved samples, and resets to
/// zero on each new epoch. `host_time_ns` is elapsed monotonic session time,
/// already normalized by a platform adapter, rather than a raw hardware tick.
/// This worker-side recorder currently requires adjacent blocks to chain the
/// preceding chunk's `end_ns`; fractional nanoseconds are truncated per block.
/// Native adapters must retain observed hardware-clock measurements separately
/// and explicitly reconcile drift/discontinuities. That adapter is not yet
/// implemented; do not pass callback-arrival timestamps as a substitute.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FramePosition {
    pub epoch: u32,
    pub first_frame: u64,
    pub host_time_ns: u64,
}

/// An owned, interleaved, nonempty block containing at most two seconds of audio.
#[derive(Clone, Debug, PartialEq)]
pub struct AudioBlock {
    pub format: StreamFormat,
    pub position: FramePosition,
    pub samples: Vec<f32>,
}

impl AudioBlock {
    pub fn new(
        format: StreamFormat,
        position: FramePosition,
        samples: Vec<f32>,
    ) -> Result<Self, AudioError> {
        let block = Self {
            format,
            position,
            samples,
        };
        block.validate()?;
        Ok(block)
    }

    /// Return frame count. Call [`Self::validate`] before consuming blocks built
    /// through public fields; an invalid zero-channel format returns zero here.
    pub fn frames(&self) -> u64 {
        self.samples
            .len()
            .checked_div(usize::from(self.format.channels))
            .unwrap_or(0) as u64
    }

    /// Validate geometry and that the exclusive frame/time endpoints fit `u64`.
    /// Sub-nanosecond duration fractions are rounded down.
    pub fn validate(&self) -> Result<(), AudioError> {
        self.format.validate()?;
        if self.samples.is_empty() {
            return Err(AudioError::EmptyBlock);
        }
        if !self
            .samples
            .len()
            .is_multiple_of(usize::from(self.format.channels))
        {
            return Err(AudioError::IncompleteFrame {
                samples: self.samples.len(),
                channels: self.format.channels,
            });
        }
        let frames = self.frames();
        let max_frames = u64::from(self.format.sample_rate) * 2;
        if frames > max_frames {
            return Err(AudioError::BlockTooLong { frames, max_frames });
        }
        self.position
            .first_frame
            .checked_add(frames)
            .ok_or(AudioError::FrameRangeOverflow)?;
        // Validated two-second geometry bounds this multiplication.
        let duration_ns = frames * 1_000_000_000 / u64::from(self.format.sample_rate);
        self.position
            .host_time_ns
            .checked_add(duration_ns)
            .ok_or(AudioError::HostTimeOverflow)?;
        Ok(())
    }
}

/// Convert finite floating-point capture samples to signed 24-bit integer PCM.
///
/// Finite samples are clamped to [-1, 1], multiplied by 2^23, rounded to the
/// nearest integer with ties away from zero, then clamped to the signed 24-bit
/// range. NaN and infinities fail the whole conversion. FLAC losslessly preserves
/// these **converted integers**, not arbitrary original floating-point samples.
pub fn pcm24(samples: &[f32]) -> Result<Vec<i32>, AudioError> {
    samples
        .iter()
        .enumerate()
        .map(|(index, &sample)| {
            if !sample.is_finite() {
                return Err(AudioError::NonFiniteSample { index });
            }
            let scaled = (f64::from(sample).clamp(-1.0, 1.0) * 8_388_608.0).round();
            Ok((scaled as i32).clamp(-8_388_608, 8_388_607))
        })
        .collect()
}

/// Explicit reason for an observed discontinuity in a track's timeline.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GapReason {
    Pause,
    DeviceChange,
    Overflow,
    Unavailable,
    /// The app's recorder thread fell further behind capture than its bounded
    /// in-memory queue allows, so the app (not the helper) dropped this audio.
    RecorderBacklog,
    /// Audio the app captured and queued for its recorder, but the app or its
    /// recorder stopped before saving it (found by recovery afterwards).
    RecorderInterrupted,
}

impl GapReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pause => "pause",
            Self::DeviceChange => "device_change",
            Self::Overflow => "overflow",
            Self::Unavailable => "unavailable",
            Self::RecorderBacklog => "recorder_backlog",
            Self::RecorderInterrupted => "recorder_interrupted",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AudioError {
    UnknownTrack,
    InvalidSampleRate(u32),
    InvalidChannels(u16),
    EmptyBlock,
    IncompleteFrame { samples: usize, channels: u16 },
    BlockTooLong { frames: u64, max_frames: u64 },
    FrameRangeOverflow,
    HostTimeOverflow,
    NonFiniteSample { index: usize },
}

impl fmt::Display for AudioError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownTrack => write!(f, "track must be mic or system"),
            Self::InvalidSampleRate(rate) => {
                write!(f, "sample rate {rate} is outside 8000..=192000 Hz")
            }
            Self::InvalidChannels(channels) => {
                write!(f, "channel count {channels} is outside 1..=8")
            }
            Self::EmptyBlock => write!(f, "audio blocks must contain at least one frame"),
            Self::IncompleteFrame { samples, channels } => {
                write!(
                    f,
                    "{samples} interleaved samples do not form {channels}-channel frames"
                )
            }
            Self::BlockTooLong { frames, max_frames } => {
                write!(
                    f,
                    "block contains {frames} frames, exceeding the limit {max_frames}"
                )
            }
            Self::FrameRangeOverflow => write!(f, "audio frame range overflows u64"),
            Self::HostTimeOverflow => write!(f, "audio endpoint time overflows u64 nanoseconds"),
            Self::NonFiniteSample { index } => write!(f, "sample {index} is not finite"),
        }
    }
}

impl Error for AudioError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn position() -> FramePosition {
        FramePosition {
            epoch: 3,
            first_frame: 100,
            host_time_ns: 1_000_000_000,
        }
    }

    #[test]
    fn track_names_are_stable_and_strict() {
        for kind in [TrackKind::Mic, TrackKind::System] {
            assert_eq!(kind.as_str().parse::<TrackKind>(), Ok(kind));
        }
        assert_eq!("MIC".parse::<TrackKind>(), Err(AudioError::UnknownTrack));
    }

    #[test]
    fn rejects_unsupported_formats_before_frame_arithmetic() {
        assert_eq!(
            StreamFormat::new(0, 1),
            Err(AudioError::InvalidSampleRate(0))
        );
        assert!(StreamFormat::new(7_999, 1).is_err());
        assert!(StreamFormat::new(192_001, 1).is_err());
        assert_eq!(
            StreamFormat::new(48_000, 0),
            Err(AudioError::InvalidChannels(0))
        );
        assert!(StreamFormat::new(48_000, 9).is_err());
        assert!(StreamFormat::new(8_000, 1).is_ok());
        assert!(StreamFormat::new(192_000, 8).is_ok());
        let malformed = AudioBlock {
            format: StreamFormat {
                sample_rate: 48_000,
                channels: 0,
            },
            position: position(),
            samples: vec![0.0],
        };
        assert_eq!(malformed.validate(), Err(AudioError::InvalidChannels(0)));
    }

    #[test]
    fn counts_frames_not_interleaved_samples() {
        let format = StreamFormat::new(48_000, 2).unwrap();
        let block = AudioBlock::new(format, position(), vec![0.0; 960]).unwrap();
        assert_eq!(block.frames(), 480);
        assert_eq!(block.position, position());
        assert_eq!(
            AudioBlock::new(format, position(), vec![0.0; 3]),
            Err(AudioError::IncompleteFrame {
                samples: 3,
                channels: 2
            })
        );
        assert_eq!(
            AudioBlock::new(format, position(), vec![]),
            Err(AudioError::EmptyBlock)
        );
    }

    #[test]
    fn bounds_owned_block_memory_to_two_seconds() {
        let format = StreamFormat::new(8_000, 2).unwrap();
        assert!(AudioBlock::new(format, position(), vec![0.0; 32_000]).is_ok());
        assert_eq!(
            AudioBlock::new(format, position(), vec![0.0; 32_002]),
            Err(AudioError::BlockTooLong {
                frames: 16_001,
                max_frames: 16_000
            })
        );
    }

    #[test]
    fn rejects_frame_and_time_endpoint_overflow() {
        let format = StreamFormat::new(48_000, 1).unwrap();
        let mut start = position();
        start.first_frame = u64::MAX;
        assert_eq!(
            AudioBlock::new(format, start, vec![0.0]),
            Err(AudioError::FrameRangeOverflow)
        );
        start.first_frame = 0;
        start.host_time_ns = u64::MAX;
        assert_eq!(
            AudioBlock::new(format, start, vec![0.0]),
            Err(AudioError::HostTimeOverflow)
        );
    }

    #[test]
    fn converts_pcm_rails_clamps_and_rounds_ties_away_from_zero() {
        let half_lsb = 0.5 / 8_388_608.0;
        assert_eq!(
            pcm24(&[-2.0, -1.0, -0.5, -half_lsb, 0.0, half_lsb, 0.5, 1.0, 2.0]).unwrap(),
            vec![
                -8_388_608, -8_388_608, -4_194_304, -1, 0, 1, 4_194_304, 8_388_607, 8_388_607
            ]
        );
    }

    #[test]
    fn rejects_non_finite_pcm_without_publishing_partial_output() {
        for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert_eq!(
                pcm24(&[0.0, value]),
                Err(AudioError::NonFiniteSample { index: 1 })
            );
        }
    }
}
