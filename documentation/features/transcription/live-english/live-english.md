# Local Streaming English Transcription

`vt-stt` runs the official `sherpa-onnx` English streaming Zipformer model on the CPU. Mic and System have separate recognition streams. The desktop session layer supplies mono audio from its inference worker and persists completed transcript segments. Audio recording remains a separate path.

This feature supplies changing partial text, final text, segment-level session times and acoustic word-time estimates from native token alignment. An optional separately installed Silero VAD pack gates inference incrementally; native-rate recording is independent and is never gated. Mic/System labels remain channel labels, not voice identification. Diarization, punctuation restoration and offline Parakeet refinement are separate features.

## User flow

1. In the desktop model setup, choose to download the local English model. No model download happens merely because recording starts.
2. Choose Standard (506,956,414-byte archive; 190,180,941-byte selected files) or Compact (310,414,022-byte archive; 72,466,360-byte selected files). Fresh desktop setups recommend Standard; legacy Compact directories and explicit selections are preserved. The app verifies the selected archive and four required files before publication; manifests and licences add a small amount.
3. Wait for setup to finish. Cancellation stops installation and leaves the setup available for retry.
4. Optionally install the standalone Silero VAD pack in Settings. Its explicit public download is 643,854 bytes; cancellation and integrity verification apply independently of the larger English/refinement packs.
5. Start a recording after setup. The session worker creates the local recognizer with its selected VAD path, keeps Mic and System separate, and displays changing partial text followed by completed segments. Legacy `StreamingRecognizer::new` remains available without VAD. The desktop layer labels Mic as `Me` and System as `Them`.
6. Pausing, input discontinuities and device-rate changes finalize the previous recognition context. Resumed input is anchored to its supplied session time rather than joined across the missing range.
7. Stopping flushes the remaining context and drains the live transcript writer before the live run is sealed. The session layer stores final text for the meeting detail view and export. Final lines appear in the live view immediately, before they are saved.
8. A saved meeting can use the desktop `transcribe_meeting` command to process its stored audio with this same engine. The separate offline Parakeet refinement pass is documented under [batch refinement](../batch-refinement/batch-refinement.md).

Recording can exist without an installed model; the session layer owns that fallback and any later saved-audio job. Installing a model during a recording does not itself attach a recognizer to that already running recording.

## Technical flow

### Installation and verification

`ModelManager::new(root)` treats `root` as the models directory and preserves `LiveModel::EnglishSmall` (June 26) as the default. `ModelManager::for_model(root, LiveModel::EnglishStandard)` explicitly selects the June 21 catalog. `model_dir()` appends that variant’s exact model ID. These are live acoustic model choices, not full application Lite/Balanced/Accurate profiles.

`installed()` is a cheap setup check: it validates the manifest against the source-pinned catalog, checks file types and sizes, and rejects symlinked weight files. It does not hash every model on each UI poll. `verify()` and recognizer construction hash the actual contents of all four files before the native recognizer is created.

An explicit `install()` performs these phases:

1. Create the models root, take its OS installation lock, and check cancellation. A concurrent cooperating installer receives `SttError::Busy`.
2. Fully verify any existing installation. If it is already valid, return success without another download.
3. Create a temporary installation directory on the same filesystem as the destination.
4. Download only over HTTPS from the catalog's official GitHub release URL. Redirects are limited to five and to the allowed GitHub release-asset hosts. The connection timeout is 15 seconds, each awaited network step has a 30-second stall limit, and the client has a 600-second request limit.
5. Check cancellation every 100 ms while waiting on the network, and between download, hashing and extraction operations. Download progress reserves its final 5% for verification and extraction.
6. Require the exact archive size and source-pinned SHA-256 before extraction. Extract only the required files. Reject unexpected roots, traversal, links, unsupported entry types, duplicate selected files, excessive entry counts and expansion beyond 2 GiB.
7. Synchronize selected files, write and synchronize `manifest.json`, verify all selected content against the source-pinned file catalog, and synchronize the candidate directory on Unix.
8. Publish the candidate by filesystem rename and synchronize the models root on Unix. If replacing an invalid existing installation, move it aside inside the temporary work directory first; an immediate publish failure attempts to restore it. This is not a promise of crash-atomic replacement of an existing directory.

`install_from_archive()` accepts an already downloaded copy of this exact official archive and runs the same checksum, extraction and publication checks. It does not accept arbitrary model archives. Network, checksum, extraction, filesystem and publication errors are returned to the caller. Temporary work is cleaned when the operation exits; retries begin with verification of the published installation. Process termination can leave an unpublished temporary directory, which is not treated as installed.

### Explicit English live-model catalogs

The core supports two pinned variants. `LiveModel` is `Copy`, `Eq`, defaults to `EnglishSmall` and serializes as `english_small` / `english_standard`. Its `id()`, `source()`, `archive_bytes()`, `archive_sha256()`, `license()`, `installed_bytes()` and `files()` expose immutable catalog metadata. `files()` returns encoder, FP32 decoder, joiner and tokens with exact filenames, sizes and SHA-256 values. `installed_bytes()` counts those four artifacts, excluding the small manifest/license overhead.

| Core variant | Official model ID | Archive bytes | Required artifact bytes |
|---|---|---:|---:|
| `EnglishSmall` (default) | `sherpa-onnx-streaming-zipformer-en-2023-06-26` | 310,414,022 | 72,466,360 |
| `EnglishStandard` (explicit) | `sherpa-onnx-streaming-zipformer-en-2023-06-21` | 506,956,414 | 190,180,941 |

The June 21 [official archive](https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-streaming-zipformer-en-2023-06-21.tar.bz2) is pinned to SHA-256 `455f40e556aa2b20ac9d3bffd603b58002075c1193b4070938540c11efe0a4da`. It is trained on LibriSpeech and GigaSpeech according to the [official sherpa model documentation](https://k2-fsa.github.io/sherpa/onnx/pretrained_models/online-transducer/zipformer-transducer-models.html#csukuangfj-sherpa-onnx-streaming-zipformer-en-2023-06-21-english). Both the [conversion-owner card](https://huggingface.co/csukuangfj/sherpa-onnx-streaming-zipformer-en-2023-06-21) and [training-owner card](https://huggingface.co/marcoyang/icefall-libri-giga-pruned-transducer-stateless7-streaming-2023-04-04) identify Apache-2.0. The exact archive README confirms the source training repository and license.

| June 21 selected file | Bytes | SHA-256 |
|---|---:|---|
| `encoder-epoch-99-avg-1.int8.onnx` | 187,823,992 | `32c98281c7bd8b63e3e142d007251b37f120572e8fdea9a4f5a79ce22b10ec4f` |
| `decoder-epoch-99-avg-1.onnx` | 2,092,566 | `9da02b77cb08826756ec6a88635f35a40374e4164e7c6359121a9145958a6ceb` |
| `joiner-epoch-99-avg-1.int8.onnx` | 259,335 | `831477d390e59a61f1b6a6f763b9903e6c6366ff6034f1ddba613be82637122f` |
| `tokens.txt` | 5,048 | `49e3c2646595fd907228b3c6787069658f67b17377c60aeb8619c4551b2316fb` |

Both variants retain the same explicit download/offline-import boundary, shared install lock, HTTPS redirect constraints, cancellation, safe extraction, full verification, synchronization and rename publication. A selected manager rejects an installation carrying the other variant’s otherwise supported manifest. The recognizer resolves a supported variant from the fully verified manifest and uses its pinned filenames; an arbitrary folder name cannot select unverified weights. Manifest source/archive identity, full license text, attribution, conversion/acquisition notice and exact file catalog are all checked against source constants. Existing valid June 26 installations keep their exact metadata and remain compatible.

The core package defaults remain Small. The desktop exposes explicit model selection through `live_model_catalog` / `select_live_model` and uses Standard for fresh setups, preserving a legacy Compact directory and every explicit preference. Selection never downloads or removes weights, and cannot change during capture/processing/setup. Both live and replay desktop constructors share `live_models::recognizer`: Standard uses explicit VAD threshold 0.2, Compact retains 0.5, and disabled/unavailable VAD runs ungated. Native token alignment, resampling and endpoint policies remain shared. These acoustic choices do not implement full application profiles or establish broad quality acceptance.

### Recognition and resampling

`StreamingRecognizer::new(model_dir)` is equivalent to `with_vad(model_dir, None)`, resolves the pinned catalog from its verified manifest, and creates one official `sherpa-onnx` 2.0.1 recognizer with CPU execution, two inference threads and greedy decoding. It creates independent Mic and System streams, each with its own input origin, sample count, last text and resampler state.

`accept(track, samples, sample_rate, start_ms)` accepts finite mono `f32` input at 8,000–192,000 Hz, with at most two seconds per call. Empty input returns no events. The desktop worker downmixes native channels before this boundary; the separate recording path keeps native rate and channels.

Each non-16-kHz stream owns the official bandlimited `LinearResampler`, configured for a 16-kHz output. Resampler state continues across packets; the engine always receives 16-kHz audio. A source-rate change, or a difference greater than five milliseconds between the next supplied start time and the accumulated stream endpoint, finalizes and replaces that track's context. `reset_track()` provides the same explicit per-track boundary.

The decoder consumes ready frames incrementally. Endpointing uses 2.4 seconds of trailing silence without a speech requirement, 0.8 seconds after speech, or a 15-second utterance limit. Endpoint results become final events and the decoder resets. Changing nonempty partial text becomes a partial event; repeated identical partial text is suppressed.

### Optional live Silero VAD

`VadModelManager::new(models_root)` provides `model_dir`, `model_path`, cheap `installed`, full `verify`, explicit progress/cancellable `install`, and offline `install_from_file`. The standalone directory is `sherpa-onnx-silero-vad-v4`, containing the exact upstream `silero_vad.onnx` and a provenance/license manifest. The file is 643,854 bytes, SHA-256 `9e2449e1087496d8d4caba907f23e0bd3f78d91fa552479bb9c23ac09cbb1fd6`, from the [official release](https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/silero_vad.onnx). It is the k2-fsa maintained 16kHz Silero v4 export, under the Silero Team's MIT license; `silero-vad-LICENSE.txt` is bundled and its full text is retained in the manifest.

Setup uses the same models-root install lock, HTTPS release-host allowlist, maximum five redirects, 15-second connection timeout, 30-second per-step stall limit and 100ms network cancellation polling as the existing installers; the small VAD request has a 120-second total limit. Fixed-size/hash verification precedes publication. Copying checks cancellation every 64 KiB, is bounded to the exact catalog size, and rejects weight symlinks. A synchronized candidate is renamed into place, with immediate rename-failure rollback as above. There is no archive extraction. Offline import can reuse the pinned VAD file from an explicitly installed refinement pack without downloading another model.

`StreamingRecognizer::with_vad(streaming_model_dir, Some(vad_path))` accepts either that standalone directory or the exact pinned ONNX file. It retains the default detector threshold of 0.5. `StreamingRecognizer::with_vad_options(model_dir, vad_path, LiveVadOptions { threshold })` requires a VAD path and permits an explicit finite threshold in `0.1..=0.9`. `LiveVadOptions` is Copy, serializable/deserializable, PartialEq and defaults to 0.5; unknown serialized fields are rejected. Invalid options return `SttError::InvalidVadOptions` before model-file I/O or native initialization. It hashes the requested weight before native construction; an invalid requested VAD is an error, not a silent fallback. The application owns choosing an ungated fallback and reporting it. Each track creates its own CPU detector, with one VAD thread, library default threshold 0.5 (desktop Standard explicitly requests 0.2), 512-sample/32ms analysis windows, 32 ms minimum speech and 160 ms silence hysteresis. Native onset needs consecutive positive windows; the gate retains 320 ms pre-roll so onset-decision delay does not remove initial phonemes.

Only the native detector threshold varies through this explicit API. It does not alter sample amplitude, gate pre-roll/hangover, track resets, decoder endpoints, model selection or raw recording. Legacy `with_vad` delegates to default options, and `new` / `with_vad(None)` stay ungated. Lower thresholds are a quality experiment for quiet speech, not a calibrated preset or a default change.

The gate consumes VAD `detected()` decisions and immediately feeds speech and bounded pre-roll to the streaming decoder. It never waits for a completed VAD speech region. After detection drops, one second of real audio continues into the decoder, allowing its existing 800 ms trailing-silence endpoint. On gate close, any remaining result is flushed with decoder-only right context and the inference stream is replaced. Later speech gets an absolute new stream origin; skipped silence is never compressed out of session timestamps. Pre-roll contains only audio not already submitted to the decoder, so opening/reopening does not replay words.

There are at most 512 pending analysis samples and 5,120 pre-roll samples per track. The native detector also buffers active speech internally: its result queue is cleared after every window, and it resets at 16 seconds independently of the live ASR stream. The open gate/hangover survives this internal reset, preserving incremental speech through re-detection. Native detector storage has a 20-second cap. This prevents multi-hour active utterances or silence from growing VAD buffering. A real track reset, pause, gap or source-format change resets detector, gate, pre-roll, pending samples, sinc resampler and inference origin together. Stop flushes the resampler and any real residual window while the gate is open before the normal 300 ms decoder tail.

### Acoustic word times

`TranscriptEvent` now also exposes `words: Vec<WordTiming>`. The official online API supplies native BPE tokens and timestamps; sherpa's transducer conversion uses 10 ms feature frames with 4x subsampling, giving 40 ms acoustic alignment granularity. Word pieces and punctuation are merged using their actual timestamps; word ends use the final token's 40 ms frame extent, bounded by the next word and the real audio endpoint. Native endpoint `start_time` and the gate's absolute stream origin are both included. The shared alignment merger is also used by offline refinement with its separate 80 ms model frame.

Missing, nonfinite, mismatched, decreasing or out-of-context online alignment produces an empty word list while preserving usable recognized text. The code never uniformly divides sentence duration into word times. These are acoustic frame estimates, not phoneme-level forced alignment; partial word text/times can change as the partial hypothesis changes. Consumers must clear stale timings when editing recognized text.


`finish()` flushes both resamplers, adds 0.3 seconds of recognition-only silence for right context, finishes input and drains the decoder. Reported endpoints remain bounded by actual received audio, so the padding does not extend meeting time. Repeating `finish()` returns no events; later `accept()` and `reset_track()` calls return an error.

`TranscriptEvent` exposes `track`, `start_ms`, `end_ms`, `text`, `is_final` and `words`. Every segment and word remains bounded by real session audio; inference-only zero padding never extends those times.

## Key types and entry points

| Type or function | Source | Role |
|---|---|---|
| `ModelManager` | `crates/vt-stt/src/models.rs` | Explicit installation, local verification and fixed model path |
| `ModelManifest`, `FileRecord` | `crates/vt-stt/src/models.rs` | Private persisted catalog/provenance representation |
| `StreamingRecognizer`, `TrackState` | `crates/vt-stt/src/recognize.rs` | Shared CPU model and independent per-track contexts |
| `TranscriptEvent` | `crates/vt-stt/src/recognize.rs` | Partial/final text, segment times and native acoustic word estimates |
| `VadModelManager` | `crates/vt-stt/src/vad_models.rs` | Explicit standalone pinned VAD setup and integrity checks |
| `LiveVad`, `SpeechGate` | `crates/vt-stt/src/live_vad.rs` | Per-track32ms decisions, bounded pre-roll/hangover and native buffering |
| `SttError`, `Result` | `crates/vt-stt/src/lib.rs` | Typed network, filesystem, verification, cancellation and audio failures |
| `transcribe` example | `crates/vt-stt/examples/transcribe.rs` | Offline archive installation and real WAV recognition smoke test |

## Integration points

| Integration | Current behavior |
|---|---|
| `AppService` in `crates/vt-session/src/lib.rs` | Stores model setup status, starts installation on a named background thread and forwards progress/cancellation |
| Inference worker in `crates/vt-session/src/session.rs` | Consumes queued mono packets, calls the recognizer, finalizes on resets and persists final segments |
| `save_transcript` in `crates/vt-session/src/session.rs` | Stores final text and keeps changing partials in runtime state; supplies channel labels |
| `vt-core::TrackKind` | Keeps Mic and System identity explicit across the STT boundary |
| Official `sherpa-onnx` crate | Statically linked CPU recognition and bandlimited resampling; its build downloads a matching native release when not pre-provided |
| Resource license texts | `crates/vt-stt/resources/Apache-2.0.txt`, `sherpa-onnx-LICENSE.txt`, `onnxruntime-LICENSE.txt` for model provenance and desktop packaging |

The API is worker-side and may allocate, hash files, perform network and filesystem I/O, or run native inference. It is not safe for real-time capture callbacks. Capture and inference queue policies belong to the session/capture feature rather than this crate.

## Commands, routes and access control

There are no HTTP routes or user accounts. Tauri commands in `src-tauri/src/main.rs` call the local session service:

| Command | Purpose |
|---|---|
| `install_model` | Start the explicit local-model setup worker |
| `cancel_model_download` | Set the English-model setup cancellation flag |
| `install_vad_model` | Start the explicit standalone VAD setup worker |
| `cancel_vad_model_download` | Cancel standalone VAD setup |
| `app_snapshot` | Read model setup status and meeting runtime state |
| `transcribe_meeting` | Start the session layer's saved-audio transcription job |

The app and model files run under the local OS user with ordinary filesystem permissions. No cloud API key or account is required. Only public model artifacts are requested over the network; recorded audio, transcript text and voice data are never transmitted by this feature. Model files and metadata are not encrypted.

## Persisted schema and SQL artifacts

This crate defines no SQL tables or migrations. Transcript persistence is owned by `vt-session`; its schema is not duplicated here. The STT artifact is `<models-root>/<model-id>/manifest.json`:

| Field | Meaning |
|---|---|
| `id` | Fixed model identifier |
| `source` | Official archive URL |
| `archive_sha256` | Source-pinned archive digest |
| `license`, `license_text` | License identifier and full license text |
| `attribution` | Public upstream project attribution |
| `conversion` | ONNX export provenance and selected precision |
| `acquisition_terms` | Public download/access conditions |
| `files` | Four required filenames mapped to exact SHA-256 and byte size |

The selected artifacts are an INT8 encoder, FP32 decoder, INT8 joiner and token text. Checksums and expected sizes are defined in `crates/vt-stt/src/models.rs`; editing the local manifest cannot replace those source-pinned expectations. SQL artifacts: none.

## Validation and present limits

Default tests cover per-variant metadata/provenance checks, cross-variant rejection, duplicate/unsafe archives, shared locking, symlink rejection, cancellation, catalog verification, malformed archive handling, selected-file extraction, input validation, streaming resampler continuity, suppression of above-Nyquist energy, bounded gate pre-roll, real-silence hangover, no replay at reopening and VAD-file symlink rejection. A real upstream speech fixture was decoded successfully outside the repository. An explicit opt-in test fed that fixture through both streams with different session origins and verified both text and independent offsets.

The opt-in test requires `VT_STT_MODEL_DIR` and `VT_STT_FIXTURE_WAV`; it never downloads models. These checks demonstrate the installed engine and tested timing cases, not a meeting-corpus WER target, hardware capture quality, a live latency guarantee or completion of the full application roadmap.


The opt-in `real_live_vad_is_incremental_retains_onsets_tracks_gaps_words_and_bounded_silence` test additionally requires `VT_VAD_MODEL_FILE`. It imports that verified public file into an isolated temporary standalone installation and exercises 20 ms 16 kHz Mic packets, 100 ms 48 kHz System packets, cropped leading speech, partials before utterance completion, a silence final within4 seconds, explicit gaps, native word bounds, a continuous-speech 15-second ASR final, internal 16-second VAD resets and 65 seconds of silence. The tested public sentence retains “after early” and “yellow lamps” on both tracks exactly once. No capture devices or private recordings are used.

The 20–100 ms packet cadence and 32 ms VAD windows are incremental processing boundaries, not a guarantee of 20–100 ms end-to-end recognized text. Zipformer's native chunk/right-context requirements, word acoustics, capture/inference queues, CPU load and device latency still determine visible partial latency. A single public fixture does not establish a meeting-corpus WER target, background-noise robustness, low-volume speech recall, a hardware latency SLO or all quality gates. Silero may reject speech it classifies as silence; raw audio remains available for independent offline refinement.

Primary implementation references: [official Silero export documentation](https://k2-fsa.github.io/sherpa/onnx/vad/silero-vad.html), [native Silero onset/hysteresis](https://github.com/k2-fsa/sherpa-onnx/blob/master/sherpa-onnx/csrc/silero-vad-model.cc), [native VAD buffering](https://github.com/k2-fsa/sherpa-onnx/blob/master/sherpa-onnx/csrc/voice-activity-detector.cc), and [online token timestamp conversion/endpoint offsets](https://github.com/k2-fsa/sherpa-onnx/blob/master/sherpa-onnx/csrc/online-recognizer-transducer-impl.h).

## Session live transcript persistence and recovery

This section describes `vt-session`'s use of the engine during a recording (decision trail: [live transcript persistence](trail/2026-11-10-live-transcript-persistence.md), [worker recovery and truthful state](../../capture/recording-health/trail/2026-11-10-live-worker-recovery-and-truthful-state.md)).

**Saving final lines.** The `speech-inference` thread never touches SQLite. `session::save_transcript` publishes each nonblank final to `Runtime::live_finals` (shown at once; `AppService::detail_for_run` merges unsaved finals and partials with saved segments, de-duplicated by ID) and pushes it to a `TranscriptSink`. The session-owned `live-transcript-writer` thread (`live_transcript.rs`) saves queued lines in batches of at most 64 (`BATCH_LIMIT`) with a queue limit of 512 unsaved lines (`QUEUE_LIMIT`). `store::save_live_segments` uses one IMMEDIATE transaction with point operations only: a primary-key check that the run is the meeting's running active live run, an idempotent insert per segment and one incremental search row per new segment, then marks the meeting's summaries stale (removing their search rows only when summaries exist). For new segments this equals what `rebuild_fts` would produce.

**Lock budget and failure.** Saves run on the shared writer ([database access](../../storage/database-access/database-access.md)), so in-process writers queue rather than fail; contention now comes mainly from the recorder's own connection. The retained stop-gap `LiveSaveBudget::PRODUCTION` (3 attempts, 300 ms wait each, 50 ms then 100 ms backoff, 1.05 s worst case, `store::retry_busy`) still bounds each save and is rarely exercised; its removal is a later step. On a lock the batch is kept and retried after the next arrival or after 1 s (`RETRY_INTERVAL`) with "Live transcript lines could not be saved yet; they are kept and retried. Raw audio is preserved. Cause: <error>". A deterministic error is isolated per line; that line is dropped as a `segment_rejected` STT gap with "A live transcript line could not be saved and was skipped. Raw audio is preserved; refine saved audio to recover it. Cause: <error>". A line evicted from a full queue, or still unsaved at the 5 s stop deadline (`CLOSE_DEADLINE`), becomes a `persistence_failed` gap with "Some live transcript lines could not be saved because the database stayed busy. Raw audio is preserved; refine saved audio to recover them." Stop closes and joins the writer after the speech worker and before the audit is finalized and the run is sealed.

**Recovery.** The speech worker (`SpeechWorker` behind the `LiveRecognizer` trait: `accept`, `reset_track`, `finish`, `rebuild`) recovers in place from a per-message error: a `recognizer_error` STT gap plus a failure entry, a reset of the affected track, and an engine rebuild only for `SttError::Engine` or a failed reset. The same cause twice in a row with no success between is terminal, and 20 recoveries per recording (`STT_RECOVERY_BUDGET`) end the worker (`recovery_budget_exhausted`). The hypothesis returned by a source-gap reset and by `finish` is saved once, capped at the audio already fed so it never runs past a gap start, and skipped if identical to the track's last published final. The speaker worker is not restarted, and sustained-backlog shedding of live STT is unchanged and sticky. When live transcription stops, `RecordingView.live_transcription` reports the reason; see [Recording Health](../../capture/recording-health/recording-health.md#live-worker-state-failures-and-recovery).

Known limits: the recorder's chunk writer (`crates/vt-record`) now retries a database lock for up to `MANIFEST_LOCK_BUDGET` (60 s) and `save_clock` on flush is skipped on a lock, so a lock no longer ends a recording unless it outlasts that budget (see [durable recording](../../recording/durable-recording/durable-recording.md#manifest-write-lock-tolerance)). Since 2026-11-11 the recorder waits on its own thread behind a bounded queue (see [durable recording](../../recording/durable-recording/durable-recording.md#recorder-thread-and-bounded-queue)), so a recorder wait no longer blocks the capture consumer loop or live transcription input; the queue absorbs up to 60 s of audio per track before a `recorder_backlog` gap is recorded. The 2026-11-10 stall's database-lock trigger is inferred, not proven. Nothing here has run in a real recording yet; the engine rebuild path is covered only through a fake recognizer.

## Desktop echo preprocessing

The desktop now places `echo_pipeline::worker` before streaming ASR and the independent speaker worker. Native mono packets retain hardware/source-clock times; the initial unflushed recorder buffer uses its first source observation as the origin rather than adding buffered duration to each new packet timestamp. This is covered across the first two-second flush boundary. `vt-dsp` emits 16 kHz frames, applies AEC only with verified route/reference evidence, and preserves other Mic audio through explicit bypass. Both consumers receive the same inference samples; raw FLAC remains independent. Saved transcription uses the same source-preserving stage and capture-time route history. See [echo control](../../dsp/echo-control/echo-control.md).

The opt-in `cached_standard_archive_installs_and_initializes_the_production_recognizer` test uses `VT_STT_STANDARD_ARCHIVE` and optional `VT_STT_STANDARD_MODELS_ROOT` / `VT_STT_TEST_VAD_PATH`. It imports the actual official cached archive through the unchanged size/hash/extraction/publication path, verifies every retained artifact, rejects that archive for a clean Small manager, and initializes the real Standard recognizer with and without VAD. It performs no network request, capture or playback.

Explicit threshold tests reject nonfinite/out-of-range values before initialization. The opt-in `real_threshold_options_preserve_legacy_default_onsets_tracks_gaps_and_word_bounds` compares actual legacy/default-option events and exercises 0.2/0.35 on the same public fixture through both tracks and explicit gaps. `native_options_keep_generated_silence_and_noise_storage_bounded` uses only the pinned VAD plus generated silence and deterministic low-level noise at 0.2/0.35/0.5, checking storage/reset bounds and reporting gate openings without claiming noise calibration. Production corpus comparison remains a separate measured quality activity.

## Desktop selection and measured recommendation

`crates/vt-session/src/live_models.rs` persists the explicit `live_model` enum in `app_settings`, reports pinned choice metadata and supplies the per-model VAD configuration. An existing Small directory stays selected until an explicit choice; fresh setups select Standard. An idle-only switch resets only the selected status, preserves installed packs and makes no network call. Explicit download, cancellation and verify/repair remain in Settings. Failed selection retains the authoritative choice and explains the error.

Live creation and replay publication retain requested engine identity, exact file/archive pins and VAD options atomically with their new run. They never relabel earlier versions. Capture pins whether STT was configured at start; a download completing mid-call does not activate it. Full file verification remains mandatory before native inference, and the desktop helper rejects a supported but wrong selected catalog before fallback.

Public production comparisons measure Standard/0.2 WER 53.31%/48.25% on the development/held-out windows, versus Standard/0.5 68.87%/63.52% and Compact/0.5 100%/97.93%. These are adjacent windows of one AMI recording, not independent meeting or global calibration evidence. Clean public control text is retained; a generated silence/noise control emits zero ASR events. Both model profiles also pass real AppService workers with a paced fake helper, exact native PCM24 archival and sealed provenance/audits; no OS capture/output device is used. See the [production inference evidence](../../quality/production-inference/production-inference.md) for conditions, resources and limits.

Offline operator examples use the same verified installers and desktop lock: `vt-stt/examples/install_live_model.rs` imports only the exact official archive and optional pinned Silero file; `vt-session/examples/select_live_model.rs` verifies installed weights and explicitly selects them for an idle closed app. Neither starts audio or sends meeting content.

Persisted model choices that contain malformed JSON or an unsupported variant keep the app usable for recording and repair. The original value is retained, the UI shows a supported candidate with a warning, and live recognition/model setup remain disabled until an explicit supported choice is confirmed. No fallback model is silently activated or downloaded.
