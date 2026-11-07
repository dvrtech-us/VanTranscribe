//! Explicit offline cache installation and bounded-file refinement smoke test.
use std::{
    path::Path,
    sync::{Arc, atomic::AtomicBool},
};
use vt_core::TrackKind;
use vt_stt::{BatchRefiner, RefinementModelManager};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() == 5 && args[1] == "install-cache" {
        let manager = RefinementModelManager::new(args[2].clone().into());
        manager.install_from_archives(
            Path::new(&args[3]),
            Path::new(&args[4]),
            Arc::new(AtomicBool::new(false)),
        )?;
        println!("{}", manager.model_dir().display());
        return Ok(());
    }
    if args.len() != 3 {
        return Err(
            "usage: refine MODEL_DIR WAV | refine install-cache MODELS_ROOT ARCHIVE VAD".into(),
        );
    }
    let wave = sherpa_onnx::Wave::read(&args[2]).ok_or("invalid WAV")?;
    let rate = wave.sample_rate() as u32;
    let mut engine = BatchRefiner::new(Path::new(&args[1]))?;
    for (i, packet) in wave.samples().chunks((rate / 10) as usize).enumerate() {
        for segment in engine.accept(TrackKind::Mic, packet, rate, i as u64 * 100)? {
            println!("{segment:?}")
        }
    }
    for segment in engine.finish()? {
        println!("{segment:?}")
    }
    Ok(())
}
