# Offline batch refinement

VanTranscribe's `vt-stt` library refines recorded mono audio using the actual NVIDIA Parakeet-TDT 0.6B v3 INT8 model through sherpa-onnx 2.0.1's offline `nemo_transducer` recognizer. It is separate from live Zipformer recognition. All inference is local and uses the CPU provider.

## User flow

1. The application explicitly offers refinement-model setup. `RefinementModelManager::install` downloads the official 487,170,055-byte Parakeet archive and 643,854-byte Silero VAD model (487,813,909 bytes total).
2. Setup reports progress and accepts cancellation. Hash verification and extraction complete before a candidate replaces an installation.
3. The session worker opens `BatchRefiner`, sends no more than two seconds of finite mono audio per call with the track and session time, and persists returned refined segments/words.
4. Explicit input gaps call `reset_track`; `finish` flushes both independent tracks. Missing/corrupt models return an error; there is no streaming-model substitution.

## Technical flow

`BatchRefiner::new` verifies every installed model hash before creating one shared two-thread offline recognizer and one Silero VAD per track. A persistent sherpa sinc resampler converts each track to 16kHz; it preserves filter state across packets. A rate change or timestamp discontinuity over 5ms flushes and resets that track. Valid sample rates are 8–192kHz. NaN/infinity, oversized chunks, invalid rates and overflowing time ranges are rejected before native calls.

Silero receives at most 512 frames per native call, with threshold 0.5, minimum speech 0.2 seconds and minimum silence 0.35 seconds. Its built-in onset context is retained. Native `max_speech_duration=28` is a soft threshold adjustment, so the library additionally flushes and resets VAD at nonoverlapping 29-second epochs. Even continuous speech cannot accumulate a multi-hour native waveform. Each region includes at most 200ms of decoder-only zero tail and remains under 30 seconds. Real timestamp bounds exclude that tail. Adjacent epochs never submit duplicate audio; hard cuts can reduce recognition quality for a word crossing the boundary. The application should not claim forced alignment precision.

Parakeet produces space-prefixed BPE pieces with native acoustic token timestamps. `merge_words` combines pieces and punctuation into words. Native token durations are used when available; otherwise an end is bounded by the last token's 80ms encoder frame and the next word/real region end. These are frame-level acoustic estimates, not uniform divisions of sentence duration or phoneme-level forced alignment. Missing, nonfinite, mismatched or decreasing token alignments fail explicitly. Model punctuation/capitalization are preserved in segment text.

## Key classes

| Component | Source | Responsibility |
|---|---|---|
| `RefinementModelManager` | `crates/vt-stt/src/refinement_models.rs` | Explicit model installation, cheap metadata polling and full integrity checks |
| `BatchRefiner` | `crates/vt-stt/src/refine.rs` | Two-track bounded VAD, resampling and offline recognition |
| `RefinedSegment`, `WordTiming` | `crates/vt-stt/src/refine.rs` | Session-relative aligned output |
| `extract_catalog` | `crates/vt-stt/src/models.rs` | Shared archive safeguards also used by live-model setup |
| `refine` example | `crates/vt-stt/examples/refine.rs` | Explicit verified cache import and fixture transcription |

## Installation and provenance

`REFINEMENT_FILES` exports `(filename, bytes, sha256)` for all three ONNX weights, tokens and VAD; `REFINEMENT_MODEL_ID`, `REFINEMENT_MODEL_SOURCE`, `REFINEMENT_ARCHIVE_SHA256` identify the exact release. Session processing runs should persist these identifiers. The installed manifest includes full model/VAD license texts, attribution, conversion and public acquisition terms. The bundle's `vt-stt/resources/REFINEMENT-NOTICE.txt`, `CC-BY-4.0.txt`, `silero-vad-LICENSE.txt`, and native license resources retain notices independently of a download.

`installed` checks metadata and catalog identity only; `verify` and recognizer construction hash all files. HTTPS downloads permit only GitHub and official release-asset redirects, cap redirects at five, enforce exact sizes/hashes, check cancellation every 100ms during requests, and time out a stalled response after 30 seconds. Extraction shares the existing safety checks: expected root, normal relative paths only, no links, bounded expansion (2GiB), entry count and selected files only. Cancellation checks occur during extraction and hashing. A shared install lock prevents concurrent model mutation. Publication syncs files/directories and restores the previous installation if candidate rename fails.

`install_from_archive(archive, cancel)` requires a canonical `silero_vad.onnx` sibling; `install_from_archives(archive, vad, cancel)` supports arbitrary cache paths and performs the same hash/publication checks.

## Integration points and access control

Only explicit setup performs network requests. Inference runs on the session worker, never in a capture callback. Track kind and session-relative milliseconds are supplied by the caller; microphone/system audio are never concatenated. No network route or additional permission is introduced. This library writes only its configured model root and does not edit meeting recordings.

## Database and SQL

This library owns no database tables or SQL artifacts. The session layer maps `RefinedSegment` and `WordTiming` into transcript storage and owns processing-run provenance and publication transactions.

## Primary sources

- [Official sherpa Parakeet model setup](https://k2-fsa.github.io/sherpa/onnx/pretrained_models/offline-transducer/nemo-transducer-models.html#sherpa-onnx-nemo-parakeet-tdt-0-6b-v3-int8-25-european-languages)
- [Official release metadata (asset sizes and SHA-256)](https://api.github.com/repos/k2-fsa/sherpa-onnx/releases/tags/asr-models)
- [NVIDIA model card and CC BY 4.0 terms](https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3)
- [Upstream VAD implementation and soft max-duration semantics](https://github.com/k2-fsa/sherpa-onnx/blob/master/sherpa-onnx/csrc/voice-activity-detector.cc)
- [Silero license](https://github.com/snakers4/silero-vad/blob/master/LICENSE)

## Verification

On 2026-11-06, `cargo test -p vt-stt --lib` passed 15 unit tests (two explicit model fixture tests remain opt-in). `cargo clippy -p vt-stt --all-targets -- -D warnings` passed. The ignored `real_parakeet_two_tracks_gaps_timestamps_and_bounded_epochs` test was explicitly run against the verified temporary model cache and public upstream WAV: it passed native 16kHz microphone/48kHz system input, separate clocks, a 60-second discontinuity, bounded word times, idempotent finish, and 65 seconds of silence crossing forced epochs.

The final `refine` example transcribed: “After early nightfall the yellow lamps would light up here and there the squalid quarter of the brothels.” Its real region was 376–6210ms; native aligned words included `yellow` 1816–2136ms and `lamps` 2136–2616ms. No microphone capture or user recordings were used.

Reproduce the opt-in test by setting `VT_REFINEMENT_MODEL_DIR` to a verified refinement installation and `VT_STT_FIXTURE_WAV` to the public WAV, then running `cargo test -p vt-stt real_parakeet -- --ignored --nocapture`. Tests never download models automatically.
