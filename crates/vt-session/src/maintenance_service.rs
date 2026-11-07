//! Runtime reservations for crash-safe storage maintenance. No user content is logged.
use super::*;
use chrono::{DateTime, Utc};
use serde::de::DeserializeOwned;
use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    thread,
    time::Duration,
};

const POLICY_KEY: &str = "maintenance_policy";
const LAST_CHECK_KEY: &str = "maintenance_last_success";
const LAST_ATTEMPT_KEY: &str = "maintenance_last_attempt";
const LAST_REPORT_KEY: &str = "maintenance_last_report";

fn load<T: DeserializeOwned>(root: &Path, key: &str) -> Result<Option<T>> {
    use rusqlite::OptionalExtension;
    let value: Option<String> = store::open(root)?
        .query_row(
            "SELECT value_json FROM app_settings WHERE key=?1",
            [key],
            |r| r.get(0),
        )
        .optional()?;
    value
        .map(|value| serde_json::from_str(&value).map_err(Into::into))
        .transpose()
}
fn save<T: Serialize>(root: &Path, key: &str, value: &T) -> Result<()> {
    store::open(root)?.execute("INSERT INTO app_settings(key,value_json) VALUES (?1,?2) ON CONFLICT(key) DO UPDATE SET value_json=excluded.value_json",rusqlite::params![key,serde_json::to_string(value)?])?;
    Ok(())
}
fn idle(runtime: &Runtime) -> Result<()> {
    if runtime.saved_audio_readers > 0 {
        bail!("Wait for saved audio to finish loading before changing stored recordings.");
    }
    if runtime.processing
        || runtime.current.as_ref().is_some_and(|c| {
            matches!(
                c.view.status.as_str(),
                "recording" | "paused" | "processing"
            )
        })
    {
        bail!("Finish recording and processing before changing stored recordings.");
    }
    Ok(())
}
impl AppService {
    fn with_maintenance<T>(
        &self,
        id: &str,
        stage: &str,
        stop_player: bool,
        job: impl FnOnce() -> Result<T>,
    ) -> Result<T> {
        if self.shutting_down.load(Ordering::Relaxed) {
            bail!("VanTranscribe is closing.");
        }
        {
            let mut runtime = self
                .runtime
                .lock()
                .map_err(|_| anyhow::anyhow!("Session state unavailable"))?;
            // Shutdown observes this reservation under the same mutex.
            if self.shutting_down.load(Ordering::Relaxed) {
                bail!("VanTranscribe is closing.");
            }
            idle(&runtime)?;
            runtime.processing = true;
            runtime.processing_view = Some(ProcessingView {
                meeting_id: id.into(),
                kind: "maintenance".into(),
                stage: stage.into(),
                progress: 0.0,
                error: None,
            });
        }
        let result=catch_unwind(AssertUnwindSafe(|| {
            if stop_player {audio_service::stop(self)?;}
            job()
        })).unwrap_or_else(|_|Err(anyhow::anyhow!("Storage maintenance was interrupted. Reopen the app to recover the confirmed operation.")));
        if let Ok(mut runtime) = self.runtime.lock() {
            runtime.processing = false;
            runtime.processing_view = None;
        }
        result
    }
    pub fn maintenance_policy(&self) -> Result<MaintenancePolicy> {
        let policy: MaintenancePolicy = load(&self.root, POLICY_KEY)?.unwrap_or_default();
        policy.validate()?;
        Ok(policy)
    }
    pub fn save_maintenance_policy(&self, policy: MaintenancePolicy) -> Result<MaintenancePolicy> {
        policy.validate()?;
        if self.shutting_down.load(Ordering::Relaxed) {
            bail!("VanTranscribe is closing.");
        }
        let runtime = self
            .runtime
            .lock()
            .map_err(|_| anyhow::anyhow!("Session state unavailable"))?;
        idle(&runtime)?;
        let mut db = store::open(&self.root)?;
        let tx = db.transaction()?;
        tx.execute("INSERT INTO app_settings(key,value_json) VALUES (?1,?2) ON CONFLICT(key) DO UPDATE SET value_json=excluded.value_json", rusqlite::params![POLICY_KEY,serde_json::to_string(&policy)?])?;
        // A newly saved policy may be applied at the next idle check.
        tx.execute(
            "DELETE FROM app_settings WHERE key IN (?1,?2)",
            [LAST_ATTEMPT_KEY, LAST_CHECK_KEY],
        )?;
        tx.commit()?;
        Ok(policy)
    }
    pub fn preview_deletion(&self, id: &str, kind: DeletionKind) -> Result<DeletionPreview> {
        self.with_maintenance(id, "Checking stored recording", false, || {
            retention::plan(&self.root, id, kind)
        })
    }
    pub fn commit_deletion(&self, token: &str) -> Result<MaintenanceReport> {
        let id = retention::meeting_for_token(&self.root, token)?;
        let mut attempted = false;
        let outcome = self.with_maintenance(&id, "Removing selected data", true, || {
            attempted = true;
            retention::commit(&self.root, token)
        });
        let mut report = match outcome {
            Ok(report) => report,
            Err(error) => {
                if attempted && retention::has_pending(&self.root, &id).unwrap_or(false) {
                    let report = MaintenanceReport {
                        pending_cleanup: vec![token.into()],
                        errors: vec![error.to_string()],
                        ..Default::default()
                    };
                    // Keep the original error even if disk pressure prevents
                    // the extra UI receipt. The confirmed intent stays durable.
                    let _ = save(&self.root, LAST_REPORT_KEY, &report);
                }
                return Err(error);
            }
        };
        if let Err(error) = save(&self.root, LAST_REPORT_KEY, &report) {
            report.errors.push(format!(
                "The operation finished, but its status could not be saved: {error}"
            ));
        }
        // A failed session can remain selected after its meeting is deleted.
        if let Ok(mut runtime) = self.runtime.lock()
            && let Some(current) = runtime.current.as_ref()
        {
            let exists = store::open(&self.root).and_then(|db| {
                db.query_row(
                    "SELECT EXISTS(SELECT 1 FROM app_meetings WHERE id=?1)",
                    [&current.view.id],
                    |r| r.get::<_, bool>(0),
                )
                .map_err(Into::into)
            });
            match exists {Ok(false)=>{runtime.current=None;runtime.partials.clear();},Ok(true)=>(),Err(error)=>report.errors.push(format!("The operation finished, but the session display could not be refreshed: {error}"))}
        }
        Ok(report)
    }
    pub fn preview_cleanup(&self) -> Result<PolicyPreview> {
        self.with_maintenance("", "Checking retention policy", false, || {
            retention::preview_policy(&self.root, &self.maintenance_policy()?)
        })
    }
    pub fn apply_cleanup(&self, preview: PolicyPreview) -> Result<MaintenanceReport> {
        let mut report =
            self.with_maintenance("", "Cleaning eligible stored data", true, || {
                let current = self.maintenance_policy()?;
                if serde_json::to_value(&current)? != serde_json::to_value(&preview.policy)? {
                    bail!("Retention preferences changed. Preview cleanup again.");
                }
                retention::apply_policy(&self.root, &preview)
            })?;
        if let Err(error) = save(&self.root, LAST_REPORT_KEY, &report) {
            report.errors.push(format!(
                "The operation finished, but its status could not be saved: {error}"
            ));
        }
        Ok(report)
    }
    pub fn last_maintenance_report(&self) -> Result<MaintenanceReport> {
        Ok(load(&self.root, LAST_REPORT_KEY)?.unwrap_or_default())
    }
    fn automatic_maintenance(&self) -> Result<()> {
        if self.shutting_down.load(Ordering::Relaxed) {
            return Ok(());
        }
        {
            let runtime = self
                .runtime
                .lock()
                .map_err(|_| anyhow::anyhow!("Session state unavailable"))?;
            if idle(&runtime).is_err() {
                return Ok(());
            }
            // Automatic maintenance does not interrupt saved-audio listening.
            if self
                .playback
                .lock()
                .map_err(|_| anyhow::anyhow!("Playback state unavailable"))?
                .as_ref()
                .is_some_and(|p| matches!(p.snapshot().status.as_str(), "playing" | "buffering"))
            {
                return Ok(());
            }
        }
        let now = Utc::now();
        let last_attempt =
            load::<DateTime<Utc>>(&self.root, LAST_ATTEMPT_KEY)?
                .or(load::<DateTime<Utc>>(&self.root, LAST_CHECK_KEY)?);
        if let Some(last) = last_attempt
            && now.signed_duration_since(last).num_seconds() < 3600
        {
            return Ok(());
        }
        let report = self.with_maintenance("", "Applying retention policy", false, || {
            // Recheck after reserving the runtime: a Play request may have won
            // the race between the idle check and this reservation.
            if self
                .playback
                .lock()
                .map_err(|_| anyhow::anyhow!("Playback state unavailable"))?
                .as_ref()
                .is_some_and(|p| matches!(p.snapshot().status.as_str(), "playing" | "buffering"))
            {
                return Ok(None);
            }
            // Record attempts as well as successes, so a persistent failure
            // cannot regenerate full inventories on every scheduler tick.
            save(&self.root, LAST_ATTEMPT_KEY, &now)?;
            // Biometric expiry is keyless and applies even when memory is off.
            vt_speakers::expire(&store::open(&self.root)?, now)?;
            audio_service::stop(self)?;
            let mut report = retention::recover(&self.root)?;
            let preview = retention::preview_policy(&self.root, &self.maintenance_policy()?);
            match preview {
                Ok(preview) => {
                    let current = retention::apply_policy(&self.root, &preview)?;
                    report.completed += current.completed;
                    report.meetings_deleted += current.meetings_deleted;
                    report.audio_purged += current.audio_purged;
                    report.runs_deleted += current.runs_deleted;
                    report.bytes_reclaimed += current.bytes_reclaimed;
                    report.pending_cleanup.extend(current.pending_cleanup);
                    report.errors.extend(current.errors);
                }
                Err(error) => report.errors.push(error.to_string()),
            }
            Ok(Some(report))
        })?;
        let Some(report) = report else {
            return Ok(());
        };
        save(&self.root, LAST_REPORT_KEY, &report)?;
        if report.errors.is_empty() && report.pending_cleanup.is_empty() {
            save(&self.root, LAST_CHECK_KEY, &now)?;
        }
        Ok(())
    }
    /// Called once by the desktop shell. Tests opt in explicitly; the weak
    /// worker never keeps a recordings folder or app service alive while idle.
    pub fn start_maintenance(self: &Arc<Self>) -> Result<()> {
        let weak = Arc::downgrade(self);
        thread::Builder::new()
            .name("storage-maintenance".into())
            .spawn(move || {
                let mut seconds = 0u16;
                loop {
                    thread::sleep(Duration::from_secs(1));
                    let Some(service) = weak.upgrade() else {
                        break;
                    };
                    if service.shutting_down.load(Ordering::Relaxed) {
                        break;
                    }
                    seconds += 1;
                    if seconds == 60 {
                        seconds = 0;
                        if let Err(error) = service.automatic_maintenance() {
                            let report = MaintenanceReport {
                                errors: vec![error.to_string()],
                                ..Default::default()
                            };
                            let _ = save(&service.root, LAST_REPORT_KEY, &report);
                        }
                    }
                }
            })?;
        Ok(())
    }
}

pub(super) fn recover_startup(root: &Path) -> Result<()> {
    vt_speakers::expire(&store::open(root)?, Utc::now())?;
    let report = retention::recover(root)?;
    if report.completed > 0 || !report.pending_cleanup.is_empty() || !report.errors.is_empty() {
        // Recovery is already durable. A receipt write must not prevent the
        // library from opening, especially when disk pressure caused the fault.
        if save(root, LAST_REPORT_KEY, &report).is_err() {
            eprintln!("Storage recovery completed, but its status could not be saved.");
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "maintenance_service_tests.rs"]
mod tests;
