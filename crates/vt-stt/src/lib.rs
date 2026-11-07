//! Local CPU streaming recognition and bounded offline Parakeet refinement.
//! All inference belongs on a background worker, never a capture callback.
//! Only explicit model-manager installation calls access the network.

mod live_vad;
mod models;
mod recognize;
mod refine;
mod refinement_models;
mod vad_models;

pub use live_vad::LiveVadOptions;
pub use models::{
    ARCHIVE_BYTES, LiveModel, LiveModelFile, MODEL_ID, MODEL_LICENSE, MODEL_SOURCE, ModelManager,
};
pub use recognize::{StreamingRecognizer, TranscriptEvent};
pub use refine::{BatchRefiner, RefinedSegment, WordTiming};
pub use refinement_models::{
    REFINEMENT_ARCHIVE_BYTES, REFINEMENT_ARCHIVE_SHA256, REFINEMENT_FILES, REFINEMENT_MODEL_ID,
    REFINEMENT_MODEL_LICENSE, REFINEMENT_MODEL_SOURCE, REFINEMENT_SETUP_BYTES,
    RefinementModelManager,
};
pub use vad_models::{
    VAD_MODEL_BYTES, VAD_MODEL_FILE, VAD_MODEL_ID, VAD_MODEL_LICENSE, VAD_MODEL_SHA256,
    VAD_MODEL_SOURCE, VadModelManager,
};

#[derive(Debug, thiserror::Error)]
pub enum SttError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Network(#[from] reqwest::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("model installation was cancelled")]
    Cancelled,
    #[error("model setup is already running")]
    Busy,
    #[error("model installation is invalid: {0}")]
    InvalidModel(String),
    #[error("speech recognizer could not be initialized; verify or reinstall its model")]
    Engine,
    #[error("invalid audio input: {0}")]
    InvalidAudio(String),
    #[error("invalid live VAD options: {0}")]
    InvalidVadOptions(String),
}

pub type Result<T> = std::result::Result<T, SttError>;
