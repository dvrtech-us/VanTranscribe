# Initial Architecture Decisions

- Date: 2026-11-06
- Related: design.md

## A. Tauri 2 + Rust core

### Context
VanTranscribe needs low-level audio capture (Core Audio taps, WASAPI), a real-time pipeline, local ML inference, and a web-style UI, on macOS first and Windows later (design.md sections 1, 4).

### Decision
Use Tauri 2 with a Rust core (workspace crates) for the app. The user chose Tauri + Rust.

### Alternatives Considered
- Swift-native: strong macOS integration, but no Windows support.
- Electron: higher memory and battery cost.
- C#/.NET: the organization default for desktop, but a weaker audio/ML ecosystem for this use case.

### Consequences
- Positive: one shared codebase above the platform layer (section 17); lean runtime; Rust suits real-time audio.
- Negative: Apple-only APIs (FoundationModels) need a Swift bridge; macOS process-tap bindings in Rust are a risk (R2).

## B. Svelte + TypeScript + Bootstrap 5 UI

### Context
Tauri provides a web UI layer; the project needs a light, keyboard-first UI with light and dark themes (section 13).

### Decision
Build the UI in Svelte + TypeScript with Bootstrap 5.

### Alternatives Considered
Not enumerated in design.md.

### Consequences
- Positive: Bootstrap aligns with the organization's web default; MIT/Apache-licensed stack (section 16).
- Negative: none recorded in design.md.

## C. Two-track capture with Me/Them split

### Context
Remote meetings contain the local user and remote participants; diarization of a mixed signal is harder (section 5.1). Self-capture of recorded playback must be avoided.

### Decision
Record Mic and System as independent tracks and never mix them for processing. Mic is labeled "Me" in remote mode; System is diarized into Speaker N. On macOS 14.4+, the default System backend is a Core Audio process tap that excludes the app's own PID.

### Alternatives Considered
- `cpal` 0.17 loopback: acceptable for M0 spike, but may not support excluding our process (to verify).
- ScreenCaptureKit audio: fallback for macOS 13.x to 14.3; needs Screen Recording permission.

### Consequences
- Positive: reliable Me vs. Them with no ML; halves the diarization problem; no self-capture.
- Negative: laptop speakers cause bleed into the mic, requiring AEC and de-duplication (section 5.4); System Audio Recording permission cannot be queried, only inferred (R3).

## D. sherpa-onnx as the single local ML runtime

### Context
The pipeline needs STT, VAD, speaker embeddings and diarization, cross-platform (sections 6, 7).

### Decision
Use sherpa-onnx (ONNX Runtime; CoreML EP on macOS, DirectML/CPU on Windows) for all of these, through the `sherpa-rs` binding, falling back to a thin hand-maintained FFI over sherpa-onnx's C API if `sherpa-rs` is incomplete.

### Alternatives Considered
Parakeet EOU (FluidAudio, Swift), Kyutai STT, whisper.cpp sliding window for live STT; `whisper-rs` as multilingual fallback (section 6).

### Consequences
- Positive: one runtime for STT, VAD and embeddings; cross-platform.
- Negative: dependent on `sherpa-rs` completeness (R1), to be spiked in M0.

## E. Two-pass transcript

### Context
Published streaming diarization error is about 30% DER vs. about 15% offline (section 7.1).

### Decision
Run a fast live streaming pass for instant feedback, then a post-meeting refinement pass: batch STT with Parakeet TDT 0.6B v3 plus offline diarization, then reconcile labels (sections 7.3, 8).

### Alternatives Considered
A single live pass only; cloud STT with built-in diarization (opt-in).

### Consequences
- Positive: instant feedback and high final accuracy; user renames carry over via reconciliation.
- Negative: live speaker labels are noisier and the UI must say they are refined later; extra processing time (target 3 min per 1 h meeting).

## F. Pluggable Summarizer

### Context
Providers differ widely in context size, from about 4K tokens (Apple Foundation Models, to verify) to large cloud contexts (section 9).

### Decision
Define a `Summarizer` trait with providers: Apple Foundation Models, local OpenAI-compatible server, cloud (user-supplied key), or none. Use map-reduce when the transcript exceeds about 60% of context, single pass otherwise.

### Alternatives Considered
A single fixed provider.

### Consequences
- Positive: privacy-first default with cloud opt-in; the small Apple model is usable on long meetings.
- Negative: Swift bridge needed for Apple FM; prompt and output quality varies by provider.

## G. Opt-in encrypted voiceprints

### Context
Cross-meeting speaker memory needs stored voice embeddings, which are biometric data (Illinois BIPA, GDPR Art. 9; sections 7.4, 11).

### Decision
Voiceprints are off by default, require an explicit toggle and explanation, are stored only locally with a separate column-level encryption key, and can be deleted individually or all at once.

### Alternatives Considered
No speaker memory (manual rename only, the v1 baseline).

### Consequences
- Positive: reduced legal exposure; user control.
- Negative: feature is delivered later (M4); added key-management complexity.

## H. Models downloaded on demand; MIT app

### Context
Model licenses vary (CC-BY-4.0, MIT, Apache-2.0, gated or NVIDIA licenses) (section 16).

### Decision
License the app MIT. Never bundle models; download on first run from canonical sources with a sha256 manifest, showing license and attribution in Settings and About.

### Alternatives Considered
Bundling models in the installer.

### Consequences
- Positive: small installer; license changes isolated (R7); defaults prefer Apache/MIT/CC-BY models.
- Negative: first-run download (about 1 GB Balanced) and a network dependency for setup.
