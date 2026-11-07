# English speaker-extractor comparison

Date: 2026-11-07. Continues the [first failed calibration](2026-11-07-speaker-matching-calibration-validation.md#actual-native-result) within the unchanged accepted M0–M5 design. Prior Anthropic/Codex design reviews stand. The next concrete work is a separately versioned model comparison on calibration only; the held-out set remains unscored.

## Frozen inputs and model provenance

The ordinary English VoxCeleb WeSpeaker ResNet34 is a design-listed alternative, selected after the original ERes2Net calibration failed. [Official sherpa release assets](https://github.com/k2-fsa/sherpa-onnx/releases/tag/speaker-recongition-models) contain `wespeaker_en_voxceleb_resnet34.onnx`, asset 198883656, 26,534,365 bytes, updated 2024-10-14. The source [WeSpeaker model card](https://huggingface.co/Wespeaker/wespeaker-voxceleb-resnet34/blob/main/README.md) declares Apache-2.0, while its [pretrained-model policy](https://github.com/wenet-e2e/wespeaker/blob/master/docs/pretrained.md#model-license) assigns CC BY 4.0 to VoxCeleb weights. Both license notices and attribution are retained. Individual release-API digest fields are null; the publisher does provide the model hash in its separate `checksum.txt`. Downloaded bytes were locally frozen before any candidate identity score: SHA-256 `5ef208a9da1453335308a6b6f4e6dfbd7e183a38b604de0a57664f45d257fe94`. The independent local hash agrees with the publisher checksum. This is content integrity evidence, not a signature or independent scientific validation.

The research profile is `wespeaker-resnet34-v1`, 256 dimensions. Its raw extraction contract is `638b9f488bb743fb3c09eac905d8e9711469f129e88f57ffcfa19c9abdae1094`, and unavailable-calibration version is `4e71553593f13bb96e7d823b43f48451e9f5787bfd881883a0c12b5d242f9522`. Existing ERes2Net raw and enrollment versions remain unchanged. Native dimensions must match the selected profile; exact model bytes, strict manifest, both license notices and attribution are checked before initialization.

Only `embedding_profile`, `expected_embedding_contract_sha256` and `model_dir` differ from the earlier native-ready manifest. The new `voice-eval-wespeaker-input-v2.json` SHA is `262b11f5f54fe705b80d21532088a1b49c5e5b7946274f872825cae7e68d57cd`; frozen corpus partition remains `90b40f63eea00a794e8cb0e3805e24ec211100a18824a42191882d23d9ac8454`. The pre-score change record is `wespeaker-pre-score-comparison.json` under the external `ami-room-v3` cache. Every person, role, frame window, waveform, gallery and fitting-grid rule is unchanged. This run is explicitly calibration-only regardless of its result. No installed app model setting or private recording is touched.

## Evidence status

Implementation checks and final higher-model implementation review are complete with no blockers. The first native calibration-only comparison has completed below its recognition target; the failed result is preserved below. The original failed report is retained. Every candidate result remains research-only and non-production-eligible; the corpus's size, repeated-person/project correlation, shared channel, oracle masks and lack of in-house/remote-channel evidence still apply.

Before any candidate score, the new contract description was clarified to state exported-graph/native-metadata feature normalization; no implementation or weights changed. The superseded unscored input and revision journal are retained. WeSpeaker callers supply normalized f32 samples; native code applies the model metadata scaling once. No extra ERes2Net global-mean rule is injected. Model manifest verification requires both license notices and exact accompanying Apache/WeSpeaker attribution files.

The real public native-constructor probe passes (0.88 seconds): strict model manifest, all weight bytes/hash and both notices verify, and the native extractor reports exactly 256 dimensions. Ordinary profile/source guards pass (five cases, two intentional opt-in tests ignored in that focused group). The `--calibration-only` extension passes twelve focused cases, three real CLI tests and strict Clippy. Both 512- and 256-dimensional passing calibration tests assert the exact calibration extraction count and zero held-out calls. Version output supports profile selection without loading a model. Final integrated checks pass 390 ordinary Rust tests across 23 binaries, zero failures and 28 intentional opt-in/helper tests ignored. Strict all-target/all-feature Clippy and formatting pass. UI is unchanged from its prior 71-case gate. Final implementation review found no blockers. The first candidate calibration completed below target.

The publisher checksum file is 3,477 bytes, SHA-256 `c3a928cc16d165ce869553f71fcbe6fc1c1b513aee0665414548780a979b2325`. Its filename-first entry matches the independently downloaded model. Acquisition metadata and a later `publisher-checksum-verification.json` retain when each fact was established; the original API null-digest observation was not overwritten.

## Final implementation review

Opus found no blocking defects and independently traced the calibration-only guard, unchanged default construction, version/dimension checks, weight verification, native preprocessing and meaningful positive/negative tests. The optional manifest provenance files are present and hashed in the real input; actual reports must confirm both verification booleans before citation. The review correctly notes that validation still hashes held-out WAV bytes for integrity without computing embeddings or identity scores. One narrow CLI hardening rejects a following option where a path is required, preventing a malformed `--manifest --calibration-only` invocation from swallowing the mode flag. The real CLI regression places a file with that literal option name in its working directory and asserts early rejection before creating output. A publisher-checksum wording discrepancy in the draft trail was corrected. No model/fitting/dataset change follows this review.

The final scoped CLI guard passes all three real CLI tests, strict Clippy and formatting; the release CLI rebuilds. The single candidate run began at 12:30:55 UTC with `--calibration-only` explicitly in its command. Its executable SHA-256 is `e591fefb5a7d58bd85ab73146f1a3ac311e700b6b6a074ce54ecd2a35d2c773c`. Process/log metadata are `wespeaker-calibration-v1-process.json` and `wespeaker-calibration-v1.log` in the external cache, output `wespeaker-resnet34-calibration-run-v1`. Installed app metadata showed zero meetings/active processing immediately before launch. The command opens no capture/output devices or app database.

## Ordinary published-model result and preprocessing audit

The single ordinary WeSpeaker run completed with exit 0 at 12:33:57 UTC, 181.61 seconds wall time / 180.11 seconds in the runner. `COMPLETE` exists; `INCOMPLETE`/error do not. Report SHA-256 is `3c1288ad04d7530a1ebc53b87ece0ee1ca35b0526414e33209d09ffcbd7d74b4`. Native inference, annotation verification and frozen partition/export binding are true. Mode is explicitly calibration-only; held-out is null/empty, and no frozen held-out policy file exists.

| Calibration quantity | Actual result |
|---|---:|
| Correct known / all known | 13 / 150 = 8.67% |
| Wrong known | 0 / 150 |
| Known abstentions | 137 / 150 |
| Unknown false identification | 0 / 42 observed |
| Candidate score / margin | 0.82 / 0.08 |
| Unthresholded top-one known diagnostic | 71 / 150 |
| Held-out identity scores | 0 |

Independent recount confirms the policy, raw counts and fixed-grid maximum. The original failed ERes2Net result is also preserved. No candidate was promoted or installed.

The poor ordinary-model result triggered a concrete preprocessing audit, not a change to corpus labels or the fitting objective. The physical SHA-pinned ONNX metadata has no `feature_normalize_type`; the input graph starts `feats → Transpose → Unsqueeze → Conv` without input mean subtraction. [WeSpeaker reference ONNX inference](https://github.com/wenet-e2e/wespeaker/blob/master/wespeaker/bin/infer_onnx.py) explicitly subtracts the per-feature time mean before feeding the exported graph. [Sherpa 2.0.1 native extraction](https://github.com/k2-fsa/sherpa-onnx/blob/v2.0.1/sherpa-onnx/csrc/speaker-embedding-extractor-general-impl.h) does so only for metadata `feature_normalize_type=global-mean`. Thus the ordinary published integration omits an externally required normalization. No claim that English speaker models are intrinsically worse follows from this run.

A separately versioned `wespeaker-resnet34-cmn-v1` candidate adds only that ONNX metadata entry. The complete original 26,534,365-byte file remains a byte-identical prefix; graph and neural parameter bytes are unchanged. Appending 39 bytes gives 26,534,404 bytes, SHA-256 `6928c40c3b5c695352998c261cb6f2250c4e52a991e324c0f63c2e10fe7c31f8`. This derived container is not covered by the publisher's original-container checksum; the source is verified against that checksum, and the exact deterministic transformation is independently recorded in `cmn-derivation.json`. Its identity/version and notices remain distinct from both earlier profiles. The subsequent validated calibration-only run is recorded below. Original app defaults and all frozen corpus rows stay unchanged; held-out is still unscored.

The independent read-only peer audit confirms serialized GraphProto SHA-256 `ae4056790ba6eb3fa2207d828e63bacb071424b9a0c918d3ec8d093ceff84855` and locates later mean/subtraction operations only in statistics pooling and embedding-space centering. CMN is a concrete missing operation, not a performance-driven speculative filter. Three native/reference differences remain: Povey versus Hamming window, `snip_edges=false` versus reference default true, and high cutoff 7,600 versus 8,000 Hz at 16 kHz. Therefore the CMN candidate restores this one expected operation but does not claim full upstream feature parity. It keeps the existing native settings in its separately versioned calibration contract.

The metadata-corrected raw contract is `1f1d6fbd14574251738805424bcd3f94c66c41f08ab23ef72d3d83e89fe32ba0`; unavailable-calibration version is `f08d4bc75e641ecd7a8a42f55acc10df42a1e8e38970679e9b4d8a5caf6f4ff9`. The new pre-score input `voice-eval-wespeaker-cmn-input.json` has SHA-256 `a8a50d56872c9df5933093e12b004cefb0a3d8da8eaa6a2c011a328a54950e5c`. Only the explicit extractor profile, contract hash and model directory change from the initial ERes2Net input. The pre-score journal is `wespeaker-cmn-pre-score-comparison.json`; no candidate identity score preceded its creation.

## CMN correction validation

The repository preparation tool reproduces the separately prepared official-source derivative byte for byte and preserves the original full-file prefix exactly. All six network-free script cases pass: exact append, no overwrite/source preservation, wrong source/derived pins, size/symlink guards, bounded growth and inode-owned failure cleanup. Four ordinary profile tests and strict Clippy pass; both original profile golden identities remain unchanged. The actual CMN model constructor passes one explicit opt-in case in 0.88 seconds, verifying model/notices/full digest and native dimension 256 without extracting an identity embedding. Final integrated checks pass 392 ordinary Rust tests across 23 binaries, zero failures, 29 intentional opt-in/helper tests ignored, strict all-target/all-feature Clippy and formatting. All six preparation tests pass. The final Opus follow-up found no blocking code, transformation or provenance defects and independently decoded the fixed protobuf suffix. The first CMN calibration-only run has completed below the recognition target; its result is recorded below.

The CMN run began at 12:49:31 UTC with explicit calibration-only mode, executable SHA-256 `153b55f0b8c50da49fcc96252ec117418329670f00c15aaa1c282235b974b69d`. Its process/log records are `wespeaker-cmn-calibration-v1-process.json` and `wespeaker-cmn-calibration-v1.log`; output is `wespeaker-resnet34-cmn-calibration-run-v1`, all under the external `ami-room-v3` cache. Installed app metadata again showed zero meetings/active processing immediately before launch. The review notes remaining feature differences and procedural cross-run held-out discipline; neither becomes a production-acceptance claim.

## Corrected native result

The CMN run completed successfully at 12:52:44 UTC in 193.12 seconds wall time (191.81 seconds inside the runner). Report SHA-256 is `9042188a218e8f2132de3494ef23d91a8936cf9809a776b7a4c123d2db94f5ad`. Its final marker is `COMPLETE`, with no incomplete/error marker. Both provenance-file verification flags and actual native inference are true. The chosen policy is score 0.65 / margin 0.04. Held-out is null/empty and there is no frozen held-out policy file.

| Fixed fifty-person calibration | Original ERes2Net | Ordinary WeSpeaker | WeSpeaker with required CMN |
|---|---:|---:|---:|
| Correct known / 150 | 113 (75.33%) | 13 (8.67%) | 130 (86.67%) |
| Wrong known / 150 | 1 | 0 | 3 |
| Known abstentions / 150 | 36 | 137 | 17 |
| False unknown / 42 | 0 | 0 | 0 |
| Unthresholded top-one known / 150, diagnostic only | 141 | 71 | 141 |
| Held-out identity scores | 0 | 0 | 0 |

An independent Python recount confirms 130 correct, 3 wrong-known, 17 known abstentions and 0 false-unknown, including the selected tie-break and maximum feasible fixed-grid count. All three reports have identical calibration query IDs, fixed truth and fitting-algorithm identifiers. Restoring the missing operation materially improves separation, but 130/150 remains below the required 135/150 (90%). Known-wrong is exactly 2% observed; total false identification is 3/192 (1.56%). These are calibration/model-selection results, not held-out or production acceptance. Repeated subjects/projects remain correlated. No threshold or test-set row was relaxed to pass.

The repository keeps [compact aggregate evidence](../../features/quality/evaluation/measurements/2026-11-07-open-set-calibration.json) without raw vectors, audio, trial-level identities or names; SHA-256 `06d4210eec7af34e88fb32354f3deb59f583143e3b19397ec4dbb08efa641224`. Original detailed reports/caches remain outside the repository. The installed app's model defaults, enrollment versions and automatic matching availability are unchanged. No release pin is generated; the full goal remains open.

## Next bounded technical question

The pinned Rust/public C extractor API exposes neither the remaining frontend settings nor feature-frame input. Adding arbitrary metadata keys would have no effect, and generic stream options are not read by the feature path. A read-only local/source audit found that the underlying C++ API supports a custom `OnlineStream(FeatureExtractorConfig)` accepted by `SpeakerEmbeddingExtractor::Compute`, with symbols present in cached native static libraries. Exact matching C++ headers are not in that prebuilt bundle. The route taken is the version-pinned adapter in `crates/vt-speaker-frontend`, with an isolated FFI boundary. `high_freq = 0` uses the Kaldi convention (offset from Nyquist), so at 16 kHz it is 8 kHz. Trying another model before measuring that frontend would have obscured the cause. The held-out set stays reserved.

## Reference frontend

`wespeaker-resnet34-reference-frontend-v1` keeps the CMN ONNX bytes (`6928c40c3b5c695352998c261cb6f2250c4e52a991e324c0f63c2e10fe7c31f8`, 26,534,404 bytes) and the same metadata transformation. Its raw contract is `27d8ed7041366fde7527d485bc6e5b2970f9b500a3cbd8f244d8f2d0e0c3c7bd`. Unavailable-calibration version is `0aba11d81ffddc6845c8f9bcc376c1c55f17168c479e6b6e7c49ce344ef9e8ec`. `VoiceEmbedder::new_with_profile` refuses this profile, so the sherpa default stream cannot be scored under the new contract. `xtask` builds `ReferenceEmbedder` only after pack verification. The app default remains ERes2Net. Automatic matching stays off.

Before any identity score, raw log-mel frames were compared with `torchaudio.compliance.kaldi.fbank` 2.11.0 using the WeSpeaker ONNX recipe: Hamming, snipped edges, 80 bins, 25/10 ms, dither 0, low 20 Hz, high 0, mean subtraction off. The pass limit was fixed at a maximum absolute error of 0.001. Four synthetic fixtures and one frozen calibration window (WAV SHA-256 `71284119af7620ccb34afa9c5339082e033329f286c05d91d96bda91b721f2c2`, source frames 895680–975680) all matched in frame and bin counts. The largest absolute error was 0.00033665 on the two-second noise fixture. The window error was 0.00031137. Detail is in [the feature record](../../features/quality/evaluation/measurements/2026-11-07-reference-frontend-features.json).

The pre-score input `voice-eval-wespeaker-reference-frontend-input.json` has SHA-256 `6531838462582710277417dfc121feef712d794f1ff9d5b1145509e23900b50c`. The journal `wespeaker-reference-frontend-pre-score-comparison.json` was written at 13:47:54 UTC with `candidate_identity_scores_computed` false. Only the profile, contract hash and model directory differ from the CMN input. Query IDs match the CMN report. The partition hash is unchanged.

The calibration-only run started at 13:48:26 UTC and finished at 13:51:40 UTC. Executable SHA-256 is `54fd34daa42ce721499d18e7923fded55a0863f3f00129a05ffc212a60114814`. Output is `wespeaker-resnet34-reference-frontend-calibration-run-v1`. Report SHA-256 is `9d37359b5a5242206106b1f273ada85fef03a3fd7a4754461b29f2c62a1daf6f`. `COMPLETE` exists. `INCOMPLETE` does not. Native inference, annotation verification and partition binding are true. Mode is calibration-only. Held-out is null and its trial list is empty. No frozen held-out policy file was published.

| Fixed fifty-person calibration | CMN native frontend | Reference frontend |
|---|---:|---:|
| Correct known / 150 | 130 (86.67%) | 138 (92.00%) |
| Wrong known / 150 | 3 | 0 |
| Known abstentions / 150 | 17 | 12 |
| False unknown / 42 | 0 | 0 |
| Selected score / margin | 0.65 / 0.04 | 0.62 / 0.01 |
| Held-out identity scores | 0 | 0 |

An independent recount of the stored calibration trials, using the same fixed grid and tie-break, confirms 138 correct, 0 wrong-known, 12 known abstentions and 0 false-unknown, and confirms that 0.62 / 0.01 is the grid maximum. 138/150 meets the 90% calibration recognition target (135/150) with both error rates at 0. This is a calibration-only pass for this research candidate. It is not held-out acceptance, not a release pin, and not authority to enable automatic matching. Repeated subjects and projects remain correlated. The installed app defaults are unchanged.
