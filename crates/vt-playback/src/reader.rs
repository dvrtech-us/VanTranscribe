//! Verified bounded archive access shared by saved inference and playback.
use crate::{Result, manifest::Manifest};
use std::path::Path;
use vt_core::TrackKind;

pub enum SavedAudioPacket {
    Audio {
        track: TrackKind,
        samples: Vec<f32>,
        rate: u32,
        start_ms: u64,
    },
    Gap {
        track: TrackKind,
        start_ms: u64,
        end_ms: u64,
    },
}

#[derive(Default)]
struct Cursor {
    chunk: usize,
    frame: usize,
    decoded: Option<Vec<[f32; 2]>>,
}

/// Opens no audio device. Holds at most one decoded native chunk per track and
/// returns <=20 ms mono slices in chronological order, System first on ties.
pub struct SavedAudioReader {
    manifest: Manifest,
    cursors: [Cursor; 2],
    completed_chunks: usize,
    chunk_count: usize,
}
impl SavedAudioReader {
    pub fn open(root: &Path, meeting: &str) -> Result<Self> {
        let manifest = Manifest::open(root, meeting)?;
        let chunk_count = manifest.tracks.iter().map(Vec::len).sum();
        Ok(Self {
            manifest,
            cursors: Default::default(),
            completed_chunks: 0,
            chunk_count,
        })
    }
    pub fn progress(&self) -> f64 {
        self.completed_chunks as f64 / self.chunk_count.max(1) as f64
    }
    fn next_time(&self, index: usize) -> Option<u64> {
        let cursor = &self.cursors[index];
        self.manifest.tracks[index].get(cursor.chunk).map(|chunk| {
            chunk.start_ns + cursor.frame as u64 * 1_000_000_000 / u64::from(chunk.rate)
        })
    }
    pub fn next_packet(&mut self) -> Result<Option<SavedAudioPacket>> {
        let index = match (self.next_time(0), self.next_time(1)) {
            (None, None) => return Ok(None),
            (Some(_), None) => 0,
            (None, Some(_)) => 1,
            (Some(mic), Some(system)) => usize::from(system <= mic),
        };
        let cursor = &mut self.cursors[index];
        let chunk = &self.manifest.tracks[index][cursor.chunk];
        let track = if index == 0 {
            TrackKind::Mic
        } else {
            TrackKind::System
        };
        let packet = if chunk.path.is_none() {
            SavedAudioPacket::Gap {
                track,
                start_ms: chunk.start_ns / 1_000_000,
                end_ms: chunk.end_ns / 1_000_000,
            }
        } else {
            if cursor.decoded.is_none() {
                cursor.decoded = Some(self.manifest.decode(chunk)?);
            }
            let decoded = cursor.decoded.as_ref().expect("decoded current chunk");
            let end = (cursor.frame + (chunk.rate as usize / 50).max(1)).min(decoded.len());
            let start_ms = (chunk.start_ns
                + cursor.frame as u64 * 1_000_000_000 / u64::from(chunk.rate))
                / 1_000_000;
            let samples = decoded[cursor.frame..end]
                .iter()
                .map(|frame| (frame[0] + frame[1]) * 0.5)
                .collect();
            cursor.frame = end;
            SavedAudioPacket::Audio {
                track,
                samples,
                rate: chunk.rate,
                start_ms,
            }
        };
        if chunk.path.is_none() || cursor.frame == chunk.frames as usize {
            cursor.chunk += 1;
            cursor.frame = 0;
            cursor.decoded = None;
            self.completed_chunks += 1;
        }
        Ok(Some(packet))
    }
}
