//! Explicit offline installation of a pinned public acoustic model and optional
//! pinned Silero cache. Never opens audio devices or changes an app preference.
use std::{
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool},
};
use vt_stt::{LiveModel, ModelManager, VadModelManager};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if !(args.len() == 3 || args.len() == 4) {
        return Err("usage: install_live_model english_small|english_standard <models-root> <official-archive.tar.bz2> [verified-silero.onnx]".into());
    }
    let model = match args[0].as_str() {
        "english_small" => LiveModel::EnglishSmall,
        "english_standard" => LiveModel::EnglishStandard,
        _ => return Err("unknown live model".into()),
    };
    let root = PathBuf::from(&args[1]);
    let manager = ModelManager::for_model(root.clone(), model);
    manager.install_from_archive(&PathBuf::from(&args[2]), Arc::new(AtomicBool::new(false)))?;
    manager.verify()?;
    if let Some(vad) = args.get(3) {
        let vad_manager = VadModelManager::new(root);
        vad_manager.install_from_file(&PathBuf::from(vad), Arc::new(AtomicBool::new(false)))?;
        vad_manager.verify()?;
    }
    println!("Verified pinned model: {}", model.id());
    Ok(())
}
