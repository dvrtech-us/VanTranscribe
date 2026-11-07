//! Explicit setup of the standalone VAD from the pinned public file cache.
use std::{
    path::Path,
    sync::{Arc, atomic::AtomicBool},
};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 3 {
        return Err("usage: vad MODELS_ROOT CACHED_SILERO_ONNX".into());
    }
    let manager = vt_stt::VadModelManager::new(args[1].clone().into());
    manager.install_from_file(Path::new(&args[2]), Arc::new(AtomicBool::new(false)))?;
    manager.verify()?;
    println!("{}", manager.model_path().display());
    Ok(())
}
