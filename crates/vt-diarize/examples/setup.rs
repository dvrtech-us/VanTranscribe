//! Explicit developer setup; public model files only, never audio upload.
use std::{
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool},
};
fn main() {
    let mut args = std::env::args().skip(1);
    let root = args
        .next()
        .expect("usage: setup /path/to/models --acknowledge-source-notice");
    assert_eq!(
        args.next().as_deref(),
        Some("--acknowledge-source-notice"),
        "Read the model source/license acquisition notice before setup."
    );
    let manager = vt_diarize::ModelManager::new(PathBuf::from(root));
    manager
        .install(
            |done, total| {
                if done == total {
                    eprintln!("Verified {done} model download bytes")
                }
            },
            Arc::new(AtomicBool::new(false)),
            true,
        )
        .unwrap();
    println!("{}", manager.model_dir().display());
}
