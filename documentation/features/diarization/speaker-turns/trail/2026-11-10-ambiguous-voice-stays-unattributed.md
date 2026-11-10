# An ambiguous high-similarity window stays unattributed instead of opening a cluster

- Date: 2026-11-10
- Feature: speaker-turns
- Related code: crates/vt-diarize/src/cluster.rs (`Clusters::assign`), crates/vt-diarize/src/engine.rs (clean-turn emission), crates/vt-diarize/src/finalize.rs (`replay`), crates/vt-diarize/src/diagnostics.rs (`TrackDiagnostics`), crates/vt-diarize/src/tests.rs

## Context

The [clustering capacity measurement](2026-11-10-clustering-capacity-measurement.md) showed that duplicate clusters of one voice drove the 64-cluster limit. On the 2026-11-09 remote call (system track, Refined) there were 61 margin failures (best score at least ASSIGN 0.55 but less than MARGIN 0.10 ahead of the runner-up). In 54 of them the best score was at least MERGE (0.70), and the centroid cosine between the two top candidates was at least 0.70 in 56 of 61. Each new singleton opened on such a window made the next window of the same voice ambiguous again.

## Decisions

In `Clusters::assign`, when the best candidate scores at least ASSIGN but is not ahead of the second by MARGIN:

- If best is at least MERGE (0.70): the turn is left unattributed. No cluster is opened, and the embedding is not added to any cluster (no exemplar, member or speech time). `assign` returns `Ok(None)`.
- If best is below MERGE: a new cluster opens, as before.
- A turn that is a must-link or cannot-link endpoint (present in the constraint graph) keeps the previous behaviour and opens a cluster, so saved corrections are not pushed into review.

Unchanged: the thresholds (0.55 / 0.70 / 0.10); best below ASSIGN and no-candidate still open a cluster; `MAX_CLUSTERS` (64) still returns `DiarizeError::Capacity(CapacityLimit::Clusters)`.

Surfacing: for `None` the engine emits the turn with no speaker ids, no score, label "Unknown", `uncertain = true` and `provisional = false`. The embedding record is kept, so `finalize::replay` runs `assign` again over all records and decides again with the full set of clusters; a turn that is still unattributed has no cluster, so `finish` emits it as Unknown/uncertain with no speaker id and no score.

Diagnostics added to `TrackDiagnostics`: `margin_failures`, `margin_failure_best_at_least_merge`, `margin_failure_centroid_hist` (centroid cosine of the two top candidates), `ambiguous_unattributed`.

Tests changed on purpose: `second_best_margin_does_not_silently_assign_ambiguous_voice` now covers both sides of MERGE (best about 0.707 is left unattributed; best about 0.65 opens a provisional cluster); the `agglomeration_uses_silhouette_without_erasing_all_speaker_separation` fixture was rebuilt because its old ambiguous window is now unattributed. New tests cover the margin-failure diagnostics and the rule.

## Result (offline replay of a copy of the 2026-11-09 remote call, counters only, system track)

| Phase | Cap reached | Opened (no candidate / below ASSIGN / margin) | Unattributed embeddings | Singletons | Final speakers (confident) | Merges | Attributed vs Unknown time |
| --- | --- | --- | ---: | ---: | --- | ---: | --- |
| Refined | No | 1 / 7 / 10 | 14 | 14 | 17 (3) | 1 | 425,069 ms vs 68,273 ms (about 14% Unknown) |
| Live | No | 1 / 7 / 23 | 42 | 24 | 31 (7) | 0 | 333,757 ms vs 160,319 ms (about 32% Unknown) |

## Known limits

- On the 2026-11-10 95-minute in-person call (single room microphone, mic track clustered) the cap is still reached with this rule. Refined: at about 2617 s after 654 embeddings (332 unattributed, 257 assigned, 64 opened = 1 no-candidate + 20 below ASSIGN + 43 margin failures with best in [0.5, 0.7)). Live: at about 902 s after 270 embeddings (163 unattributed, 42 assigned, 64 opened = 1 + 10 + 53). Capacity handling during refinement (previous labels kept, warning) therefore still applies.
- Live labelling on the remote call is still implausible (7 confident speakers).
- The embedder is still the zh-cn ERes2Net with uncalibrated thresholds.
- Validation is these two recordings plus synthetic tests. No AMI diarization run was made (the data is not on this machine), so no DER claim is made.

## Alternatives Considered

- Assign to the best cluster when the rivals are near-duplicates: rejected in review. A cluster lying between two similar voices would absorb ambiguous windows and could collapse two speakers, and it would publish ambiguous windows as confident labels.
- Mark Unknown only when the cap is reached: treats the symptom, not the cluster growth.
- Online merging of clusters: not done.
- Changing the thresholds: not done; they remain uncalibrated placeholders.

## Consequences

- Positive: duplicate-cluster growth from ambiguous windows stops; the remote call no longer reaches the cap; ambiguous windows are shown as Unknown/uncertain rather than as a confident or invented speaker; finalize reconsiders them.
- Negative: more Unknown time (about 14% refined, 32% live on the remote call); long in-person single-microphone calls still reach the cap; live labels remain unreliable; the rule is validated on very little data.
