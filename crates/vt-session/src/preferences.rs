use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;
use vt_summarize::{Provider, SummaryConfig};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    pub auto_refine: bool,
    pub auto_summarize: bool,
    pub summary: SummaryConfig,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            auto_refine: true,
            auto_summarize: true,
            summary: SummaryConfig::default(),
        }
    }
}

pub(crate) fn load(root: &Path) -> Result<Preferences> {
    // Loading creates the settings table, so it runs on the writer.
    let saved = super::store::write(root, |db| {
        db.execute_batch(include_str!(
            "../../../documentation/features/app/desktop-session/sql/003-preferences.sql"
        ))?;
        Ok(db.query_row(
            "SELECT value_json FROM app_settings WHERE key='preferences'",
            [],
            |r| r.get::<_, String>(0),
        ))
    })?;
    match saved {
        Ok(json) => Ok(serde_json::from_str(&json)?),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(Preferences::default()),
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn save(root: &Path, prefs: &Preferences) -> Result<()> {
    if prefs.summary.provider != Provider::None {
        vt_summarize::validate_config(&prefs.summary)?;
    }
    let json = serde_json::to_string(prefs)?;
    super::store::write(root, |db| {
        db.execute("INSERT INTO app_settings(key,value_json) VALUES ('preferences',?1) ON CONFLICT(key) DO UPDATE SET value_json=excluded.value_json",[json])?;
        Ok(())
    })
}
