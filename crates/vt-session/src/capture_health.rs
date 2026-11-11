//! Capture helper diagnostics and session-loop timing, per recording.
//!
//! The helper reports heartbeats, device-change rebuilds, slow Core Audio calls
//! and per-epoch stream facts (`vt_capture::CaptureDiagnostic`). This module
//! keeps a bounded record of them in the live run's inference health report and
//! derives a capture-stall state for the runtime view. It only observes: nothing
//! here stops, restarts or kills capture.
//!
//! Stall rules, evaluated on the session loop while recording and not paused,
//! and only once the helper has sent a heartbeat (older helpers send none):
//! - heartbeats keep arriving but the helper's IOProc `callbacks` counter has
//!   not advanced for [`STALL_NS`] -> `no_audio_callbacks`;
//! - no heartbeat for [`STALL_NS`] -> `helper_silent`;
//! - a session-loop iteration of at least [`SLOW_ITERATION_NS`] is the
//!   consumer's own delay (events wait in the queue), so it restarts both
//!   clocks instead of blaming the helper; one of at least [`STALL_NS`] is
//!   recorded as `consumer_slow` with its duration.
//!
//! Privacy: only timings, counters, enumerated reasons, API call names and the
//! input transport class are kept, never device names or UIDs.
use serde::{Deserialize, Serialize};
use vt_capture::{
    CaptureDiagnostic, DiagnosticPhase, InputTransport, RebuildReason, RebuildResult,
    RebuildTrigger,
};

/// Silence (of heartbeats or of audio callbacks) that counts as a stall.
pub(crate) const STALL_NS: u64 = 5_000_000_000;
/// Session-loop iterations at least this long are counted as slow.
pub(crate) const SLOW_ITERATION_NS: u64 = 500_000_000;
/// Entries kept per record list; totals keep counting past it.
pub(crate) const MAX_RECORDS: usize = 32;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureState {
    #[default]
    Ok,
    Stalled,
    /// The session loop, not the helper, was late (see the module docs).
    ConsumerSlow,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StallKind {
    NoAudioCallbacks,
    HelperSilent,
    ConsumerSlow,
}
/// Capture state for the runtime view.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureView {
    pub state: CaptureState,
    /// Source time (same clock as transcript and gap times) of the last
    /// evidence of healthy capture before the stall.
    pub since_source_ms: Option<u64>,
    pub kind: Option<StallKind>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RebuildRecord {
    pub source_ms: u64,
    pub reason: RebuildReason,
    pub trigger: RebuildTrigger,
    pub previous_epoch: u32,
    pub epoch: u32,
    pub trigger_age_ms: u64,
    pub fingerprint_ms: u64,
    pub stop_ms: u64,
    pub start_ms: u64,
    pub result: RebuildResult,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlowCallRecord {
    pub source_ms: u64,
    pub call: String,
    pub duration_ms: u64,
    pub phase: DiagnosticPhase,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EpochRecord {
    pub source_ms: u64,
    pub epoch: u32,
    pub sample_rate: u32,
    pub mic_channels: u16,
    pub system_channels: u16,
    pub quantum: u32,
    pub input_transport: InputTransport,
    #[serde(default)]
    pub settle_ms: Option<u64>,
    #[serde(default)]
    pub settle_stable: Option<bool>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StallRecord {
    pub kind: StallKind,
    pub since_source_ms: u64,
    /// `None` while the stall is still open (or was open when recording ended).
    pub duration_ms: Option<u64>,
}

/// Bounded capture evidence for one recording, persisted with the live run's
/// inference health report.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CaptureHealth {
    pub rebuilds: Vec<RebuildRecord>,
    pub rebuilds_total: u64,
    pub slow_calls: Vec<SlowCallRecord>,
    pub slow_calls_total: u64,
    pub epochs: Vec<EpochRecord>,
    pub epochs_total: u64,
    pub stalls: Vec<StallRecord>,
    pub stalls_total: u64,
    pub heartbeats: u64,
    pub max_emit_ms: u64,
    pub max_queued_packets: u64,
    /// Session-loop iterations of at least 500 ms, and the longest one.
    pub slow_iterations: u64,
    pub max_iteration_ms: u64,
    /// Some records were not kept because a list reached [`MAX_RECORDS`].
    pub truncated: bool,
}
impl CaptureHealth {
    fn keep<T>(list: &mut Vec<T>, total: &mut u64, truncated: &mut bool, item: T) {
        *total += 1;
        if list.len() < MAX_RECORDS {
            list.push(item);
        } else {
            *truncated = true;
        }
    }
    /// Records a non-heartbeat diagnostic. Returns whether anything changed.
    pub(crate) fn record(&mut self, source_ms: u64, diagnostic: &CaptureDiagnostic) -> bool {
        let ms = |ns: u64| ns / 1_000_000;
        match diagnostic {
            CaptureDiagnostic::Rebuild {
                reason,
                trigger,
                previous_epoch,
                epoch,
                trigger_age_ns,
                fingerprint_ns,
                stop_ns,
                start_ns,
                result,
            } => Self::keep(
                &mut self.rebuilds,
                &mut self.rebuilds_total,
                &mut self.truncated,
                RebuildRecord {
                    source_ms,
                    reason: *reason,
                    trigger: *trigger,
                    previous_epoch: *previous_epoch,
                    epoch: *epoch,
                    trigger_age_ms: ms(*trigger_age_ns),
                    fingerprint_ms: ms(*fingerprint_ns),
                    stop_ms: ms(*stop_ns),
                    start_ms: ms(*start_ns),
                    result: *result,
                },
            ),
            CaptureDiagnostic::SlowCall {
                call,
                duration_ns,
                phase,
            } => Self::keep(
                &mut self.slow_calls,
                &mut self.slow_calls_total,
                &mut self.truncated,
                SlowCallRecord {
                    source_ms,
                    call: call.chars().take(64).collect(),
                    duration_ms: ms(*duration_ns),
                    phase: *phase,
                },
            ),
            CaptureDiagnostic::Epoch {
                epoch,
                sample_rate,
                mic_channels,
                system_channels,
                quantum,
                input_transport,
            } => Self::keep(
                &mut self.epochs,
                &mut self.epochs_total,
                &mut self.truncated,
                EpochRecord {
                    source_ms,
                    epoch: *epoch,
                    sample_rate: *sample_rate,
                    mic_channels: *mic_channels,
                    system_channels: *system_channels,
                    quantum: *quantum,
                    input_transport: *input_transport,
                    settle_ms: None,
                    settle_stable: None,
                },
            ),
            CaptureDiagnostic::Settled {
                epoch,
                settle_ns,
                stable,
                ..
            } => {
                let Some(record) = self.epochs.iter_mut().rev().find(|e| e.epoch == *epoch) else {
                    return false;
                };
                record.settle_ms = Some(ms(*settle_ns));
                record.settle_stable = Some(*stable);
            }
            CaptureDiagnostic::Heartbeat { .. } | CaptureDiagnostic::Unknown => return false,
        }
        true
    }
}

/// Derives the capture-stall state from heartbeats and session-loop timing.
/// All times are monotonic nanoseconds supplied by the caller.
#[derive(Default)]
pub(crate) struct CaptureMonitor {
    last_iteration: Option<u64>,
    heartbeat_at: Option<u64>,
    heartbeat_source_ms: u64,
    callbacks: Option<u64>,
    advanced_at: u64,
    advanced_source_ms: u64,
    /// Open stall: its index in `CaptureHealth::stalls` (if kept) and start.
    open: Option<(Option<usize>, u64)>,
    view: CaptureView,
}
impl CaptureMonitor {
    pub(crate) fn view(&self) -> &CaptureView {
        &self.view
    }
    fn open(
        &mut self,
        kind: StallKind,
        since_source_ms: u64,
        now: u64,
        health: &mut CaptureHealth,
    ) {
        let index = (health.stalls.len() < MAX_RECORDS).then_some(health.stalls.len());
        CaptureHealth::keep(
            &mut health.stalls,
            &mut health.stalls_total,
            &mut health.truncated,
            StallRecord {
                kind,
                since_source_ms,
                duration_ms: None,
            },
        );
        self.open = Some((index, now));
        self.view = CaptureView {
            state: CaptureState::Stalled,
            since_source_ms: Some(since_source_ms),
            kind: Some(kind),
        };
    }
    fn close(&mut self, now: u64, health: &mut CaptureHealth) -> bool {
        let Some((index, started)) = self.open.take() else {
            return false;
        };
        if let Some(record) = index.and_then(|i| health.stalls.get_mut(i)) {
            record.duration_ms = Some(now.saturating_sub(started) / 1_000_000);
        }
        self.view = CaptureView::default();
        true
    }
    fn restart_clocks(&mut self, now: u64) {
        if self.heartbeat_at.is_some() {
            self.heartbeat_at = Some(now);
        }
        self.advanced_at = now;
    }
    /// Called at the top of every session-loop iteration. Returns whether the
    /// view or the health record changed.
    pub(crate) fn iteration(
        &mut self,
        now: u64,
        source_ms: u64,
        health: &mut CaptureHealth,
    ) -> bool {
        let Some(last) = self.last_iteration.replace(now) else {
            self.restart_clocks(now);
            return false;
        };
        let duration = now.saturating_sub(last);
        if duration < SLOW_ITERATION_NS {
            return false;
        }
        health.slow_iterations += 1;
        health.max_iteration_ms = health.max_iteration_ms.max(duration / 1_000_000);
        // Heartbeats and audio wait in the queue while the loop is late; judge
        // the helper only on what arrives after the loop catches up.
        self.restart_clocks(now);
        if duration >= STALL_NS && self.open.is_none() {
            CaptureHealth::keep(
                &mut health.stalls,
                &mut health.stalls_total,
                &mut health.truncated,
                StallRecord {
                    kind: StallKind::ConsumerSlow,
                    since_source_ms: source_ms,
                    duration_ms: Some(duration / 1_000_000),
                },
            );
            self.view = CaptureView {
                state: CaptureState::ConsumerSlow,
                since_source_ms: Some(source_ms),
                kind: Some(StallKind::ConsumerSlow),
            };
        }
        true
    }
    /// A helper heartbeat. Returns whether the view or the record changed.
    pub(crate) fn heartbeat(
        &mut self,
        now: u64,
        source_ms: u64,
        diagnostic: &CaptureDiagnostic,
        health: &mut CaptureHealth,
    ) -> bool {
        let CaptureDiagnostic::Heartbeat {
            callbacks,
            queued_packets,
            max_emit_ns,
            ..
        } = diagnostic
        else {
            return false;
        };
        health.heartbeats += 1;
        health.max_emit_ms = health.max_emit_ms.max(max_emit_ns / 1_000_000);
        health.max_queued_packets = health.max_queued_packets.max(*queued_packets);
        self.heartbeat_at = Some(now);
        self.heartbeat_source_ms = source_ms;
        let advanced = self.callbacks.is_none_or(|old| *callbacks != old);
        if self.callbacks.is_none() {
            self.advanced_at = now;
        }
        self.callbacks = Some(*callbacks);
        if advanced {
            self.advanced_at = now;
            self.advanced_source_ms = source_ms;
        }
        let mut changed = false;
        if self.view.state == CaptureState::ConsumerSlow && advanced {
            self.view = CaptureView::default();
            changed = true;
        }
        if self.open.is_some() && (advanced || self.view.kind == Some(StallKind::HelperSilent)) {
            changed |= self.close(now, health);
        }
        changed
    }
    /// Evaluates the stall rules. `active` is recording and not paused or
    /// stopping. Returns whether the view or the record changed.
    pub(crate) fn evaluate(&mut self, now: u64, active: bool, health: &mut CaptureHealth) -> bool {
        if !active {
            self.restart_clocks(now);
            let changed = self.close(now, health) || self.view != CaptureView::default();
            self.view = CaptureView::default();
            return changed;
        }
        let Some(heartbeat_at) = self.heartbeat_at else {
            return false;
        };
        if self.open.is_some() {
            return false;
        }
        if now.saturating_sub(heartbeat_at) >= STALL_NS {
            let since = self.heartbeat_source_ms;
            self.open(StallKind::HelperSilent, since, now, health);
            return true;
        }
        if now.saturating_sub(self.advanced_at) >= STALL_NS {
            let since = self.advanced_source_ms;
            self.open(StallKind::NoAudioCallbacks, since, now, health);
            return true;
        }
        false
    }
    /// Recording ended: an open stall keeps `duration_ms: None` (still open).
    pub(crate) fn finish(&mut self) {
        self.open = None;
        self.view = CaptureView::default();
    }
}

#[cfg(test)]
#[path = "capture_health_tests.rs"]
mod tests;
