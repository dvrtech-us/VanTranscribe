# Reference-backed evaluation

`cargo xtask eval` scores explicitly supplied local reference/hypothesis files. It does not capture audio, run models, download datasets, infer missing ground truth, or certify a milestone. The shipped example is entirely fictional and validates the scorer's arithmetic.

The separate optional [production inference runner](../production-inference/production-inference.md) now creates actual local Zipformer/VAD, Parakeet and live/refined diarization hypotheses from pinned public audio/model inputs and invokes this scorer. The plain `eval` command remains model-free. Public measured results still do not establish the in-house or hardware acceptance gates.

## User flow

1. Prepare a consented/public corpus with known transcripts, RTTM speaker annotations and explicit UEM evaluation regions. Keep measurement metadata and annotation provenance with it.
2. Write a schema-version 1 manifest using the format below. Paths resolve relative to that manifest, not the current working directory.
3. Run `cargo xtask eval --manifest PATH --output PATH`.
4. Inspect each recording/channel, corpus aggregates, raw error counts, source SHA-256 fingerprints, measurement coverage and target comparisons. Missing evidence is unavailable, not zero or success.
5. Review corpus/hardware qualification separately before making any product-quality or milestone claim. Reports always say `acceptance: "not_claimed"`.

The included smoke command is:

```sh
cargo xtask eval \
  --manifest xtask/fixtures/synthetic/manifest.json \
  --output /private/tmp/vantranscribe-synthetic-evaluation.json
```

This fictional example intentionally has errors: aggregate DER and JER are both one third; live WER is one third and batch WER is zero. These are arithmetic examples, not VanTranscribe benchmark results.

## Manifest and file formats

```json
{
  "schema_version": 1,
  "corpus": {
    "id": "unique-corpus-version",
    "kind": "synthetic",
    "description": "What was annotated and how",
    "provenance": ["Public source or consent/annotation record"]
  },
  "normalization": "unicode_lowercase_punctuation_to_space",
  "recordings": [{
    "id": "meeting-id",
    "channel": "1",
    "diarization": {
      "reference_rttm": "reference.rttm",
      "hypothesis_rttm": "hypothesis.rttm",
      "uem": "evaluation.uem"
    },
    "transcripts": [{
      "id": "utterance-or-meeting-pair",
      "reference": "reference.txt",
      "hypotheses": {"live": "live.txt", "batch": "batch.txt"}
    }],
    "bleed": {"reference": "bleed-reference.json", "hypothesis": "bleed-hypothesis.json"},
    "observations": "observations.json"
  }]
}
```

Corpus kinds are `synthetic`, `public` and `consented`; the harness records the caller's declaration, not an independent consent verification. Corpus ID, description and provenance entries must be nonempty. Each recording/channel is unique and supplies at least one evidence family. Diarization, transcript, bleed and observation families can be omitted independently; supplied families must be complete. Unknown JSON fields are rejected.

RTTM accepts ten-column `SPEAKER` records, plus blank/comment lines:

```text
SPEAKER meeting-id 1 0.0 2.0 <NA> <NA> speaker-A <NA> <NA>
```

The fourth/fifth fields are onset and positive duration in seconds. UEM is explicit, with recording ID, channel, start and exclusive end:

```text
meeting-id 1 0.0 10.0
```

Overlapping/adjacent UEM ranges are unioned. Both annotations are clipped to the evaluation support after reference collars are removed. Hypothesis speech outside UEM cannot add false alarms or influence mapping. A reference file containing other recordings but no matching ID/channel is an error. An explicitly empty reference file represents known non-speech over the supplied UEM. An empty hypothesis or a missing recording in a multi-recording hypothesis is an all-miss prediction, not a missing reference. Missing files, invalid/nonfinite/negative intervals, `<NA>` speaker IDs, and overlapping/duplicate intervals of the same speaker are errors. Different speakers may overlap and are scored.

Transcripts are UTF-8 plain text. Arbitrary hypothesis-role names are reported independently. The special roles `live` and `batch` are compared only on the exact transcript units that supply both, preventing a ratio from unrelated test sets. Units may be whole meetings or aligned utterances. Split very large pairs into known reference-paired utterance files rather than assuming line alignment.

Bleed reference JSON contains every evaluated final line and its human/reference category:

```json
{"segments":[
  {"id":"line-1","track":"mic","start_ms":1000,"end_ms":2000,"kind":"bleed_duplicate"},
  {"id":"line-2","track":"mic","start_ms":2100,"end_ms":2400,"kind":"genuine_short_reply"}
]}
```

Categories are `bleed_duplicate`, `genuine_short_reply` and `genuine_other`. Hypotheses explicitly supply one decision for every reference ID:

```json
{"annotations":[
  {"segment_id":"line-1","hidden_as_bleed":true},
  {"segment_id":"line-2","hidden_as_bleed":false}
]}
```

Missing/duplicate/extra IDs and invalid track/time geometry fail. The harness does not infer duplicate speech from text similarity.

Optional observations use a declared monotonic session clock and measured event times:

```json
{
  "clock":"monotonic_session_ms",
  "metadata":{"machine":"Measured machine","source":"Experiment/log reference","os":"OS version","profile":"Balanced","provider":"cpu","notes":"Sampling method"},
  "latency":[{"kind":"partial","reference_time_ms":1000,"emitted_time_ms":1800}],
  "resources":[{"measured_at_ms":1000,"cpu_total_percent":20,"rss_bytes":700000000}]
}
```

Latency kinds are `partial` (speech onset → first visible partial), `final` (utterance end → final text), and `speaker_label` (final text → live label). The caller must measure those actual events; audio chunk duration and callback arrival time are not substitutes. Emission precedes reference, nonfinite/negative values and empty observations are errors. CPU is percent of the whole machine, not percent of one core. CPU/RSS samples must have the same machine/OS/profile/provider context within one manifest; mixed resource contexts require separate manifests. Metadata source can differ between trials. Omit unmeasured CPU/RSS; do not insert zero RSS. Resource statistics are sample-based, not an automatic thermal/battery or sustained-load qualification.

## Metric definitions

| Metric | Fixed definition |
|---|---|
| WER | Minimum Levenshtein substitutions + deletions + insertions, divided by reference words; aggregate counts before division |
| Normalization | Mandatory choice: `exact_whitespace` preserves case/punctuation and splits Unicode whitespace; `unicode_lowercase_punctuation_to_space` lowercases characters, keeps Unicode alphanumeric characters, replaces every other character with space, then splits whitespace |
| DER collar | Exactly `pyannote.metrics(collar=0.25)`: **total width 250 ms, ±125 ms** about every reference segment boundary; overlap is retained |
| DER mapping | One global maximum co-occurrence-duration Hungarian assignment per recording/channel, after UEM/collar exclusion; zero-weight pairs remain unmatched |
| DER components | Integrate missed `max(R−H,0)`, false alarm `max(H−R,0)`, confusion `min(R,H)−correctly_mapped`; divide their sum by reference speaker-seconds, including overlapping speakers separately |
| JER | Same duration-optimal assignment as pyannote; for each reference speaker use `(reference + mapped hypothesis − 2×intersection) / union`; an unmatched reference contributes 1; average across reference speakers |
| Bleed annotation rate | Hypothesis lines hidden as bleed / all annotated final lines |
| Visible duplicate rate | Ground-truth duplicate lines left visible / all annotated final lines |
| Lost short replies | Ground-truth genuine short replies hidden as bleed; denominator/count also reported |
| Latency | Nearest-rank `ceil(q×N)` p50/p95/p99, plus maximum and sample count; no interpolation |
| CPU/RSS | Percentiles/max of supplied samples; CPU also includes an explicitly unweighted sample mean |

Word normalization does not perform NFC/NFKC normalization, expand numbers or preserve apostrophes/hyphens: for example `don't` becomes `don t` under punctuation-to-space normalization. Equal-edit-distance WER paths prefer substitution, deletion, then insertion; the total WER does not depend on that tie convention.

Diarization timestamps are canonicalized to one nanosecond before boundary processing to avoid decimal `0.1+0.2` versus `0.3` roundoff changing half-open activity. The Hungarian solver is an independent deterministic implementation over sorted label names. Equal-overlap optimal mappings can choose different labels from a different solver; per-recording mappings are retained for audit, and exact SciPy tie-breaking is not claimed. JER follows pyannote's duration assignment, **not** a separate intersection-over-union-optimal assignment. Unmatched hypothesis speakers affect DER but are not directly included in reference-speaker-averaged JER.

Empty-reference conventions are explicit: DER is 0 when reference and hypothesis are both empty, and 1 when there is hypothesis error but zero reference speaker-time; pyannote JER is 1 when no reference speaker survives scoring. Those diarization edge values do not by themselves satisfy the DER target. WER is `null` when there are no reference words, retaining raw insertion counts. Zero annotated bleed lines and absent short-reply examples yield unavailable rate/short-reply comparisons. Missing latency/resource families remain `null`.

Corpus DER aggregates component durations before division. Corpus JER aggregates per-reference-speaker errors/counts, not an unweighted meeting mean. Speaker galleries remain independent across meetings/channels, including identical label strings reused in different recordings.

## Targets and practical limits

Reports compare measured evidence to the approved design's targets: refined DER ≤20%; live WER ≤1.5× batch WER on identical units; probable-bleed annotation and visible duplicate rates <2%; zero lost genuine short replies; maximum partial/final/speaker-label latencies ≤1500/4000/1000 ms; reference-machine mean live CPU ≤25%; maximum RSS ≤1,200,000,000 bytes (decimal 1.2 GB). The visible-duplicate comparison is a reference-backed diagnostic alongside the literal annotation-rate target. No fixed WER or JER threshold is invented.

The four-speaker in-house corpus, real speaker/headphone/double-talk coverage, hardware/profile qualification, and sustained CPU/thermal/battery/device lifecycle evidence remain external acceptance work. A synthetic or narrow public fixture cannot satisfy those obligations. CI scheduling and private/consented corpus provisioning are outside this crate; this change supplies a reproducible command and checked-in fictional smoke corpus.

Inputs are limited to 64 MiB per file and 200,000 RTTM records; times must fit 0–1,000,000,000 seconds. A scored gallery has at most 256 identities per recording/channel, with bounded dense-overlap work. WER uses linear-row memory, at most 100,000 words per side and 100 million alignment cells per pair; larger input must be split into reference-paired units. These limits fail explicitly rather than returning partial scores.

## Technical flow and key types

1. `main.rs` strictly parses the `eval --manifest --output` command.
2. `Inputs` reads required UTF-8 files, records SHA-256/size/path fingerprints, and rejects files changing between repeated reads.
3. `evaluate_manifest` validates provenance/scope, parses each evidence family, computes independent recording/channel metrics, then aggregates raw components.
4. `write_report` writes compact JSON through a synchronized temporary file and atomically publishes it. An output path resolving to any input is rejected.

| Component | Source | Responsibility |
|---|---|---|
| `evaluate_manifest`, `Report`, `Inputs` | `xtask/src/lib.rs` | Validated corpus orchestration, provenance, aggregates and target comparisons |
| `score`, `maximum_assignment` | `xtask/src/diarization.rs` | Boundary sweep, fixed collars, overlap counts and own Hungarian assignment |
| `WordScore`, `Normalization` | `xtask/src/words.rs` | Explicit token normalization and Levenshtein counts |
| `BleedScore`, `Measurements` | `xtask/src/observations.rs` | Reference-labelled bleed decisions and measured event/resource statistics |
| CLI regression | `xtask/tests/cli.rs` | Actual subprocess invocation, compact output and invalid options |
| Fictional corpus | `xtask/fixtures/synthetic/` | Hand-computable, shareable metric examples without audio |

The `.cargo/config.toml` alias preserves the existing deployment environment configuration. Root workspace membership/dependency locking is owned by the project workspace.

## Integration, access control and persistence

There are no HTTP routes, accounts, application DB migrations or SQL artifacts. This developer command reads only explicitly named local files and writes the requested report; it opens no microphone, output device or meeting database and performs no network requests. It emits counts/mappings and source fingerprints, not transcript text. Reports still contain corpus IDs, labels, provenance and measurement metadata and should be handled according to their source corpus permissions.

## Primary references

- [pyannote collar extrusion](https://github.com/pyannote/pyannote-metrics/blob/develop/src/pyannote/metrics/utils.py) explicitly constructs each exclusion as boundary ± half the `collar` argument.
- [pyannote DER and JER](https://github.com/pyannote/pyannote-metrics/blob/develop/src/pyannote/metrics/diarization.py): both use the Hungarian duration mapper; JER then iterates reference speakers and averages union-normalized errors, returning 1 when none exist.
- [pyannote HungarianMapper and local error counts](https://github.com/pyannote/pyannote-metrics/blob/develop/src/pyannote/metrics/matcher.py) maximize co-occurrence and leave zero-overlap pairs unmapped.
- [pyannote identification error denominator/empty edge](https://github.com/pyannote/pyannote-metrics/blob/develop/src/pyannote/metrics/identification.py).

## Speaker-memory research commands

With the explicit `inference` feature, `voice-eval` runs the pinned native embedding extractor on a separately prepared public corpus. It does not acquire data, open devices, access Keychain, use an application database or install a calibration profile. `voice-version` prints the raw extraction contract and the unchanged legacy unavailable-calibration version without running inference.

```sh
cargo run --offline --locked --release -p xtask --features inference -- voice-version
cargo run --offline --locked --release -p xtask --features inference -- voice-eval \
  --manifest /path/to/prepared-corpus/manifest.json \
  --output /path/to/new-research-directory
```

The separate [public-corpus preparation tools](../../../../xtask/scripts/voice_corpus/README.md) explicitly acquire licensed AMI source ranges. Their network effects belong to those developer commands; `voice-eval` itself stays offline. The [frozen-protocol record](../../../planning/Quality/2026-11-07-open-set-speaker-corpus.md) records primary sources, availability exclusions and scientific limits.

### Input and processing contract

The strict schema-1 manifest supplies a protocol ID, channel, licence URL, annotation/partition hashes, verified model directory, expected raw embedding-contract SHA-256 and two partitions named `calibration` then `held_out`. Each partition has exactly fifty unique gallery people, one enrollment for each, known probes for every person and unknown probes. Person and project sets are disjoint between partitions. Known probes use a different meeting from enrollment; stronger cross-project probes within a partition are allowed rather than forcing the weaker same-project condition. A sample supplies an opaque ID/person/meeting/project and windows with local relative path, SHA-256, original source URL/frame interval/ETag/rate and optional full-source hash. Optional local annotation and frozen-partition files are actually hash-verified; exact exported labels, roles and source-frame windows are bound to the partition when supplied. An unverified declared digest is never reported as a verified source file.

Audio windows are canonical PCM16 little-endian mono WAV, one to five seconds at a declared 8–192 kHz rate. Paths must stay within the canonical manifest directory, including symlink resolution. The model directory may be external but its production constructor verifies the complete pinned pack. Each sample stays on one original source/rate; enrollment needs twenty seconds, probes fifteen, and a sample is capped at 120 seconds/120 windows. Up to 4,096 samples and a 16 MiB manifest are accepted. Original frame ranges cannot overlap globally or be reused between samples. All window hashes/geometry are checked before inference and again when read for extraction.

Native normalized window vectors form a duration-weighted normalized mean. Only in-memory means enter the shared distinct-person ranker in `vt-speakers`; vectors are zeroized where owned and never serialized. The command processes calibration first and fits a frozen grid: automatic score −1.00 through 1.00 in 0.01 steps, runner-up margin 0.01 through 2.00 in 0.01 steps. It maximizes correct recognition over all known calibration trials subject to both known-wrong/known and unknown-false/unknown being at most 2%; ties choose the higher score threshold, then higher margin. The algorithm is fixed before any held-out scoring. A missing feasible policy or calibration recognition below 90% produces an unavailable report without extracting held-out embeddings.

For an eligible candidate, the complete research-policy JSON is synchronized and pinned before any held-out embedding is extracted. Held-out trials use that policy once without retuning. Identity labels are fixed: unlike DER, no Hungarian relabeling can turn a wrong person into a correct identification. Reports include raw counts, all-known recognition including abstentions, wrong-known rate, unknown false identification and total false identification; zero denominators are unavailable. Wilson intervals are descriptive binomial-trial summaries with an explicit warning that repeated people/projects are correlated. Observed target comparisons do not certify population error rates.

The output is a new directory only. `INCOMPLETE` and an error receipt remain on failure; a synchronized report and final atomic `COMPLETE` marker establish a finished research run. A publication failure cannot intentionally leave a success marker. Reports declare `research_only=true`, `production_eligible=false`, `runtime_pin_generated=false`, distinguish native inference from injected test engines and never enable app recognition. The model, manifest, source-window and optional frozen-metadata hashes remain auditable. Source ETags are descriptive and a clipped-window digest cannot establish an original full-file hash.

| Component | Source | Responsibility |
|---|---|---|
| `speaker_calibration::run/version` | `xtask/src/speaker_calibration.rs` | Strict corpus binding, native means, calibration-only fitting, frozen policy and held-out reporting |
| `rank_for_evaluation`, `evaluate_identification` | `crates/vt-speakers/src/matching.rs` | Shared compatible-person scoring and fixed-label denominators |
| `VoiceEmbedder`, `voice_embedding_contract_sha256` | `crates/vt-diarize/src/voice_embedding.rs` | Pinned bounded extraction and raw feature identity |
| `VoiceEmbeddingProfile` | `crates/vt-diarize/src/voice_profile.rs` | Typed profile/version/dimension and pinned source/derived-model verification |
| `ReferenceEmbedder` | `crates/vt-speaker-frontend/src/lib.rs` | Research-only Hamming frontend over the pinned CMN WeSpeaker file |
| `compare_reference_features.py` | `xtask/scripts/voice_frontend/` | Raw log-mel comparison against torchaudio's Kaldi fbank; no identity score |
| `prepare_wespeaker_cmn.py` | `xtask/scripts/voice_model/` | Exclusive bounded metadata-only model preparation with exact source/derived pins |
| Publication/leakage/feature tests | `xtask/src/speaker_calibration/tests.rs`, `xtask/tests/cli.rs` | Fictional inputs, no private audio and no fabricated native-inference claim |

There are no additional application routes, credentials, database tables or SQL artifacts for this research path. A public room-microphone benchmark does not establish remote-codec/phone, gallery sizes above fifty, in-house, end-to-end diarization, hardware or long-call acceptance.

The policy file and its directory entry are synchronized before held-out extraction, so the freeze ordering survives a crash. The once-only held-out restriction is procedural across separate invocations: repeating a run into a new directory consumes the same held-out set and must be disclosed. Public results are cited only after both `annotation_file_verified` and `partition_file_and_export_binding_verified` are true. In the research metrics, `embedding_version` denotes the raw extraction-contract hash, not a production identification/calibration identity. The executable links the speaker library but does not invoke its database or Keychain APIs.

The [first native fifty-person calibration result](../../../planning/Architecture/2026-11-07-speaker-matching-calibration-validation.md#actual-native-result) is 113/150 known recognition, one wrong-known, 36 known abstentions and 0/42 unknown false identifications. It misses the 90% calibration gate, so no held-out inference ran and automatic speaker matching remains unavailable. No runtime pin was emitted.

### Calibration-only model comparisons

`voice-eval --calibration-only` selects an explicit research mode that never extracts or ranks held-out samples, including when the calibration target passes. The report states `selected_mode: "calibration_only"`, keeps `held_out` null and its trials empty, and separately reports calibration-policy availability. It produces no frozen policy for a held-out pass. Existing invocations retain the full-mode behavior described above. Candidate model selection uses calibration only.

The optional manifest `embedding_profile` defaults to `eres2net-v1`. The explicitly selected `wespeaker-resnet34-v1` research profile uses pinned English VoxCeleb WeSpeaker ResNet34 weights and 256-dimensional vectors. It requires its own extraction-contract hash and strict model manifest, exact weight bytes/SHA-256 and Apache-2.0 file before native construction. Original ERes2Net uses its unchanged 512-dimensional full-pack verification. Native output dimensions must agree with the chosen profile. `VoiceEmbeddingProfile` in `crates/vt-diarize/src/voice_profile.rs` owns identities and verification; `VoiceEmbedder::new_with_profile` shares the same bounded per-span preprocessing/normalization, while `new` and application callers retain the original profile. There is no app-model switch, automatic migration or calibrated runtime pin.

```sh
cargo run --offline --locked --release -p xtask --features inference -- voice-version --profile wespeaker-resnet34-v1
cargo run --offline --locked --release -p xtask --features inference -- voice-eval --calibration-only \
  --manifest /path/to/prepared-corpus/wespeaker-input.json \
  --output /path/to/new-calibration-comparison
```

See the [English extractor comparison checkpoint](../../../planning/Architecture/2026-11-07-english-speaker-calibration-comparison.md) for provenance, scientific scope and measured status.

The ordinary published English WeSpeaker candidate also failed calibration (13/150 recognized, 137 abstentions, 0 wrong-known and 0/42 unknown false IDs). Its [source/metadata audit](../../../planning/Architecture/2026-11-07-english-speaker-calibration-comparison.md#ordinary-published-model-result-and-preprocessing-audit) identified missing external feature mean normalization: reference WeSpeaker ONNX inference requires it, while the published sherpa metadata omits its activation flag. A distinct metadata-corrected profile has been implemented and measured; the failed original remains immutable and has not consumed held-out identity scores.

The CMN candidate is explicit `embedding_profile: "wespeaker-resnet34-cmn-v1"`; its model manifest additionally requires `transformation: "onnx-metadata-feature-normalize-global-mean-v1"`. Omission remains `unmodified` only for the historical ordinary pack. The [pinned preparation tool](../../../../xtask/scripts/voice_model/README.md) appends a fixed 39-byte metadata entry to the exact known source, rejects overwrite/symlinks/wrong bytes and cleans up only its own partial output. It reproduces the separately pinned derived container without rewriting graph or parameters. The model’s selected modification/attribution notice is verified byte for byte before native initialization.

The corrected CMN candidate recognizes 130/150 known probes (86.67%), with three wrong-known, 17 abstentions and 0/42 unknown false IDs. It misses the 90% calibration gate. Held-out identity scoring remains zero for that candidate. [Aggregate evidence](measurements/2026-11-07-open-set-calibration.json) preserves the exact counts, policy, report/input/model hashes and compiled source pins. App recognition stays unavailable.

`wespeaker-resnet34-reference-frontend-v1` uses the same CMN weights through `crates/vt-speaker-frontend`. The adapter requests Hamming, snipped edges and `high_freq = 0` (8 kHz at 16 kHz) and returns raw 256-dimensional output for the same L2 normalization. `VoiceEmbedder::new` and `new_with_profile` do not construct it. The research command does, after pack verification. Raw log-mel frames matched `torchaudio.compliance.kaldi.fbank` 2.11.0 within a predeclared maximum absolute error of 0.001 on four synthetic fixtures and one frozen calibration window. The later calibration-only run recognizes 138/150 known probes (92%), with 0 wrong-known, 12 abstentions and 0/42 unknown false IDs, at score 0.62 / margin 0.01. Held-out trials remain empty because the command was calibration-only. That pass does not enable application matching or publish a release pin. Counts and hashes are in [the calibration record](measurements/2026-11-07-reference-frontend-calibration.json) and [the feature record](measurements/2026-11-07-reference-frontend-features.json).

```sh
python3 xtask/scripts/voice_frontend/compare_reference_features.py \
  --probe-bin target/debug/examples/feature_probe \
  --output /path/to/new-feature-report.json
```
