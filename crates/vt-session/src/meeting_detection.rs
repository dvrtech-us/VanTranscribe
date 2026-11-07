//! Opt-in advisory detection. Only settings persist; observations are ephemeral.
use super::*;
use rusqlite::OptionalExtension;
use std::{
    collections::BTreeSet,
    sync::{Weak, mpsc},
    thread,
    time::Duration,
};
use vt_capture::{MeetingApp, MeetingAppKind, MeetingAppSnapshot};

const SETTINGS_KEY: &str = "meeting_detection";
const MODE_KEY: &str = "capture_mode";
const FRESH_NS: u64 = 6_000_000_000;
const RELEASE_NS: u64 = 60_000_000_000;
const POLL: Duration = Duration::from_secs(2);
type Notifier = dyn Fn(&str, &str) + Send + Sync;
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DetectionPolicy {
    #[default]
    Prompt,
    Suppress,
    AlwaysRecord,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DetectionInput {
    Default,
    Device { id: String },
}
impl DetectionInput {
    fn device(&self) -> Option<String> {
        match self {
            Self::Default => None,
            Self::Device { id } => Some(id.clone()),
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DetectionRule {
    pub app_id: String,
    pub policy: DetectionPolicy,
    #[serde(default)]
    pub participant_notice_attested: bool,
    #[serde(default)]
    pub input: Option<DetectionInput>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DetectionSettings {
    pub enabled: bool,
    pub rules: Vec<DetectionRule>,
}
#[derive(Clone, Debug, Serialize)]
pub struct DetectionPrompt {
    pub episode_id: String,
    pub app: MeetingApp,
    pub message: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct DetectionEnded {
    pub episode_id: String,
    pub meeting_id: String,
    pub app_id: String,
    pub message: String,
}
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DetectionStatus {
    Disabled,
    Watching,
    Unknown,
    Blocked,
}
#[derive(Clone, Debug, Serialize)]
pub struct DetectionSnapshot {
    pub context_generation: u64,
    pub enabled: bool,
    pub status: DetectionStatus,
    pub error: Option<String>,
    pub prompts: Vec<DetectionPrompt>,
    pub ended: Option<DetectionEnded>,
    pub catalog: Vec<MeetingApp>,
}
struct Episode {
    id: String,
    dismissed: bool,
    auto_attempted: bool,
    notified: bool,
    absent_since: Option<u64>,
}
struct Observation {
    at: u64,
    apps: BTreeMap<String, MeetingApp>,
}
struct Association {
    meeting_id: String,
    app: MeetingApp,
    episode_id: String,
    absent_since: Option<u64>,
    ended: Option<DetectionEnded>,
    dismissed: bool,
    notified: bool,
    start_notification_pending: bool,
    expected_stop: bool,
}
struct StartEvidence {
    meeting_id: String,
    ready: bool,
    failed: bool,
}
pub(crate) struct DetectionState {
    settings: DetectionSettings,
    pub(crate) mode: CaptureMode,
    ready: bool,
    setup_open: bool,
    mode_needs_repair: bool,
    automatic_error: Option<String>,
    revision: u64,
    pub(crate) context_generation: u64,
    observation: Option<Observation>,
    episodes: BTreeMap<String, Episode>,
    automatic_armed: BTreeSet<String>,
    association: Option<Association>,
    error: Option<String>,
    notifier: Option<Arc<Notifier>>,
}
pub(crate) struct DetectedStart {
    pub episode_id: String,
    pub automatic: bool,
    pub checked_input: Option<DetectionInput>,
    pub expected_revision: Option<u64>,
    pub checked_at: Option<u64>,
}
fn catalog_app(id: &str) -> Option<MeetingApp> {
    vt_capture::known_meeting_apps()
        .iter()
        .find(|app| app.id == id)
        .cloned()
}
fn valid_input(input: &Option<DetectionInput>) -> bool {
    match input {
        Some(DetectionInput::Default) => true,
        Some(DetectionInput::Device { id }) => {
            !id.is_empty() && id.len() <= 512 && !id.chars().any(char::is_control)
        }
        None => false,
    }
}
fn validate_settings(mut settings: DetectionSettings) -> Result<DetectionSettings> {
    let mut ids = BTreeSet::new();
    if !settings.enabled {
        settings
            .rules
            .retain(|rule| catalog_app(&rule.app_id).is_some() && ids.insert(rule.app_id.clone()));
        for rule in &mut settings.rules {
            if rule.policy != DetectionPolicy::AlwaysRecord
                || !rule.participant_notice_attested
                || !valid_input(&rule.input)
            {
                rule.policy = if rule.policy == DetectionPolicy::Suppress {
                    DetectionPolicy::Suppress
                } else {
                    DetectionPolicy::Prompt
                };
                rule.participant_notice_attested = false;
                rule.input = None;
            }
        }
        return Ok(settings);
    }
    if settings.rules.len() > vt_capture::known_meeting_apps().len() {
        bail!("Too many meeting app rules.");
    }
    for rule in &mut settings.rules {
        if catalog_app(&rule.app_id).is_none() || !ids.insert(rule.app_id.clone()) {
            bail!("Choose each supported meeting app at most once.");
        }
        if rule.policy == DetectionPolicy::AlwaysRecord {
            if !rule.participant_notice_attested || !valid_input(&rule.input) {
                bail!(
                    "Always record requires explicit participant-notice attestation and a saved valid microphone choice for this app."
                );
            }
        } else {
            rule.participant_notice_attested = false;
            rule.input = None;
        }
    }
    Ok(settings)
}

fn load_value<T: serde::de::DeserializeOwned>(root: &Path, key: &str) -> Result<Option<T>> {
    let value: Option<String> = store::open(root)?
        .query_row(
            "SELECT value_json FROM app_settings WHERE key=?1",
            [key],
            |row| row.get(0),
        )
        .optional()?;
    value
        .map(|value| serde_json::from_str(&value).map_err(Into::into))
        .transpose()
}
fn save_value(root: &Path, key: &str, value: &impl Serialize) -> Result<()> {
    store::open(root)?.execute("INSERT INTO app_settings(key,value_json) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value_json=excluded.value_json",rusqlite::params![key,serde_json::to_string(value)?])?;
    Ok(())
}
impl DetectionState {
    pub(crate) fn load(root: &Path) -> Self {
        let loaded_settings = load_value(root, SETTINGS_KEY)
            .and_then(|settings| validate_settings(settings.unwrap_or_default()));
        let loaded_mode =
            load_value(root, MODE_KEY).map(|mode| mode.unwrap_or(CaptureMode::Remote));
        let mode_needs_repair = loaded_mode.is_err();
        let invalid = loaded_settings.is_err() || mode_needs_repair;
        let mode = loaded_mode.unwrap_or(CaptureMode::Remote);
        let settings = if invalid {
            DetectionSettings::default()
        } else {
            loaded_settings.unwrap_or_default()
        };
        let error=invalid.then(||"Meeting detection settings could not be read. Detection stays off until its settings are explicitly saved.".into());
        Self {
            settings,
            mode,
            ready: false,
            setup_open: true,
            mode_needs_repair,
            automatic_error: None,
            revision: 0,
            context_generation: 0,
            observation: None,
            episodes: BTreeMap::new(),
            automatic_armed: BTreeSet::new(),
            association: None,
            error,
            notifier: None,
        }
    }
    fn rule(&self, id: &str) -> DetectionRule {
        self.settings
            .rules
            .iter()
            .find(|rule| rule.app_id == id)
            .cloned()
            .unwrap_or(DetectionRule {
                app_id: id.into(),
                policy: DetectionPolicy::Prompt,
                participant_notice_attested: false,
                input: None,
            })
    }
    fn fresh(&self, now: u64) -> Option<&Observation> {
        self.observation
            .as_ref()
            .filter(|observation| now >= observation.at && now - observation.at <= FRESH_NS)
    }
    fn context_allows(&self) -> bool {
        self.settings.enabled && self.ready && !self.setup_open && self.mode == CaptureMode::Remote
    }
    fn suspend_automatic(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        self.automatic_armed.clear();
        for episode in self.episodes.values_mut() {
            episode.auto_attempted = true;
        }
    }
    fn reset_evidence(&mut self) {
        self.suspend_automatic();
        self.observation = None;
        for episode in self.episodes.values_mut() {
            episode.absent_since = None;
        }
        if let Some(source) = &mut self.association {
            source.absent_since = None;
            source.ended = None;
        }
    }
    pub(crate) fn clock_unavailable(&mut self) {
        if !self.settings.enabled {
            return;
        }
        self.unknown(Some(
            "The suspend-aware observation clock is unavailable. Meeting detection is paused."
                .into(),
        ));
    }
    fn unknown(&mut self, error: Option<String>) {
        self.reset_evidence();
        self.error =
            Some(error.unwrap_or_else(|| "Meeting app activity could not be verified.".into()));
    }
    fn observe(
        &mut self,
        snapshot: MeetingAppSnapshot,
        now: u64,
        current: Option<&str>,
        busy: bool,
    ) {
        if !self.settings.enabled {
            return;
        }
        if !snapshot.complete || snapshot.error.is_some() {
            self.unknown(snapshot.error);
            return;
        }
        let mut apps = BTreeMap::new();
        for app in snapshot.apps {
            let Some(canonical) = catalog_app(&app.id) else {
                self.unknown(None);
                return;
            };
            if serde_json::to_value(&canonical).ok() != serde_json::to_value(&app).ok()
                || apps.insert(app.id.clone(), canonical).is_some()
            {
                self.unknown(None);
                return;
            }
        }
        if self.fresh(now).is_none() {
            self.reset_evidence();
        }
        self.error = None;
        // A verified absence baseline is required before unattended policy can
        // authorize a later activity edge. Existing/busy episodes stay manual.
        if self.context_allows() && !busy {
            for rule in &self.settings.rules {
                if rule.policy == DetectionPolicy::AlwaysRecord && !apps.contains_key(&rule.app_id)
                {
                    self.automatic_armed.insert(rule.app_id.clone());
                }
            }
        }
        for id in apps.keys() {
            let edge = self.automatic_armed.remove(id);
            let automatic = edge
                && !busy
                && self.context_allows()
                && self.rule(id).policy == DetectionPolicy::AlwaysRecord;
            if automatic {
                self.episodes.remove(id);
            }
            let episode = self.episodes.entry(id.clone()).or_insert_with(|| Episode {
                id: uuid::Uuid::new_v4().to_string(),
                dismissed: false,
                auto_attempted: !automatic,
                notified: false,
                absent_since: None,
            });
            episode.absent_since = None;
            if busy || current.is_some() {
                episode.auto_attempted = true;
            }
        }
        self.episodes.retain(|id, episode| {
            if apps.contains_key(id) {
                true
            } else {
                let since = *episode.absent_since.get_or_insert(now);
                now.saturating_sub(since) <= RELEASE_NS
            }
        });
        if self.association.as_ref().is_some_and(|source| {
            Some(source.meeting_id.as_str()) != current && !source.start_notification_pending
        }) {
            self.association = None;
        }
        if let Some(source) = &mut self.association {
            if apps.contains_key(&source.app.id) {
                source.absent_since = None;
                source.ended = None;
            } else {
                let since = *source.absent_since.get_or_insert(now);
                if now.saturating_sub(since) > RELEASE_NS
                    && !source.dismissed
                    && source.ended.is_none()
                {
                    source.ended = Some(DetectionEnded {
                        episode_id: source.episode_id.clone(),
                        meeting_id: source.meeting_id.clone(),
                        app_id: source.app.id.clone(),
                        message:
                            "Meeting may have ended. Recording continues until you choose Stop."
                                .into(),
                    });
                }
            }
        }
        self.observation = Some(Observation { at: now, apps });
    }
    pub(crate) fn settings(&self) -> DetectionSettings {
        self.settings.clone()
    }
    pub(crate) fn snapshot(
        &self,
        now: impl Into<Option<u64>>,
        busy: bool,
        current: Option<&str>,
        closing: bool,
    ) -> DetectionSnapshot {
        let now = now.into();
        let fresh = now.and_then(|now| self.fresh(now));
        let context =
            self.settings.enabled && self.ready && self.mode == CaptureMode::Remote && !closing;
        let status = if !self.settings.enabled {
            DetectionStatus::Disabled
        } else if !context || self.setup_open || busy {
            DetectionStatus::Blocked
        } else if fresh.is_none() {
            DetectionStatus::Unknown
        } else {
            DetectionStatus::Watching
        };
        let mut prompts = Vec::new();
        if context
            && !busy
            && let Some(observation) = fresh
        {
            for (id, app) in &observation.apps {
                if let Some(episode) = self.episodes.get(id)
                    && !episode.dismissed
                {
                    let rule = self.rule(id);
                    if rule.policy == DetectionPolicy::Prompt
                        || (rule.policy == DetectionPolicy::AlwaysRecord && episode.auto_attempted)
                    {
                        let message = if matches!(&app.kind, MeetingAppKind::Browser) {
                            format!("{} is using the microphone. Record?", app.name)
                        } else {
                            format!("Meeting detected in {}. Record?", app.name)
                        };
                        prompts.push(DetectionPrompt {
                            episode_id: episode.id.clone(),
                            app: app.clone(),
                            message,
                        });
                    }
                }
            }
        }
        let ended = if context && fresh.is_some() {
            self.association
                .as_ref()
                .filter(|source| Some(source.meeting_id.as_str()) == current && !source.dismissed)
                .and_then(|source| source.ended.clone())
        } else {
            None
        };
        DetectionSnapshot {
            context_generation: self.context_generation,
            enabled: self.settings.enabled,
            status,
            error: self
                .error
                .clone()
                .or_else(|| self.automatic_error.clone())
                .or_else(|| {
                    if self.settings.enabled && context && fresh.is_none() {
                        Some("Meeting app observations are unavailable or stale.".into())
                    } else {
                        None
                    }
                }),
            prompts,
            ended,
            catalog: vt_capture::known_meeting_apps().to_vec(),
        }
    }
    fn auto_candidate(&self, now: u64, busy: bool) -> Option<String> {
        if busy || !self.context_allows() {
            return None;
        }
        self.fresh(now)?.apps.keys().find_map(|id| {
            let episode = self.episodes.get(id)?;
            let rule = self.rule(id);
            (rule.policy == DetectionPolicy::AlwaysRecord
                && !episode.dismissed
                && !episode.auto_attempted
                && rule.participant_notice_attested
                && rule.input.is_some())
            .then(|| episode.id.clone())
        })
    }
    pub(crate) fn validate_start(
        &mut self,
        request: &DetectedStart,
        now: Option<u64>,
        input: Option<String>,
    ) -> Result<(MeetingApp, Option<String>)> {
        let now = now.ok_or_else(|| {
            anyhow::anyhow!(
                "The suspend-aware observation clock is unavailable. Meeting detection is paused."
            )
        })?;
        if request.automatic
            && request
                .checked_at
                .is_none_or(|at| now < at || now - at > FRESH_NS)
        {
            bail!("Microphone preflight is stale. Wait for fresh activity.");
        }
        if request.automatic && request.expected_revision != Some(self.revision) {
            bail!("Meeting detection settings or interaction context changed during preflight.");
        }
        if !self.settings.enabled
            || !self.ready
            || self.mode != CaptureMode::Remote
            || (request.automatic && self.setup_open)
        {
            bail!("Meeting detection is not ready for a remote recording.");
        }
        let observation = self.fresh(now).ok_or_else(|| {
            anyhow::anyhow!(
                "Meeting app observation is stale. Wait for fresh activity before recording."
            )
        })?;
        let id = observation
            .apps
            .keys()
            .find(|id| {
                self.episodes
                    .get(*id)
                    .is_some_and(|episode| episode.id == request.episode_id && !episode.dismissed)
            })
            .cloned()
            .ok_or_else(|| {
                anyhow::anyhow!("That detected activity episode is no longer available.")
            })?;
        let app = observation.apps[&id].clone();
        let rule = self.rule(&id);
        if rule.policy == DetectionPolicy::Suppress {
            bail!("This meeting app is suppressed.");
        }
        let input = if request.automatic {
            if rule.policy != DetectionPolicy::AlwaysRecord
                || !rule.participant_notice_attested
                || self.episodes[&id].auto_attempted
            {
                bail!("Always record was not explicitly authorized for this activity episode.");
            }
            if rule.input != request.checked_input {
                bail!("The saved microphone rule changed. Wait for a fresh activity episode.");
            }
            rule.input
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("Always record needs a saved microphone choice."))?
                .device()
        } else {
            input
        };
        if request.automatic {
            self.episodes.get_mut(&id).unwrap().auto_attempted = true;
        }
        Ok((app, input))
    }
    pub(crate) fn started(
        &mut self,
        meeting: &str,
        mode: CaptureMode,
        source: Option<&(MeetingApp, String)>,
        automatic: bool,
    ) {
        self.mode = mode;
        self.automatic_error = None;
        self.automatic_armed.clear();
        // A recording covers currently observed activity; Stop must not queue
        // another unattended recording for a simultaneous app episode.
        for episode in self.episodes.values_mut() {
            episode.auto_attempted = true;
        }
        self.association = source.map(|(app, episode)| {
            if let Some(item) = self.episodes.get_mut(&app.id) {
                item.dismissed = true;
                item.auto_attempted = true;
            }
            Association {
                meeting_id: meeting.into(),
                app: app.clone(),
                episode_id: episode.clone(),
                absent_since: None,
                ended: None,
                dismissed: false,
                notified: false,
                start_notification_pending: automatic,
                expected_stop: false,
            }
        });
    }
    pub(crate) fn expected_stop(&mut self, id: &str) {
        if let Some(source) = self
            .association
            .as_mut()
            .filter(|source| source.meeting_id == id)
        {
            source.expected_stop = true;
            source.start_notification_pending = false;
        }
    }
    fn notifications(
        &mut self,
        now: u64,
        busy: bool,
        current: Option<&str>,
        evidence: Option<&StartEvidence>,
    ) -> Vec<(String, String)> {
        let context_allowed = self.context_allows();
        let mut messages = Vec::new();
        let mut failed = None;
        if let Some(source) = &mut self.association
            && source.start_notification_pending
            && let Some(evidence) =
                evidence.filter(|evidence| evidence.meeting_id == source.meeting_id)
        {
            if evidence.failed && !source.expected_stop {
                source.start_notification_pending = false;
                failed = Some(source.app.id.clone());
            } else if evidence.ready
                && context_allowed
                && Some(source.meeting_id.as_str()) == current
                && !source.expected_stop
                && source.ended.is_none()
            {
                source.start_notification_pending = false;
                messages.push((
                    "VanTranscribe recording started".into(),
                    format!(
                        "Recording started for {} under your Always record rule.",
                        source.app.name
                    ),
                ));
            }
        }
        if let Some(id) = failed {
            if let Some(episode) = self.episodes.get_mut(&id) {
                episode.dismissed = false;
                episode.auto_attempted = true;
                episode.notified = true;
            }
            self.association = None;
            self.automatic_error=Some("Automatic recording did not start. Open recording setup to review permissions and microphone settings.".into());
            messages.push(("VanTranscribe automatic recording unavailable".into(),"Automatic recording did not start. Open recording setup to review permissions and microphone settings.".into()));
        }
        if !self.context_allows() {
            return Vec::new();
        }
        let snapshot = self.snapshot(now, busy, current, false);
        let mut notifications = messages;
        for prompt in snapshot.prompts {
            if let Some(episode) = self.episodes.get_mut(&prompt.app.id)
                && !episode.notified
            {
                episode.notified = true;
                notifications.push(("VanTranscribe recording suggestion".into(), prompt.message));
            }
        }
        if let Some(ended) = snapshot.ended
            && let Some(source) = &mut self.association
            && !source.notified
        {
            source.notified = true;
            notifications.push(("VanTranscribe recording reminder".into(), ended.message));
        }
        notifications
    }
}
pub(super) fn busy(runtime: &Runtime) -> bool {
    runtime.processing
        || runtime.saved_audio_readers > 0
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
}
pub(super) fn recording_id(runtime: &Runtime) -> Option<&str> {
    runtime
        .current
        .as_ref()
        .filter(|current| matches!(current.view.status.as_str(), "recording" | "paused"))
        .map(|current| current.view.id.as_str())
}
fn notify(sink: Option<Arc<Notifier>>, messages: Vec<(String, String)>) {
    if let Some(sink) = sink {
        for (title, body) in messages {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| sink(&title, &body)));
        }
    }
}
impl AppService {
    pub fn reset_detection_context(&self) {
        let Ok(_runtime) = self.runtime.lock() else {
            return;
        };
        if let Ok(mut state) = self.meeting_detection.lock() {
            state.context_generation = state.context_generation.wrapping_add(1);
            state.ready = false;
            state.setup_open = true;
            state.reset_evidence();
        }
    }
    pub fn set_detection_notifier(&self, notifier: Arc<Notifier>) {
        self.meeting_detection
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .notifier = Some(notifier);
    }
    #[cfg(test)]
    fn update_meeting_detection(&self, settings: DetectionSettings) -> Result<Snapshot> {
        let generation = self
            .meeting_detection
            .lock()
            .map_err(|_| anyhow::anyhow!("Meeting detection state unavailable"))?
            .context_generation;
        self.update_meeting_detection_for_generation(settings, generation)
    }
    pub fn update_meeting_detection_for_generation(
        &self,
        settings: DetectionSettings,
        expected_generation: u64,
    ) -> Result<Snapshot> {
        let settings = validate_settings(settings)?;
        {
            let runtime = self
                .runtime
                .lock()
                .map_err(|_| anyhow::anyhow!("Session state unavailable"))?;
            Self::ensure_not_maintenance(&runtime)?;
            let mut state = self
                .meeting_detection
                .lock()
                .map_err(|_| anyhow::anyhow!("Meeting detection state unavailable"))?;
            if expected_generation != state.context_generation {
                bail!(
                    "The meeting detection page context changed. Refresh before saving detection settings."
                );
            }
            let mut db = store::open(&self.root)?;
            let tx = db.transaction()?;
            for (key, value) in [
                (SETTINGS_KEY, serde_json::to_string(&settings)?),
                (MODE_KEY, serde_json::to_string(&state.mode)?),
            ] {
                tx.execute("INSERT INTO app_settings(key,value_json) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value_json=excluded.value_json",rusqlite::params![key,value])?;
            }
            tx.commit()?;
            state.settings = settings;
            state.mode_needs_repair = false;
            state.reset_evidence();
            state.error = None;
            state.automatic_error = None;
        }
        self.snapshot()
    }
    #[cfg(test)]
    fn set_detection_context(&self, mode: CaptureMode, setup_open: bool) -> Result<Snapshot> {
        let generation = self
            .meeting_detection
            .lock()
            .map_err(|_| anyhow::anyhow!("Meeting detection state unavailable"))?
            .context_generation;
        self.set_detection_context_for_generation(mode, setup_open, generation)
    }
    pub fn set_detection_context_for_generation(
        &self,
        mode: CaptureMode,
        setup_open: bool,
        expected_generation: u64,
    ) -> Result<Snapshot> {
        {
            let runtime = self
                .runtime
                .lock()
                .map_err(|_| anyhow::anyhow!("Session state unavailable"))?;
            let mut state = self
                .meeting_detection
                .lock()
                .map_err(|_| anyhow::anyhow!("Meeting detection state unavailable"))?;
            if expected_generation != state.context_generation {
                bail!(
                    "The meeting detection page context changed. Refresh before updating recording setup."
                );
            }
            if busy(&runtime) && mode != state.mode {
                bail!("Finish active recording or processing before changing capture mode.");
            }
            if mode != state.mode || state.mode_needs_repair {
                if busy(&runtime) {
                    bail!("Finish active work before repairing or changing capture mode.");
                }
                save_value(&self.root, MODE_KEY, &mode)?;
                state.mode_needs_repair = false;
            }
            if mode != state.mode || !state.ready {
                state.mode = mode;
                state.reset_evidence();
            } else if state.setup_open != setup_open {
                state.suspend_automatic();
            }
            state.ready = true;
            state.setup_open = setup_open;
        }
        self.snapshot()
    }
    pub fn dismiss_detection(&self, episode_id: &str, suppress: bool) -> Result<Snapshot> {
        {
            let runtime = self
                .runtime
                .lock()
                .map_err(|_| anyhow::anyhow!("Session state unavailable"))?;
            Self::ensure_not_maintenance(&runtime)?;
            let mut state = self
                .meeting_detection
                .lock()
                .map_err(|_| anyhow::anyhow!("Meeting detection state unavailable"))?;
            let id = state
                .episodes
                .iter()
                .find(|(_, episode)| episode.id == episode_id)
                .map(|(id, _)| id.clone())
                .ok_or_else(|| anyhow::anyhow!("That activity episode is no longer available."))?;
            if suppress {
                let mut settings = state.settings.clone();
                settings.rules.retain(|rule| rule.app_id != id);
                settings.rules.push(DetectionRule {
                    app_id: id.clone(),
                    policy: DetectionPolicy::Suppress,
                    participant_notice_attested: false,
                    input: None,
                });
                save_value(&self.root, SETTINGS_KEY, &settings)?;
                state.settings = settings;
            }
            state.revision = state.revision.wrapping_add(1);
            state.episodes.get_mut(&id).unwrap().dismissed = true;
            state.automatic_error = None;
        }
        self.snapshot()
    }
    pub fn dismiss_detection_ended(&self, episode_id: &str) -> Result<Snapshot> {
        {
            let _runtime = self
                .runtime
                .lock()
                .map_err(|_| anyhow::anyhow!("Session state unavailable"))?;
            let mut state = self
                .meeting_detection
                .lock()
                .map_err(|_| anyhow::anyhow!("Meeting detection state unavailable"))?;
            let source = state
                .association
                .as_mut()
                .filter(|source| source.episode_id == episode_id)
                .ok_or_else(|| {
                    anyhow::anyhow!("That recording reminder is no longer available.")
                })?;
            source.dismissed = true;
            source.ended = None;
        }
        self.snapshot()
    }
    pub fn start_detected_recording(
        self: &Arc<Self>,
        episode_id: &str,
        title: String,
        consent: bool,
        input_device: Option<String>,
    ) -> Result<Snapshot> {
        self.start_with_source(
            title,
            CaptureMode::Remote,
            consent,
            input_device,
            Some(DetectedStart {
                episode_id: episode_id.into(),
                automatic: false,
                checked_input: None,
                expected_revision: None,
                checked_at: None,
            }),
        )
    }
    fn automatic_input(&self, episode_id: &str, revision: u64) -> Result<(DetectionInput, u64)> {
        let input = {
            let runtime = self
                .runtime
                .lock()
                .map_err(|_| anyhow::anyhow!("Session state unavailable"))?;
            if busy(&runtime) {
                bail!("Finish active work first.");
            }
            let state = self
                .meeting_detection
                .lock()
                .map_err(|_| anyhow::anyhow!("Meeting detection state unavailable"))?;
            if state.revision != revision || !state.context_allows() {
                bail!("Meeting detection context changed during preflight.");
            }
            let id = state
                .episodes
                .iter()
                .find(|(_, episode)| episode.id == episode_id)
                .map(|(id, _)| id)
                .ok_or_else(|| anyhow::anyhow!("Activity episode expired"))?;
            state
                .rule(id)
                .input
                .ok_or_else(|| anyhow::anyhow!("Choose a microphone for Always record."))?
        };
        // Read-only and bounded; native capture independently rechecks the
        // selected device and Bluetooth guard at its actual capture boundary.
        let began = self
            .resources
            .observation_ns()
            .ok_or_else(|| anyhow::anyhow!("The observation clock is unavailable."))?;
        let devices = vt_capture::list_devices(&self.helper)?;
        let ended = self
            .resources
            .observation_ns()
            .ok_or_else(|| anyhow::anyhow!("The observation clock is unavailable."))?;
        if ended < began || ended - began > FRESH_NS {
            bail!("Microphone preflight expired during a system suspension.");
        }
        match &input {
            DetectionInput::Default => {
                if !devices
                    .devices
                    .iter()
                    .any(|device| device.is_default && !device.bluetooth)
                {
                    bail!(
                        "Automatic recording needs an available non-Bluetooth default microphone. Open setup to choose an input explicitly."
                    );
                }
            }
            DetectionInput::Device { id } => {
                if !devices.devices.iter().any(|device| device.id == *id) {
                    bail!("The saved microphone is unavailable. Open setup to choose an input.");
                }
            }
        }
        Ok((input, began))
    }
    fn start_evidence(&self) -> Option<StartEvidence> {
        let id = {
            let _runtime = self.runtime.lock().ok()?;
            self.meeting_detection
                .lock()
                .ok()?
                .association
                .as_ref()
                .filter(|source| source.start_notification_pending)
                .map(|source| source.meeting_id.clone())?
        };
        // Exists on the committed recorder manifest: even quiet PCM counts.
        // No permission/clock/status message alone is evidence of saved audio.
        let db = store::open(&self.root).ok()?;
        db.query_row("SELECT status='failed',EXISTS(SELECT 1 FROM audio_chunks WHERE meeting_id=?1 AND state='ready' AND frame_count>0) FROM app_meetings WHERE id=?1",[&id],|row|Ok(StartEvidence{meeting_id:id.clone(),failed:row.get(0)?,ready:row.get(1)?})).ok()
    }
    #[cfg(test)]
    fn apply_detection(self: &Arc<Self>, snapshot: MeetingAppSnapshot) {
        self.apply_detection_at(snapshot, self.resources.observation_ns());
    }
    fn apply_detection_at(
        self: &Arc<Self>,
        snapshot: MeetingAppSnapshot,
        observed_at: Option<u64>,
    ) {
        if self.shutting_down.load(Ordering::Relaxed) {
            return;
        }
        let evidence = self.start_evidence();
        let Some(now) = self.resources.observation_ns() else {
            let _runtime = self
                .runtime
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            self.meeting_detection
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .clock_unavailable();
            return;
        };
        let Some(observed_at) = observed_at.filter(|at| now >= *at && now - *at <= FRESH_NS) else {
            let _runtime = self
                .runtime
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            self.meeting_detection.lock().unwrap_or_else(|error|error.into_inner()).unknown(Some("Meeting app probe expired during suspension or its observation clock is unavailable.".into()));
            return;
        };
        let (candidate, sink, messages) = {
            let Ok(runtime) = self.runtime.lock() else {
                return;
            };
            if self.shutting_down.load(Ordering::Relaxed) {
                return;
            }
            let mut state = self
                .meeting_detection
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            state.observe(
                snapshot,
                observed_at,
                recording_id(&runtime),
                busy(&runtime),
            );
            let messages = state.notifications(
                now,
                busy(&runtime),
                recording_id(&runtime),
                evidence.as_ref(),
            );
            let candidate = state
                .auto_candidate(now, busy(&runtime))
                .map(|episode| (episode, state.revision));
            (candidate, state.notifier.clone(), messages)
        };
        notify(sink, messages);
        if let Some((episode_id, revision)) = candidate {
            let result =
                self.automatic_input(&episode_id, revision)
                    .and_then(|(input, checked_at)| {
                        self.start_with_source(
                            String::new(),
                            CaptureMode::Remote,
                            true,
                            None,
                            Some(DetectedStart {
                                episode_id: episode_id.clone(),
                                automatic: true,
                                checked_input: Some(input),
                                expected_revision: Some(revision),
                                checked_at: Some(checked_at),
                            }),
                        )
                    });
            if result.is_err() {
                let Ok(runtime) = self.runtime.lock() else {
                    return;
                };
                let mut state = self
                    .meeting_detection
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                if state.revision == revision
                    && !busy(&runtime)
                    && state.context_allows()
                    && let Some(episode) = state
                        .episodes
                        .values_mut()
                        .find(|episode| episode.id == episode_id && !episode.dismissed)
                {
                    episode.auto_attempted = true;
                    state.automatic_error=Some("Automatic recording did not start. Open recording setup to review permissions and microphone settings.".into());
                }
            }
        }
    }
    pub fn start_meeting_detection(self: &Arc<Self>) -> Result<()> {
        let mut slot = self
            .detection_worker
            .lock()
            .map_err(|_| anyhow::anyhow!("Meeting detection worker unavailable"))?;
        if slot.is_some() {
            return Ok(());
        }
        if self.shutting_down.load(Ordering::Relaxed) {
            bail!("VanTranscribe is closing.");
        }
        *slot = Some(DetectionWorker::start(Arc::downgrade(self))?);
        Ok(())
    }
    pub(crate) fn signal_detection_stop(&self) {
        if let Ok(slot) = self.detection_worker.lock()
            && let Some(worker) = slot.as_ref()
        {
            let _ = worker.stop.try_send(());
        }
    }
    pub(crate) fn join_detection(&self) {
        let worker = self
            .detection_worker
            .lock()
            .ok()
            .and_then(|mut slot| slot.take());
        drop(worker);
    }
}
pub(crate) struct DetectionWorker {
    stop: mpsc::SyncSender<()>,
    handle: Option<thread::JoinHandle<()>>,
}
impl DetectionWorker {
    fn start(service: Weak<AppService>) -> Result<Self> {
        let (stop, rx) = mpsc::sync_channel(1);
        let handle = thread::Builder::new()
            .name("meeting-detection".into())
            .spawn(move || {
                loop {
                    if rx.try_recv().is_ok() {
                        break;
                    }
                    let began = Instant::now();
                    if let Some(service) = service.upgrade() {
                        if service.shutting_down.load(Ordering::Relaxed) {
                            break;
                        }
                        let enabled = service
                            .meeting_detection
                            .lock()
                            .map(|state| {
                                state.settings.enabled && state.mode == CaptureMode::Remote
                            })
                            .unwrap_or(false);
                        if enabled {
                            let observed_at = service.resources.observation_ns();
                            let result = observed_at
                                .map(|_| vt_capture::meeting_apps(&service.helper))
                                .transpose()
                                .map(|result| {
                                    result.unwrap_or(MeetingAppSnapshot {
                                        complete: false,
                                        apps: Vec::new(),
                                        error: Some("Observation clock unavailable".into()),
                                    })
                                })
                                .unwrap_or_else(|_| MeetingAppSnapshot {
                                    complete: false,
                                    apps: Vec::new(),
                                    error: Some(
                                        "Meeting app observation failed or timed out.".into(),
                                    ),
                                });
                            if rx.try_recv().is_ok() {
                                break;
                            }
                            service.apply_detection_at(result, observed_at);
                        }
                    } else {
                        break;
                    }
                    match rx.recv_timeout(POLL.saturating_sub(began.elapsed())) {
                        Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        Err(mpsc::RecvTimeoutError::Timeout) => (),
                    }
                }
            })?;
        Ok(Self {
            stop,
            handle: Some(handle),
        })
    }
}
impl Drop for DetectionWorker {
    fn drop(&mut self) {
        let _ = self.stop.try_send(());
        if let Some(handle) = self.handle.take()
            && handle.thread().id() != thread::current().id()
        {
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
#[path = "meeting_detection_tests.rs"]
mod tests;
