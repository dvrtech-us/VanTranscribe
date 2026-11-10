//! Source-watermark load shedding. This module never changes durable audio.
use crate::{AppService, Result, store};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::{
    sync::{Arc, mpsc},
    thread,
    time::Duration,
};
use vt_core::TrackKind;

pub(crate) const LAG_LIMIT_MS: u64 = 10_000;
const CONFIRM_NS: u64 = 2_000_000_000;
const STAGE_NS: u64 = 5_000_000_000;
const FRESH_SOURCE_NS: u64 = 2_000_000_000;
const MAX_GAPS: usize = 128;
const AUDIT_WARNING: &str =
    "Live inference history could not be saved; processing gaps may be incomplete";
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BacklogStage {
    #[default]
    Normal,
    PartialsDisabled,
    SpeakersDisabled,
    SttDisabled,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct BacklogSnapshot {
    pub stage: BacklogStage,
    pub lag_ms: u64,
    pub partials_enabled: bool,
    pub speakers_enabled: bool,
    pub stt_enabled: bool,
    pub warning: Option<String>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Worker {
    Dsp,
    Stt,
    Speakers,
}
impl Worker {
    fn index(self) -> usize {
        match self {
            Self::Dsp => 0,
            Self::Stt => 1,
            Self::Speakers => 2,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InferenceGap {
    pub worker: String,
    pub track: String,
    pub generation: u64,
    pub start_ms: u64,
    pub end_ms: u64,
    pub reason: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SheddingEvent {
    pub stage: BacklogStage,
    pub source_ms: u64,
    pub lag_ms: u64,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct BacklogReport {
    #[serde(default)]
    pub complete: bool,
    pub stages: Vec<SheddingEvent>,
    pub gaps: Vec<InferenceGap>,
    pub warnings: Vec<String>,
    pub truncated: bool,
    pub max_lag_ms: u64,
}
#[derive(Clone, Default)]
struct Track {
    start: Option<u64>,
    end: Option<u64>,
    epoch: Option<u32>,
    completed: [Option<u64>; 3],
}
#[derive(Default)]
pub(crate) struct Controller {
    tracks: [Track; 2],
    configured: [bool; 3],
    enabled: [bool; 3],
    disk_suspended: bool,
    audit_warning_unpersisted: bool,
    pub(crate) generation: u64,
    stage: BacklogStage,
    paused: bool,
    source_tick: Option<u64>,
    overload_since: Option<u64>,
    last_stage: Option<u64>,
    lag_ms: u64,
    report: BacklogReport,
    revision: u64,
}
fn index(track: TrackKind) -> usize {
    usize::from(track == TrackKind::System)
}
fn worker_name(worker: Worker) -> &'static str {
    match worker {
        Worker::Dsp => "dsp",
        Worker::Stt => "stt",
        Worker::Speakers => "speakers",
    }
}
impl Controller {
    pub(crate) fn configure(&mut self, stt: bool, speakers: bool) {
        *self = Self {
            configured: [stt || speakers, stt, speakers],
            enabled: [stt || speakers, stt, speakers],
            ..Default::default()
        };
    }
    pub(crate) fn allowed(&self, worker: Worker) -> bool {
        !self.disk_suspended
            && self.enabled[worker.index()]
            && match worker {
                Worker::Stt => self.stage < BacklogStage::SttDisabled,
                Worker::Speakers => self.stage < BacklogStage::SpeakersDisabled,
                Worker::Dsp => self.allowed(Worker::Stt) || self.allowed(Worker::Speakers),
            }
    }
    pub(crate) fn partials(&self) -> bool {
        self.allowed(Worker::Stt) && self.stage < BacklogStage::PartialsDisabled
    }
    pub(crate) fn current(&self, generation: u64) -> bool {
        self.generation == generation && !self.paused
    }
    pub(crate) fn stopped(&mut self, worker: Worker) {
        self.enabled[worker.index()] = false;
        self.overload_since = None;
    }
    pub(crate) fn suspend_disk(&mut self) {
        if !self.disk_suspended {
            for worker in [Worker::Dsp, Worker::Stt, Worker::Speakers] {
                if self.configured[worker.index()] {
                    self.defer_pending(worker, "disk_pressure");
                }
            }
            self.disk_suspended = true;
            self.overload_since = None;
        }
    }
    pub(crate) fn raw(
        &mut self,
        track: TrackKind,
        start: u64,
        end: u64,
        epoch: u32,
        now: u64,
    ) -> u64 {
        let i = index(track);
        if self.tracks[i].epoch.is_some_and(|old| old != epoch) {
            self.reset("source_epoch", false);
        } else if self.tracks[i]
            .end
            .is_some_and(|old| start > old.saturating_add(1))
        {
            self.reset("source_discontinuity", false);
        }
        let item = &mut self.tracks[i];
        item.epoch = Some(epoch);
        item.start.get_or_insert(start);
        item.end = Some(item.end.unwrap_or(start).max(end));
        self.source_tick = Some(now);
        for worker in [Worker::Dsp, Worker::Stt, Worker::Speakers] {
            if self.configured[worker.index()] && !self.allowed(worker) {
                let reason = if self.disk_suspended {
                    "disk_pressure"
                } else if match worker {
                    Worker::Stt => self.stage >= BacklogStage::SttDisabled,
                    Worker::Speakers => self.stage >= BacklogStage::SpeakersDisabled,
                    Worker::Dsp => self.stage >= BacklogStage::SttDisabled,
                } {
                    "sustained_backlog"
                } else {
                    "worker_unavailable"
                };
                self.gap(worker, track, self.generation, start, end, reason);
            }
        }
        self.generation
    }
    pub(crate) fn complete(&mut self, worker: Worker, track: TrackKind, generation: u64, end: u64) {
        if !self.current(generation) || !self.allowed(worker) {
            return;
        }
        let item = &mut self.tracks[index(track)];
        item.completed[worker.index()] = Some(item.completed[worker.index()].unwrap_or(0).max(end));
    }
    fn lag(&self) -> u64 {
        let mut lag = 0;
        for item in &self.tracks {
            if let (Some(start), Some(end)) = (item.start, item.end) {
                for worker in [Worker::Dsp, Worker::Stt, Worker::Speakers] {
                    if self.allowed(worker) {
                        lag = lag.max(end.saturating_sub(
                            item.completed[worker.index()].unwrap_or(start).max(start),
                        ));
                    }
                }
            }
        }
        lag
    }
    pub(crate) fn evaluate(&mut self, now: u64, disk_suspended: bool) -> Option<BacklogStage> {
        if disk_suspended {
            self.suspend_disk();
        }
        self.lag_ms = self.lag();
        if self.lag_ms > self.report.max_lag_ms {
            self.report.max_lag_ms = self.lag_ms;
            self.revision += 1;
        }
        if self.paused
            || disk_suspended
            || self
                .source_tick
                .is_none_or(|tick| now.saturating_sub(tick) > FRESH_SOURCE_NS)
            || self.lag_ms <= LAG_LIMIT_MS
        {
            self.overload_since = None;
            return None;
        }
        let since = *self.overload_since.get_or_insert(now);
        if now.saturating_sub(since) < CONFIRM_NS
            || self
                .last_stage
                .is_some_and(|last| now.saturating_sub(last) < STAGE_NS)
        {
            return None;
        }
        let next = if self.partials() {
            BacklogStage::PartialsDisabled
        } else if self.allowed(Worker::Speakers) {
            BacklogStage::SpeakersDisabled
        } else if self.allowed(Worker::Stt) {
            BacklogStage::SttDisabled
        } else {
            return None;
        };
        self.stage = next;
        self.last_stage = Some(now);
        let source = self.tracks.iter().filter_map(|t| t.end).max().unwrap_or(0);
        self.report.stages.push(SheddingEvent {
            stage: next,
            source_ms: source,
            lag_ms: self.lag_ms,
        });
        self.revision += 1;
        for worker in [Worker::Stt, Worker::Speakers] {
            if self.enabled[worker.index()] && !self.allowed(worker) {
                self.defer_pending(worker, "sustained_backlog");
            }
        }
        Some(next)
    }
    pub(crate) fn reset(&mut self, reason: &str, paused: bool) {
        if self.tracks.iter().any(|track| track.end.is_some())
            && self.configured.iter().any(|enabled| *enabled)
        {
            self.warn(if reason == "pause" {
                "Recording pause reset live inference context; buffered hypotheses may be incomplete; reprocessing source audio may recover them."
            } else {
                "A source discontinuity reset live inference context; buffered hypotheses may be incomplete. Refine saved audio for complete coverage."
            });
        }
        for worker in [Worker::Dsp, Worker::Stt, Worker::Speakers] {
            if self.enabled[worker.index()] {
                self.defer_pending(worker, reason);
            }
        }
        self.generation = self.generation.wrapping_add(1);
        self.tracks = Default::default();
        self.paused = paused;
        self.source_tick = None;
        self.overload_since = None;
        self.lag_ms = 0;
    }
    pub(crate) fn resume(&mut self) {
        self.paused = false;
        self.overload_since = None;
        self.source_tick = None;
    }
    pub(crate) fn defer_pending(&mut self, worker: Worker, reason: &str) {
        for track in [TrackKind::Mic, TrackKind::System] {
            let item = &self.tracks[index(track)];
            if let (Some(start), Some(end)) = (item.start, item.end) {
                self.gap(
                    worker,
                    track,
                    self.generation,
                    item.completed[worker.index()].unwrap_or(start).max(start),
                    end,
                    reason,
                );
            }
        }
    }
    pub(crate) fn gap(
        &mut self,
        worker: Worker,
        track: TrackKind,
        generation: u64,
        start: u64,
        end: u64,
        reason: &str,
    ) {
        if end <= start {
            return;
        }
        let name = worker_name(worker);
        if let Some(old) = self.report.gaps.iter_mut().rev().find(|gap| {
            gap.worker == name
                && gap.track == track.as_str()
                && gap.generation == generation
                && gap.reason == reason
                && gap.end_ms >= start
                && end >= gap.start_ms
        }) {
            old.start_ms = old.start_ms.min(start);
            old.end_ms = old.end_ms.max(end);
        } else if self.report.gaps.len() < MAX_GAPS {
            self.report.gaps.push(InferenceGap {
                worker: name.into(),
                track: track.as_str().into(),
                generation,
                start_ms: start,
                end_ms: end,
                reason: reason.into(),
            });
        } else {
            self.report.truncated = true;
            self.warn("Detailed inference-gap history exceeded its bound; remaining deferred coverage is unknown. Raw audio is preserved.");
        }
        self.revision += 1;
    }
    pub(crate) fn warn(&mut self, message: &str) {
        if self.report.warnings.len() < 12 && !self.report.warnings.iter().any(|m| m == message) {
            self.report.warnings.push(message.into());
            self.revision += 1;
        }
    }
    pub(crate) fn snapshot(&self, disk: bool) -> BacklogSnapshot {
        let mut warning=match self.stage {
            BacklogStage::Normal=>self.report.warnings.last().cloned(),
            BacklogStage::PartialsDisabled=>Some("Live partial text is disabled after sustained inference lag. Raw audio is preserved.".into()),
            BacklogStage::SpeakersDisabled=>Some("Live speaker labels are deferred after sustained inference lag. Refine saved audio for labels.".into()),
            BacklogStage::SttDisabled=>Some("Live speech transcription is deferred after sustained inference lag. Raw audio is preserved; refine saved audio for complete transcription.".into()),
        };
        if self.audit_warning_unpersisted
            || self
                .report
                .warnings
                .iter()
                .any(|message| message == AUDIT_WARNING)
        {
            append_warning(&mut warning, AUDIT_WARNING);
        }
        BacklogSnapshot {
            stage: self.stage,
            lag_ms: self.lag_ms,
            partials_enabled: !disk && self.partials(),
            speakers_enabled: !disk && self.allowed(Worker::Speakers),
            stt_enabled: !disk && self.allowed(Worker::Stt),
            warning,
        }
    }
    pub(crate) fn report(&self) -> (u64, BacklogReport) {
        (self.revision, self.report.clone())
    }
}

fn append_warning(target: &mut Option<String>, message: &str) {
    for line in message.lines() {
        match target {
            Some(existing) if existing.lines().any(|old| old == line) => (),
            Some(existing) => {
                existing.push('\n');
                existing.push_str(line);
            }
            None => *target = Some(line.into()),
        }
    }
}
fn final_audit_failure(service: &AppService, id: &str) {
    // The ledger is optional evidence. Do not fail raw capture, and never
    // replace the existing source/capture warning with this separate failure.
    let (persisted,existing)=match store::open(&service.root) {
        Ok(db)=>match db.query_row("UPDATE app_meetings SET error=CASE WHEN COALESCE(error,'')='' THEN ?2 WHEN instr(error,?2)>0 THEN error ELSE error||char(10)||?2 END WHERE id=?1 RETURNING error",params![id,AUDIT_WARNING],|row|row.get::<_,String>(0)) {
            Ok(warning)=>(true,Some(warning)),
            Err(_)=>(false,db.query_row("SELECT error FROM app_meetings WHERE id=?1",[id],|row|row.get::<_,Option<String>>(0)).ok().flatten()),
        },
        Err(_)=>(false,None),
    };
    {
        let mut controller = service.backlog.lock().unwrap_or_else(|e| e.into_inner());
        controller.warn(AUDIT_WARNING);
        controller.audit_warning_unpersisted = !persisted;
    }
    if let Ok(mut runtime) = service.runtime.lock() {
        if let Some(current) = runtime
            .current
            .as_mut()
            .filter(|current| current.view.id == id)
        {
            if let Some(existing) = &existing {
                append_warning(&mut current.view.error, existing);
            }
            append_warning(&mut current.view.error, AUDIT_WARNING);
        }
        append_warning(&mut runtime.live_processing.backlog.warning, AUDIT_WARNING);
    }
}
impl AppService {
    pub(crate) fn audit_warning_unpersisted(&self) -> bool {
        self.backlog
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .audit_warning_unpersisted
    }

    pub(crate) fn inference_allowed(&self, worker: Worker) -> bool {
        !self
            .inference_suspended
            .load(std::sync::atomic::Ordering::Relaxed)
            && self
                .backlog
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .allowed(worker)
    }
    pub(crate) fn inference_current(&self, generation: u64) -> bool {
        self.backlog
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .current(generation)
    }
    pub(crate) fn inference_complete(
        &self,
        worker: Worker,
        track: TrackKind,
        generation: u64,
        end: u64,
    ) {
        self.backlog
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .complete(worker, track, generation, end);
    }
    pub(crate) fn inference_gap(
        &self,
        worker: Worker,
        track: TrackKind,
        generation: u64,
        start: u64,
        end: u64,
        reason: &str,
    ) {
        self.backlog
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .gap(worker, track, generation, start, end, reason);
    }
    pub(crate) fn refresh_backlog(&self) {
        let disk = self
            .inference_suspended
            .load(std::sync::atomic::Ordering::Relaxed);
        let snapshot = {
            let mut controller = self.backlog.lock().unwrap_or_else(|e| e.into_inner());
            controller.evaluate(self.resources.tick_ns(), disk);
            controller.snapshot(disk)
        };
        if let Ok(mut runtime) = self.runtime.lock() {
            if !snapshot.partials_enabled {
                runtime.partials.clear();
            }
            if !snapshot.speakers_enabled {
                runtime.live_processing.diarization_active = false;
            }
            if !snapshot.stt_enabled
                && let Some(current) = runtime.current.as_mut()
            {
                current.view.transcription_active = false;
                if snapshot.stage == BacklogStage::SttDisabled
                    && let Some(warning) = &snapshot.warning
                {
                    append_warning(&mut current.view.error, warning);
                }
            }
            runtime.live_processing.backlog = snapshot;
        }
    }
}
pub(crate) fn initialize(root: &std::path::Path) -> Result<()> {
    store::open(root)?.execute_batch(include_str!(
        "../../../documentation/features/capture/recording-health/sql/002-inference-health.sql"
    ))?;
    Ok(())
}
pub(crate) fn info(db: &Connection, run: &str) -> Result<Option<BacklogReport>> {
    let metadata = db
        .query_row(
            "SELECT kind,status,engines_json FROM app_processing_runs WHERE id=?1",
            [run],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()?;
    let Some((kind, status, engines)) = metadata else {
        return Ok(None);
    };
    let sealed = kind == "live" && status != "running";
    let expected = serde_json::from_str::<serde_json::Value>(&engines)
        .ok()
        .and_then(|value| {
            value
                .get("capture_inference_health_expected")
                .and_then(|value| value.as_bool())
        })
        .unwrap_or(false);
    let present=db.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='app_run_inference_health' AND type='table')",[],|r|r.get::<_,bool>(0))?;
    let json: Option<String> = if present {
        db.query_row(
            "SELECT info_json FROM app_run_inference_health WHERE run_id=?1",
            [run],
            |r| r.get(0),
        )
        .optional()?
    } else {
        None
    };
    let mut report: Option<BacklogReport> =
        json.map(|json| serde_json::from_str(&json)).transpose()?;
    if sealed {
        if report.is_none() && expected {
            report = Some(BacklogReport::default());
        }
        if let Some(report) = report.as_mut().filter(|report| !report.complete) {
            report.truncated = true;
            if !report
                .warnings
                .iter()
                .any(|warning| warning == AUDIT_WARNING)
            {
                if report.warnings.len() >= 12 {
                    report.warnings.pop();
                }
                report.warnings.push(AUDIT_WARNING.into());
            }
        }
    }
    Ok(report)
}

fn persist(service: &AppService, id: &str, run: &str, report: &BacklogReport) -> Result<()> {
    store::open(&service.root)?.execute("INSERT INTO app_run_inference_health(run_id,meeting_id,info_json) VALUES (?1,?2,?3) ON CONFLICT(run_id) DO UPDATE SET info_json=excluded.info_json",params![run,id,serde_json::to_string(report)?])?;
    Ok(())
}
pub(crate) struct AuditWriter {
    stop: mpsc::SyncSender<()>,
    worker: Option<thread::JoinHandle<()>>,
}
impl AuditWriter {
    pub(crate) fn start(service: Arc<AppService>, id: String, run: String) -> Result<Self> {
        let (stop, rx) = mpsc::sync_channel(1);
        let worker=thread::Builder::new().name("inference-health".into()).spawn(move||{
            let mut saved=None;
            loop {
                let finishing=rx.recv_timeout(Duration::from_secs(1))!=Err(mpsc::RecvTimeoutError::Timeout);
                let (revision,mut report)=service.backlog.lock().unwrap_or_else(|e|e.into_inner()).report();
                report.complete=finishing;
                if finishing || saved!=Some(revision) {
                    if persist(&service,&id,&run,&report).is_ok() {saved=Some(revision);}
                    else {
                        service.backlog.lock().unwrap_or_else(|e|e.into_inner()).warn("Live inference status persistence was delayed. Raw audio is preserved.");
                        if finishing {final_audit_failure(&service,&id);}
                    }
                }
                if finishing {break;}
            }
        })?;
        Ok(Self {
            stop,
            worker: Some(worker),
        })
    }
    pub(crate) fn finish(mut self) {
        let _ = self.stop.try_send(());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
impl Drop for AuditWriter {
    fn drop(&mut self) {
        let _ = self.stop.try_send(());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
#[path = "backlog_tests.rs"]
mod tests;

#[cfg(test)]
pub(crate) type FakeAccept = dyn Fn(Worker, &crate::session::Inference) -> Result<Vec<vt_stt::TranscriptEvent>>
    + Send
    + Sync;
#[cfg(test)]
pub(crate) type FakeTurns =
    dyn Fn(&crate::session::Inference) -> Result<Vec<vt_diarize::SpeakerTurn>> + Send + Sync;
#[cfg(test)]
pub(crate) struct TestWorkers {
    pub stt: bool,
    pub speakers: bool,
    pub accept: Arc<FakeAccept>,
    /// When set, the fake speaker worker publishes these turns through the real
    /// live publisher using `live_save` as its lock-contention budget.
    pub speaker_turns: Option<Arc<FakeTurns>>,
    pub live_save: store::LiveSaveBudget,
}
