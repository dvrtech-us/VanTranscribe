//! Explicit live acoustic-model choice; full application profiles remain separate.
use super::*;
use rusqlite::OptionalExtension;
use vt_stt::LiveModel;
const KEY: &str = "live_model";

#[derive(Clone, Debug, Serialize)]
pub struct LiveModelChoice {
    pub model: LiveModel,
    pub title: String,
    pub recommended: bool,
    pub model_id: String,
    pub installed: bool,
    pub download_bytes: u64,
    pub installed_bytes: u64,
    pub source: String,
    pub license: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct LiveModelCatalog {
    pub selected: LiveModel,
    pub choices: Vec<LiveModelChoice>,
    pub selection_warning: Option<String>,
}
pub(super) struct Selection {
    pub model: LiveModel,
    pub warning: Option<String>,
}
fn fallback_model(root: &Path) -> Result<LiveModel> {
    if ModelManager::for_model(root.join("models"), LiveModel::EnglishSmall)
        .model_dir()
        .try_exists()?
    {
        Ok(LiveModel::EnglishSmall)
    } else {
        Ok(LiveModel::EnglishStandard)
    }
}
pub(super) fn selection(root: &Path) -> Result<Selection> {
    let saved: Option<String> = store::read(root, |db| {
        Ok(db
            .query_row(
                "SELECT value_json FROM app_settings WHERE key=?1",
                [KEY],
                |r| r.get(0),
            )
            .optional()?)
    })?;
    if let Some(saved) = saved {
        return match serde_json::from_str(&saved) {
            Ok(model) => Ok(Selection { model, warning: None }),
            Err(_) => Ok(Selection { model: fallback_model(root)?, warning: Some("The saved live-model choice is invalid or unsupported. Confirm a supported model in Settings before using live transcription.".into()) }),
        };
    }
    Ok(Selection {
        model: fallback_model(root)?,
        warning: None,
    })
}
#[cfg(test)]
pub(super) fn selected(root: &Path) -> Result<LiveModel> {
    Ok(selection(root)?.model)
}
pub(super) fn manager(root: &Path) -> Result<ModelManager> {
    let selection = selection(root)?;
    if let Some(warning) = selection.warning {
        bail!("{warning}");
    }
    Ok(ModelManager::for_model(
        root.join("models"),
        selection.model,
    ))
}
pub(super) fn vad_options(model: LiveModel) -> vt_stt::LiveVadOptions {
    vt_stt::LiveVadOptions {
        threshold: match model {
            LiveModel::EnglishSmall => 0.5,
            LiveModel::EnglishStandard => 0.2,
        },
    }
}
pub(super) fn recognizer(
    manager: &ModelManager,
    vad: Option<&Path>,
) -> Result<vt_stt::StreamingRecognizer> {
    if !manager.installed() {
        bail!(
            "The selected English model is missing, incomplete or from a different catalog. Verify or repair it in Settings."
        );
    }
    let directory = manager.model_dir();
    match vad {
        Some(path) => Ok(vt_stt::StreamingRecognizer::with_vad_options(
            &directory,
            path,
            vad_options(manager.model()),
        )?),
        None => Ok(vt_stt::StreamingRecognizer::new(&directory)?),
    }
}
pub(super) fn provenance(root: &Path, enabled: bool) -> Result<String> {
    provenance_body(root, enabled, None)
}
pub(super) fn capture_provenance(
    root: &Path,
    enabled: bool,
    inference_suspended: bool,
) -> Result<String> {
    provenance_body(root, enabled, Some(inference_suspended))
}
fn provenance_body(root: &Path, enabled: bool, capture: Option<bool>) -> Result<String> {
    let selection = selection(root)?;
    let model = selection.model;
    let audio = audio_preferences::load(root)?;
    let vad_requested = audio.live_vad_enabled;
    let mut metadata = serde_json::json!({"live_stt":{"enabled":enabled,"requested_model":selection.warning.is_none().then_some(model.id()),"selection_warning":selection.warning,"vad_requested":vad_requested,"requested_vad_options":vad_options(model),"archive_sha256":model.archive_sha256(),"provider":"cpu","threads":2,"runtime":"sherpa-onnx-2.0.1","files":model.files().iter().map(|file|serde_json::json!({"name":file.name,"bytes":file.bytes,"sha256":file.sha256})).collect::<Vec<_>>()}});
    if let Some(suspended) = capture {
        metadata["capture_inference_health_expected"] = serde_json::json!(
            !suspended
                && (enabled
                    || (audio.diarization_enabled
                        && vt_diarize::ModelManager::new(root.join("models")).installed()))
        );
        metadata["capture_inference_suspended_at_start"] = serde_json::json!(suspended);
    }
    Ok(metadata.to_string())
}
impl AppService {
    pub(crate) fn live_model_manager(&self) -> Result<ModelManager> {
        manager(&self.root)
    }
    pub fn live_model_catalog(&self) -> Result<LiveModelCatalog> {
        let selection = selection(&self.root)?;
        let selected = selection.model;
        let choices = [LiveModel::EnglishSmall, LiveModel::EnglishStandard]
            .into_iter()
            .map(|model| LiveModelChoice {
                model,
                recommended: model == LiveModel::EnglishStandard,
                title: match model {
                    LiveModel::EnglishSmall => "English · compact",
                    LiveModel::EnglishStandard => "English · standard",
                }
                .into(),
                model_id: model.id().into(),
                installed: ModelManager::for_model(self.root.join("models"), model).installed(),
                download_bytes: model.archive_bytes(),
                installed_bytes: model.installed_bytes(),
                source: model.source().into(),
                license: model.license().into(),
            })
            .collect();
        Ok(LiveModelCatalog {
            selected,
            choices,
            selection_warning: selection.warning,
        })
    }
    pub fn select_live_model(&self, model: LiveModel) -> Result<Snapshot> {
        {
            let mut runtime = self
                .runtime
                .lock()
                .map_err(|_| anyhow::anyhow!("Session state unavailable"))?;
            if self.shutting_down.load(Ordering::Relaxed) {
                bail!("VanTranscribe is closing.");
            }
            if runtime.processing
                || runtime.model.busy
                || runtime.current.as_ref().is_some_and(|current| {
                    matches!(
                        current.view.status.as_str(),
                        "recording" | "paused" | "processing"
                    )
                })
            {
                bail!(
                    "Finish recording, processing and speech-model setup before changing the live model."
                );
            }
            let json = serde_json::to_string(&model)?;
            store::write(&self.root, |db| {
                db.execute("INSERT INTO app_settings(key,value_json) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value_json=excluded.value_json", rusqlite::params![KEY,json])?;
                Ok(())
            })?;
            let installed = ModelManager::for_model(self.root.join("models"), model).installed();
            runtime.model = ModelStatus {
                installed,
                progress: if installed { 1.0 } else { 0.0 },
                ..Default::default()
            };
        }
        self.snapshot()
    }
}

#[cfg(test)]
#[path = "live_models_tests.rs"]
mod tests;
