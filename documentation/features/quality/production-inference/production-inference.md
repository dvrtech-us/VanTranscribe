# Production inference benchmarks

The optional `xtask` inference runner executes VanTranscribe's production local CPU speech and speaker engines on explicitly supplied public audio. It writes auditable hypotheses and passes them to the existing reference-backed scorer. It never opens a microphone/output device, discovers meetings, uses an application database, downloads data/models, or calls a cloud service. Public-corpus results do not satisfy the separate in-house four-speaker, gallery-of-50, hardware, or long-call acceptance gates.

## User flow

1. Obtain a licensed public corpus explicitly and retain its licence, attribution, source URLs and immutable source hashes. Check free space and remote sizes before acquiring large assets. The runner does not acquire anything.
2. Prepare a finite mono WAV and corresponding reference text, RTTM and UEM. Preserve real source timing and identities; do not manufacture timestamps or labels for missing annotations.
3. Install the existing pinned production models through their explicit setup workflow, or supply already verified cache directories.
4. Write an inference manifest, then run:

   ```sh
   cargo run --locked --release -p xtask --features inference -- infer \
     --manifest /path/to/inference-input.json \
     --output /path/to/new-evidence-directory
   ```

5. Inspect `COMPLETE.json`, `inference.json`, live/batch transcripts and events, live/refined turn JSON and RTTM, and both score reports. Failed runs retain diagnostic stage files and `INCOMPLETE.json`; those are not finished benchmark evidence. Existing output directories are never merged or replaced.
6. Reproduce scoring without models through `cargo xtask eval --manifest NEW_DIRECTORY/refined-manifest.json --output NEW_REPORT.json`.

The feature is absent from ordinary `xtask` builds. Calling `infer` without `--features inference` fails explicitly. Scorer tests remain independent of model availability.

## Manifest

All source paths resolve relative to the input manifest; absolute paths are also accepted. Each `PinnedFile` has `path` and an exact lowercase `sha256`. The schema rejects unknown fields.

```json
{
  "schema_version": 1,
  "public_corpus": {
    "id": "corpus-version-and-window",
    "recording_id": "rttm-recording-id",
    "license": "CC-BY-4.0",
    "attribution": "Actual corpus owner and attribution",
    "source_urls": ["https://official-source.example/corpus"],
    "derivation": "Exact crop, reference derivation and overlap ordering"
  },
  "audio": {"path": "audio.wav", "sha256": "actual SHA256"},
  "reference_text": {"path": "reference.txt", "sha256": "actual SHA256"},
  "reference_rttm": {"path": "reference.rttm", "sha256": "actual SHA256"},
  "uem": {"path": "regions.uem", "sha256": "actual SHA256"},
  "annotation_sources": [{"path": "original-annotations.zip", "sha256": "actual SHA256"}],
  "models": {
    "streaming": "/cache/sherpa-onnx-streaming-zipformer-en-2023-06-26",
    "refinement": "/cache/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8",
    "diarization": "/cache/pyannote3-eres2net-2024-v1"
  },
  "paced_live": false
}
```

`public_corpus` is an explicit operator declaration and provenance record, not an automated licence/consent verifier. This command is scoped to public benchmarks; it offers no private-meeting identifier, database or discovery mode. Audio is bounded to 64 MiB, mono 8–192 kHz and at most 600 seconds. References and annotation sources have separate file-size bounds, and UEM cannot extend beyond the audio. Model constructors enforce the existing pinned model identities; the runner additionally records all model manifest and ONNX/text file hashes and rechecks sources/models before publishing reports.

## Technical flow

Inference phases execute sequentially, with fresh engine state and the same Mic source track:

1. `StreamingRecognizer::with_vad` runs production Zipformer with the pinned Silero model. Input packets span approximately 20 ms using native sample-count clocks. Real partial/final events and native word timings are retained. Only actual final events form `live.txt`.
2. `BatchRefiner` runs production Parakeet plus bounded Silero utterance segmentation. Real segment and word timings form `batch-events.json`; emitted text forms `batch.txt`.
3. `Diarizer` in in-person/live mode analyzes audio independently from transcript words/segments. Live RTTM uses only actual `accept`/tail-reset turn updates, before `finish` can retrospectively replay or merge identities.
4. A separate in-person/refined `Diarizer` passes through the same source and publishes its final clustering. Live source context is 5 seconds and refined source context is 10 seconds; both retain a 2.5-second ownership stride and the pinned model's 10-second native input. Actual context metadata is recorded from `Phase::segmentation_context()`. No reference speaker count/prior, name, constraint or word timing is supplied to either engine.
5. Unknown, uncertain and provisional turns remain visible in turn JSON. They do not gain invented RTTM speaker labels and therefore count as missed reference speech where appropriate. Known overlap assignments emit one RTTM line per actual assigned identity.
6. The existing evaluator produces separate live/refined DER/JER reports and paired live/batch WER on the identical reference window, with the fixed 250 ms total collar and overlap scoring.

STT model threads are two; VAD, speaker segmentation and embedding each use one as configured by production constructors. Engines are dropped between phases. The allocator/native runtime may retain memory; the observed process RSS includes that real process behavior.

| Component | Source | Responsibility |
| --- | --- | --- |
| `inference::run`, `Input`, `PinnedFile` | `xtask/src/inference/mod.rs` | Explicit sources, pin checks, output safety, production phases and scorer manifests |
| `stages::live`, `batch`, `diarize` | `xtask/src/inference/stages.rs` | Actual production API calls, native timestamps and measured emissions |
| `Telemetry` | `xtask/src/inference/telemetry.rs` | Read-only process CPU/RSS sampling and machine metadata |
| `diagnostics::run` | `xtask/src/inference/diagnostics.rs` | Explicit separate ungated/control/fixed-gain ablations |
| `production_diarization::run` | `xtask/src/inference/production_diarization.rs` | Diarization-only production phase/context comparison, prior-artifact parity and separate reference protocols |
| `prepare_ami.py` | `xtask/scripts/prepare_ami.py` | Bounded local AMI crop and transparent NXT-to-reference derivation |
| Runner/derivation tests | `xtask/src/inference/tests.rs`, `xtask/scripts/test_prepare_ami.py` | Source pins, uncertainty, strict schema and annotation/time preservation |

## Timings and resource interpretation

`inference.json` retains first-constructor load duration, processing wall time, audio duration, real-time factor, packet-call duration percentiles, actual event emissions, runner binary hash and settings. Constructor time includes production model verification; OS file caches are not cleared, so it is not a claim of uncached disk cold-start behavior.

Unpaced mode measures throughput. Source audio time and wall time are different clocks; it does not produce capture/UI latency target observations. Optional `paced_live: true` schedules each live packet at its real source end using a monotonic clock and records emission/delivery delays. Those core delivery delays still exclude capture callbacks, worker queues and rendering. The existing evaluator's speech-onset-to-visible-partial, utterance-end-to-final, and final-to-speaker-label targets remain unavailable because this runner does not observe those complete product events. The observations latency array remains empty in both modes; paced delivery delays and emission wall times live in the event/turn artifacts. Actual call durations and source/emission traces are retained without relabelling them as UI latency.

Between native calls, `ps` is sampled no more often than once per second. CPU percent is divided by the logical CPU count to estimate percent of the whole machine; it is the platform's process `%cpu` estimate, not interval CPU accounting. RSS converts reported KiB to bytes. Samples are observations rather than true peak RSS, sustained thermal/battery qualification, or live-concurrent-worker performance. Unavailable probes are marked and omitted from scoring rather than replaced by zero.

## Explicit diagnostics

An optional manifest `diagnostics` object can request `ungated: true`, `gains: [2,4,8,16]`, and a pinned `control_wav`. These results are written under `diagnostics/`, separately from the unchanged baseline.

- Ungated recognition uses actual production `StreamingRecognizer::new` with identical packet clocks, track and finish flow.
- The explicit short public control uses both production constructors. Without a supplied reference transcript, no control WER is invented.
- Gain ablations multiply a temporary corpus sample copy and clamp to [-1,1]. Each records clipping counts, RMS/peak, streaming and batch WER, refined DER/JER and engine timings. They never modify the source WAV, model, baseline hypotheses or application preferences.
- A native VAD diagnostic counts actual 512-sample detector windows with the same production detector settings, clear sequence and 16-second epoch reset. It distinguishes detector behavior from ASR/gate behavior without exposing raw audio in logs. It does not claim to count production gate-open windows.

Diagnostics establish failure mechanisms and candidate changes. They do not automatically select a gain or change production defaults.

## Public AMI reference preparation

The [official AMI download page](https://groups.inf.ed.ac.uk/ami/download/) supplies signals and transcription under CC BY 4.0. The bounded development corpus uses official `ES2002a.Array1-01.wav` (40,727,594 bytes) and manual annotations v1.6.2 (22,887,865-byte ZIP). Their SHA-256 hashes are respectively `38887e3bf7b597fe348f3818e9fc9844d7a1e15382aab52e5316dc3228fb6c1d` and `b56e5babb2496b8795deeeda7e71178d7fbc9963f94276cf2a3f4b56ebbc9f9d`. Source audio and annotations stay in `/private/tmp/vantranscribe-ami-eval`, outside the repository.

The `/private/tmp/vantranscribe-ami-eval` paths in the commands below were removed by a restart on 2026-11-10; AMI data now lives under `/Volumes/dev/vantranscribe-eval` (see [evaluation](../evaluation/evaluation.md#test-fixtures-and-evaluation-data-locations)). `prepare_ami.py` reads only these explicit local inputs. It retains the released licence and each A–D source XML, crops PCM16 samples without resampling, and preserves source offsets in provenance. Reference RTTM is **derived from official NXT segment annotations**, not described as an official prebuilt RTTM: `transcriber_start/end` are clipped to the selected window and same-speaker intervals are unioned. Reference text uses non-punctuation `<w>` items whose actual word midpoint is inside the window, ordered by start time, speaker and end. Non-lexical segment intervals remain in RTTM. Simultaneous words have no unique serial transcript order; whole-window WER includes that limitation. XML DTDs/entities are rejected.

The first [0,300) second slice contains all four annotated speakers, 479 lexical `<w>` items (514 tokens after the scorer's explicit normalization), and 53 merged reference intervals. It is a narrow, speaker-imbalanced far-field example, not a representative meeting test suite. Timed references, UEM and real acoustic difficulty are retained even when they produce poor scores.

Prepare that window from the already acquired assets with:

```sh
python3 xtask/scripts/prepare_ami.py \
  --audio /private/tmp/vantranscribe-ami-eval/ES2002a.Array1-01.wav \
  --annotations /private/tmp/vantranscribe-ami-eval/ami_public_manual_1.6.2.zip \
  --output /private/tmp/vantranscribe-ami-eval/new-first300 \
  --streaming /cache/sherpa-onnx-streaming-zipformer-en-2023-06-26 \
  --refinement /cache/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8 \
  --diarization /cache/pyannote3-eres2net-2024-v1
```

The generated `inference-input.json` pins the crop, original archive/WAV, timed words and each source XML. Adding `--start` or `--duration` is explicit; it does not search for an easier window. `--paced-live` enables real-time packet scheduling in the generated manifest.

`--rttm-kind only_words` explicitly selects the standard word-only speech support instead of the preserved legacy `segments` protocol. Corpus IDs include that protocol. The word-only intervals were checked against the [primary BUTSpeechFIT reference](https://github.com/BUTSpeechFIT/AMI-diarization-setup): interval geometry matches exactly after clipping and origin translation, up to identity-name permutation. The untouched [300,600) second holdout contains 731 lexical words but only **three lexical speakers**; the fourth participant appears in non-lexical vocal regions. Preparing this holdout's word-only references requires explicit `--allow-fewer-speakers`. Neither the waveform nor the reference timestamps are changed to obtain four lexical voices.

## Measured development evidence

The [2026-11-07 measurement record](measurements/2026-11-07-ami-es2002a-first300.json) retains source/model/binary fingerprints, raw error components, timings and qualified observations from the first 300 seconds. Local full artifacts are under `/private/tmp/vantranscribe-ami-eval/run-diagnostics-v1/`. The unmodified public audio has RMS 0.004634 and peak 0.30447.

| Corpus input | Live WER | Batch WER | Refined DER | Refined JER |
| --- | ---: | ---: | ---: | ---: |
| Raw baseline | 100.00% | 54.09% | 54.54% | 68.08% |
| Separate gain 2 diagnostic | 94.36% | 51.95% | 54.03% | 68.10% |
| Separate gain 4 diagnostic | 79.57% | 43.58% | 54.26% | 66.67% |
| Separate gain 8 diagnostic | 74.32% | 43.39% | 54.15% | 66.56% |
| Separate gain 16 diagnostic | 74.71% | 39.88% | 56.92% | 68.47% |

Raw live DER/JER are 56.74%/72.55%. Refined errors are dominated by missed speech: 102.406 seconds missed out of 195.141 scored reference speaker-seconds, with uncertain/unknown product assignments deliberately not promoted into named speakers. Raw native VAD detects 3,282 of 9,375 windows, while gated streaming ASR emits no events; raw ungated WER is 99.61%. The same known public short control emits the expected full sentence through both constructors. This isolates a real quiet-far-field inference weakness rather than a broken runner feed. Fixed gain helps deletions but leaves many substitutions and does not resolve diarization. No gain or production setting was selected automatically.

On the measured Mac16,7 / Darwin 27.2 arm64 release run, sampled baseline process RSS reaches roughly 1.44 GB; this exceeds the nominal 1.2 GB diagnostic comparison. Sampled process CPU estimates and individual phase durations are retained in the record; accelerated sequential phases do not establish live-concurrent hardware behavior. The first constructor loads were approximately 0.8–2.1 seconds for ASR and 0.15–0.18 seconds for diarization, with warm OS caches explicitly possible. Capture/UI latency remains unavailable. These measured failures identify further quality work; public or synthetic evidence has not passed the in-house milestone.

## Configuration, model and context audit

The separate [configuration/model/context evidence](measurements/2026-11-07-config-model-context-audit.json) preserves both corpus windows, candidate file hashes/licences, original and secondary-reference scores, resources and controlled source diffs. `probe --manifest PATH --output NEW_DIRECTORY` is available under the same optional inference feature for explicitly declared native-model experiments. Its manifest names one or two pinned public input manifests and bounded named variants with pinned encoder/decoder/joiner/tokens, licence/source URLs, fixed corpus-only gain, greedy/beam decoding and endpoint settings. It never installs or activates a candidate. Native candidate experiments bypass VAD and are labelled separately from production `infer` baselines.

The production June 26, 2023 model configuration matches the [official exact-model INT8 example](https://k2-fsa.github.io/sherpa/onnx/pretrained_models/online-transducer/zipformer-transducer-models.html#csukuangfj-sherpa-onnx-streaming-zipformer-en-2023-06-26-english): 16 kHz/80 mel bins, INT8 encoder/joiner, FP32 decoder, CPU and greedy search. Pinned sherpa 2.0.1's [C API conversion](https://github.com/k2-fsa/sherpa-onnx/blob/v2.0.1/sherpa-onnx/c-api/c-api.cc) leaves the native [normalized [-1,1] feature contract](https://github.com/k2-fsa/sherpa-onnx/blob/v2.0.1/sherpa-onnx/csrc/features.h) intact; multiplying by 32768 is not a missing Zipformer preprocessing step. Official 1.2-second/20-second endpoint rules give the same failed raw WER as the production 0.8-second/15-second rules. FP32 weights and four-path beam search do not rescue the quiet raw audio.

| Explicit native experiment, VAD off | Development WER | Next-300s holdout WER |
| --- | ---: | ---: |
| Current June 26 INT8, raw | 99.61% | 94.57% |
| Current June 26 FP32, raw | 99.22% | 94.70% |
| Current June 26 beam-4, raw | 98.44% | 92.37% |
| Current June 26 beam-4, separate gain 8 | 74.12% | 74.77% |
| June 21 LibriSpeech+GigaSpeech INT8, raw | 52.33% | 50.19% |
| June 21 INT8, separate gain 8 | 51.75% | 46.31% |
| Nemotron 560 ms INT8, raw | 49.42% | 48.25% |
| Nemotron, separate gain 8 | 46.11% | 41.01% |

The richer-training June 21 model is [Apache-2.0 at both the conversion owner and original training owner](https://huggingface.co/csukuangfj/sherpa-onnx-streaming-zipformer-en-2023-06-21). Its official archive is 506,956,414 bytes, SHA-256 `455f40e556aa2b20ac9d3bffd603b58002075c1193b4070938540c11efe0a4da`. Native constructor measurements are 0.334–0.421 seconds, RTF about 0.027, with sampled process RSS 504–670 MB across sequential variants. Those constructor measurements exclude the separate experiment pin checks and may use warm OS caches. Process CPU sample means are approximately 20.5–22.0% of the logical machine under the documented `ps` method.

The second/final candidate follows the [official pinned sherpa Nemotron example](https://github.com/k2-fsa/sherpa-onnx/blob/v2.0.1/c-api-examples/streaming-nemotron-c-api.c). Its model owner declares the [NVIDIA Open Model License](https://huggingface.co/nvidia/nemotron-speech-streaming-en-0.6b), **not Apache-2.0 or CC BY**. The 463,945,051-byte archive SHA-256 is `78e2b79fcf7271553a74402a76b771b09ea40117a39566a79f52235b23db6358`. Measured constructors are 0.768–0.839 seconds, RTF 0.322–0.330, sampled RSS up to 1.290 GB, and CPU sample means about 14.5–14.6%. Its modest raw WER improvement over June 21 costs roughly twelve times the processing wall time and exceeds the nominal RSS target in some samples. No automatic default replacement or new licence acceptance flow is implemented.

For diarization, a temporary copied production crate changes **only** `WINDOW: 80000 → 160000` samples for offline processing. Ownership stride, clustering, short-turn/overlap/Unknown policy, thresholds and constraints are byte-identical; a second byte-identical 5-second control reproduces the production development score. The source diff and hashes are retained with the measurement. This tests a bounded 10-second source context against the canonical model's native 10-second input without changing live behavior.

| Same product assignment policy | Legacy segment-reference DER, development / holdout | Secondary standard-only-words DER, development / holdout |
| --- | ---: | ---: |
| Production 5-second context | 54.54% / 68.81% | 47.53% / 65.31% |
| Controlled offline 10-second context | 46.13% / 50.70% | 38.02% / 43.73% |

Full native 10-second diarization additionally changes clustering and short-turn handling, so it is a separate diagnostic. Default complete-linkage cosine **distance** 0.5 fragments into 25/31 clusters. A diagnostic 0.75 distance gives secondary word-only DER 29.24%/36.57%, still 12/14 clusters and above the design target. It does not implement product Unknown, user constraints or identity review and is not a calibrated default recommendation. Original segment-based scores remain available alongside every secondary score; references were never silently replaced to improve a result.

The next supported decision is to validate June 21 as an explicit streaming model profile through the full production VAD/setup workflow, and consider 10-second context only for offline refinement with its existing safeguards. Model setup/licence notices, reproducible source pins, regression coverage and a separate approval of changed defaults remain required work. Neither this experiment nor the public holdout establishes in-house acceptance, a calibrated speaker threshold, live UI latency, or sustained hardware performance.

## Production catalogue and VAD comparison

The live-only `probe` mode `production_live_comparison` validates installed catalogue models through the **actual production** `StreamingRecognizer::with_vad` and `StreamingRecognizer::new` APIs. It does not reimplement the native recognizer or change model defaults. The catalogue model manifest ID and SHA-256 are explicit, and production constructors independently verify pinned weight identities and resolve their exact filenames.

The comparison config contains `schema_version: 1`, `mode: "production_live_comparison"`, one or two prepared public `clips` manifest paths, and one or two `models` objects with `id`, `directory` and `manifest_sha256`. It also requires a pinned `vad` file and an attributed `control` containing pinned `audio`, `attribution`, HTTPS `source_urls` and `license_note`. Optional `paced_control: true` schedules only the bounded public control at source speed. No batch or speaker model is constructed in this mode, and unused paths in prepared clip manifests are not activated.

Every clip runs both VAD settings on untouched PCM with the same 20 ms source-clock packets, Mic track, production endpoint/decoder settings, two configured STT threads and fresh engine state. The optional VAD remains one thread. The command retains WER, real events/word timings with checked source bounds, constructor/processing durations, per-phase CPU/RSS observations and an unchanged-input declaration backed by before/after file hashes. The short public control runs through the same feed and finish flow; without a supplied reference transcript it does not receive an invented WER score. Paced core delivery observations remain separate from capture/UI latency targets.

The output retains each model/VAD transcript, events and metrics, a comparison `results.json`, complete model/source provenance, and the same INCOMPLETE/COMPLETE publication markers. Unknown or duplicate model IDs, source/manifest hash changes and missing public control attribution fail explicitly. Original native-candidate/gain experiments and production baselines remain separate artifacts.

The [production-catalogue/VAD record](measurements/2026-11-07-production-catalog-vad.json) validates both pinned catalogue manifests through that real API. Source PCM and gain remain unchanged:

| Production model/path | Development WER | Untouched holdout WER |
| --- | ---: | ---: |
| June 26 Small, VAD default 0.5 | 100.00% | 97.93% |
| June 26 Small, VAD off | 99.61% | 94.57% |
| June 21 Standard, VAD default 0.5 | 68.87% | 63.52% |
| June 21 Standard, VAD off | 52.33% | 50.19% |

The candidate benefit survives the actual gate, but the default gate materially increases Standard deletions. Standard sampled RSS stays below 674 MB, constructors measure 0.681–0.727 seconds including production verification, and every emitted event/word time stays within the source bounds. All four paced public control runs emit the same full sentence. Maximum observed packet-delivery delays are 41–45 ms; these are core callback observations, not capture/UI or semantic onset latency.

## Bounded production VAD threshold study

`production_live_comparison` optionally accepts `vad_thresholds` (up to three unique finite values in 0.1–0.9, one model), a pinned `default_baseline` results file, and `generated_non_speech_control: true`. It invokes the production `StreamingRecognizer::with_vad_options` with `LiveVadOptions { threshold }`. No sample gain, model, decoder, endpoint, pre-roll, hangover or source-clock setting changes. Existing `with_vad` still uses the original default 0.5.

When a pinned default baseline is supplied, the study must include explicit 0.5. Native event text/finality, source start/end and word timings are compared exactly against the preserved default event files; different wall-clock/resource measurements are excluded. A mismatch leaves the study incomplete and fails. Threshold filenames include their exact f32 bits to avoid overwriting nearby numerical cases.

The [threshold evidence](measurements/2026-11-07-production-vad-thresholds.json) tests Standard on the same raw public inputs:

| Explicit Standard VAD threshold | Development WER | Untouched holdout WER |
| --- | ---: | ---: |
| 0.2 | 53.31% | 48.25% |
| 0.35 | 62.26% | 56.27% |
| 0.5, unchanged default | 68.87% | 63.52% |

Explicit 0.5 matches the old default's native event/text/word projection exactly on both AMI slices and the clean control. All three clean controls emit the same full sentence. The fixed 60-second generated control consists of 30 seconds of exact zeros followed by 30 seconds of deterministic colored noise with source SHA-256 recorded over its f32 little-endian samples. Every threshold emits **zero ASR events and zero final words** on that control. This does not establish that the VAD gate never opened, or a corpus-level false-positive rate. There is no synthetic speech, invented reference WER or acquired noise dataset.

Threshold 0.2 measures constructors of 0.676/0.703 seconds, RTF 0.0254/0.0274, sampled RSS 451/705 MB and process CPU sample means 18.3/19.3% on the development/holdout runs. These are the same qualified process measurements described above. Full artifacts remain at `/private/tmp/vantranscribe-ami-eval/production-vad-thresholds-v1/`; the preceding default comparison remains unchanged at `production-live-comparison-v1/`.

The evidence supports discussing an explicit **Standard model profile with threshold 0.2** as a candidate configuration. It improves both measured windows and preserves the tested controls, while the old Small/default-0.5 behavior remains intact. It is not global VAD calibration: one AMI meeting and one generated non-speech control cannot establish broad noise/channel behavior, in-house quality acceptance or a safe automatic default. No app activation or preference change is performed by the benchmark.

## Production diarization context comparison

The `probe` mode `production_diarization_context` runs only the real production `Diarizer`; it does not construct ASR or rerun WER. Its strict schema requires one or two `clips`, each containing pinned `legacy_manifest`, `word_only_manifest`, `baseline_five_turns` and `experimental_ten_turns` files. The manifests are the existing prepared public inputs, and the prior turn files are preserved artifacts rather than newly synthesized baselines. `model_directory` and exact `model_manifest_sha256` select the already verified diarization pack. An explicitly attributed `control` contains pinned `audio`, HTTPS `source_urls`, `attribution` and `license_note`.

```sh
cargo run --locked --release -p xtask --features inference -- probe \
  --manifest /private/tmp/vantranscribe-ami-eval/production-diarization-context.json \
  --output /private/tmp/vantranscribe-ami-eval/new-context-evidence
```

Fresh in-person/Mic engines process unchanged raw PCM through 20 ms source-clock packets. `Phase::Live` has 5-second source context; `Phase::Refined` has 10 seconds. Both retain a 2.5-second ownership stride, the same pinned 10-second native segmentation input, one segmentation thread, one embedding thread, no constraints or reference speaker count, and the unchanged experimental clustering thresholds. Short-turn, overlap, uncertainty and constraint safeguards remain production behavior. `Phase::segmentation_context()` and the actual result's optional context metadata must agree; native turns must stay within source bounds.

The live-emitted view uses actual updates before retrospective finish. A separately named **finalized Live5s control** uses that engine's final assignments solely for the matched before/after comparison with refined10; it is never presented as live labels. Both reference protocols receive independent DER/JER scores from the existing fixed-collar, overlap-aware scorer. Unknown/uncertain/provisional turns remain unscored identities and can count as missed speech. Prior-artifact parity compares every turn field except its phase-dependent provenance string; timings, IDs, assignments, scores and uncertainty bits remain part of the comparison.

Results retain constructor/processing times, sampled CPU/RSS, turn/unknown/overlap counts, warnings, model/input hashes and separate context settings. Output directories are new, guarded by INCOMPLETE/COMPLETE markers, and source/model hashes are rechecked before publication. The clean control gets actual turn counts and source-bound checks, with no invented DER/JER when speaker references are absent. Adjacent windows of one public meeting do not establish in-house or gallery acceptance, and unpaced processing is not product latency.

The [production context record](measurements/2026-11-07-production-diarization-context.json) confirms that the real implementation retains the copied-crate experiment's measured benefit. All four final-assignment projections match the pinned prior 5-second control and 10-second experiment exactly, excluding only the phase-dependent provenance string. Clustering and model-threshold source files are byte-identical to the control; finalization changes only context warning/metadata. Raw source, model hashes, 2.5-second ownership stride, experimental assign/merge/margin thresholds 0.55/0.70/0.10, and absent user constraints/reference-count prior remain unchanged.

| Reference protocol / window | Finalized Live5s control DER / JER | Production Refined10s DER / JER |
| --- | ---: | ---: |
| Original segments, development [0,300) | 54.54% / 68.08% | 46.13% / 58.60% |
| Original segments, holdout [300,600) | 68.81% / 69.56% | 50.70% / 59.84% |
| Secondary only-words, development | 47.53% / 63.18% | 38.02% / 47.64% |
| Secondary only-words, holdout | 65.31% / 53.25% | 43.73% / 38.60% |

Actual **emittedLive5s** remains separate: original-reference DER is 56.74%/77.51% on development/holdout, and secondary only-words DER is 49.41%/75.03%. The improved retrospective controls are not relabelled as live accuracy.

| Bounded production result | Development 5s → 10s | Holdout 5s → 10s |
| --- | ---: | ---: |
| Total turns | 94 → 98 | 132 → 139 |
| Unknown/uncertain/provisional turns | 48 → 46 | 80 → 79 |
| Overlap turns | 9 → 10 | 16 → 19 |
| Processing wall seconds | 10.91 → 16.63 | 13.40 → 17.39 |

AMI constructor times are 159–176 ms with warm OS caches possible; maximum sampled process RSS is 484.5 MB and mean process CPU estimates are approximately 7.0–7.1% of the logical machine. These sequential direct-core observations include retained allocator/runtime memory and do not measure peak RSS or concurrent app performance. The clean public control remains source-bounded in both phases, with one assigned identity and zero uncertain/overlap turns; 5s emits two final turns while 10s emits one. No control DER/JER is invented without speaker references.

Refined results still contain **19/13 distinct assigned IDs including provisional/uncertain assignments** on development/holdout; only **6/3 identities survive masking into scored RTTM**. This fragmentation, substantial remaining missed speech and scores above the target remain visible. The holdout has three lexical speakers under only-words but four participants including non-lexical regions under the original protocol. Better scores here do not establish four-speaker identity correctness, calibrated thresholds, DER ≤20%, or any in-house/gallery gate. Full immutable run artifacts remain at `/private/tmp/vantranscribe-ami-eval/production-diarization-context-v1/`; original baselines and experiment records remain unchanged.

## Public live-worker profile integration

The separately ignored tests in `crates/vt-session/src/live_speaker_tests.rs` exercise the actual `AppService` live worker pipeline, beyond the direct-core benchmark. A parameterized helper preserves the legacy Small case and adds the pinned Standard case. Each creates an isolated temporary application root and database, hardlinks explicitly cached public model files and a known 6.625-second public WAV, and streams two identical 16 kHz tracks through a paced fake NDJSON capture helper. It never starts a real capture/output device, downloads anything, or opens an existing meeting database. Native STT, VAD and speaker worker readiness must be observed before feeding audio; automatic refinement and summaries remain disabled.

The cases check legacy Small-directory retention and fresh Standard selection, then read the sealed live run's `engines_json` from storage. Requested model, archive/file hashes, VAD request and exact f32 threshold must match the selected application profile: Small/0.5 or Standard/0.2. Both tracks must contain final words with source-bounded times and real speaker turns; the Remote Mic `Me` label remains an explicit channel prior. Every archived FLAC is hash-checked and decoded back to all 106,000 original PCM24 frames per track. The persisted inference audit must contain no shedding events or control/inference gaps and must not be truncated. Exposed meeting detail must contain no embedding vectors, the public source must remain byte-identical, and playback must remain inactive.

Run the explicit local cases sequentially after placing the already verified caches at the paths named in the test:

```sh
cargo test --locked -p vt-session live_speaker_tests:: -- \
  --ignored --nocapture --test-threads=1
```

This is bounded functional backend integration evidence for the application profile wiring. It does not measure WER on meetings, GUI rendering, source-onset latency, real device capture, sustained thermal behavior or the separate in-house acceptance gates. The runner itself still does not activate application preferences; the application's reviewed profile selection is owned by `vt-session/src/live_models.rs`.

The [public live-worker record](measurements/2026-11-07-public-live-worker-profiles.json) reports both cases passing in 24.87 seconds total. Each retained 36 final words and four speaker turns, exact raw frames on both tracks, and maximum source-watermark backlog of 290 ms with zero shedding or inference gaps. Strict all-target session Clippy passed. These observations remain separate from the AMI WER study and from capture/UI latency targets.

## Integration, access and persistence

There are no HTTP routes, Tauri commands, accounts, meeting DB tables, migrations or SQL artifacts. The only writes are to a new explicitly named evidence directory and explicit preparation outputs. Audio/model caches are not deleted or modified. Reports contain public corpus labels, text, hypotheses and filesystem paths; distribute them with corpus attribution and licence. The code does not authenticate the operator's public-source declaration.

The plain `eval` command remains a model-free scorer. Production inference is an optional extension; its results follow the same fixed metric definitions and `acceptance: not_claimed` rule documented in [reference-backed evaluation](../evaluation/evaluation.md).
