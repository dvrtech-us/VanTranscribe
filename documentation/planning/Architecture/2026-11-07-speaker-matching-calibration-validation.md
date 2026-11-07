# Open-set speaker matching and public-corpus evidence

Date: 2026-11-07. Continues the accepted full M0–M5 design and [consented memory checkpoint](2026-11-07-consented-voice-memory-validation.md). The accepted Anthropic/Codex design reviews remain the basis; no repeat preimplementation review is needed. The previous turn installed the reviewed enrollment/People increment and passed 364 ordinary Rust tests and 71 browser flows. This increment adds matching/evaluation infrastructure and runtime evidence; it does not redefine the full goal around those parts.

## Implemented contracts

The pure matcher ranks distinct people by the normalized centroid of at most ten compatible exemplars per person, compares only exact version/channel material and excludes stale/shared-room entries. It never serializes vectors or reads a key, recording or provider. Automatic outcomes require a bound reviewed release calibration, covered gallery size, threshold, a distinct-person runner-up margin and at least fifteen seconds of clean speech. A singleton supplies no fabricated runner-up; it can only suggest within validated coverage. Threshold coverage is bounded and stepped, with a strictly stronger automatic threshold above fifty people; missing profiles/coverage fail closed. No runtime profile or release pin is shipped, and application `matching_available` stays false.

A raw extraction-contract hash now excludes the semantic calibration version. The existing uncalibrated enrollment version is unchanged (`64e49963b30d2f237d53cfd2dabc5cafbd7eaed9eb2ed52bc1e63dd757c2d62e`); its regression passes. The raw hash is `d0e9574d06f367f35d6ad1ab4e8b2a6c13c2e586ed56ff159c95f1ceb6925928`. A future identification version combines that raw contract with a semantic calibration ID; the reviewed calibration artifact is independently pinned by content hash, avoiding circular self-hashing. Existing prints cannot silently acquire a new calibrated identity.

## Evidence and scope

The [corpus protocol](../Quality/2026-11-07-open-set-speaker-corpus.md) freezes subject/project-disjoint calibration and held-out partitions before any identification scores. ES2002, which informed earlier diarization development, belongs only to calibration. Each partition has a fixed fifty-person gallery, unknown identities and different meeting recordings for enrollment/probes. Most unknown identities share project teammates with the gallery, making unknown-person rejection a real comparison rather than merely separating recording contexts. These are staged same-project sessions and do not establish unrelated real-call, in-house or remote-codec acceptance.

Acquisition uses bounded, SHA-recorded HTTP ranges from the official public corpus, strict response geometry/ETag checks and no full-file fallback. Native embeddings stayed in memory. The research command fit on calibration only, reported all-known recognition including abstentions and separate known/unknown false identification, and emitted no runtime pin or production-eligible artifact. The final implementation review found no blockers. Calibration missed its frozen recognition gate, so held-out inference did not run.

The [Keychain checkpoint](2026-11-07-private-keychain-validation.md) records an actual signed production-provider temporary-key roundtrip, duplicate refusal, second-process read, root isolation and exact removal, without changing default/search-list configuration or using a real voiceprint. A separately owned private Keychain was also physically locked: noninteractive reads and duplicate adds failed closed, unlocking restored the same key, and exact cleanup completed. Installed-app upgrade ACL behavior remains unproven.

## Frozen native-ready inputs and checks

The availability audit completed before identity scoring. Whole ES2010, IS1003 and IS1007 projects were excluded for three unavailable files and one incompatible stereo source; no channel replacement/downmix or score-based filtering occurred. V3 keeps the original phase assignments and person-ranking rule, with fifty gallery people and fourteen identity-unknown people per phase. Calibration has 150 known/42 unknown probes; held out has 145 known/42 unknown. The 479 samples comprise 2,788 canonical windows. All are nonzero and nonconstant, no origin frames overlap, no waveform duplicates occur across distinct origins, and twenty ES2002a control windows match a separately pinned complete source byte for byte.

The final partition SHA is `90b40f63eea00a794e8cb0e3805e24ec211100a18824a42191882d23d9ac8454`. Template SHA is `7381f9506eb5a98def3f3f6715f64038e511029b0f66cc7f2a024006f3e8e954`. Filling only the verified existing model-directory path yields `voice-eval-input.json`, SHA `b94125faf45b8cbc72bf8afdce2680b3d282a57723a04954a648572d82c39339`. All files remain under the external public-corpus cache, not installed application data.

Final integrated gates pass 385 ordinary Rust tests across 23 binaries with zero failures and 27 intentional opt-in/child cases ignored, strict all-target/all-feature Clippy and formatting. Ten research cases include fixed truth, source/phase leakage, stronger within-phase cross-project probes, calibration-only fitting, feature gating and a failed-publication counterexample. The publication case first reproduced a false COMPLETE marker, then passed after correcting the final marker order and failure cleanup. Twelve network-free Python checks pass. The feature-gated release CLI builds and prints the pre-pinned raw and unchanged legacy hashes. UI code is unchanged from its last 71-case passing gate. The final higher-model implementation review found no blockers. After ten focused research checks, strict Clippy, formatting and twelve Python checks passed again for the narrow hardening changes, the actual native run began at 12:09:43 UTC using the frozen input and completed successfully at 12:12:08 UTC with an unavailable calibration result.

## Final implementation review

Opus returned no blocking implementation defects. Its statistical-resolution finding is recorded in the corpus protocol: even a perfect held-out result cannot establish the 2% error criteria with these denominators. Before native execution, the root independently verified that both optional provenance paths are present and their actual files match the frozen hashes. Reports must confirm both verification booleans before results are cited. Three narrow hardening changes follow the review: synchronize the frozen-policy directory entry before held-out inference, constrain each preparation candidate row to selected projects, and reject unequal export/partition counts. They change no frozen input or fitting algorithm. Per-invocation one-shot enforcement does not prevent a separate new output invocation from reusing held-out data; exactly one native identity-scoring invocation has now completed, and it used calibration only.

The single native run is `ami-room-v3/eres2net-research-run-v1`, with authoritative process/result metadata in `native-run-v1-process.json` and progress in `native-run-v1.log`. Executable SHA-256 is `e58f14f9b2b484338564c4bae401ac9cc5d01a069de240c6a3a43ef74a9c2a03`. The requested niceness increase was denied by macOS; the run continued at actual niceness 0 and was not restarted. At 21 seconds, it used 213,216 KiB RSS and about one CPU core; this isolated observation is not a hardware acceptance result. Installed application metadata showed zero meetings/active processing before launch.

## Actual native result

The single run completed with exit 0 in 144.63 seconds wall time (143.13 seconds inside the validated runner). Its report SHA-256 is `ed567ad6d624c9e42f72b0095748c6a8b6c4d416bf2262b5b7619af6ffd92a12`. `COMPLETE` exists; `INCOMPLETE` and `error.json` do not. Native inference, annotation-file verification and exact frozen-partition/export binding are all true. The native constructor verified the full pinned model pack; all 2,788 input waveform hashes and geometry were verified before scoring. No source, partition, input or score-policy algorithm was changed after scores began.

| Calibration quantity | Actual result |
|---|---:|
| Gallery identities | 50 |
| Correct known recognition, all-known denominator | 113 / 150 = 75.33% |
| Wrong known identity | 1 / 150 = 0.67% |
| Known abstentions | 36 / 150 = 24.00% |
| Unknown false identification | 0 / 42 = 0% observed |
| Total false identification | 1 / 192 = 0.52% |
| Fixed-grid candidate score / margin | 0.53 / 0.13 |
| Held-out trials extracted or scored | 0 |

The frozen grid's maximum feasible known recognition is below 90%, so status is `unavailable_calibration_recognition_below_target`. `held_out` is null, its trials array is empty, and no frozen candidate policy for a held-out pass was published. All production-eligibility/runtime-pin flags remain false. The held-out set remains unused for model selection and threshold tuning.

An independent Python recount of the scalar calibration trials confirms the counts, selected tie-break and maximum feasible grid result. Unthresholded top-one known ranking is 141/150; this is a diagnostic only, not automatic recognition. Thirty-six known probes miss the margin, and four miss the score threshold (sets can overlap). The error/rejection separation, rather than simply returning a nearest identity, must improve before a usable calibration can be released. Calibration Wilson 95% descriptive intervals are 67.86–81.54% recognition, 0.12–3.68% known-wrong and 0–8.38% unknown-false. Repeated people/projects are correlated; these are not acceptance confidence bounds.

The independently recorded result is `native-run-v1-independent-recount.json` in the external corpus cache. No private recording, enrolled person, runtime key, model preference or installed app data was changed. The installed enrollment/People increment remains available; automatic matching remains unavailable. The full M0–M5 objective remains open. Future extractor comparisons must use calibration only, preserve this failed result, adopt an explicit new extraction/version contract, and leave held-out data unused until a fixed candidate passes calibration.

The subsequent [English-model comparison and preprocessing correction](2026-11-07-english-speaker-calibration-comparison.md#corrected-native-result) improves the best calibration result to 130/150 (86.67%) while retaining this failed original and leaving all held-out identity scores unused. It remains below the gate; the original app defaults and automatic-matching unavailability are unchanged.
