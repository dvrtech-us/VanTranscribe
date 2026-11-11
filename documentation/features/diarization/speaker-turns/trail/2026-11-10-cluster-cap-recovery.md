# The cluster cap merges, evicts or leaves speech Unknown instead of failing

- Date: 2026-11-10
- Feature: speaker-turns
- Related code: crates/vt-diarize/src/cluster.rs (`Clusters::make_room`, `cap_merge`, `cap_evict`, `resolve`), crates/vt-diarize/src/engine.rs (window reconciliation), crates/vt-diarize/src/finalize.rs (`CAP_WARNING`), crates/vt-diarize/src/diagnostics.rs, crates/vt-diarize/src/tests.rs, crates/vt-session/src/real_call_replay_tests.rs. Commit 4fdc69b.

## Context

The [ambiguous-voice entry](2026-11-10-ambiguous-voice-stays-unattributed.md) stated that the 64-cluster limit still returned `DiarizeError::Capacity(CapacityLimit::Clusters)` and was still reached on a 95-minute single-microphone in-person call; refinement then kept the previous run's speaker labels. This entry changes that behaviour. The earlier entry is unchanged and describes the code of its time.

## Decisions

At the cap, when a window needs a new cluster, `make_room` runs in order: (1) `cap_merge(false, 56)`, merging duplicate pairs with centroid cosine at least `MERGE` (0.70) where at least one cluster is provisional (under 3,000 ms, `ESTABLISHED_MS`) and the pair is not cannot-linked, until `CAP_HEADROOM` = 8 slots are free; (2) `cap_evict`, removing the oldest unanchored single-member cluster under 3,000 ms; (3) `cap_merge(true, 63)`, merging the single most similar pair of established clusters; then, if any merge happened, re-scoring the window against the remaining clusters (0.55 / 0.10) and joining a clear winner (`cap_reassigned`); then, with no free slot, `Capacity(Clusters)` for a constraint endpoint only, otherwise the window is left unattributed (`cap_unattributed`). The larger cluster absorbs the other; ties keep the lowest indices so replay is deterministic. Merged and evicted ids are kept in a `retired` map; turns built in the same window are rewritten through `resolve`, earlier published turns are reconciled by `finish`. `CAP_WARNING` is added to the result warnings when merges, evictions or unattributed windows occurred, and the diagnostics gained `cap_merges`, `cap_evictions`, `cap_evicted_ms`, `cap_unattributed`, `cap_reassigned` and `cap_reached()`. The ignored replay test now also prints whether the warning was raised.

## Alternatives Considered

- Keep returning `Capacity(Clusters)` at the cap (the previous behaviour; refinement kept the previous run's labels). The commit message records the choice to merge, evict or leave Unknown instead; no other alternative is recorded.

## Consequences

- Positive: an ordinary window never fails the speaker pass at the cap; constraint errors are preserved so saved corrections are not silently dropped; the effect is visible through the warning and counters.
- Negative: similar voices can be merged and evicted speech shown as Unknown; the warning says so. Live turns published before a merge keep their id until the final result.
- Not verified: no replay measurement of this change on the real recordings is recorded in the repository (the measurement copies were lost in a machine restart), and label quality on single-microphone in-person audio is unvalidated. Coverage is synthetic tests only.
