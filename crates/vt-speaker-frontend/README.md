# Pinned reference-style native frontend

This optional research crate constructs a real `OnlineStream` with the pinned
public C++ `FeatureExtractorConfig` and passes that owned stream to the real
`SpeakerEmbeddingExtractor::Compute`. It never reinterprets an opaque sherpa C
handle, replaces a class declaration, or changes a trained model. SDK headers
and original notices are hash-verified against `vendor/provenance.json`; native
version `2.0.1`, commit `af2d8d81`, and ONNX Runtime `1.28.2` must agree before any
custom C++ class is constructed. Supported native target: macOS arm64.

`ReferenceEmbedder::new(&model_file)` accepts only the exact pinned CMN ONNX
file (SHA-256 `6928c40c3b5c695352998c261cb6f2250c4e52a991e324c0f63c2e10fe7c31f8`,
26,534,404 bytes). `extract(&mut self, &[f32])` accepts 16,000–80,000 finite
normalized mono samples in `[-1,1]` at 16 kHz and returns RAW finite native
256-dimensional output. Existing higher-level code applies its common L2
normalization. The handle is owned, not cloned, not Send/Sync, and destroyed by
the same C++ bridge that allocates it. No downloader, device, key, logging or
persistent-vector API is present. The caller verifies pack provenance/notices.

The isolated unsafe Rust module documents each C ABI pointer, ownership and
buffer lifetime. C++ functions catch exceptions and return static status codes.
Each inference call owns a new stream. The frontend uses Hamming, snipped edges,
80 mel bins, low/high 20/0 Hz, 25/10 ms frames, dither zero, normalized-input
conversion to Kaldi amplitude, DC removal, preemphasis 0.97, and a power-of-two
FFT with the pinned native Fbank power-spectrum implementation. Raw log-mel parity with `torchaudio.compliance.kaldi.fbank` 2.11.0 was measured
before identity scoring. The predeclared maximum absolute error is 0.001.
Synthetic fixtures and one frozen calibration window passed; the largest
absolute error was 0.00033665. That measurement is not an identity or
model-quality acceptance claim. The calibration-only result is recorded in
`documentation/planning/Architecture/2026-11-07-english-speaker-calibration-comparison.md`.

Build/test offline with the existing pinned sherpa static libraries. The
`sherpa-onnx-sys=2.0.1` dependency remains the only library locator/link owner;
this crate adds no library-cache path guessing or native download operation.
For a clean environment, explicitly point `SHERPA_ONNX_LIB_DIR` at an already
acquired matching static SDK library directory before invoking Cargo, because
that upstream dependency can acquire prebuilt libraries when its cache is absent.

```sh
cargo test --offline -p vt-speaker-frontend --features feature-probe
cargo clippy --offline -p vt-speaker-frontend --all-targets --features feature-probe -- -D warnings
```

`reference_features` is available only under `feature-probe` (and unit tests).
It returns `FeatureMatrix { frames, bins, values }` in row-major order before
extractor CMN. For explicitly generated synthetic input only:

```sh
cargo run --offline -p vt-speaker-frontend --features feature-probe \
  --example feature_probe -- --input /path/to/synthetic.f32le \
  --output /path/to/new-probe.bin
```

The new output contains one UTF-8 JSON header line followed by raw row-major
little-endian float32 values. The header records frames, bins, value byte count
and SHA-256. No model is opened by this probe. Files and devices are never read
unless explicitly named by the developer. The opt-in extractor lifecycle test
uses `VT_REFERENCE_FRONTEND_MODEL` pointing to the exact CMN ONNX file and
synthetic generated tones; it computes no person-identity score.
