# Local Streaming English Baseline

## Pipeline invariants

- The official `sherpa-onnx` 2.0.1 Rust API supplies the local streaming engine. CPU inference uses one shared model configured for two threads.
- Mic and System always have independent stream, origin, sample-count and resampler state. A channel label is not a detected speaker identity.
- Input is finite mono `f32`, 8,000–192,000 Hz and no more than two seconds per call. Invalid rates, oversized packets, nonfinite samples and overflowing input time ranges are rejected before native waveform submission.
- Resampling is stateful and bandlimited; native recognition receives 16-kHz audio. Native-rate recording is outside this inference transformation.
- A source-rate change or timestamp difference greater than five milliseconds finalizes the old track context and anchors a new context to the next packet's supplied session time.
- `reset_track()` finalizes only its requested track. `finish()` finalizes both tracks once; further input is rejected.
- Nonempty changing partial text is emitted without persistence by this crate. Final events are distinguished explicitly with `is_final`.
- Segment times use session milliseconds. The 0.3-second finalization padding never extends the reported real-audio endpoint. Native token alignment supplies bounded word-time estimates; absent/invalid online alignment leaves words empty. Sentence duration is never divided uniformly.
- Optional VAD gates inference only. Native-rate raw audio recording is never gated, dropped or rewritten by the recognizer.
- `LiveVadOptions` defaults to threshold 0.5 and permits only finite values in 0.1..=0.9. Explicit options require a pinned VAD path, are validated before model I/O/native initialization, and change only the detector threshold.
- Legacy `with_vad(Some)` uses default options. No threshold option changes amplitude, model, resampling, context, track clocks, decoder endpoints or raw audio.
- `StreamingRecognizer::new` remains ungated; `with_vad` requires the exact pinned requested VAD and fails instead of silently disabling it.
- VAD receives 512-sample windows and opens with 320 ms bounded pre-roll. One second of real silence hangover preserves native endpointing; gate closure flushes decoder-only context.
- Per-track pending/pre-roll storage is bounded to 512 + 5,120 samples. The native VAD queue is cleared every window and native detector resets at 16 seconds while the open gate is retained.
- Skipped silence is excluded from inference work and retained in absolute session clock offsets. Gap/pause/rate resets clear both VAD and recognizer context.
- No recording, audio or transcript network transfer occurs in this feature. Application model downloads occur only through explicit installation.

## Model and installation invariants

- `ModelManager::new` and `LiveModel::default` remain `EnglishSmall`, model ID `sherpa-onnx-streaming-zipformer-en-2023-06-26`, Apache-2.0.
- `ModelManager::for_model(..., LiveModel::EnglishStandard)` explicitly selects `sherpa-onnx-streaming-zipformer-en-2023-06-21`, Apache-2.0, LibriSpeech + GigaSpeech. This is a live-model choice, not a complete application profile.
- Standard archive size/hash are 506,956,414 bytes / `455f40e556aa2b20ac9d3bffd603b58002075c1193b4070938540c11efe0a4da`; required artifact bytes total 190,180,941. Its encoder/joiner are INT8 and decoder FP32.
- Both managers enforce their explicitly selected catalog; recognizer construction resolves only a fully verified supported manifest and its pinned filenames. The core default remains Small; desktop fresh setups use the measured Standard selection while preserving existing Compact directories and explicit choices.
- The Compact official archive is exactly 310,414,022 bytes. Its source-pinned SHA-256 is `639e25b578e9e997131402199419c13a941f8e4e198e2da1ce57dbf5cf401282`.
- Compact’s four retained artifacts total 72,466,360 bytes before manifest/license metadata. They are the INT8 encoder, FP32 decoder, INT8 joiner and token text.
- `installed()` validates pinned metadata and file sizes. `verify()` and recognizer construction additionally hash actual file contents against the source-pinned file catalog.
- A local manifest cannot replace the catalog's source/archive identity, full license/provenance notice, required filenames, sizes or digests. Unknown manifest/file-record fields and weight-file symlinks are rejected.
- One OS file lock serializes cooperating installers per models root.
- Downloads use HTTPS and at most five redirects to the allowed official release hosts. Connection timeout is 15 seconds, awaited network-step stall timeout is 30 seconds and request timeout is 600 seconds.
- Network cancellation is polled every 100 ms. Hashing and extraction check cancellation between chunks; a cancelled or failed operation does not publish a partial candidate as installed.
- Extraction accepts only normal relative paths beneath the expected model root. Links, unsupported entries, duplicate selected files, excessive entry counts and expansion over 2 GiB are rejected.
- Files and manifest are verified before candidate publication. File/directory synchronization is used on Unix; Windows directory synchronization is not supplied by this crate.
- Installing an already valid model does not download again. Offline archive import requires the same exact official archive size and digest.

- The standalone VAD ID is `sherpa-onnx-silero-vad-v4`, file `silero_vad.onnx`, MIT, exactly 643,854 bytes, SHA-256 `9e2449e1087496d8d4caba907f23e0bd3f78d91fa552479bb9c23ac09cbb1fd6`. Its manifest includes source, exact size/hash, full license, attribution, conversion and acquisition terms.
- Standalone VAD setup uses the common lock/HTTPS/redirect/stall/cancellation protections, a 120-second request timeout, bounded cancellable copy and verified candidate publication. No archive is extracted.

## Flag and failure rules

| Condition | Severity | Blocks? |
|---|---|---|
| No model installation | Setup required | Recognizer construction; session may continue recording audio |
| Setup check succeeds but full hash check fails | Integrity error | Native engine creation; reinstall can repair the installation |
| Another installer holds the lock | Busy | Installation |
| Explicit cancellation | Cancelled | Current setup operation |
| Network failure, timeout, unexpected redirect host or changed archive size/hash | Error | Model publication |
| Unsafe archive, missing selected files or excessive expansion | Error | Model publication |
| Filesystem synchronization, extraction or rename failure | Error | Current installation; caller may retry after resolving the filesystem condition |
| Invalid audio geometry, rate, nonfinite values or time overflow | Error | Submitted input packet |
| Sample-rate change or timing discontinuity over five milliseconds | Context reset | No; preceding text is finalized before fresh input |
| Native model creation fails after verification | Engine error | Live STT; recording fallback belongs to the session layer |
| Input after `finish()` | Error | Further recognition calls |
| Nonfinite/out-of-range VAD threshold | Invalid options | Constructor before file/native work |
| Requested VAD missing or hash-invalid | Integrity/setup error | VAD-enabled construction; caller explicitly owns ungated fallback |
| Native online word alignment absent/invalid | Word alignment unavailable | No; recognized text is preserved with empty words |

## Side effects in order

1. Explicit setup creates the models root, obtains the installation lock and verifies any published installation.
2. Download writes to a unique temporary work directory and reports progress, reserving the final 5% for validation/extraction.
3. Archive verification precedes selected extraction; synchronized model files and manifest undergo full catalog verification.
4. The candidate is renamed into the model directory. An invalid existing directory is moved aside first, and an immediate publish failure attempts rollback. Root synchronization follows on Unix.
5. Temporary work is cleaned on normal exit or returned error. Process termination may leave unpublished temporary directories, which are not accepted as installed.
6. Recognition verifies published files, constructs the CPU engine and creates two independent streams.
7. Input produces partial/final events; session-owned consumers persist final text and display partials.
8. Stop or explicit context reset flushes resampler and model right context before final text is emitted.

## Configuration defaults

| Setting | Value |
|---|---|
| Engine | Official `sherpa-onnx` 2.0.1 |
| Live model default / explicit alternative | EnglishSmall (June 26) / EnglishStandard (June 21) |
| Execution provider | CPU |
| Model threads | 2 |
| Decoder | Greedy search |
| Recognition input | 16,000 Hz mono |
| Endpoint without speech requirement | 2.4 s trailing silence |
| Endpoint following speech | 0.8 s trailing silence |
| Utterance endpoint limit | 15 s |
| Context discontinuity tolerance | 5 ms |
| Finalization right context | 0.3 s silence, excluded from reported input duration |
| Model manifest cap | 128 KiB |
| Extraction expansion cap | 2 GiB |
| Optional VAD | Silero v4 official 16 kHz export, CPU, 1 thread per track |
| VAD threshold / minimum speech / silence hysteresis | Default 0.5 / 32 ms / 160 ms |
| Explicit threshold bounds | Finite 0.1..=0.9; experimental, no default change |
| VAD window / pre-roll / real hangover | 32 ms / 320 ms / 1 s |
| Native VAD reset / buffer capacity | 16 s / 20 s |
| Live native token frame extent | 40 ms |

## Access control and privacy

- The local OS user controls model files, meeting data and app access; this feature adds no account or HTTP authorization boundary.
- No credentials, cloud-provider API keys or access forms are required for the default model.
- Model provenance and license texts contain public artifact metadata. Feature docs and tests do not include private meeting transcripts or personal data.
- Diarization, biometric memory, batch refinement and cloud inference are outside this baseline.
- 20–100 ms packets do not establish an end-to-end partial-text latency guarantee; hardware/corpus quality targets require separate measurement.

## Desktop policy

- Fresh desktop setup selects Standard with explicit VAD threshold 0.2; Compact retains 0.5. Core default APIs remain Small/0.5.
- Changing selection is idle-only, preserves installed packs, never downloads and applies only to a new capture/replay job. Mid-capture download completion never activates STT.
- Live/replay requested model, file pins and VAD options belong to the new run; historical engine metadata remains unchanged.
- Public-corpus recommendation is limited to the measured source/control conditions; it does not establish global calibration, hardware budgets or full profile acceptance.

- An invalid saved live-model choice does not prevent startup and is not silently rewritten. Live STT/setup require explicit confirmation of a supported choice; a displayed candidate is not an active recognizer.
