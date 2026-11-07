# Production inference baseline

- Inference is available only through the explicit optional `inference` Cargo feature.
- The command reads an explicit public corpus manifest and verified local cached models; it opens no audio device, app database or network connection.
- Exact source hashes, corpus licence/attribution/URLs/derivation and pinned production model provenance are retained.
- All four baseline phases analyze the same source samples and source clocks, with no reference identity, count, transcript or constraint supplied to inference.
- Streaming text comes from real Zipformer final events with production Silero VAD. Batch text comes from real Parakeet refinement.
- Live RTTM uses emitted live updates, not retrospective `finish` assignments. Refined RTTM uses the separate refined pass.
- Unknown/uncertain/provisional intervals never gain synthesized speaker labels or timestamps. Their reference speech can count as missed.
- Separate live/refined reports use existing fixed-collar overlap-aware metrics; live/batch WER is paired on the same annotated window.
- Missing references, source pin changes, invalid bounds and model verification failures fail; absent resource/target-latency measurements remain unavailable.
- Raw WAV, annotation sources, model files and app preferences remain unchanged, including during gain diagnostics.
- Existing output directories are never overwritten. Only a successfully published `COMPLETE.json` marks a finished evidence set.
- Source and model payload hashes are rechecked before reports are published.
- First-constructor load measurements do not claim flushed OS caches. Unpaced source-to-wall timing is not product latency.
- CPU/RSS are explicitly sampled process observations, not true peaks, thermal qualification or a live-concurrency acceptance claim.
- All reports state `acceptance: not_claimed`; public data does not replace in-house/gallery/hardware acceptance.
- Experimental `probe` variants remain separate from production baselines and do not activate cached candidates or change app defaults.
- Legacy transcriber-segment and standard only-words references are explicit independent protocols; rescoring preserves original hypotheses/scores.
- A holdout with three lexical speakers is reported as three; non-lexical vocal activity is not relabelled as lexical speech.
- Live-only catalogue comparisons invoke production constructors with verified manifests, raw gain 1 and identical packet/endpoint settings for VAD on/off; they never activate a model in the app.
- Threshold studies alter only explicit production `LiveVadOptions.threshold`; existing default 0.5 remains unchanged and exact native-event parity is required when a preserved default baseline is supplied.
- Generated non-speech controls retain a deterministic source hash and report actual emitted text/event counts; zero ASR output is not described as zero gate openings or global calibration.
- Explicit ignored public live-worker integration tests use an isolated temporary app root, a paced fake capture helper and verified hardlinked public caches; both Small/0.5 and Standard/0.2 must seal exact original PCM24 on both tracks with persisted requested-profile provenance, final word bounds and no inference shedding/gaps. They do not open real devices or existing meeting databases.
- The diarization-only context probe instantiates production Live5s and Refined10s with unchanged 2.5s ownership stride, native10s input, raw PCM, constraints/prior, model pins and experimental thresholds; no ASR runs. Live emitted assignments and finalizedLive5s retrospective controls remain distinct, and legacy/only-words reference protocols are scored separately.

| Condition | Severity | Blocks? |
| --- | --- | --- |
| Missing licence/source/attribution/derivation, invalid schema or source SHA | Error | Yes |
| Unknown JSON field, output already exists, invalid WAV/UEM bound | Error | Yes |
| Missing or mismatched pinned production model | Error | Yes |
| Source/model changes during inference | Error | Yes; no finished marker |
| Missing process probe | Unavailable | No; omit scorer observation family |
| No model speech/text/identity emitted | Measured failure | No fabricated hypothesis or acceptance |
| Diagnostic improves a corpus score | Diagnostic only | No automatic production change |

Defaults: unpaced replay, approximately 20 ms source packets, in-person Mic track, no speaker prior/constraints, two STT threads, one VAD/segmentation/embedding thread, sequential engines. Bounds: 600-second mono WAV, 8–192 kHz, 64 MiB audio/source files, 16 MiB transcript/RTTM, 1 MiB UEM/manifest, at most four diagnostic gains drawn from 2/4/8/16 and a 30-second explicit control.

Side effects: validate declared public inputs → hash sources/models → validate reference scope → create new output/INCOMPLETE marker → run real engines → persist hypotheses/observations → recheck hashes → score → publish COMPLETE marker. No capture permissions or external service credentials are used. The operator owns source licence compliance; there is no separate account/access-control surface.

- The evaluator latency-observation array remains empty in paced and unpaced modes; per-event paced delivery and emission traces are separate core evidence.
