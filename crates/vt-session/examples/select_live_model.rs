//! Explicit offline configuration for an idle, closed desktop app. The normal
//! desktop folder lock and model verification apply; no audio device is opened.
use std::path::PathBuf;
use vt_session::{AppService, LiveModel};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 2 {
        return Err(
            "usage: select_live_model <app-data-directory> english_small|english_standard".into(),
        );
    }
    let model = match args[1].as_str() {
        "english_small" => LiveModel::EnglishSmall,
        "english_standard" => LiveModel::EnglishStandard,
        _ => return Err("unknown live model".into()),
    };
    let root = PathBuf::from(&args[0]);
    vt_stt::ModelManager::for_model(root.join("models"), model).verify()?;
    let service = AppService::new(root.clone(), root.join(".configuration-helper-unused"))?;
    let snapshot = service.select_live_model(model)?;
    if !snapshot.models.installed {
        return Err("selected model verification is unavailable".into());
    }
    println!("Configured verified English model: {}", model.id());
    Ok(())
}
