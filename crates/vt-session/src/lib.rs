//! Desktop session orchestration. Native capture, recording and inference run
//! on separate workers. Persistent meeting state is independent of the webview.
mod audio_preferences;
mod audio_service;
mod backlog;
mod capture_health;
mod health;
mod live_models;
mod live_transcript;
pub use backlog::{
    BacklogReport, BacklogSnapshot, BacklogStage, InferenceGap, LiveState, LiveStopReason,
    LiveTranscriptionView, SheddingEvent, WorkerFailure,
};
pub use capture_health::{
    CaptureHealth, CaptureState, CaptureView, EpochRecord, RebuildRecord, SlowCallRecord,
    StallKind, StallRecord,
};
pub use live_models::{LiveModelCatalog, LiveModelChoice};
pub use vt_stt::LiveModel;
mod maintenance_service;
mod meeting_detection;
mod model_setup;
mod preferences;
pub use meeting_detection::{
    DetectionEnded, DetectionInput, DetectionPolicy, DetectionPrompt, DetectionRule,
    DetectionSettings, DetectionSnapshot, DetectionStatus,
};
mod processing;
mod retention;
mod session;
mod source_timing;
pub use health::{DiskState, HealthSnapshot};
pub use retention::{
    DeletionKind, DeletionPreview, MaintenancePolicy, MaintenanceReport, PolicyPreview,
};
pub use source_timing::{SourceTiming, TimingProvenance};
mod speaker_pipeline;
mod voice_memory;
pub use voice_memory::EnrollVoiceRequest;
pub use vt_speakers::{MemoryStatus, PersonInfo, VoiceprintInfo};
mod speaker_reconcile;
mod store;

use anyhow::{Result, bail};
pub use audio_preferences::AudioPreferences;
pub use preferences::Preferences;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Instant,
};
use vt_core::TrackKind;
pub use vt_playback::{PlaybackSnapshot, Waveform};
use vt_stt::{ModelManager, RefinementModelManager};
pub use vt_summarize::SummaryConfig;
mod echo_pipeline;
mod echo_store;
pub use echo_store::{EchoInfo, RouteObservation};

pub use store::{
    BleedCandidate, BleedDecision, Correction, DiarizationInfo, Meeting, MeetingDetail, Note,
    RunInfo, Segment, SpeakerAssignment, SpeakerConstraint, SpeakerInfo, SpeakerReview,
    SummaryInfo, TranscriptWord,
};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CaptureMode {
    Remote,
    InPerson,
}
impl CaptureMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Remote => "remote",
            Self::InPerson => "in_person",
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct RecordingView {
    pub id: String,
    pub title: String,
    pub status: String,
    pub elapsed_ms: u64,
    pub mic_level: f32,
    pub system_level: f32,
    pub gaps: u64,
    pub transcription_configured: bool,
    pub transcription_active: bool,
    /// `None` when live transcription was not configured for this recording.
    pub live_transcription: Option<LiveTranscriptionView>,
    /// Capture-stall state derived from helper heartbeats.
    pub capture: CaptureView,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct ModelStatus {
    pub installed: bool,
    pub busy: bool,
    pub progress: f64,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Settings {
    pub meeting_detection: DetectionSettings,
    pub capture_mode: CaptureMode,
    pub data_dir: String,
    pub auto_refine: bool,
    pub auto_summarize: bool,
    pub summary_config: vt_summarize::SummaryConfig,
    pub live_vad_enabled: bool,
    pub diarization_enabled: bool,
    pub echo_cancellation_enabled: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct ProcessingView {
    pub meeting_id: String,
    pub kind: String,
    pub stage: String,
    pub progress: f64,
    pub error: Option<String>,
}
#[derive(Clone, Debug, Default, Serialize)]
pub struct LiveProcessing {
    pub backlog: BacklogSnapshot,
    pub vad_active: bool,
    pub diarization_active: bool,
    pub vad_warning: Option<String>,
    pub diarization_warning: Option<String>,
    pub output_route: Option<vt_capture::OutputRoute>,
    pub echo: Option<EchoInfo>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Snapshot {
    pub meeting_detection: DetectionSnapshot,
    pub source_timing: Option<SourceTiming>,
    pub health: HealthSnapshot,
    pub recording: Option<RecordingView>,
    pub meetings: Vec<Meeting>,
    pub models: ModelStatus,
    pub refinement_models: ModelStatus,
    pub vad_models: ModelStatus,
    pub diarization_models: ModelStatus,
    pub playback: Option<PlaybackSnapshot>,
    pub live_processing: Option<LiveProcessing>,
    pub processing: Option<ProcessingView>,
    pub settings: Settings,
}

enum Control {
    Pause,
    Resume,
    Stop,
}
struct Current {
    view: RecordingView,
    control: mpsc::Sender<Control>,
}
#[derive(Default)]
struct Runtime {
    saved_audio_readers: usize,
    source_timeline: source_timing::SourceTimeline,
    health: HealthSnapshot,
    current: Option<Current>,
    partials: BTreeMap<TrackKind, Segment>,
    /// Final live lines handed to the transcript writer but not yet saved (or
    /// dropped). Shown with saved segments so the UI sees finals at once.
    live_finals: Vec<Segment>,
    model: ModelStatus,
    refinement_model: ModelStatus,
    vad_model: ModelStatus,
    diarization_model: ModelStatus,
    processing: bool,
    processing_view: Option<ProcessingView>,
    live_processing: LiveProcessing,
}

pub struct AppService {
    meeting_detection: Mutex<meeting_detection::DetectionState>,
    detection_worker: Mutex<Option<meeting_detection::DetectionWorker>>,
    #[cfg(test)]
    test_workers: Mutex<Option<Arc<backlog::TestWorkers>>>,
    backlog: Mutex<backlog::Controller>,
    resources: Arc<dyn health::ResourceProbe>,
    health_monitor: Mutex<health::HealthMonitor>,
    inference_suspended: AtomicBool,
    resource_stop: AtomicBool,
    root: PathBuf,
    helper: PathBuf,
    runtime: Mutex<Runtime>,
    cancel_download: Arc<AtomicBool>,
    cancel_processing: Arc<AtomicBool>,
    cancel_refinement_download: Arc<AtomicBool>,
    cancel_vad_download: Arc<AtomicBool>,
    cancel_diarization_download: Arc<AtomicBool>,
    shutting_down: AtomicBool,
    preferences: Mutex<Preferences>,
    audio_preferences: Mutex<AudioPreferences>,
    playback: Mutex<Option<vt_playback::Playback>>,
    _instance_lock: std::fs::File,
}

impl AppService {
    pub fn new(root: PathBuf, helper: PathBuf) -> Result<Arc<Self>> {
        Self::with_resources(root, helper, Arc::new(health::SystemResources::default()))
    }
    pub(crate) fn with_resources(
        root: PathBuf,
        helper: PathBuf,
        resources: Arc<dyn health::ResourceProbe>,
    ) -> Result<Arc<Self>> {
        std::fs::create_dir_all(&root)?;
        let root = root.canonicalize()?;
        let instance_lock = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(root.join(".desktop.lock"))?;
        instance_lock.try_lock().map_err(|error|anyhow::anyhow!("This recordings folder is already in use, or cannot be locked. Close the other VanTranscribe instance and retry: {error}"))?;
        store::initialize(&root)?;
        health::initialize(&root)?;
        backlog::initialize(&root)?;
        maintenance_service::recover_startup(&root)?;
        store::recover_interrupted(&root)?;
        let prefs = preferences::load(&root)?;
        let audio_prefs = audio_preferences::load(&root)?;
        let model_selection = live_models::selection(&root)?;
        let installed = model_selection.warning.is_none()
            && ModelManager::for_model(root.join("models"), model_selection.model).installed();
        let refinement_installed = RefinementModelManager::new(root.join("models")).installed();
        let vad_installed =
            vt_stt::VadModelManager::new(root.join("models")).installed() || refinement_installed;
        let diarization_installed = vt_diarize::ModelManager::new(root.join("models")).installed();
        let detection = meeting_detection::DetectionState::load(&root);
        let service = Arc::new(Self {
            meeting_detection: Mutex::new(detection),
            detection_worker: Mutex::new(None),
            #[cfg(test)]
            test_workers: Mutex::new(None),
            backlog: Mutex::new(backlog::Controller::default()),
            resources,
            health_monitor: Mutex::new(health::HealthMonitor::default()),
            inference_suspended: AtomicBool::new(false),
            resource_stop: AtomicBool::new(false),
            root,
            helper,
            runtime: Mutex::new(Runtime {
                model: ModelStatus {
                    installed,
                    error: model_selection.warning,
                    ..Default::default()
                },
                refinement_model: ModelStatus {
                    installed: refinement_installed,
                    ..Default::default()
                },
                vad_model: ModelStatus {
                    installed: vad_installed,
                    ..Default::default()
                },
                diarization_model: ModelStatus {
                    installed: diarization_installed,
                    ..Default::default()
                },
                ..Default::default()
            }),
            cancel_download: Arc::new(AtomicBool::new(false)),
            cancel_processing: Arc::new(AtomicBool::new(false)),
            cancel_refinement_download: Arc::new(AtomicBool::new(false)),
            cancel_vad_download: Arc::new(AtomicBool::new(false)),
            cancel_diarization_download: Arc::new(AtomicBool::new(false)),
            shutting_down: AtomicBool::new(false),
            preferences: Mutex::new(prefs),
            audio_preferences: Mutex::new(audio_prefs),
            playback: Mutex::new(None),
            _instance_lock: instance_lock,
        });
        service.refresh_health(true)?;
        Ok(service)
    }

    pub(crate) fn refresh_health(&self, force: bool) -> Result<HealthSnapshot> {
        let tick = self.resources.tick_ns();
        if !force
            && !self
                .health_monitor
                .lock()
                .map_err(|_| anyhow::anyhow!("resource state unavailable"))?
                .due(tick)
        {
            return Ok(self
                .runtime
                .lock()
                .map_err(|_| anyhow::anyhow!("session state unavailable"))?
                .health
                .clone());
        }
        let mut snapshot = HealthSnapshot::from_free(self.resources.free_bytes(&self.root).ok());
        snapshot.checked_at = Some(chrono::Utc::now().to_rfc3339());
        let persist = self
            .health_monitor
            .lock()
            .map_err(|_| anyhow::anyhow!("resource state unavailable"))?
            .observe(tick, snapshot.disk_state);
        let (meeting, source_ms) = {
            let mut runtime = self
                .runtime
                .lock()
                .map_err(|_| anyhow::anyhow!("session state unavailable"))?;
            let active = runtime.current.as_ref().is_some_and(|c| {
                matches!(
                    c.view.status.as_str(),
                    "recording" | "paused" | "processing"
                )
            });
            if active && (snapshot.inference_suspended || snapshot.disk_state == DiskState::Unknown)
            {
                self.inference_suspended.store(true, Ordering::Relaxed);
            }
            if active && self.inference_suspended.load(Ordering::Relaxed) {
                snapshot.inference_suspended = true;
                runtime.partials.clear();
                runtime.live_processing.vad_active = false;
                runtime.live_processing.diarization_active = false;
                if let Some(current) = runtime.current.as_mut() {
                    current.view.transcription_active = false;
                }
            }
            if active && snapshot.disk_state == DiskState::Critical {
                self.resource_stop.store(true, Ordering::Relaxed);
            }
            runtime.health = snapshot.clone();
            (
                runtime
                    .current
                    .as_ref()
                    .filter(|_| active)
                    .map(|c| c.view.id.clone()),
                runtime
                    .source_timeline
                    .snapshot(self.resources.uptime_ns())
                    .elapsed_ms,
            )
        };
        if persist {
            // Periodic (unforced) checks run on the capture consumer loop, which
            // must not wait seconds for a lock; it treats lock errors as skips.
            let busy_timeout = if force {
                std::time::Duration::from_secs(5)
            } else {
                store::LOOP_BUSY_TIMEOUT
            };
            health::persist(
                &self.root,
                meeting.as_deref(),
                source_ms,
                &snapshot,
                true,
                busy_timeout,
            )?;
        }
        Ok(snapshot)
    }
    fn ensure_not_maintenance(runtime: &Runtime) -> Result<()> {
        if runtime.processing
            && runtime
                .processing_view
                .as_ref()
                .is_none_or(|job| job.kind == "maintenance")
        {
            bail!("Wait for storage maintenance to finish before changing this meeting.");
        }
        Ok(())
    }
    pub(crate) fn ensure_saved_audio(&self, id: &str) -> Result<()> {
        if retention::has_pending(&self.root, id)? {
            bail!(
                "Saved audio is unavailable while confirmed storage maintenance is pending recovery."
            );
        }
        if !retention::audio_available(&self.root, id)? {
            bail!(
                "Saved audio was removed or is unavailable. The transcript and notes remain available."
            );
        }
        Ok(())
    }

    pub(crate) fn ensure_processing_space(&self) -> Result<()> {
        let snapshot = self.refresh_health(true)?;
        if !snapshot.automatic_processing_allowed {
            bail!(
                "At least 500 MB of available recording disk space is required before processing saved audio."
            );
        }
        Ok(())
    }
    fn recording_budget(&self, encoded_upper_bound: u64) -> Result<()> {
        let snapshot = self.refresh_health(true)?;
        if let Some(free) = snapshot.free_bytes
            && free < encoded_upper_bound.saturating_add(2_000_000)
        {
            self.resource_stop.store(true, Ordering::Relaxed);
            bail!(
                "There is not enough available disk space to safely publish the buffered audio chunk."
            );
        }
        Ok(())
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn snapshot(&self) -> Result<Snapshot> {
        let runtime = self
            .runtime
            .lock()
            .map_err(|_| anyhow::anyhow!("session state unavailable"))?;
        let recording = runtime.current.as_ref().map(|current| {
            let mut view = current.view.clone();
            if matches!(view.status.as_str(), "recording" | "paused" | "processing") {
                view.elapsed_ms = runtime
                    .source_timeline
                    .snapshot(self.resources.uptime_ns())
                    .elapsed_ms;
            }
            view
        });
        let source_timing = runtime
            .current
            .as_ref()
            .map(|_| runtime.source_timeline.snapshot(self.resources.uptime_ns()));
        let health = runtime.health.clone();
        let model = runtime.model.clone();
        let refinement_model = runtime.refinement_model.clone();
        let vad_model = runtime.vad_model.clone();
        let diarization_model = runtime.diarization_model.clone();
        let processing = runtime.processing_view.clone();
        let live_processing = runtime
            .current
            .as_ref()
            .map(|_| runtime.live_processing.clone());
        let (meeting_detection, detection_settings, capture_mode) = {
            let mut detection = self
                .meeting_detection
                .lock()
                .map_err(|_| anyhow::anyhow!("Meeting detection state unavailable"))?;
            let observed_now = self.resources.observation_ns();
            if observed_now.is_none() {
                detection.clock_unavailable();
            }
            (
                detection.snapshot(
                    observed_now,
                    meeting_detection::busy(&runtime),
                    meeting_detection::recording_id(&runtime),
                    self.shutting_down.load(Ordering::Relaxed),
                ),
                detection.settings(),
                detection.mode,
            )
        };
        drop(runtime);
        let prefs = self
            .preferences
            .lock()
            .map_err(|_| anyhow::anyhow!("settings unavailable"))?
            .clone();
        let audio_prefs = self
            .audio_preferences
            .lock()
            .map_err(|_| anyhow::anyhow!("audio settings unavailable"))?
            .clone();
        let playback = self
            .playback
            .lock()
            .map_err(|_| anyhow::anyhow!("playback state unavailable"))?
            .as_ref()
            .map(vt_playback::Playback::snapshot);
        Ok(Snapshot {
            meeting_detection,
            source_timing,
            health,
            recording,
            meetings: store::meetings(&self.root)?,
            models: model,
            refinement_models: refinement_model,
            vad_models: vad_model,
            diarization_models: diarization_model,
            playback,
            live_processing,
            processing,
            settings: Settings {
                capture_mode,
                meeting_detection: detection_settings,
                data_dir: self.root.display().to_string(),
                auto_refine: prefs.auto_refine,
                auto_summarize: prefs.auto_summarize,
                summary_config: prefs.summary,
                live_vad_enabled: audio_prefs.live_vad_enabled,
                diarization_enabled: audio_prefs.diarization_enabled,
                echo_cancellation_enabled: audio_prefs.echo_cancellation_enabled,
            },
        })
    }

    pub fn start(
        self: &Arc<Self>,
        title: String,
        mode: CaptureMode,
        consent: bool,
        input_device: Option<String>,
    ) -> Result<Snapshot> {
        self.start_with_source(title, mode, consent, input_device, None)
    }
    fn start_with_source(
        self: &Arc<Self>,
        title: String,
        mode: CaptureMode,
        consent: bool,
        mut input_device: Option<String>,
        source: Option<meeting_detection::DetectedStart>,
    ) -> Result<Snapshot> {
        if self.shutting_down.load(Ordering::Relaxed) {
            bail!("VanTranscribe is closing.");
        }
        if !consent {
            bail!("Confirm that participants know this meeting is being recorded.");
        }
        if !self.helper.is_file() {
            bail!("The audio capture helper is missing. Rebuild or reinstall VanTranscribe.");
        }
        let preflight = self.refresh_health(true)?;
        if matches!(
            preflight.disk_state,
            DiskState::Critical | DiskState::Unknown
        ) {
            bail!(
                "Recording cannot start until at least 100 MB of available disk space is verified."
            );
        }
        let mut runtime = self
            .runtime
            .lock()
            .map_err(|_| anyhow::anyhow!("session state unavailable"))?;
        if self.shutting_down.load(Ordering::Relaxed) {
            bail!("VanTranscribe is closing.");
        }
        if runtime.processing
            || runtime.current.as_ref().is_some_and(|c| {
                matches!(
                    c.view.status.as_str(),
                    "recording" | "paused" | "processing"
                )
            })
        {
            bail!("A recording or transcription is already running.");
        }
        if source.is_some() && meeting_detection::busy(&runtime) {
            bail!("Finish active work and model setup before starting a detected recording.");
        }
        let detected = if let Some(request) = &source {
            let (app, input) = self
                .meeting_detection
                .lock()
                .map_err(|_| anyhow::anyhow!("Meeting detection state unavailable"))?
                .validate_start(
                    request,
                    self.resources.observation_ns(),
                    input_device.clone(),
                )?;
            input_device = input;
            Some((app, request.episode_id.clone()))
        } else {
            None
        };
        let id = uuid::Uuid::new_v4().to_string();
        // The runtime reservation prevents a racing Play command from opening
        // another output stream between this stop and the capture start.
        audio_service::stop(self)?;
        let title = if title.trim().is_empty() {
            format!("Meeting {}", chrono::Local::now().format("%b %-d, %H:%M"))
        } else {
            title.trim().chars().take(200).collect()
        };
        let engines = live_models::capture_provenance(
            &self.root,
            runtime.model.installed,
            preflight.inference_suspended,
        )?;
        let engines = if let Some((app, _)) = &detected {
            let mut value: serde_json::Value = serde_json::from_str(&engines)?;
            value["detected_source_app"] = serde_json::to_value(app)?;
            let automatic = source.as_ref().is_some_and(|request| request.automatic);
            value["recording_trigger"] = serde_json::json!(if automatic {
                "always_record"
            } else {
                "detected_prompt"
            });
            if automatic {
                value["participant_notice_attested"] = serde_json::json!(true);
            }
            serde_json::to_string(&value)?
        } else {
            engines
        };
        let recorder = vt_record::Recording::create(&self.root, &id)?;
        store::create_meeting_with_engines(&self.root, &id, &title, mode.as_str(), &engines)?;
        let (tx, rx) = mpsc::channel();
        self.inference_suspended
            .store(preflight.inference_suspended, Ordering::Relaxed);
        self.resource_stop.store(false, Ordering::Relaxed);
        runtime.source_timeline = source_timing::SourceTimeline::default();
        runtime.health = preflight;
        runtime.partials.clear();
        runtime.live_processing = LiveProcessing::default();
        let transcription_configured = runtime.model.installed;
        runtime.current = Some(Current {
            view: RecordingView {
                id: id.clone(),
                title,
                status: "recording".into(),
                elapsed_ms: 0,
                mic_level: 0.0,
                system_level: 0.0,
                gaps: 0,
                transcription_configured,
                transcription_active: false,
                live_transcription: None,
                capture: CaptureView::default(),
                error: None,
            },
            control: tx,
        });
        self.meeting_detection
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .started(
                &id,
                mode,
                detected.as_ref(),
                source.as_ref().is_some_and(|request| request.automatic),
            );
        drop(runtime);
        let service = self.clone();
        std::thread::Builder::new()
            .name("meeting-session".into())
            .spawn(move || session::run(service, id, mode, input_device, recorder, rx))?;
        self.snapshot()
    }

    fn control(&self, action: Control) -> Result<Snapshot> {
        let mut runtime = self
            .runtime
            .lock()
            .map_err(|_| anyhow::anyhow!("session state unavailable"))?;
        let current = runtime
            .current
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("No recording is active."))?;
        let status = match action {
            Control::Pause if current.view.status == "recording" => "paused",
            Control::Resume if current.view.status == "paused" => "recording",
            Control::Stop if matches!(current.view.status.as_str(), "recording" | "paused") => {
                "processing"
            }
            _ => bail!("This recording action is not available right now."),
        };
        current
            .control
            .send(action)
            .map_err(|_| anyhow::anyhow!("The recording worker has stopped."))?;
        current.view.status = status.into();
        if status == "processing" {
            self.meeting_detection
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .expected_stop(&current.view.id);
        }
        drop(runtime);
        self.snapshot()
    }
    pub fn pause(&self) -> Result<Snapshot> {
        self.control(Control::Pause)
    }
    pub fn resume(&self) -> Result<Snapshot> {
        self.control(Control::Resume)
    }
    pub fn stop(&self) -> Result<Snapshot> {
        self.control(Control::Stop)
    }
    pub fn shutdown(&self) {
        self.shutting_down.store(true, Ordering::Relaxed);
        self.signal_detection_stop();
        let _ = audio_service::stop(self);
        self.cancel_download.store(true, Ordering::Relaxed);
        self.cancel_processing.store(true, Ordering::Relaxed);
        self.cancel_refinement_download
            .store(true, Ordering::Relaxed);
        self.cancel_vad_download.store(true, Ordering::Relaxed);
        self.cancel_diarization_download
            .store(true, Ordering::Relaxed);
        if let Ok(runtime) = self.runtime.lock()
            && let Some(current) = &runtime.current
        {
            let _ = current.control.send(Control::Stop);
        }
    }

    pub fn wait_for_shutdown(&self) {
        self.shutdown();
        self.join_detection();
        let deadline = Instant::now() + std::time::Duration::from_secs(35);
        while Instant::now() < deadline {
            let active = self
                .runtime
                .lock()
                .map(|runtime| {
                    runtime.processing
                        || runtime.model.busy
                        || runtime.refinement_model.busy
                        || runtime.vad_model.busy
                        || runtime.diarization_model.busy
                        || runtime.current.as_ref().is_some_and(|current| {
                            matches!(
                                current.view.status.as_str(),
                                "recording" | "paused" | "processing"
                            )
                        })
                })
                .unwrap_or(false);
            if !active {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    pub fn detail(&self, id: &str) -> Result<MeetingDetail> {
        self.detail_for_run(id, None)
    }
    pub fn detail_for_run(&self, id: &str, run: Option<&str>) -> Result<MeetingDetail> {
        // Snapshot unsaved live lines before reading the database: a line the
        // writer commits in between is then in both, and deduplicated by ID,
        // rather than in neither.
        let live = self.runtime.lock().ok().and_then(|runtime| {
            runtime
                .current
                .as_ref()
                .is_some_and(|c| c.view.id == id)
                .then(|| {
                    runtime
                        .live_finals
                        .iter()
                        .chain(runtime.partials.values())
                        .cloned()
                        .collect::<Vec<_>>()
                })
        });
        let mut detail = match run {
            Some(run) => store::detail_run(&self.root, id, run)?,
            None => store::detail(&self.root, id)?,
        };
        if let Some(live) = live
            && detail.run_id == detail.active_run_id
        {
            let saved = detail
                .segments
                .iter()
                .map(|s| s.id.clone())
                .collect::<std::collections::BTreeSet<_>>();
            detail
                .segments
                .extend(live.into_iter().filter(|s| !saved.contains(&s.id)));
            detail.segments.sort_by_key(|s| s.start_ms);
        }
        Ok(detail)
    }
    pub fn save_note(&self, id: &str, text: &str) -> Result<()> {
        let runtime = self
            .runtime
            .lock()
            .map_err(|_| anyhow::anyhow!("session state unavailable"))?;
        Self::ensure_not_maintenance(&runtime)?;
        let timing = {
            if runtime.current.as_ref().is_some_and(|c| {
                c.view.id == id
                    && matches!(
                        c.view.status.as_str(),
                        "recording" | "paused" | "processing"
                    )
            }) {
                Some(
                    runtime
                        .source_timeline
                        .note_position(self.resources.uptime_ns())
                        .ok_or_else(|| {
                            anyhow::anyhow!(
                                "Wait for source timing or audio before adding a timestamped note."
                            )
                        })?,
                )
            } else {
                None
            }
        };
        let timing = if let Some(timing) = timing {
            timing
        } else {
            let playback = self
                .playback
                .lock()
                .map_err(|_| anyhow::anyhow!("playback state unavailable"))?;
            if let Some(position) = playback
                .as_ref()
                .map(vt_playback::Playback::snapshot)
                .filter(|p| p.meeting_id == id)
            {
                SourceTiming {
                    synchronized: true,
                    provenance: TimingProvenance::SavedPlaybackPosition,
                    elapsed_ms: position.position_ms,
                    latest_audio_ms: Some(position.position_ms),
                    warning: None,
                }
            } else {
                SourceTiming {
                    provenance: TimingProvenance::UntimedNote,
                    ..Default::default()
                }
            }
        };
        if text.trim().is_empty() {
            bail!("Notes must contain 1–32,000 bytes of text.");
        }
        store::save_note_with_metadata(&self.root, id, timing.elapsed_ms, text, |db, note| {
            health::note_timing(db, note, &timing)
        })?;
        Ok(())
    }
    pub fn rename(&self, id: &str, title: &str) -> Result<()> {
        let mut runtime = self
            .runtime
            .lock()
            .map_err(|_| anyhow::anyhow!("session state unavailable"))?;
        Self::ensure_not_maintenance(&runtime)?;
        store::rename(&self.root, id, title)?;
        if let Some(current) = runtime.current.as_mut().filter(|c| c.view.id == id) {
            current.view.title = title.trim().into();
        }
        Ok(())
    }
    pub fn export(&self, id: &str, format: &str) -> Result<PathBuf> {
        let runtime = self
            .runtime
            .lock()
            .map_err(|_| anyhow::anyhow!("session state unavailable"))?;
        Self::ensure_not_maintenance(&runtime)?;
        store::export(&self.root, id, format)
    }

    pub fn install_model(self: &Arc<Self>) -> Result<()> {
        let mut runtime = self
            .runtime
            .lock()
            .map_err(|_| anyhow::anyhow!("session state unavailable"))?;
        if self.shutting_down.load(Ordering::Relaxed) {
            bail!("VanTranscribe is closing.");
        }
        if runtime.model.busy {
            bail!("A model download is already running.");
        }
        let manager = self.live_model_manager()?;
        runtime.model.busy = true;
        runtime.model.progress = 0.0;
        runtime.model.error = None;
        self.cancel_download.store(false, Ordering::Relaxed);
        drop(runtime);
        let service = self.clone();
        let cleanup = service.clone();
        let spawn = std::thread::Builder::new()
            .name("model-download".into())
            .spawn(move || {
                let update = service.clone();
                let result = manager.install(
                    move |received, total| {
                        if let Ok(mut runtime) = update.runtime.lock() {
                            runtime.model.progress = if total > 0 {
                                received as f64 / total as f64
                            } else {
                                0.0
                            };
                        }
                    },
                    service.cancel_download.clone(),
                );
                if let Ok(mut runtime) = service.runtime.lock() {
                    runtime.model.busy = false;
                    match result {
                        Ok(()) => {
                            runtime.model.installed = true;
                            runtime.model.progress = 1.0;
                        }
                        Err(e) => runtime.model.error = Some(e.to_string()),
                    }
                }
            });
        if let Err(error) = spawn {
            if let Ok(mut runtime) = cleanup.runtime.lock() {
                runtime.model.busy = false;
                runtime.model.error = Some(error.to_string());
            }
            return Err(error.into());
        }
        Ok(())
    }
    pub fn cancel_model_download(&self) {
        self.cancel_download.store(true, Ordering::Relaxed);
    }

    pub fn input_devices(&self) -> Result<vt_capture::DeviceList> {
        Ok(vt_capture::list_devices(&self.helper)?)
    }

    pub fn play_audio(&self, id: &str, position: Option<u64>) -> Result<Snapshot> {
        audio_service::play(self, id, position)?;
        self.snapshot()
    }
    pub fn pause_audio(&self) -> Result<Snapshot> {
        audio_service::pause(self)?;
        self.snapshot()
    }
    pub fn seek_audio(&self, id: &str, position: u64) -> Result<Snapshot> {
        audio_service::seek(self, id, position)?;
        self.snapshot()
    }
    pub fn stop_audio(&self) -> Result<Snapshot> {
        audio_service::stop(self)?;
        self.snapshot()
    }
    pub fn set_playback_volumes(&self, id: &str, mic: f32, system: f32) -> Result<Snapshot> {
        audio_service::volumes(self, id, mic, system)?;
        self.snapshot()
    }
    pub fn audio_waveform(&self, id: &str, bins: usize) -> Result<Waveform> {
        audio_service::waveform(self, id, bins)
    }
    pub fn update_audio_preferences(&self, preferences: AudioPreferences) -> Result<Snapshot> {
        {
            let mut saved = self
                .audio_preferences
                .lock()
                .map_err(|_| anyhow::anyhow!("audio settings unavailable"))?;
            audio_preferences::save(&self.root, &preferences)?;
            *saved = preferences;
        }
        self.snapshot()
    }

    pub fn transcribe(self: &Arc<Self>, id: &str) -> Result<()> {
        session::transcribe_saved(self.clone(), id)
    }

    pub fn refine(self: &Arc<Self>, id: &str) -> Result<()> {
        processing::refine(self.clone(), id)
    }
    pub fn generate_summary(self: &Arc<Self>, id: &str) -> Result<()> {
        processing::generate_summary(self.clone(), id)
    }
    pub fn cancel_processing(&self) {
        self.cancel_processing.store(true, Ordering::Relaxed);
    }
    pub fn search(&self, query: &str) -> Result<Vec<Meeting>> {
        store::search_meetings(&self.root, query)
    }
    pub fn set_bleed_decision(
        &self,
        id: &str,
        run: &str,
        segment: &str,
        decision: &str,
    ) -> Result<()> {
        let runtime = self
            .runtime
            .lock()
            .map_err(|_| anyhow::anyhow!("Session state unavailable"))?;
        if runtime.processing
            || runtime
                .current
                .as_ref()
                .is_some_and(|c| c.view.status == "processing")
        {
            bail!("Wait for processing to finish before reviewing bleed.");
        }
        store::set_bleed_decision(&self.root, id, run, segment, decision)
    }
    pub fn resolve_bleed_review(&self, id: &str, run: &str, decision: &str) -> Result<()> {
        let runtime = self
            .runtime
            .lock()
            .map_err(|_| anyhow::anyhow!("Session state unavailable"))?;
        if runtime.processing
            || runtime
                .current
                .as_ref()
                .is_some_and(|c| c.view.status == "processing")
        {
            bail!("Wait for processing to finish before reviewing bleed.");
        }
        store::resolve_bleed_review(&self.root, id, run, decision)
    }
    pub fn rename_speaker(&self, id: &str, speaker: &str, name: &str) -> Result<()> {
        let runtime = self
            .runtime
            .lock()
            .map_err(|_| anyhow::anyhow!("session state unavailable"))?;
        if runtime.processing
            || runtime
                .current
                .as_ref()
                .is_some_and(|current| current.view.status == "processing")
        {
            bail!("Wait for processing to finish before changing speakers.");
        }
        store::rename_speaker(&self.root, id, speaker, name)
    }
    pub fn set_speaker_shared_room(
        &self,
        id: &str,
        speaker: &str,
        shared_room: bool,
    ) -> Result<MeetingDetail> {
        let runtime = self
            .runtime
            .lock()
            .map_err(|_| anyhow::anyhow!("Session state unavailable"))?;
        if self.shutting_down.load(Ordering::Relaxed)
            || runtime.processing
            || runtime.current.as_ref().is_some_and(|c| {
                matches!(
                    c.view.status.as_str(),
                    "recording" | "paused" | "processing"
                )
            })
        {
            bail!("Finish recording and processing before changing shared-room settings.");
        }
        store::set_speaker_shared_room(&self.root, id, speaker, shared_room)?;
        store::detail(&self.root, id)
    }
    pub fn add_speaker_constraint(
        &self,
        id: &str,
        run: &str,
        left: &str,
        right: &str,
        kind: &str,
    ) -> Result<()> {
        let runtime = self
            .runtime
            .lock()
            .map_err(|_| anyhow::anyhow!("session state unavailable"))?;
        if runtime.processing
            || runtime
                .current
                .as_ref()
                .is_some_and(|current| current.view.status == "processing")
        {
            bail!("Wait for processing to finish before changing speakers.");
        }
        store::add_speaker_constraint(&self.root, id, run, left, right, kind)
    }
    pub fn resolve_speaker_review(
        &self,
        id: &str,
        run: &str,
        review: &str,
        previous: Option<&str>,
        target: Option<&str>,
    ) -> Result<()> {
        let runtime = self
            .runtime
            .lock()
            .map_err(|_| anyhow::anyhow!("session state unavailable"))?;
        if runtime.processing
            || runtime
                .current
                .as_ref()
                .is_some_and(|current| current.view.status == "processing")
        {
            bail!("Wait for processing to finish before changing speakers.");
        }
        store::resolve_speaker_review(&self.root, id, run, review, previous, target)
    }
    pub fn revoke_speaker_constraint(&self, id: &str, constraint: &str) -> Result<()> {
        let runtime = self
            .runtime
            .lock()
            .map_err(|_| anyhow::anyhow!("session state unavailable"))?;
        if runtime.processing
            || runtime
                .current
                .as_ref()
                .is_some_and(|current| current.view.status == "processing")
        {
            bail!("Wait for processing to finish before changing speakers.");
        }
        store::revoke_speaker_constraint(&self.root, id, constraint)
    }
    pub fn correct_segment(
        &self,
        id: &str,
        run: &str,
        segment: &str,
        replacement: &str,
    ) -> Result<()> {
        let runtime = self
            .runtime
            .lock()
            .map_err(|_| anyhow::anyhow!("session state unavailable"))?;
        Self::ensure_not_maintenance(&runtime)?;
        store::correct_segment(&self.root, id, run, segment, replacement)
    }
    pub fn resolve_correction(
        &self,
        id: &str,
        correction: &str,
        segment: Option<&str>,
    ) -> Result<()> {
        let runtime = self
            .runtime
            .lock()
            .map_err(|_| anyhow::anyhow!("session state unavailable"))?;
        Self::ensure_not_maintenance(&runtime)?;
        store::resolve_correction(&self.root, id, correction, segment)
    }
    pub fn update_preferences(
        &self,
        prefs: Preferences,
        key: Option<&str>,
        clear_key: bool,
    ) -> Result<Snapshot> {
        if prefs.summary.provider != vt_summarize::Provider::None {
            vt_summarize::validate_config(&prefs.summary)?;
        }
        if clear_key {
            vt_summarize::secret_delete(&prefs.summary)?;
        }
        if let Some(key) = key.filter(|key| !key.trim().is_empty()) {
            vt_summarize::secret_set(&prefs.summary, key)?;
        }
        preferences::save(&self.root, &prefs)?;
        *self
            .preferences
            .lock()
            .map_err(|_| anyhow::anyhow!("settings unavailable"))? = prefs;
        self.snapshot()
    }
    pub fn summary_capabilities(&self) -> Result<serde_json::Value> {
        let helper = self.helper.with_file_name("vt-summary-helper");
        let apple =
            vt_summarize::apple_availability(&helper).unwrap_or(vt_summarize::AppleAvailability {
                available: false,
                reason: Some(
                    "Apple summary component is unavailable; update or rebuild the app.".into(),
                ),
                context_tokens: 4096,
            });
        let local_servers=["http://127.0.0.1:11434/v1","http://127.0.0.1:1234/v1"].into_iter().map(|endpoint| {
            match vt_summarize::discover_local_models(endpoint) {
                Ok(status)=>serde_json::json!({"endpoint":endpoint,"available":status.available,"models":status.models,"reason":status.reason}),
                Err(_)=>serde_json::json!({"endpoint":endpoint,"available":false,"models":[],"reason":"No local model server is available."}),
            }
        }).collect::<Vec<_>>();
        Ok(serde_json::json!({"apple":apple,"local_servers":local_servers}))
    }
    pub fn install_refinement_model(self: &Arc<Self>) -> Result<()> {
        let mut runtime = self
            .runtime
            .lock()
            .map_err(|_| anyhow::anyhow!("session state unavailable"))?;
        if runtime.refinement_model.busy {
            bail!("Refinement model setup is already running.");
        }
        runtime.refinement_model.busy = true;
        runtime.refinement_model.progress = 0.0;
        runtime.refinement_model.error = None;
        self.cancel_refinement_download
            .store(false, Ordering::Relaxed);
        drop(runtime);
        let service = self.clone();
        let cleanup = self.clone();
        let spawn = std::thread::Builder::new()
            .name("refinement-model-setup".into())
            .spawn(move || {
                let manager = RefinementModelManager::new(service.root.join("models"));
                let update = service.clone();
                let result = manager.install(
                    move |received, total| {
                        if let Ok(mut runtime) = update.runtime.lock() {
                            runtime.refinement_model.progress = if total > 0 {
                                received as f64 / total as f64
                            } else {
                                0.0
                            };
                        }
                    },
                    service.cancel_refinement_download.clone(),
                );
                if let Ok(mut runtime) = service.runtime.lock() {
                    runtime.refinement_model.busy = false;
                    match result {
                        Ok(()) => {
                            runtime.refinement_model.installed = true;
                            runtime.vad_model.installed = true;
                            runtime.refinement_model.progress = 1.0;
                        }
                        Err(error) => runtime.refinement_model.error = Some(error.to_string()),
                    }
                }
            });
        if let Err(error) = spawn {
            if let Ok(mut runtime) = cleanup.runtime.lock() {
                runtime.refinement_model.busy = false;
                runtime.refinement_model.error = Some(error.to_string());
            }
            return Err(error.into());
        }
        Ok(())
    }
    pub fn cancel_refinement_model_download(&self) {
        self.cancel_refinement_download
            .store(true, Ordering::Relaxed);
    }
    pub fn install_vad_model(self: &Arc<Self>) -> Result<()> {
        model_setup::install(self.clone(), model_setup::Pack::Vad, true)
    }
    pub fn cancel_vad_model_download(&self) {
        self.cancel_vad_download.store(true, Ordering::Relaxed);
    }
    pub fn install_diarization_model(self: &Arc<Self>, acknowledge: bool) -> Result<()> {
        model_setup::install(self.clone(), model_setup::Pack::Diarization, acknowledge)
    }
    pub fn cancel_diarization_model_download(&self) {
        self.cancel_diarization_download
            .store(true, Ordering::Relaxed);
    }
    fn vad_path(&self) -> Option<PathBuf> {
        if !self.audio_preferences.lock().ok()?.live_vad_enabled {
            return None;
        }
        let standalone = vt_stt::VadModelManager::new(self.root.join("models"));
        if standalone.installed() {
            return Some(standalone.model_path());
        }
        let refinement = RefinementModelManager::new(self.root.join("models"));
        refinement
            .installed()
            .then(|| refinement.model_dir().join(vt_stt::VAD_MODEL_FILE))
    }
}

#[cfg(test)]
mod audio_service_tests;
#[cfg(test)]
mod live_speaker_tests;
#[cfg(test)]
mod maintenance_integration_tests;
#[cfg(test)]
mod real_call_replay_tests;
#[cfg(test)]
mod summary_tests;
#[cfg(test)]
mod tests;
