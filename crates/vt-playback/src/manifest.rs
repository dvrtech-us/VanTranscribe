use crate::{PlaybackError, Result};
use cap_std::{ambient_authority, fs::Dir};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use sha2::{Digest, Sha256};
use std::{
    io::{Cursor, Read},
    path::Path,
    time::Duration,
};
use vt_core::StreamFormat;

pub(crate) const MAX_CHUNKS: usize = 100_000;
const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, Debug)]
pub(crate) struct Chunk {
    pub start_ns: u64,
    pub end_ns: u64,
    pub rate: u32,
    pub channels: u16,
    pub frames: u64,
    pub path: Option<String>,
    pub hash: Option<String>,
}
pub(crate) struct Manifest {
    dir: Dir,
    pub meeting: String,
    pub duration_ns: u64,
    pub tracks: [Vec<Chunk>; 2],
}
impl Manifest {
    pub fn open(root: &Path, meeting: &str) -> Result<Self> {
        if meeting.is_empty()
            || meeting.len() > 64
            || !meeting
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
        {
            return Err(PlaybackError::Invalid("Invalid meeting identity.".into()));
        }
        let root = root.canonicalize()?;
        let db = Connection::open_with_flags(
            root.join("vantranscribe.db"),
            OpenFlags::SQLITE_OPEN_READ_ONLY
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )?;
        db.busy_timeout(Duration::from_secs(5))?;
        let exists: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM meetings WHERE id=?1)",
            [meeting],
            |row| row.get(0),
        )?;
        if !exists {
            return Err(PlaybackError::Invalid("Meeting not found.".into()));
        }
        let dir = Dir::open_ambient_dir(&root, ambient_authority())?;
        let mut q=db.prepare("SELECT track,seq,epoch,frame_count,sample_rate,channels,start_ns,end_ns,state,path,sha256,kind FROM audio_chunks WHERE meeting_id=?1 ORDER BY track,start_ns,seq LIMIT ?2")?;
        let rows = q.query_map(rusqlite::params![meeting, MAX_CHUNKS as i64 + 1], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, u32>(4)?,
                r.get::<_, u16>(5)?,
                r.get::<_, i64>(6)?,
                r.get::<_, i64>(7)?,
                r.get::<_, String>(8)?,
                r.get::<_, Option<String>>(9)?,
                r.get::<_, Option<String>>(10)?,
                r.get::<_, String>(11)?,
            ))
        })?;
        let mut tracks: [Vec<Chunk>; 2] = [vec![], vec![]];
        let mut count = 0;
        let mut duration_ns = 0;
        for row in rows {
            count += 1;
            if count > MAX_CHUNKS {
                return Err(PlaybackError::Invalid(
                    "Recording exceeds the playback manifest limit.".into(),
                ));
            }
            let (track, seq, epoch, frames, rate, channels, start, end, state, path, hash, kind) =
                row?;
            let track = match track.as_str() {
                "mic" => 0,
                "system" => 1,
                _ => return Err(PlaybackError::Invalid("Unknown audio track.".into())),
            };
            StreamFormat::new(rate, channels)
                .map_err(|error| PlaybackError::Invalid(error.to_string()))?;
            if seq < 0 || epoch < 0 || start < 0 || end <= start || frames < 0 {
                return Err(PlaybackError::Invalid("Invalid audio timeline.".into()));
            }
            let start_ns = start as u64;
            let end_ns = end as u64;
            if tracks[track]
                .last()
                .is_some_and(|old| old.end_ns > start_ns)
            {
                return Err(PlaybackError::Invalid(
                    "Overlapping chunks on one audio track.".into(),
                ));
            }
            let (path, hash) = match (kind.as_str(), state.as_str()) {
                ("audio", "ready") => {
                    if frames == 0
                        || frames > i64::from(rate) * 2
                        || end_ns - start_ns != frames as u64 * 1_000_000_000 / u64::from(rate)
                    {
                        return Err(PlaybackError::Invalid(
                            "Audio chunk exceeds its native frame/time bounds.".into(),
                        ));
                    }
                    let expected = format!(
                        "meetings/{meeting}/audio/{}/{epoch}-{seq}.flac",
                        if track == 0 { "mic" } else { "system" }
                    );
                    if path.as_deref() != Some(&expected)
                        || hash.as_deref().is_none_or(|hash| {
                            hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit())
                        })
                    {
                        return Err(PlaybackError::Invalid(
                            "Audio path or hash does not match the manifest identity.".into(),
                        ));
                    }
                    (path, hash)
                }
                ("gap", "gap") => (None, None),
                // Pending audio must be recovered by the recorder, never played as verified speech.
                ("audio", "pending") => {
                    return Err(PlaybackError::Invalid(
                        "Recording contains unpublished audio; recover it before playback.".into(),
                    ));
                }
                _ => {
                    return Err(PlaybackError::Invalid(
                        "Invalid audio publication state.".into(),
                    ));
                }
            };
            tracks[track].push(Chunk {
                start_ns,
                end_ns,
                rate,
                channels,
                frames: frames as u64,
                path,
                hash,
            });
            duration_ns = duration_ns.max(end_ns);
        }
        // Saved meeting duration includes intentional leading/trailing silence and pause intervals.
        let has_app: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='app_meetings')",
            [],
            |r| r.get(0),
        )?;
        if has_app {
            let duration: Option<i64> = db
                .query_row(
                    "SELECT duration_ms FROM app_meetings WHERE id=?1",
                    [meeting],
                    |r| r.get(0),
                )
                .optional()?;
            if let Some(duration) = duration.filter(|d| *d >= 0) {
                duration_ns =
                    duration_ns.max((duration as u64).checked_mul(1_000_000).ok_or_else(|| {
                        PlaybackError::Invalid("Meeting duration overflow.".into())
                    })?);
            }
        }
        if duration_ns == 0 {
            return Err(PlaybackError::Invalid(
                "This recording has no playable timeline.".into(),
            ));
        }
        Ok(Self {
            dir,
            meeting: meeting.into(),
            duration_ns,
            tracks,
        })
    }
    pub fn decode(&self, chunk: &Chunk) -> Result<Vec<[f32; 2]>> {
        let path = chunk
            .path
            .as_ref()
            .ok_or_else(|| PlaybackError::Invalid("A gap has no encoded audio.".into()))?;
        // Dir::open resolves beneath the held root capability, including symlinks and races.
        let file = self.dir.open(path)?;
        let meta = file.metadata()?;
        if !meta.is_file() || meta.len() > MAX_FILE_BYTES {
            return Err(PlaybackError::Invalid(
                "Encoded audio chunk is not a bounded regular file.".into(),
            ));
        }
        let mut bytes = Vec::with_capacity(meta.len() as usize);
        file.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_FILE_BYTES
            || format!("{:x}", Sha256::digest(&bytes))
                != chunk.hash.as_deref().unwrap_or("").to_lowercase()
        {
            return Err(PlaybackError::Invalid(
                "Audio chunk integrity verification failed.".into(),
            ));
        }
        let mut reader = claxon::FlacReader::new(Cursor::new(bytes))
            .map_err(|e| PlaybackError::Decode(e.to_string()))?;
        let info = reader.streaminfo();
        if info.sample_rate != chunk.rate
            || info.channels != u32::from(chunk.channels)
            || info.bits_per_sample != 24
            || info.samples.is_some_and(|n| n != chunk.frames)
        {
            return Err(PlaybackError::Invalid(
                "FLAC format does not match the audio manifest.".into(),
            ));
        }
        let mut decoded = Vec::with_capacity(chunk.frames as usize);
        let mut samples = reader.samples();
        for _ in 0..chunk.frames {
            let mut frame = [0.0f32; 2];
            for channel in 0..chunk.channels {
                let sample = samples
                    .next()
                    .ok_or_else(|| PlaybackError::Decode("Truncated audio chunk.".into()))?
                    .map_err(|e| PlaybackError::Decode(e.to_string()))?
                    as f32
                    / 8_388_608.0;
                if chunk.channels == 1 {
                    frame = [sample; 2];
                } else if chunk.channels == 2 {
                    frame[usize::from(channel)] = sample;
                } else {
                    frame[0] += sample / f32::from(chunk.channels);
                    frame[1] = frame[0];
                }
            }
            decoded.push(frame);
        }
        if samples.next().is_some() {
            return Err(PlaybackError::Invalid(
                "FLAC sample count exceeds the manifest.".into(),
            ));
        }
        Ok(decoded)
    }
}

pub(crate) struct Timeline {
    pub manifest: Manifest,
    cache: [Option<(usize, Vec<[f32; 2]>)>; 2],
}
impl Timeline {
    pub fn new(manifest: Manifest) -> Self {
        Self {
            manifest,
            cache: [None, None],
        }
    }
    pub fn frame(&mut self, time_ns: u64) -> Result<[[f32; 2]; 2]> {
        let mut result = [[0.0; 2]; 2];
        for (track, output) in result.iter_mut().enumerate() {
            let chunks = &self.manifest.tracks[track];
            let index = chunks.partition_point(|chunk| chunk.end_ns <= time_ns);
            let Some(chunk) = chunks
                .get(index)
                .filter(|chunk| chunk.start_ns <= time_ns && chunk.path.is_some())
            else {
                continue;
            };
            if self.cache[track]
                .as_ref()
                .is_none_or(|(cached, _)| *cached != index)
            {
                self.cache[track] = Some((index, self.manifest.decode(chunk)?));
            }
            let frames = &self.cache[track].as_ref().expect("cache just populated").1;
            let source =
                (time_ns - chunk.start_ns) as f64 * f64::from(chunk.rate) / 1_000_000_000.0;
            let first = (source.floor() as usize).min(frames.len() - 1);
            let second = (first + 1).min(frames.len() - 1);
            let weight = (source - first as f64) as f32;
            for (channel, sample) in output.iter_mut().enumerate() {
                *sample = frames[first][channel]
                    + (frames[second][channel] - frames[first][channel]) * weight;
            }
        }
        Ok(result)
    }
}
