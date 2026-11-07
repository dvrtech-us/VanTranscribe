# Public corpus production inference evidence

- Date: 2026-11-07
- Feature: production-inference
- Related code: `xtask/src/inference/`, `xtask/scripts/prepare_ami.py`, `crates/vt-session/src/live_speaker_tests.rs`, `crates/vt-session/src/live_models.rs`

## Context

The existing evaluator scored supplied hypotheses correctly but did not establish whether production model pipelines emitted useful transcripts or speaker turns on real annotated meeting audio. The prior Chinese demonstration had no reference-backed English WER/DER/JER evidence and did not calibrate the speaker thresholds.

Selecting a useful application profile also requires checking that candidate gains survive the production VAD gate and actual live worker pipeline. Direct-core WER alone cannot establish worker readiness, sealed model provenance or preservation of the original recording. The bounded public evidence supported the separately discussed Standard/0.2 recommendation while retaining legacy Small/0.5 behavior.

The copied-crate source-context experiment also requires a production-path check: the canonical segmentation model accepts ten seconds, while the live path intentionally uses a shorter source context. Refinement can use the full real context without changing live latency or discarding the existing uncertainty and constraint safeguards.

## Decisions

Add an explicit optional inference feature around the existing production crate APIs. Run engines sequentially against the same public source; retain actual words, events, turn assignments, model/source hashes, binary identity, settings, constructor time, call durations and process observations. Distinguish true live emitted assignments from retrospective final clustering. Both pacing modes leave product target latency unavailable; paced delivery and wall-time traces remain separate event artifacts.

Acquire only one official AMI far-field WAV and one manual annotation ZIP after checking licence, lengths and local free space. Derive a bounded five-minute four-speaker reference from the official NXT annotation clocks. Preserve the source and derivation rather than inventing official RTTM files or uniform timestamps.

Keep optional raw/ungated/public-control/fixed-gain diagnostics separately labelled. Never replace a poor baseline with an improved diagnostic or automatically modify production configuration. Record acoustic amplitude/clipping and native detector counts to distinguish runner, detector and recognizer effects.

Audit the exact official model feature contract and evaluate endpoint/FP32 settings before considering replacement models. Evaluate two explicitly licensed candidates on the original development window and untouched next 300 seconds. June 21 LibriSpeech+GigaSpeech Zipformer gives the better measured cost/quality tradeoff; Nemotron's small additional raw improvement costs substantially more time/memory and uses a different custom licence. Benchmark execution never activates or replaces an application model automatically.

A one-constant copied-crate experiment isolates 10-second offline source context while keeping all product assignment safeguards. A separate native clustering sweep is labelled as a different algorithm/policy. The standard BUTSpeechFIT only-words reference is checked for exact interval geometry and used only for secondary rescoring, retaining original segment-based evidence. The holdout has three lexical speakers and four participants only when non-lexical annotations are included; this distinction is preserved.

Use a bounded diarization-only production probe to validate the implemented Live5s/Refined10s phase contract without rerunning unrelated ASR. Record actual source/native/ownership context, constructor and processing time, sampled resources, unknown/overlap counts, fixed reference scores and pinned prior-artifact parity. Compare refined10 against a clearly labelled finalizedLive5s retrospective control; keep actual emittedLive scores separate. Preserve raw source, both reference protocols, model weights, thresholds, ownership stride and absent user constraints/reference count. Check the existing clean public control for native bounds and counts without inventing speaker-reference scores.

Compare both catalogue model IDs through the actual production recognizer APIs, rather than assuming native-candidate gains survive VAD. This confirms a substantial Standard benefit but reveals material default-gate deletions. A bounded Standard-only 0.2/0.35/0.5 production-option study preserves raw PCM and every other setting, requires exact default/explicit-0.5 native-event parity, and retains both public clean controls and deterministic silence/noise false-output observations. Threshold 0.2 improves both windows and preserves those controls; this supports the explicit profile decision without claiming global calibration.

Validate both application profiles through the actual `AppService` STT/VAD and independent speaker workers. Preserve the existing opt-in Small case and parameterize its implementation by `LiveModel`; add an independently named Standard case using the previously installed, pinned public cache. Both use only the existing public WAV and a paced fake capture helper in an isolated temporary app root, with no devices, network or existing user meetings. Check fresh/legacy selection, requested model/threshold/file provenance read from the sealed live run, source word bounds, exact decoded raw PCM24 on both tracks, and a complete backlog audit without shedding or inference gaps.

## Alternatives considered

- Hand-authored model hypotheses: retained only for scorer arithmetic tests; insufficient to assess production inference.
- Choosing a cleaner dataset after a failure: rejected because the quiet far-field failure is relevant to the approved in-person use case.
- Scoring retrospective clusters as live labels: rejected; live updates must be measured before finish replay.
- Treating accelerated packet times as UI latency: rejected because source time and wall time differ and the application rendering events are not observed.
- Downloading the full AMI corpus: unnecessary for this first bounded experiment; source/cache size remains explicit.
- Direct-core comparison only: retained for WER and native measurements, but insufficient to cover actual session workers and store integration.
- Replacing the Small integration case: rejected because legacy behavior must remain covered independently of the new recommendation.

## Consequences

- Positive: real source-backed failures are reproducible and can guide scoped production fixes.
- Positive: model acquisition stays explicit; ordinary scorer builds/tests stay fast and offline.
- Positive: both actual application live-worker cases passed in 24.87 seconds total. Each archived both 106,000-frame tracks bit exactly as PCM24, retained 36 final words and four speaker turns, and observed maximum source-watermark backlog of 290 ms with no shedding or gaps. Strict all-target session Clippy passed; exact source/code/model fingerprints and limits are retained in [the integration measurement record](../measurements/2026-11-07-public-live-worker-profiles.json).
- Positive: the [actual production context record](../measurements/2026-11-07-production-diarization-context.json) retains exact assignment parity with all four prior 5s/10s artifacts, excluding only phase provenance. Finalized5s→Refined10s original-reference DER improves 54.54→46.13% on development and 68.81→50.70% on holdout; separate only-words DER improves 47.53→38.02% and 65.31→43.73%. Actual emittedLive stays separately scored on the retained 5s context; no ASR reruns or source/threshold changes are involved.
- Negative: one five-minute public window is narrow and speaker-imbalanced; segment-reference conventions and overlap ordering affect interpretation.
- Negative: process observations are sampled and allocator/runtime state can persist across sequential phases; they do not establish sustained live hardware performance.
- Negative: the ignored live-worker cases require explicitly prepared public caches, and their short clean control does not establish meeting accuracy or real capture reliability.
- Negative: refined10 processing increases from 10.91/13.40 to 16.63/17.39 seconds for the two windows. Refined assignments still contain 19/13 distinct IDs including provisional/uncertain assignments, with only 6/3 surviving masking into scored RTTM; remaining fragmentation and missed speech prevent a quality acceptance claim. Warm-cache constructor and sampled RSS/CPU observations are qualified rather than treated as sustained hardware gates.
- Remaining gates: approved in-house four-speaker recordings, gallery of 50, capture/UI latency, real route/double-talk coverage and sustained hardware qualification remain unsatisfied by public evidence alone.
