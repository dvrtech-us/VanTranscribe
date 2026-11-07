//! Real local-model smoke test; never opens capture devices.
use std::{
    error::Error,
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool},
};
use vt_core::TrackKind;
use vt_stt::{ModelManager, StreamingRecognizer};

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--install-archive") && args.len() == 3 {
        let manager = ModelManager::new(PathBuf::from(&args[2]));
        manager.install_from_archive(
            std::path::Path::new(&args[1]),
            Arc::new(AtomicBool::new(false)),
        )?;
        println!(
            "Verified model installed at {}",
            manager.model_dir().display()
        );
        return Ok(());
    }
    if args.len() != 2 {
        return Err("usage: transcribe <models-root> <mono.wav>\n       transcribe --install-archive <archive.tar.bz2> <models-root>".into());
    }
    let manager = ModelManager::new(PathBuf::from(&args[0]));
    let wave = sherpa_onnx::Wave::read(&args[1]).ok_or("could not read mono WAV fixture")?;
    let mut engine = StreamingRecognizer::new(&manager.model_dir())?;
    let rate = wave.sample_rate() as u32;
    let mut offset = 0;
    for packet in wave.samples().chunks((rate / 10) as usize) {
        for result in engine.accept(
            TrackKind::Mic,
            packet,
            rate,
            offset * 1_000 / u64::from(rate),
        )? {
            if result.is_final {
                println!("{}..{} {}", result.start_ms, result.end_ms, result.text);
            }
        }
        offset += packet.len() as u64;
    }
    for result in engine.finish()? {
        println!("{}..{} {}", result.start_ms, result.end_ms, result.text);
    }
    Ok(())
}
