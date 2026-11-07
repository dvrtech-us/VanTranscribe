# Implement Verified Local English Streaming Transcription

- Date: 2026-11-06
- Feature: live-english
- Related code: `crates/vt-stt/src/lib.rs`, `models.rs`, `recognize.rs`, `live_vad.rs`, `vad_models.rs`, `refine.rs`, `crates/vt-stt/examples/transcribe.rs`, desktop inference integration
- State: Working draft for this unmerged implementation change; consolidated through 2026-11-07

## Context

The approved design selects local streaming English recognition, independent Mic/System processing and inference isolated from durable recording. Optional speech detection must reduce silent inference work without delaying partials until a whole VAD region completes. Native acoustic word estimates support timed speaker assignment. A second English acoustic model must retain explicit installation and integrity guarantees without silently changing defaults or claiming complete application profiles. Earlier whole-design reviews and owner decisions remain recorded under `documentation/planning/Architecture/`.

## Decisions

- Use official `sherpa-onnx` 2.0.1 with static linkage, CPU execution, two recognizer threads, greedy decoding and independent track streams. Preserve 2.4/0.8-second silence endpoints and the 15-second utterance limit.
- Keep `ModelManager::new` and `LiveModel::default` on `EnglishSmall`, the June 26 LibriSpeech model. Its archive is 310,414,022 bytes and four retained artifacts total 72,466,360 bytes.
- Add explicit `ModelManager::for_model(root, LiveModel::EnglishStandard)` for June 21 LibriSpeech/GigaSpeech. Its archive is 506,956,414 bytes, SHA-256 `455f40e556aa2b20ac9d3bffd603b58002075c1193b4070938540c11efe0a4da`; required artifacts total 190,180,941 bytes. Both variants use INT8 encoder/joiner, FP32 decoder and tokens. These names describe live models rather than complete Lite/Balanced/Accurate application profiles.
- Expose immutable variant/file metadata. Bind manifest source/archive identity, full license text, attribution, conversion/acquisition notice, filenames, sizes and hashes to the supported source catalog. Preserve legitimate Small metadata exactly; a selected manager rejects the other catalog. Recognizers resolve filenames only from a fully verified supported manifest.
- Make setup explicit and cancellable. Retain the common OS install lock, official HTTPS/redirect limits, bounded downloads, complete archive hashes, safe regular-file extraction, selected-content verification, synchronized candidates and rename publication. Cheap UI checks use pinned metadata/types/sizes; native initialization always hashes the actual weights. Immediate replacement failure attempts rollback; this is not a general crash-atomic directory replacement protocol.
- Preserve public upstream licensing and conversion provenance in installed manifests and bundle license resources. Official sherpa documentation, both June 21 publisher/training-owner cards and the pinned archive README identify Apache-2.0. The cached archive and selected artifact hashes were recomputed locally.
- Keep persistent bandlimited resamplers and independent source origins per track. Finite mono packets remain bounded to two seconds and 8–192kHz; inference receives 16kHz. Explicit reset, source-rate change or a time discontinuity over five milliseconds finalizes the old context and anchors the next supplied time.
- Offer separate explicit Silero v4 setup, 643,854 bytes under MIT, with legacy ungated construction preserved. `with_vad` independently verifies its requested VAD. Classify 512-frame/32ms windows, retain 320ms pre-roll, feed the streaming decoder incrementally and keep one second of real silence hangover. Clear completed-region queues each window and reset native VAD buffering every 16 seconds without closing an active gate. Skipped silence never compresses absolute timestamps; raw recording remains independent.
- Expose explicit `LiveVadOptions { threshold }` with unchanged 0.5 default and finite 0.1..=0.9 bounds. Require an independently verified VAD path and reject malformed options before model I/O/native initialization. Quiet-speech investigation changes only the detector threshold: no amplitude, model, endpoint, pre-roll/hangover, reset or raw-recording alteration. Production corpus evidence is required before recommending any threshold/default.
- Merge native online BPE tokens/timestamps into bounded word estimates using the 40ms native encoder-frame extent. Share acoustic merger logic with separate offline refinement, which uses 80ms. Missing/invalid alignment preserves text with empty words; text length never supplies invented timing.
- Leave capture queue policy, persistence, channel labels and desktop fallback to the session layer. Core defaults remain Small/0.5. Explicit desktop integration now recommends Standard/0.2 for fresh setups after production-path comparisons; existing Compact directories and explicit selections remain respected. Cross-meeting voice identification and full application-profile acceptance remain outside this feature.

## Alternatives Considered

- Deprecated third-party bindings or a second resampling runtime: rejected in favor of the official statically linked recognizer and persistent bandlimited resampler.
- Shared track recognition or stateless packet conversion: rejected because reliable channel identity, anti-aliasing state and independent time origins must survive packet boundaries.
- Hashing all weights on every UI poll: rejected as repeated disk/CPU work; startup/installation retain full verification.
- Trusting arbitrary extraction paths, folder names or manifest-provided hashes: rejected because partially downloaded or substituted weights must never reach native initialization.
- Waiting for completed VAD regions, or feeding only positive frames without context: rejected because partials would be delayed and speech onset/decoder tails could be lost.
- Relying only on native max-speech duration or deriving word times from text length: rejected because neither establishes bounded waveform storage or acoustic timing.
- Calling the second model a complete application profile or silently replacing an existing selection: rejected. Production-path VAD/corpus comparisons support a fresh-setup Standard/0.2 recommendation, with broad calibration still open.

## Consequences

- Positive: real local English recognition, bounded incremental speech gates and native acoustic word estimates require no meeting-content upload or API key.
- Positive: each catalog can be explicitly installed, cancelled, verified and reproduced from its exact official offline archive, with source/license provenance retained.
- Positive: independent context boundaries preserve source offsets across gaps, skipped silence and channel-specific resets.
- Negative: setup and full file verification allocate/block on workers; Standard uses a larger artifact footprint. Native dependency build may download its pinned runtime archive.
- Negative: VAD may reject faint/noisy speech. Native frame estimates are not forced alignment; 20–100ms packets and 32ms windows do not guarantee 20–100ms visible-text latency.
- Negative: fixture success and diagnostic corpus comparisons do not establish meeting WER, physical capture fidelity, latency SLOs or complete roadmap/profile acceptance.

## Validation

- Existing public speech tests decoded both tracks with independent offsets. VAD fixtures cover onset retention, incremental partials, real-silence endpoints, gaps, native word bounds, long-utterance finals, detector resets and bounded silence. No capture devices or private recordings are used.
- Variant tests cover every pinned manifest field, cross-catalog rejection, common locking/cancellation, selected extraction, duplicate/traversal roots and weight symlinks. Sparse file fixtures cannot satisfy full content verification.
- The real June 21 cached archive passed the production size/hash/extraction/publication path and full artifact verification. Both native Standard constructors, ungated and with the pinned production VAD, initialized successfully; this explicit cache test took 134.41 seconds without network access.
- Threshold validation rejects malformed options before initialization. Native opt-in cases compare legacy/default events, leading speech, independent tracks/gaps/word bounds and bounded generated silence/noise without claiming calibration.
- Strict all-targets `vt-stt` Clippy passed. Default unit and opt-in fixture validation remain distinct from production meeting-quality acceptance.
- Source weights, downloaded speech fixtures and private transcript data are not added to the repository. Temporary evaluation installations do not activate installed app settings or change the Small default.

Invalid or future saved model variants leave the desktop open for repair while preventing silent fallback inference/download. Confirmation through the existing idle-only selection command writes a supported choice; the stored invalid value remains untouched before that action.
