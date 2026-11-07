# Local speaker turns

`vt-diarize` derives speaker activity and changes from audio independently of speech recognition. It uses pinned pyannote segmentation-3.0 INT8 and 3D-Speaker ERes2Net embeddings through sherpa-onnx 2.0.1. No audio is uploaded. Ordinary diarization never creates or consults a cross-meeting voice gallery; separate explicit enrollment is governed by [voice memory](../../speakers/voice-memory/voice-memory.md). The clustering profile is explicitly experimental and uncalibrated; this increment does not satisfy the M2b accuracy/calibration acceptance gates.

## User flow

1. Explicitly request model setup after reading the model source/acquisition notice. Model setup never enables capture or accepts a publisher's gated account agreement.
2. Select remote or in-person processing. Remote microphone turns receive a channel-based `Me` prior; system audio is clustered separately. A saved different-person correction on the microphone overrides that prior and enables actual microphone embedding/clustering for the next pass. In-person microphone voices are clustered without a `Me` assumption.
3. Feed final recorded audio on a worker, or bounded live audio on a dedicated speaker worker. The result supplies independent timed turns rather than replacing immutable transcript text.
4. Map individual recognized words to the turn at each word's timestamp midpoint. A long STT final is never embedded as one speaker.
5. Review uncertain, provisional, short and overlap turns. Apply stored user identity corrections through time anchors. Missing or ambiguous anchors require review instead of transferring a name to an unrelated serial turn ID.

## Technical flow

`ModelManager` installs only on explicit request. It downloads two official HTTPS assets, limits redirects to GitHub's release hosts, checks exact byte counts and SHA-256, extracts only the exact regular segmentation model file without executing any archive script, and atomically renames a private staging directory after verification. Cancellation drops an inflight download, polled every 50 ms; network timeout is 180 seconds and connect timeout 10 seconds. A file lock rejects concurrent setup. Invalid existing packs require explicit removal before replacement. `installed()` is a cheap pinned-manifest/size check; inference construction verifies full model hashes.

`Diarizer::accept` accepts at most two seconds of finite mono input at 8–192 kHz. Resampling, sample counters and clocks are separate per track. An explicit gap, sample-rate change, or timestamp discontinuity greater than 5 ms flushes that track's buffered context; its meeting-local speaker exemplars remain. No text or STT boundary enters the segmentation pipeline.

Source context depends on phase. `Phase::Live` preserves five seconds (80,000 real source frames at 16 kHz); `Phase::Refined` uses the canonical ten seconds (160,000 frames). Both retain a 2.5-second steady-state ownership stride. The pinned pyannote model expects ten seconds, so sherpa pads live windows and shorter terminal/gap tails. Full refined windows supply ten seconds of real source. Its one-window path returns local segmentation labels without global clustering, including simultaneous local speakers; timestamps remain clipped to actual source frames and the owned interval.

`Phase::segmentation_context()` returns a serializable `SegmentationContext { source_window_ms, ownership_stride_ms, native_window_ms }`: Live is `{5000,2500,10000}` and Refined is `{10000,2500,10000}`. **Source window is the configured maximum**, not a claim that every turn observed ten seconds of real context. On finish, gap or format change, a shorter remaining tail is native-padded and owns all its remaining actual frames; that terminal ownership can exceed the normal 2.5-second stride. No timestamp or source duration is invented for padding.

Per-track audio storage remains bounded: normal accepted input leaves less than one configured source window queued, with initial capacity of window plus 32,000 frames for the maximum two-second resampled packet (112,000 frames live, 192,000 refined). Source audio is never concatenated for a whole meeting. The larger refined context changes segmentation context only; live timing, short/Unknown/overlap handling, clustering thresholds, constraints and embedding persistence policy remain unchanged.

The public sherpa Rust diarizer does not expose raw segmentation matrices or embeddings. The implementation therefore obtains local turns from its single-window path and independently invokes `SpeakerEmbeddingExtractor` on clean turn audio. Segmentation and standalone embedding sessions each use one CPU thread and run sequentially on the speaker worker. The segmentation wrapper also constructs an internal extractor, so two copies of the embedding model can be resident.

An emitted turn shorter than 1,000 ms is `Unknown` and receives no speaker ID. No short turn inherits its predecessor. Overlapped intervals never update a clean speaker exemplar. If both local voices have clean owned turns in that window, their speaker links are retained and remapped after final clustering; otherwise the interval remains `Overlap` without an invented identity.

Clean turns use nearest-cluster cosine assignment only above 0.55 and a 0.10 second-best margin. Otherwise a new provisional cluster opens. It becomes visible after 3,000 ms of owned speech. Up to 20 L2-normalized exemplars per cluster are retained, pruning the exemplar least similar to the mean. There are at most 64 clusters per track. Cosine scores are similarities, not identification probabilities. These thresholds are placeholders tied to this exact embedding/preprocessing version, not calibrated defaults.

`finish` records `Some(config.phase.segmentation_context())` from the actual native engine, flushes both tracks, resolves persisted temporal constraints, replays up to 8,192 in-memory clean-turn embeddings with those constraints, and performs constrained agglomeration of bounded seed clusters above 0.70. A bounded exemplar-sample silhouette score with a soft live-count prior selects among that merge hierarchy. Must-links override acoustic similarity; cannot-links block assignments and merges. Transitive must-link components are computed before assignment, so a future endpoint cannot make an earlier greedy seed violate a known cannot-link. Conflicting constraint components become explicit pending reviews and Unknown regions instead of aborting transcript processing. This refinement can split a previous live seed when a supplied constraint requires it, but cannot automatically undo all mistaken unconstrained seed assignments. Global raw-embedding agglomeration and complete DER acceptance remain open.

Time anchors include track/start/end and resolve to the unique maximum-overlap non-overlap turn, requiring at least 50% anchor coverage. The standalone strict `resolve_temporal_constraints` utility returns `DiarizeError::Constraint` for unresolved anchors. The normal `Diarizer::finish` path evaluates constraints individually: missing, tied, under-covered, overlapping or unavailable embedding evidence becomes `ConstraintReview { provenance, reason, anchors }`. Conflicting resolved components are handled the same way. Every intersecting affected turn (and any correction connected to it) becomes Unknown with empty speaker IDs, `uncertain=true`, no score and `constraint-review` provenance. Those records are excluded from final clustering; valid corrections and baseline clustering outside the affected component continue. The full transcript and turn timing can therefore still be published alongside pending reviews. Model, native-audio and capacity failures remain hard errors. A defensive replay failure carries its implicated turn IDs (or exact violated-pair endpoints), expands only their connected constraint component and retries after reviewing/masking that component. Retries are bounded by the initial candidate count plus one. Independent corrections on either track remain active and are not converted into failures.

When a valid relationship depends on an already-unknown endpoint, it is temporarily deferred under the **original failed source review**. That source review gains the dependent spans and explicit dependency wording; the valid neighbor's provenance is not emitted as a separate removable failure. Fixing or removing the original correction allows the dependent relationship to apply on a later refinement. A prior-only microphone must-link records explicit user confirmation of that channel relationship, not biometric recognition.

A remote microphone cannot-link present at construction switches that track from the Me channel prior to actual embedding/clustering. If such a correction is added only at finish, its earlier microphone audio has no stored embeddings: affected microphone regions become Unknown and a review asks for refinement again, while unaffected microphone regions retain the default Me prior. A fresh refinement receives the saved correction at construction and can compute the required voices. Unresolved live constraints also mask affected outgoing provisional labels until final replay.

`DiarizationResult.segmentation_context` is optional and serde-defaulted for compatibility. Legacy results without metadata remain `None`; callers must never infer ten-second context from an older run kind. Pure synthetic finalization also leaves it unknown. Session persistence copies existing context unchanged when annotations are retained without a new acoustic pass.

Serial turn IDs are run-local only. `set_temporal_constraints` allows the caller to supply a current anchor snapshot before finalization.

Bounds are 32,768 turn records per meeting, 8,192 clean embeddings, 64 clusters per track and 20 exemplars per cluster. Reaching a cap is an explicit capacity error, never silent truncation or a complete result. Any `accept` error poisons that instance and clears buffered audio/replay vectors; future calls require a new instance, so ignoring an error cannot grow its audio queue. Unnamed diarization embeddings exist only in memory and are cleared from replay storage after successful finalization. `DiarizationResult`, `SpeakerTurn` and model manifests contain no embeddings. Per-track speaker clusters have distinct namespaces, and the system track cannot receive the remote microphone `mic:me` prior.

## Key classes

| Entry | File | Responsibility |
| --- | --- | --- |
| `VoiceEmbedder`, `voice_embedding_version` | `crates/vt-diarize/src/voice_embedding.rs` | Bounded explicit enrollment extraction and its separate uncalibrated version identity |
| `Diarizer` | `crates/vt-diarize/src/engine.rs` | Independent track clocks, source windows, native inference, turn ownership |
| `Clusters` | `crates/vt-diarize/src/cluster.rs` | Bounded exemplars, provisional assignment, transitive constraints, agglomeration/silhouette |
| Constraint planning and finalization | `crates/vt-diarize/src/finalize.rs` | Per-constraint reviews, affected-region masking, safe replay and remote-prior override |
| `DiarizerConfig`, `Phase`, `SegmentationContext`, `SpeakerTurn`, `Speaker`, `DiarizationResult`, `ConstraintReview` | `crates/vt-diarize/src/lib.rs` | Serializable integration contracts without biometric vectors |
| `Anchor`, `TemporalConstraint`, `resolve_temporal_constraints` | `crates/vt-diarize/src/lib.rs` | Durable temporal identity anchors and explicit ambiguity rejection |
| `turns_at_midpoint` | `crates/vt-diarize/src/lib.rs` | Half-open word midpoint assignment without invented word timings |
| `ModelManager` | `crates/vt-diarize/src/models.rs` | Explicit verified model acquisition and pinned provenance |

## Model provenance and license evidence

| Artifact | Exact bytes | SHA-256 |
| --- | ---: | --- |
| Official segmentation archive | 6,958,444 | `24615ee884c897d9d2ba09bb4d30da6bb1b15e685065962db5b02e76e4996488` |
| `model.int8.onnx` within that archive | 1,540,506 | `d582f4b4c6b48205de7e0643c57df0df5615a3c176189be3fc461e9d18827b5d` |
| Official ERes2Net ONNX | 39,593,761 | `1a331345f04805badbb495c775a6ddffcdd1a732567d5ec8b3d5749e3c7a5e4b` |

The actual segmentation release archive includes the MIT license, copyright 2022 CNRS; the installed pack retains that text. Its original [pyannote model card](https://huggingface.co/pyannote/segmentation-3.0) is marked MIT and describes a gated Hugging Face acquisition process. This application downloads the [public official sherpa release](https://k2-fsa.github.io/sherpa/onnx/speaker-diarization/models.html) only on request and never logs into or accepts conditions on that original repository.

The original [ERes2Net ModelScope card](https://modelscope.cn/models/iic/speech_eres2net_base_sv_zh-cn_3dspeaker_16k) explicitly declares Apache License 2.0; the [3D-Speaker project license](https://github.com/modelscope/3D-Speaker/blob/main/LICENSE) is retained as well. The converted ONNX metadata identifies that exact original model, Chinese source language, 16 kHz, 512 output dimensions, normalized samples and global-mean feature normalization. The manifest records source URLs, authors/attribution, conversion, license evidence, acquisition notice and the uncalibrated profile. Original weight revisions/conversion commits are not provided by those release assets; no revision is invented.

`EMBEDDING_VERSION` is SHA-256 over the pinned weights hash plus sherpa 2.0.1, mono 16 kHz, metadata preprocessing, post-L2 normalization and experimental calibration-v1/thresholds. It must change with any of those inputs. No comparison with another model version is supported.

## Integration, storage and access

The session layer owns explicit user setup actions, model status/progress, background-worker scheduling, immutable processing-run publication, names/identity constraint persistence and UI warnings. User names should attach through temporal constraints and root reconciliation, not by preserving a serial cluster index across reruns. The crate creates no database tables and has no SQL artifacts or HTTP application routes. `VoiceEmbedder` in `crates/vt-diarize/src/voice_embedding.rs` verifies the same pinned model pack and recomputes one to five seconds of contiguous finite mono source audio for explicit enrollment. Each call has fresh 16 kHz resampling state and returns a normalized 512-dimensional vector only in memory. Its separate `voice_embedding_version()` hashes the pinned embedding weights, sherpa/preprocessing/resampling contract, individual f32 normalization, duration-weighted f64 mean normalization and explicitly unavailable identification calibration. It neither compares people nor writes vectors; the session validates consent, actual refined segmentation-context metadata (a Refine run name alone is insufficient), source eligibility and encryption before persistence. Persist only result metadata and explicit user constraints, never unnamed vectors. Model setup accesses official download hosts; inference performs no network access or permission request.

Full live/refined duration-overlap/Hungarian split/merge identity reconciliation, calibrated cross-meeting recognition, calibrated English/channel thresholds, annotated DER/overlap scores, long-call CPU/memory/latency, overload/backpressure behavior and live five-second / refined ten-second segmentation quality remain independent acceptance work. Live labels must remain visibly provisional/experimental; a public smoke fixture is not a calibrated diarization benchmark.

## Validation

`cargo test -p vt-diarize` uses synthetic embeddings/time anchors and does not download models or read meeting audio. It covers margin ambiguity, three-second provisional visibility, per-track isolation, capacity, must/cannot conflicts, constrained merges, silhouette selection, temporal rerun matching and word midpoint boundaries.

The opt-in real test uses only an explicitly supplied public fixture and verified model pack:

```sh
VT_DIARIZE_MODEL_DIR=/path/to/verified/pyannote3-eres2net-2024-v1 \
VT_DIARIZE_FIXTURE_WAV=/path/to/official/0-four-speakers-zh.wav \
cargo test -p vt-diarize real_public -- --ignored --nocapture --test-threads=1
```

That official four-speaker demonstration passed actual segmentation/embedding inference, multiple voice clusters, audio-derived clock bounds, short-turn Unknown behavior and remote microphone channel priors. The observed smoke result was 19 turns and six experimental clusters on a known four-speaker clip, demonstrating over-segmentation and the need for calibration; it did not establish DER or overlap accuracy. The fixture is downloaded to a temporary research cache only; its separate redistribution license is not established, so it is not bundled and is not the project's annotated English evaluation set. The real native regression also verifies that a constructor microphone cannot-link overrides the Me prior, a late correction produces Unknown/review instead of aborting, and an unmatched time anchor retains the full native turn count. No private user recording was used.

The phase-context regression uses a SHA-pinned public 6.625-second control through `VT_DIARIZE_CONTEXT_WAV`. Its small `src/fixtures/live5-public-control-baseline.json` was captured from the unchanged live five-second engine before this increment; it contains derived times/labels/flags, no audio or embedding vectors. The new native live test compares update/final geometry and policy fields against that baseline (floating scores are not golden values). The refined test verifies no emission before ten seconds, two owned prefixes, a longer terminal tail, short-turn Unknown policy, independent gaps/rate changes, sample bounds, bounded buffers and explicit provenance. Existing public multi-speaker, 48 kHz/gap and user-constraint tests remain separate regressions. The constraint test now feeds sixteen seconds rather than eight, so it still supplies at least two clean owned turns under ten-second source context; split/late/unmatched assertions are preserved.

The enrollment extractor has pure version/geometry regressions and an opt-in SHA-pinned public-control test for actual finite, normalized, repeatable output. A separate AppService test repeats that public control under a synthetic single-speaker turn assignment and exercises actual decoding/extraction plus encrypted persistence with a fake key provider. Both open no devices or OS secrets and establish integration, not speaker purity or identification accuracy.

`VoiceEmbeddingProfile` now also exposes a separately versioned English WeSpeaker ResNet34 research extractor through `VoiceEmbedder::new_with_profile`. This is used only by explicit developer calibration comparisons. Existing `VoiceEmbedder::new`, application enrollment and `Diarizer` still use the original ERes2Net pack and versions. No speaker-turn clustering profile, model preference or recognition availability changes. The [comparison checkpoint](../../../planning/Architecture/2026-11-07-english-speaker-calibration-comparison.md) records the research contract and evidence.

The additional `wespeaker-resnet34-cmn-v1` research identity restores required external feature mean subtraction through a deterministic ONNX metadata addition. It retains the original graph/parameters and uses a distinct derived-container pin, transformation marker and attribution; the ordinary WeSpeaker profile remains unchanged for reproducing its failed result. Both paths remain developer research choices, and the app continues to construct ERes2Net by default.

`wespeaker-resnet34-reference-frontend-v1` keeps those CMN bytes and changes only the research frontend to Hamming, snipped edges and an 8 kHz cutoff. `VoiceEmbedder::new_with_profile` rejects that profile. The calibration command constructs it from `crates/vt-speaker-frontend` after the pack is verified. Application enrollment and diarization still use ERes2Net.
