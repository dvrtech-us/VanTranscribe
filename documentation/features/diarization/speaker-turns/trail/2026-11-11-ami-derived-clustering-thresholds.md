# Clustering thresholds derived from a public AMI calibration set

- Date: 2026-11-11
- Feature: speaker-turns
- Related code: crates/vt-diarize/src/models.rs (`ASSIGN`, `MERGE`, `MARGIN`, `EMBEDDING_VERSION`, `calibration_profile`, `verify`, `MODEL_NOTICE`, `manifest_tests`), crates/vt-diarize/src/finalize.rs (first run warning), crates/vt-diarize/src/diagnostics.rs (margin bucket comments), crates/vt-diarize/src/tests.rs.

## Context

`ASSIGN` 0.55, `MERGE` 0.70 and `MARGIN` 0.10 were unvalidated placeholders for the ERes2Net pack `pyannote3-eres2net-2024-v1`. On real calls they over-split speakers, left much speech Unknown and drove long recordings into the 64-cluster cap ([cluster-cap entry](2026-11-10-cluster-cap-recovery.md)). An evaluation on the AMI Meeting Corpus (CC BY 4.0) was run outside the repository (data, scripts and result tables in `/Volumes/dev/vantranscribe-eval`: `README.txt`, `split.json`, `threshold_rule.txt`, `frozen_thresholds.json`, `tables/product_4fdc69b.txt`).

Method, fixed before any score on the new meetings was computed:

- Split by AMI project so no person is in both sets: calibration ES2002a, ES2005d, IS1004a, IS1008c, TS3005a, TS3011a; held-out ES2006a, ES2012a, ES2016d, IS1001c, IS1006d, TS3004a, TS3008a (24 vs 28 people, none shared; `scripts/check_split.py`).
- Rule (`threshold_rule.txt`), applied to pooled calibration slices only, per slice of true speaker s, with k = 5-slice centroids from the same meeting and rounding to 0.05: `ASSIGN = r_up(max(P95(wrong_k), (P95(wrong_k) + P5(own_k)) / 2))`; `MARGIN = clamp(r_dn(P5(gap_k)), 0.05, 0.10)`; `MERGE = max(ASSIGN + 0.05, r_up(max(P95(diff_c), (P95(diff_c) + P5(same_c)) / 2)))`.
- Result for ERes2Net, frozen in `frozen_thresholds.json`: 0.50 / 0.60 / 0.05. Only then was the held-out set scored, through the product `Diarizer` (in-person mode, Array1-01 room microphone), Refined and Live.

Held-out results at commit 4fdc69b (DER = missed + false alarm + confusion over reference speech, no collar; Unknown, provisional and uncertain speech counts as missed):

| Held-out set | Phase | 0.55 / 0.70 / 0.10 | 0.50 / 0.60 / 0.05 |
| --- | --- | --- | --- |
| 7 full meetings | Refined | 53.0% | 41.3% |
| 7 full meetings | Live | 58.2% | 46.1% |
| 30 x 300 s windows | Refined | 43.7% | 38.8% |
| 30 x 300 s windows | Live | 50.4% | 43.1% |

With the new thresholds the cluster cap fired on 1 of 7 full meetings in Refined (39 evictions) and 2 of 7 in Live (62 evictions), against 5 of 7 (290 evictions) and 6 of 7 (424) with the old ones. Unknown share of emitted turn time fell from 42.5% to 31.0% (Refined) and 53.1% to 35.8% (Live).

## Decisions

- Ship `ASSIGN` 0.50, `MERGE` 0.60, `MARGIN` 0.05 for the current ERes2Net embedder. No embedder change.
- `EMBEDDING_VERSION` keeps its derivation (it hashes the thresholds) and therefore changes from `fb58e351…` to `ff29acb3…`, so a run's stored version still says which clustering profile produced it.
- Model-pack identity no longer includes the clustering profile. `models::verify` previously compared the manifest's `embedding_version` and `calibration` strings; a threshold change would have made every installed pack fail verification, show as not installed, and (because `install` refuses to replace a pack that fails verification) need manual removal and a 46 MB re-download. `verify` now compares only id, weight hashes, notices and weights. The two strings are still written on install as a record and are not rewritten afterwards, so an existing manifest keeps the old profile text. `models::manifest_tests` covers an old-manifest pack (verifies) and a wrong weight hash (rejected).
- Wording: `calibration_profile()` is now "experimental_ami_rule_v1; …; derived from 6 public AMI in-person meetings by a fixed rule, checked on 7 held-out meetings; not validated for remote calls or production; NOT identification confidence". The first run warning is now "Experimental speaker clustering; labels are not verified identities. Thresholds were tuned on a small public set of in-person English meetings only; accuracy on remote calls, overlap attribution and long-call acceptance remain unvalidated." `MODEL_NOTICE` says the same. The settings attribution text in `ui/src/lib/AudioSettings.svelte` ("English and meeting-channel thresholds are not calibrated") was outside this change and still reads that way.
- Synthetic tests were re-derived relative to the constants with asserted preconditions (details in Consequences).

## Alternatives Considered

- Keep 0.55 / 0.70 / 0.10: no evidence for them; worse on every held-out measure above.
- Switch the embedder now to English WeSpeaker ResNet34 with its own rule-derived thresholds (0.45 / 0.60 / 0.05): on the same held-out set it scored 37.2% Refined / 39.1% Live (full meetings). Undecided; it needs a new pack, download, versions and its own review, so it is not part of this change.
- Separate thresholds for remote and in-person meetings: AMI has no remote-call audio, so there is nothing to derive remote values from yet.
- Keep `EMBEDDING_VERSION` unchanged so old runs stay "current": rejected; it would label runs produced under different thresholds with the same version.

## Consequences

- Positive: the held-out DER, cap firing and Unknown share all improve; installed packs and stored voiceprints (`voice_embedding_version()`, which excludes thresholds) are unaffected.
- Limits, stated plainly: most of the remaining error is Unknown or provisional speech counted as missed. Even with the new thresholds the system over-splits (5–13 confident speakers in Refined and 6–24 in Live where each meeting has 4) and leaves roughly a third of emitted speech Unknown. AMI is scripted English meetings in three rooms recorded on one room microphone; there is no remote-call audio. Seven held-out meetings is a small set with a large spread (Refined 16.6%–55.4%). The thresholds are not production-accepted.
- Existing refined runs were produced with the old thresholds and keep the old `embedding_version`. "Remember voice" requires the current version, so it asks the user to refine such a meeting again first. Re-refining changes speaker labels: with a lower `MERGE`, voices with centroid cosine between 0.60 and 0.70 are now merge candidates in refinement and in the last-resort cap merge, so two distinct but similar voices that were kept apart before may now be merged. With a 0.05 margin, windows close to two voices are attributed to the nearer one more often instead of shown as Unknown, and a cluster can drift toward such windows.
- Tests whose fixtures changed: the ambiguity tests use `equidistant(...)` windows at `(MERGE + 1/sqrt 2) / 2` and `(ASSIGN + MERGE) / 2` (the old 0.65 window is now above `MERGE`); the silhouette test and `drifted_pair` derive their angles from `ASSIGN` and `MERGE` (the old 60 degree "far" window sat exactly on the new `ASSIGN`); the two-voice midpoint noise is `1.5 * MARGIN` (with the old 0.15 noise nearly every midpoint is now attributed to one voice, though the call still ends with two speakers and no cap); the close-voices test uses voices at `MERGE - 0.05` (0.65 is now above `MERGE`); the four-voice cap test aims midpoints at `ASSIGN - 0.02` (with the old noise it no longer reaches the cap and ends with exactly four confident speakers).
- Harness check of the shipped defaults: see the section below.

## Harness check of the shipped defaults

The evaluation patch `patches/test-a2-on-4fdc69b.patch` (experimental xtask `test_a.rs` plus `VT_EXP_*` threshold overrides) was applied to a throwaway copy of this change (`/Volumes/dev/vantranscribe-eval/tmp-verify`: committed tree at f08e44f with this change's `crates/vt-diarize/src` on top), not to the repository. `VT_A2_TAG=verify-newdefaults VT_A2_CONFIGS="eres_cur eres_frz" scripts/run_product.sh` scored the full held-out set with no threshold overrides (`eres_cur`, now the shipped defaults) and with explicit 0.50 / 0.60 / 0.05 overrides (`eres_frz`), using the evaluation's copy of the installed pack, whose manifest still carries the pre-change `embedding_version` and `calibration` strings. Results (`tables/product_verify-newdefaults.txt`): full meetings Refined 41.3%, Live 46.1%; 300 s windows Refined 38.8%, Live 43.1%; cap fired on 1/7 (39 evictions) and 2/7 (62 evictions). These equal the earlier "ERes2Net frozen" figures. The score, diagnostics and finish fields of all 74 default-path results are identical to the override-path results. The old-manifest pack verified and loaded.
