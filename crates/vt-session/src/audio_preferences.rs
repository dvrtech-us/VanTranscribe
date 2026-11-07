use anyhow::Result;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioPreferences {
    pub live_vad_enabled: bool,
    pub diarization_enabled: bool,
    pub echo_cancellation_enabled: bool,
}

impl Default for AudioPreferences {
    fn default() -> Self {
        Self {
            live_vad_enabled: true,
            diarization_enabled: false,
            echo_cancellation_enabled: true,
        }
    }
}

pub(crate) fn load(root: &Path) -> Result<AudioPreferences> {
    let saved: Option<String> = super::store::open(root)?
        .query_row(
            "SELECT value_json FROM app_settings WHERE key='audio_preferences'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    saved
        .map(|json| serde_json::from_str(&json))
        .transpose()
        .map(Option::unwrap_or_default)
        .map_err(Into::into)
}

pub(crate) fn save(root: &Path, prefs: &AudioPreferences) -> Result<()> {
    super::store::open(root)?.execute(
        "INSERT INTO app_settings(key,value_json) VALUES ('audio_preferences',?1) ON CONFLICT(key) DO UPDATE SET value_json=excluded.value_json",
        [serde_json::to_string(prefs)?]
    )?;
    Ok(())
}
