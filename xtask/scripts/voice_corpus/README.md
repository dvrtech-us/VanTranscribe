# Public AMI voice-identification corpus preparation

These developer tools prepare a reproducible **research-only** corpus for `cargo xtask voice-eval`. They never open audio devices, read the app database or Keychain, inspect model files, calculate embeddings, or activate recognition. Only public participant IDs and annotation timings enter the manifests. Audio, ZIP/XML caches and generated frame plans belong outside this repository.

The source is the [AMI Meeting Corpus](https://groups.inf.ed.ac.uk/ami/corpus/), AMI Consortium, University of Edinburgh/CSTR and participating institutions. Its [official download page](https://groups.inf.ed.ac.uk/ami/download/) supplies audio/transcription under [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/). Keep the official archive's `LICENCE.txt` with derived data. The scripts do not acquire the ZIP automatically: supply the official manual annotation release `ami_public_manual_1.6.2.zip`, SHA-256 `b56e5babb2496b8795deeeda7e71178d7fbc9963f94276cf2a3f4b56ebbc9f9d` (22,887,865 bytes).

The [protocol record](../../../documentation/planning/Quality/2026-11-07-open-set-speaker-corpus.md) defines the fixed seeds, source-frame masks, gallery size, exclusions and limitations. The a/b/c/d recordings are distinct sessions of the same staged design project; this is not natural-meeting, remote-codec or in-house acceptance. Calibration and held-out people/projects are disjoint. Unknown identities may share project/room acoustics with gallery teammates. No threshold or model score may change a frozen partition.

## Commands

Use an empty external directory. All paths are explicit; the examples contain no user-specific workspace path.

```sh
CORPUS_ROOT=/private/tmp/vantranscribe-voice-calibration
ANNOTATIONS=/path/to/ami_public_manual_1.6.2.zip

python3 xtask/scripts/voice_corpus/analyze_ami.py \
  --annotations "$ANNOTATIONS" --output "$CORPUS_ROOT/plan-v2" --protocol v2

python3 xtask/scripts/voice_corpus/freeze_partition.py \
  --plan "$CORPUS_ROOT/plan-v2/metadata-plan-v2.json" \
  --output "$CORPUS_ROOT/plan-v2/partition-v2.json"
```

The analyzer is cache-only. Inspect its counts/byte estimates and record the printed partition hash **before** acquisition or model evaluation. The published v2 pin is `d11f13458ec85f8921d8b4e423cae73fbc15d87a5fe2150d0278c302cb4e49a5`. Its prior-use exclusion reserves ES2002 for calibration. A plan using another annotation-path string has a different enclosing metadata-plan/partition hash; the exact published pin is reproduced when the declared original annotation path matches, while the selected subjects/frames remain independently comparable. Source paths are provenance, never a substitute for hashes.

```sh
python3 xtask/scripts/voice_corpus/acquire_ami.py \
  --partition "$CORPUS_ROOT/plan-v2/partition-v2.json" \
  --expected-sha256 ACTUAL_PRINTED_PARTITION_SHA256 \
  --output "$CORPUS_ROOT/ami-room-v2"
```

Acquisition is limited to two requests at a time, one MiB per request, and the exact official HTTPS `groups.inf.ed.ac.uk` Array1-01 paths. Redirects, 200 range fallbacks, weak/changed ETags, mismatched Content-Range, non-mono/non-16 kHz/non-PCM16 input, changed cache hashes and out-of-bounds source frames fail closed. Timeouts/retries/backoff are bounded. A failed source does not discard other completed sources. Resume only after the prior process is terminal, with the same frozen pin/output; existing byte hashes must verify.

After the full source audit is terminal, an availability-only v3 can exclude whole projects whose required selected sources returned 403/404/410 or invalid native geometry. Transient or unclassified failures abort this revision instead of silently dropping a project. V3 retains v2 phase assignments and person-ranking seed; it refills exactly fifty gallery people per phase or aborts. This revision must occur before **any** embedding or identity score. Preserve v2 and its failure evidence.

```sh
python3 xtask/scripts/voice_corpus/analyze_ami.py \
  --annotations "$ANNOTATIONS" --output "$CORPUS_ROOT/plan-v3" --protocol v3 \
  --parent-partition "$CORPUS_ROOT/plan-v2/partition-v2.json" \
  --availability-status "$CORPUS_ROOT/ami-room-v2/acquisition-status.json"

python3 xtask/scripts/voice_corpus/freeze_partition.py \
  --plan "$CORPUS_ROOT/plan-v3/metadata-plan-v3.json" \
  --parent-partition "$CORPUS_ROOT/plan-v2/partition-v2.json" \
  --availability-journal "$CORPUS_ROOT/plan-v3/availability-exclusions-v3.json" \
  --output "$CORPUS_ROOT/plan-v3/partition-v3.json"

python3 xtask/scripts/voice_corpus/acquire_ami.py \
  --partition "$CORPUS_ROOT/plan-v3/partition-v3.json" \
  --expected-sha256 ACTUAL_V3_PARTITION_SHA256 \
  --reuse-ranges "$CORPUS_ROOT/ami-room-v2/ranges" \
  --max-new-bytes 10485760 \
  --output "$CORPUS_ROOT/ami-room-v3"
```

The new-byte limit admits at most the configured successful response payload (including both concurrent reservations); retries remain separately bounded. A missing reuse cache fails before fetching. The reuse option links/copies immutable cache files into the new output, validates their SHA/ETag/origin geometry and fetches **only uncovered byte ranges**. It does not manufacture HTTP-response provenance for locally assembled bytes. Two-second fetch-gap coalescing reduces request count; those gaps do not enter exported speech windows. Final windows remain separate, unchanged one-to-five-second source spans.

Export only after `COMPLETE.json` exists. The verifier binds every sample/label/original frame range to the frozen partition, verifies every clip hash/native geometry and checks global source-window non-overlap. It supplies copied annotation/partition paths for native-runner hash checks. Model-directory resolution remains the runner/operator's separate step.

```sh
python3 xtask/scripts/voice_corpus/export_verified.py \
  --acquisition "$CORPUS_ROOT/ami-room-v3" \
  --partition "$CORPUS_ROOT/plan-v3/partition-v3.json" \
  --partition-sha256 ACTUAL_V3_PARTITION_SHA256 \
  --annotations "$ANNOTATIONS" \
  --embedding-contract-sha256 EXPECTED_PRODUCTION_RAW_CONTRACT_SHA256
```

Before native evaluation, run the input-only PCM audit on the exported template:

```sh
python3 xtask/scripts/voice_corpus/audit_pcm.py \
  --manifest "$CORPUS_ROOT/ami-room-v3/voice-eval-template.json" \
  --output "$CORPUS_ROOT/ami-room-v3/PCM-INPUT-AUDIT.json"
```

The audit verifies waveform hashes and original frame counts, then records nonzero counts/native peaks, constant PCM and duplicate waveform hashes across distinct origins. Any all-zero, constant/DC or duplicated-origin input stops before a model can run. It uses no RMS/quality threshold and never silently excludes or retunes a sample. Model-directory selection can change the outer manifest hash; the separate window-descriptor hash still binds the exact phase/role/identity/source-frame/waveform set.

An optional independently pinned full ES2002a source can verify exact downloaded-window bytes using `--full-control PATH --full-control-sha256 SHA256`; other full-source hashes remain null. ETags and per-range hashes must not be described as publisher-provided cryptographic full-file hashes.

## Network-free checks

```sh
python3 -m py_compile xtask/scripts/voice_corpus/*.py
python3 -m unittest discover -s xtask/scripts/voice_corpus/tests -v
```

The checks use synthetic PCM and fake HTTP responses. They prove rejection before reading a full 200 response, origin/redirect restrictions, range/ETag/payload bounds, cache integrity, native geometry, and reuse of only an uncovered source-byte gap. The exporter/PCM tests additionally check portable contained paths and stop-on-zero/DC/duplicate behavior. They perform no HTTP requests, real recording, model inference or biometric storage.
