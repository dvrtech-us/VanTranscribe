//! Worker-side, native-rate FLAC recording. No API in this crate belongs in an
//! audio callback: encoding, hashing, SQLite and filesystem operations can block.
//!
//! A committed `pending` row precedes any audio file. Publication then orders
//! temp write, file sync, rename, directory sync, and the `ready` transaction.
//! Recovery validates hashes and promotes complete pending files, or records a
//! gap. An OS file lock prevents recovery from racing a live writer.

use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use flacenc::{component::BitRepr, error::Verify};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use sha2::{Digest, Sha256};
use thiserror::Error;
use vt_core::{AudioBlock, FramePosition, GapReason, StreamFormat, TrackKind, pcm24};

const SCHEMA: &str = include_str!(
    "../../../documentation/features/recording/durable-recording/sql/001-recording.sql"
);
const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
mod verbatim;

const DATABASE_LOCK_BUDGET: Duration = Duration::from_secs(5);

#[derive(Debug, Error)]
pub enum RecordError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Database(#[from] rusqlite::Error),
    #[error(transparent)]
    Audio(#[from] vt_core::AudioError),
    #[error("invalid recording: {0}")]
    Invalid(String),
    #[error("recording is in use by another writer or recovery process")]
    Busy,
    #[error("FLAC encoding failed: {0}")]
    Encoding(String),
}

pub type Result<T> = std::result::Result<T, RecordError>;

/// Initializes the recording manifest without creating a synthetic meeting.
pub fn initialize(root: impl AsRef<Path>) -> Result<()> {
    ensure_supported()?;
    fs::create_dir_all(root.as_ref())?;
    let _ = open_database(root.as_ref())?;
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chunk {
    pub track: TrackKind,
    pub seq: u64,
    pub epoch: u32,
    pub first_frame: u64,
    pub frame_count: u64,
    pub format: StreamFormat,
    pub start_ns: u64,
    pub end_ns: u64,
    pub state: String,
    pub path: Option<String>,
    pub sha256: Option<String>,
    pub gap_reason: Option<String>,
    pub clipped_samples: u64,
}

/// An observed discontinuity. `frame_count` may be zero when a device was absent
/// and the number of missing hardware frames is unknown. Its time range remains
/// explicit; the next epoch resets frame indexes to zero.
#[derive(Clone, Copy, Debug)]
pub struct Gap {
    pub epoch: u32,
    pub first_frame: u64,
    pub frame_count: u64,
    pub start_ns: u64,
    pub end_ns: u64,
    pub reason: GapReason,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct RecoveryReport {
    pub verified_chunks: usize,
    pub adopted_chunks: usize,
    pub gaps: usize,
}

/// One meeting writer; the lock is released by the OS even after SIGKILL.
/// A failed write poisons this instance: drop it and recover before continuing.
pub struct Recording {
    root: PathBuf,
    meeting: String,
    db: Connection,
    _lock: File,
    writable: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PublishStage {
    Reserved,
    TempSynced,
    Renamed,
    Committed,
}

impl Recording {
    /// Creates a new meeting under `<root>/meetings/<meeting>`. Existing meeting
    /// IDs are never overwritten. The root contains `vantranscribe.db`.
    pub fn create(root: impl AsRef<Path>, meeting: &str) -> Result<Self> {
        ensure_supported()?;
        validate_meeting_id(meeting)?;
        fs::create_dir_all(root.as_ref())?;
        let root = fs::canonicalize(root)?;
        let _lock = acquire_lock(&root, meeting)?;
        let db = open_database(&root)?;
        if db
            .query_row("SELECT 1 FROM meetings WHERE id=?1", [meeting], |_| Ok(()))
            .optional()?
            .is_some()
        {
            return Err(RecordError::Invalid("meeting ID already exists".into()));
        }
        for track in [TrackKind::Mic, TrackKind::System] {
            fs::create_dir_all(
                root.join("meetings")
                    .join(meeting)
                    .join("audio")
                    .join(track.as_str()),
            )?;
        }
        db.execute(
            "INSERT INTO meetings(id,status) VALUES (?1,'recording')",
            [meeting],
        )?;
        Ok(Self {
            root,
            meeting: meeting.into(),
            db,
            _lock,
            writable: true,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn meeting_id(&self) -> &str {
        &self.meeting
    }

    /// Publishes one bounded worker-side block, retaining native rate/channels.
    /// FLAC is lossless relative to the documented PCM24 conversion, not `f32`.
    pub fn write_chunk(&mut self, track: TrackKind, block: &AudioBlock) -> Result<Chunk> {
        self.write_observed(track, block, |_| Ok(()))
    }

    fn write_observed(
        &mut self,
        track: TrackKind,
        block: &AudioBlock,
        mut observe: impl FnMut(PublishStage) -> Result<()>,
    ) -> Result<Chunk> {
        self.ensure_writable()?;
        // Revalidate because AudioBlock fields are public and can be modified.
        block.validate()?;
        let frames = block.frames();
        let end_ns = block
            .position
            .host_time_ns
            .checked_add(duration_ns(frames, block.format.sample_rate)?)
            .ok_or_else(|| RecordError::Invalid("timestamp overflow".into()))?;
        let seq = self.validate_next(track, block.format, block.position, frames, end_ns)?;
        let samples = pcm24(&block.samples)?;
        let clipped = block
            .samples
            .iter()
            .filter(|s| **s < -1.0 || **s > 1.0)
            .count() as u64;
        let bytes = encode(&samples, block.format)?;
        let relative = format!(
            "meetings/{}/audio/{}/{}-{}.flac",
            self.meeting,
            track.as_str(),
            block.position.epoch,
            seq
        );
        let chunk = Chunk {
            track,
            seq,
            epoch: block.position.epoch,
            first_frame: block.position.first_frame,
            frame_count: frames,
            format: block.format,
            start_ns: block.position.host_time_ns,
            end_ns,
            state: "pending".into(),
            path: Some(relative),
            sha256: Some(hash(&bytes)),
            gap_reason: None,
            clipped_samples: clipped,
        };
        // From this point a failure requires recovery; never continue over a
        // reserved range or allow finish() to hide an unresolved publication.
        self.writable = false;
        self.insert(&chunk, "audio")?;
        observe(PublishStage::Reserved)?;
        let path = self.chunk_path(&chunk)?;
        let temp = path.with_extension("flac.partial");
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        observe(PublishStage::TempSynced)?;
        // create_new above and unique manifest sequence prevent overwrite by a
        // second cooperating writer (the meeting lock serializes access).
        if path.try_exists()? {
            return Err(RecordError::Invalid(
                "chunk destination already exists".into(),
            ));
        }
        fs::rename(&temp, &path)?;
        sync_directory(path.parent().expect("chunk has a parent"))?;
        observe(PublishStage::Renamed)?;
        self.db.execute(
            "UPDATE audio_chunks SET state='ready' WHERE meeting_id=?1 AND track=?2 AND seq=?3",
            params![self.meeting, track.as_str(), to_sql(seq)?],
        )?;
        observe(PublishStage::Committed)?;
        self.writable = true;
        Ok(Chunk {
            state: "ready".into(),
            ..chunk
        })
    }

    pub fn write_gap(&mut self, track: TrackKind, format: StreamFormat, gap: Gap) -> Result<Chunk> {
        self.ensure_writable()?;
        StreamFormat::new(format.sample_rate, format.channels)?;
        let seq = self.validate_next(
            track,
            format,
            FramePosition {
                epoch: gap.epoch,
                first_frame: gap.first_frame,
                host_time_ns: gap.start_ns,
            },
            gap.frame_count,
            gap.end_ns,
        )?;
        let chunk = Chunk {
            track,
            seq,
            epoch: gap.epoch,
            first_frame: gap.first_frame,
            frame_count: gap.frame_count,
            format,
            start_ns: gap.start_ns,
            end_ns: gap.end_ns,
            state: "gap".into(),
            path: None,
            sha256: None,
            gap_reason: Some(gap.reason.as_str().into()),
            clipped_samples: 0,
        };
        self.writable = false;
        self.insert(&chunk, "gap")?;
        self.writable = true;
        Ok(chunk)
    }

    /// Flushes no hidden buffers: every successful write was already committed.
    /// `processing` means recording ended and audio is available for later jobs.
    pub fn finish(mut self) -> Result<()> {
        self.ensure_writable()?;
        self.db.execute(
            "UPDATE meetings SET status='processing' WHERE id=?1",
            [&self.meeting],
        )?;
        self.writable = false;
        // A duplicated/inherited descriptor can outlive this consumed writer.
        // Release the shared flock after the final commit rather than waiting
        // for every alias to close. No recorder operation follows this unlock.
        self._lock.unlock()?;
        Ok(())
    }

    pub fn chunks(&self) -> Result<Vec<Chunk>> {
        read_chunks(&self.db, &self.meeting)
    }

    /// Validates completed files and reconciles interrupted publications. Does
    /// not resume capture, delete damaged files, or infer an unobserved tail.
    pub fn recover(root: impl AsRef<Path>, meeting: &str) -> Result<RecoveryReport> {
        ensure_supported()?;
        validate_meeting_id(meeting)?;
        let root = fs::canonicalize(root)?;
        if !root.join("vantranscribe.db").is_file() || !root.join("meetings").join(meeting).is_dir()
        {
            return Err(RecordError::Invalid("recording does not exist".into()));
        }
        let _lock = acquire_lock(&root, meeting)?;
        let db = open_database(&root)?;
        let status: String =
            db.query_row("SELECT status FROM meetings WHERE id=?1", [meeting], |r| {
                r.get(0)
            })?;
        let recording = Self {
            root,
            meeting: meeting.into(),
            db,
            _lock,
            writable: false,
        };
        let mut report = RecoveryReport::default();
        for chunk in recording.chunks()? {
            if chunk.state == "gap" {
                continue;
            }
            let path = recording.chunk_path(&chunk)?;
            let temp = path.with_extension("flac.partial");
            let digest = chunk
                .sha256
                .as_deref()
                .ok_or_else(|| RecordError::Invalid("missing chunk checksum".into()))?;
            let mut valid = valid_file(&path, digest)?;
            if !valid && chunk.state == "pending" && valid_file(&temp, digest)? {
                File::open(&temp)?.sync_all()?;
                if path.try_exists()? {
                    preserve_damaged_file(&path)?;
                }
                fs::rename(&temp, &path)?;
                sync_directory(path.parent().expect("chunk has a parent"))?;
                valid = true;
            }
            if valid {
                // A rename may have happened just before the process died.
                // Re-sync before acknowledging adoption.
                File::open(&path)?.sync_all()?;
                sync_directory(path.parent().expect("chunk has a parent"))?;
                if chunk.state == "pending" {
                    recording.db.execute("UPDATE audio_chunks SET state='ready' WHERE meeting_id=?1 AND track=?2 AND seq=?3",
                        params![meeting, chunk.track.as_str(), to_sql(chunk.seq)?])?;
                    report.adopted_chunks += 1;
                } else {
                    report.verified_chunks += 1;
                }
            } else {
                recording.db.execute("UPDATE audio_chunks SET kind='gap',state='gap',path=NULL,sha256=NULL,gap_reason='missing_or_incomplete_chunk' WHERE meeting_id=?1 AND track=?2 AND seq=?3",
                    params![meeting, chunk.track.as_str(), to_sql(chunk.seq)?])?;
                report.gaps += 1;
            }
        }
        if status == "recording" || report.gaps > 0 || report.adopted_chunks > 0 {
            recording.db.execute(
                "UPDATE meetings SET status='recovered' WHERE id=?1",
                [meeting],
            )?;
        }
        Ok(report)
    }

    fn ensure_writable(&self) -> Result<()> {
        if self.writable {
            Ok(())
        } else {
            Err(RecordError::Invalid(
                "writer requires recovery after a failed publication".into(),
            ))
        }
    }

    fn validate_next(
        &self,
        track: TrackKind,
        format: StreamFormat,
        position: FramePosition,
        frames: u64,
        end: u64,
    ) -> Result<u64> {
        let FramePosition {
            epoch,
            first_frame: first,
            host_time_ns: start,
        } = position;
        for n in [
            first,
            frames,
            start,
            end,
            first
                .checked_add(frames)
                .ok_or_else(|| RecordError::Invalid("frame range overflow".into()))?,
        ] {
            to_sql(n)?;
        }
        if end <= start {
            return Err(RecordError::Invalid("empty or reversed time range".into()));
        }
        let previous = self.db.query_row(
            "SELECT track,seq,epoch,first_frame,frame_count,sample_rate,channels,start_ns,end_ns,state,path,sha256,gap_reason,clipped_samples FROM audio_chunks WHERE meeting_id=?1 AND track=?2 ORDER BY seq DESC LIMIT 1",
            params![self.meeting, track.as_str()], chunk_from_row).optional()?;
        match previous {
            None if epoch != 0 || first != 0 => Err(RecordError::Invalid(
                "first chunk must start at epoch 0, frame 0".into(),
            )),
            None => Ok(0),
            Some(p) => {
                if p.state == "pending" {
                    return Err(RecordError::Invalid(
                        "pending publication requires recovery".into(),
                    ));
                }
                if start != p.end_ns {
                    return Err(RecordError::Invalid(
                        "time ranges must join exactly; write an explicit gap for a discontinuity"
                            .into(),
                    ));
                }
                if epoch == p.epoch {
                    if format != p.format || first != p.first_frame + p.frame_count {
                        return Err(RecordError::Invalid(
                            "format or frame discontinuity requires a gap/new epoch".into(),
                        ));
                    }
                } else if p.epoch.checked_add(1) != Some(epoch) || first != 0 {
                    return Err(RecordError::Invalid(
                        "new epochs must increment by one and reset frame indexes".into(),
                    ));
                }
                Ok(p.seq + 1)
            }
        }
    }

    fn insert(&self, c: &Chunk, kind: &str) -> Result<()> {
        self.db.execute("INSERT INTO audio_chunks(meeting_id,track,seq,epoch,first_frame,frame_count,sample_rate,channels,start_ns,end_ns,kind,state,path,sha256,gap_reason,clipped_samples) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)",
            params![self.meeting,c.track.as_str(),to_sql(c.seq)?,c.epoch,to_sql(c.first_frame)?,to_sql(c.frame_count)?,c.format.sample_rate,c.format.channels,to_sql(c.start_ns)?,to_sql(c.end_ns)?,kind,c.state,c.path,c.sha256,c.gap_reason,to_sql(c.clipped_samples)?])?;
        Ok(())
    }

    fn chunk_path(&self, chunk: &Chunk) -> Result<PathBuf> {
        let expected = format!(
            "meetings/{}/audio/{}/{}-{}.flac",
            self.meeting,
            chunk.track.as_str(),
            chunk.epoch,
            chunk.seq
        );
        if chunk.path.as_deref() != Some(expected.as_str()) {
            return Err(RecordError::Invalid(
                "manifest chunk path does not match its identity".into(),
            ));
        }
        Ok(self.root.join(expected))
    }
}

fn validate_meeting_id(id: &str) -> Result<()> {
    if id.is_empty()
        || id.len() > 64
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(RecordError::Invalid(
            "meeting ID must be 1..64 ASCII letters, digits, '-' or '_'".into(),
        ));
    }
    Ok(())
}

fn acquire_lock(root: &Path, meeting: &str) -> Result<File> {
    let dir = root.join("meetings").join(meeting);
    fs::create_dir_all(&dir)?;
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(dir.join(".recording.lock"))?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(std::fs::TryLockError::WouldBlock) => Err(RecordError::Busy),
        Err(std::fs::TryLockError::Error(e)) => Err(e.into()),
    }
}

fn open_database(root: &Path) -> Result<Connection> {
    open_database_with_budget(root, DATABASE_LOCK_BUDGET)
}

fn transient_database_lock(error: &RecordError) -> bool {
    matches!(error, RecordError::Database(rusqlite::Error::SqliteFailure(error, _))
        if matches!(error.code, rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked))
}

fn open_database_with_budget(root: &Path, budget: Duration) -> Result<Connection> {
    let deadline = Instant::now() + budget;
    let mut backoff = Duration::from_millis(1);
    loop {
        match open_database_attempt(root, deadline) {
            Ok(db) => return Ok(db),
            Err(error) if transient_database_lock(&error) => {
                // WAL transition can return BUSY without invoking SQLite's busy
                // handler. The failed attempt has already dropped its connection
                // and rolled back any schema transaction, releasing upgrade locks.
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Err(error);
                }
                std::thread::sleep(backoff.min(remaining));
                if Instant::now() >= deadline {
                    return Err(error);
                }
                backoff = (backoff * 2).min(Duration::from_millis(25));
            }
            Err(error) => return Err(error),
        }
    }
}

fn open_database_attempt(root: &Path, deadline: Instant) -> Result<Connection> {
    let mut db = Connection::open(root.join("vantranscribe.db"))?;
    let remaining = || deadline.saturating_duration_since(Instant::now());
    // Install the handler before any pragma or schema access, then refresh its
    // allowance before each potentially blocking phase. Retries share one total
    // monotonic budget instead of restarting a five-second timeout per attempt.
    db.busy_timeout(remaining())?;
    db.pragma_update(None, "foreign_keys", "ON")?;
    db.busy_timeout(remaining())?;
    let mode: String = db.pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get(0))?;
    if !mode.eq_ignore_ascii_case("wal") {
        return Err(RecordError::Invalid(
            "SQLite could not enable WAL for the recording manifest".into(),
        ));
    }
    db.busy_timeout(remaining())?;
    db.pragma_update(None, "synchronous", "FULL")?;
    db.busy_timeout(remaining())?;
    db.pragma_update(None, "fullfsync", "ON")?;
    db.busy_timeout(remaining())?;
    // Keep version inspection and every schema statement in one rollback-safe
    // transaction. A retry never retains a partial schema or an upgrade lock.
    let transaction = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    transaction.busy_timeout(remaining())?;
    let version: u32 = transaction.pragma_query_value(None, "user_version", |row| row.get(0))?;
    match version {
        0 => transaction.execute_batch(SCHEMA)?,
        1 => (),
        _ => {
            return Err(RecordError::Invalid(format!(
                "unsupported manifest version {version}"
            )));
        }
    }
    transaction.busy_timeout(remaining())?;
    transaction.commit()?;
    // Normal recording/publication operations retain their original busy wait;
    // they are deliberately not retried after an uncertain reservation outcome.
    db.busy_timeout(DATABASE_LOCK_BUDGET)?;
    Ok(db)
}

fn read_chunks(db: &Connection, meeting: &str) -> Result<Vec<Chunk>> {
    let mut query = db.prepare("SELECT track,seq,epoch,first_frame,frame_count,sample_rate,channels,start_ns,end_ns,state,path,sha256,gap_reason,clipped_samples FROM audio_chunks WHERE meeting_id=?1 ORDER BY track,seq")?;
    let rows = query.query_map([meeting], chunk_from_row)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

fn chunk_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Chunk> {
    let track: String = r.get(0)?;
    Ok(Chunk {
        track: if track == "mic" {
            TrackKind::Mic
        } else {
            TrackKind::System
        },
        seq: unsigned_column(r, 1)?,
        epoch: r.get(2)?,
        first_frame: unsigned_column(r, 3)?,
        frame_count: unsigned_column(r, 4)?,
        format: StreamFormat {
            sample_rate: r.get(5)?,
            channels: r.get(6)?,
        },
        start_ns: unsigned_column(r, 7)?,
        end_ns: unsigned_column(r, 8)?,
        state: r.get(9)?,
        path: r.get(10)?,
        sha256: r.get(11)?,
        gap_reason: r.get(12)?,
        clipped_samples: unsigned_column(r, 13)?,
    })
}

fn unsigned_column(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<u64> {
    let value: i64 = row.get(index)?;
    u64::try_from(value).map_err(|_| rusqlite::Error::IntegralValueOutOfRange(index, value))
}

fn duration_ns(frames: u64, rate: u32) -> Result<u64> {
    frames
        .checked_mul(1_000_000_000)
        .map(|n| n / u64::from(rate))
        .ok_or_else(|| RecordError::Invalid("duration overflow".into()))
}

fn to_sql(n: u64) -> Result<i64> {
    n.try_into()
        .map_err(|_| RecordError::Invalid("value exceeds manifest integer range".into()))
}

fn encode(samples: &[i32], format: StreamFormat) -> Result<Vec<u8>> {
    // flacenc currently caps StreamInfo at 96kHz; native PCM above that rate
    // uses a standards-conformant verbatim stream without resampling.
    if format.sample_rate > 96_000 {
        return verbatim::encode(samples, format);
    }
    let config = flacenc::config::Encoder::default()
        .into_verified()
        .map_err(|e| RecordError::Encoding(format!("{e:?}")))?;
    let source = || {
        flacenc::source::MemSource::from_samples(
            samples,
            format.channels as usize,
            24,
            format.sample_rate as usize,
        )
    };
    let stream = flacenc::encode_with_fixed_block_size(&config, source(), config.block_size)
        .map_err(|e| RecordError::Encoding(format!("{e:?}")))?;
    // Predictor cost estimates can retain enormous Rice residuals on ordinary
    // transients. Check the actual constructed frame sizes before serialization
    // (FLAC stores them in only 24 bits) or any pending manifest reservation.
    let frame_bound = config.block_size * usize::from(format.channels) * 3 + 64;
    let bounded = |stream: &flacenc::component::Stream| {
        let info = stream.stream_info();
        stream.frame_count() > 0
            && info.min_block_size() >= 16
            && info.min_block_size() <= info.max_block_size()
            && info.min_frame_size() <= info.max_frame_size()
            && info.max_frame_size() <= frame_bound
            && stream.count_bits().div_ceil(8) <= samples.len() * 3 + stream.frame_count() * 64 + 42
    };
    if !bounded(&stream) {
        // Re-encode in memory with bounded constant-size PCM24 subframes;
        // publication is never retried and every source sample is preserved.
        return verbatim::encode(samples, format);
    }
    let mut sink = flacenc::bitsink::ByteSink::new();
    stream
        .write(&mut sink)
        .map_err(|e| RecordError::Encoding(format!("{e:?}")))?;
    Ok(sink.as_slice().to_vec())
}

fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn valid_file(path: &Path, digest: &str) -> Result<bool> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e.into()),
    };
    if !file.metadata()?.is_file() || file.metadata()?.len() > MAX_FILE_BYTES {
        return Ok(false);
    }
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes)?;
    Ok(bytes.len() as u64 <= MAX_FILE_BYTES && hash(&bytes) == digest)
}

fn preserve_damaged_file(path: &Path) -> Result<()> {
    // The meeting lock serializes this namespace. Keep all previous evidence
    // rather than overwriting an earlier quarantine file on repeated recovery.
    for n in 0..1_000 {
        let saved = path.with_extension(format!("flac.damaged.{n}"));
        if !saved.try_exists()? {
            fs::rename(path, saved)?;
            sync_directory(path.parent().expect("chunk has a parent"))?;
            return Ok(());
        }
    }
    Err(RecordError::Invalid(
        "too many retained damaged files".into(),
    ))
}

fn ensure_supported() -> Result<()> {
    if cfg!(unix) {
        Ok(())
    } else {
        Err(RecordError::Invalid(
            "durable recording currently requires a Unix platform".into(),
        ))
    }
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<()> {
    Ok(File::open(path)?.sync_all()?)
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> Result<()> {
    Err(RecordError::Invalid(
        "durable directory publication currently requires a Unix platform".into(),
    ))
}

#[cfg(test)]
mod tests;
