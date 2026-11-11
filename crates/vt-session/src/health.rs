use crate::{
    Result,
    source_timing::{SourceTiming, TimingProvenance},
    store,
};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use std::{path::Path, time::Instant};

pub(crate) const WARN_BYTES: u64 = 2_000_000_000;
pub(crate) const SHED_BYTES: u64 = 500_000_000;
pub(crate) const STOP_BYTES: u64 = 100_000_000;
const POLL_NS: u64 = 30_000_000_000;
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiskState {
    Healthy,
    Warning,
    InferenceSuspended,
    Critical,
    #[default]
    Unknown,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct HealthSnapshot {
    pub free_bytes: Option<u64>,
    pub checked_at: Option<String>,
    pub disk_state: DiskState,
    pub inference_suspended: bool,
    pub automatic_processing_allowed: bool,
    pub warning: Option<String>,
}
impl HealthSnapshot {
    pub(crate) fn from_free(free: Option<u64>) -> Self {
        let state = match free {
            Some(n) if n < STOP_BYTES => DiskState::Critical,
            Some(n) if n < SHED_BYTES => DiskState::InferenceSuspended,
            Some(n) if n < WARN_BYTES => DiskState::Warning,
            Some(_) => DiskState::Healthy,
            None => DiskState::Unknown,
        };
        Self {
            free_bytes: free, checked_at: None, disk_state: state,
            inference_suspended:matches!(state,DiskState::InferenceSuspended|DiskState::Critical),
            automatic_processing_allowed:matches!(state,DiskState::Healthy|DiskState::Warning),
            warning:match state {
                DiskState::Healthy=>None,
                DiskState::Warning=>Some("Less than 2 GB is available for recordings.".into()),
                DiskState::InferenceSuspended=>Some("Less than 500 MB is available. Live inference is suspended; raw audio recording continues.".into()),
                DiskState::Critical=>Some("Less than 100 MB is available. Recording is stopping and saving buffered audio.".into()),
                DiskState::Unknown=>Some("Available recording disk space could not be verified.".into()),
            },
        }
    }
}
/// Injectable resource readings; fake helpers/tests never fill a real disk.
pub(crate) trait ResourceProbe: Send + Sync {
    fn free_bytes(&self, root: &Path) -> Result<u64>;
    fn uptime_ns(&self) -> Option<u64>;
    fn tick_ns(&self) -> u64;
    /// Detection freshness must include suspend time. Test probes can use their
    /// injected monotonic tick; the macOS implementation uses a continuous clock.
    fn observation_ns(&self) -> Option<u64> {
        Some(self.tick_ns())
    }
}
pub(crate) struct SystemResources {
    started: Instant,
}
impl Default for SystemResources {
    fn default() -> Self {
        Self {
            started: Instant::now(),
        }
    }
}
impl ResourceProbe for SystemResources {
    fn free_bytes(&self, root: &Path) -> Result<u64> {
        #[cfg(unix)]
        {
            let stat = nix::sys::statvfs::statvfs(root)?;
            // POSIX fsblkcnt_t is 32-bit on Darwin and may be 64-bit elsewhere.
            #[allow(clippy::useless_conversion)]
            let blocks = u64::from(stat.blocks_available());
            blocks
                .checked_mul(stat.fragment_size())
                .ok_or_else(|| anyhow::anyhow!("Disk availability exceeded the supported range"))
        }
        #[cfg(not(unix))]
        {
            let _ = root;
            anyhow::bail!("Disk availability is unsupported on this platform")
        }
    }
    fn uptime_ns(&self) -> Option<u64> {
        #[cfg(target_os = "macos")]
        {
            let time =
                nix::time::clock_gettime(nix::time::ClockId::from_raw(nix::libc::CLOCK_UPTIME_RAW))
                    .ok()?;
            let seconds = u64::try_from(time.tv_sec()).ok()?;
            seconds
                .checked_mul(1_000_000_000)?
                .checked_add(u64::try_from(time.tv_nsec()).ok()?)
        }
        #[cfg(not(target_os = "macos"))]
        {
            None
        }
    }
    fn tick_ns(&self) -> u64 {
        self.started.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64
    }
    fn observation_ns(&self) -> Option<u64> {
        #[cfg(target_os = "macos")]
        {
            // CLOCK_MONOTONIC_RAW advances during sleep; CLOCK_UPTIME_RAW and
            // Rust Instant on Apple do not. Keep source/capture clocks separate.
            let time = nix::time::clock_gettime(nix::time::ClockId::from_raw(
                nix::libc::CLOCK_MONOTONIC_RAW,
            ))
            .ok()?;
            u64::try_from(time.tv_sec())
                .ok()?
                .checked_mul(1_000_000_000)?
                .checked_add(u64::try_from(time.tv_nsec()).ok()?)
        }
        #[cfg(not(target_os = "macos"))]
        {
            // Platform detection is not implemented elsewhere yet. A future
            // backend must supply an appropriate suspend-aware clock too.
            None
        }
    }
}
#[derive(Default)]
pub(crate) struct HealthMonitor {
    last_check: Option<u64>,
    last_persist: Option<u64>,
    last_state: Option<DiskState>,
}
impl HealthMonitor {
    pub(crate) fn due(&self, now: u64) -> bool {
        self.last_check
            .is_none_or(|last| now.saturating_sub(last) >= POLL_NS)
    }
    pub(crate) fn observe(&mut self, now: u64, state: DiskState) -> bool {
        self.last_check = Some(now);
        let persist = self.last_state != Some(state)
            || self
                .last_persist
                .is_none_or(|last| now.saturating_sub(last) >= POLL_NS);
        self.last_state = Some(state);
        if persist {
            self.last_persist = Some(now);
        }
        persist
    }
}
pub(crate) fn initialize(root: &Path) -> Result<()> {
    store::open(root)?.execute_batch(include_str!(
        "../../../documentation/features/capture/recording-health/sql/001-resource-health.sql"
    ))?;
    Ok(())
}
pub(crate) fn persist(
    root: &Path,
    meeting: Option<&str>,
    source_ms: u64,
    snapshot: &HealthSnapshot,
    append: bool,
    busy_timeout: std::time::Duration,
) -> Result<()> {
    let mut db = store::open_with_timeout(root, busy_timeout)?;
    let tx = db.transaction()?;
    let now = chrono::Utc::now().to_rfc3339();
    tx.execute("INSERT INTO app_resource_health(id,snapshot_json,checked_at) VALUES (1,?1,?2) ON CONFLICT(id) DO UPDATE SET snapshot_json=excluded.snapshot_json,checked_at=excluded.checked_at",params![serde_json::to_string(snapshot)?,now])?;
    if append && let Some(meeting) = meeting {
        tx.execute("INSERT INTO app_meeting_health(meeting_id,source_ms,free_bytes,disk_state,inference_suspended,created_at) VALUES (?1,?2,?3,?4,?5,?6)",params![meeting,i64::try_from(source_ms)?,snapshot.free_bytes.map(i64::try_from).transpose()?,serde_json::to_value(snapshot.disk_state)?.as_str(),snapshot.inference_suspended,now])?;
    }
    tx.commit()?;
    Ok(())
}
pub(crate) fn timing(
    root: &Path,
    meeting: &str,
    value: &SourceTiming,
    origin: Option<u64>,
) -> Result<()> {
    timing_with(
        root,
        meeting,
        value,
        origin,
        std::time::Duration::from_secs(5),
    )
}
pub(crate) fn timing_with(
    root: &Path,
    meeting: &str,
    value: &SourceTiming,
    origin: Option<u64>,
    busy_timeout: std::time::Duration,
) -> Result<()> {
    store::open_with_timeout(root, busy_timeout)?.execute("INSERT INTO app_source_timing(meeting_id,timing_json,origin_ns) VALUES (?1,?2,?3) ON CONFLICT(meeting_id) DO UPDATE SET timing_json=excluded.timing_json,origin_ns=excluded.origin_ns",params![meeting,serde_json::to_string(value)?,origin.map(i64::try_from).transpose()?])?;
    Ok(())
}
pub(crate) fn note_timing(db: &Connection, note: &str, value: &SourceTiming) -> Result<()> {
    let provenance = match value.provenance {
        TimingProvenance::UptimeRaw => "uptime_raw",
        TimingProvenance::LatestAudioObservation => "latest_audio_observation",
        TimingProvenance::SavedPlaybackPosition => "saved_playback_position",
        TimingProvenance::UntimedNote => "untimed_note",
        TimingProvenance::Unavailable => "unavailable",
    };
    db.execute(
        "INSERT INTO app_note_timing(note_id,provenance,synchronized) VALUES (?1,?2,?3)",
        params![note, provenance, value.synchronized],
    )?;
    Ok(())
}
#[cfg(test)]
#[path = "health_tests.rs"]
mod tests;
