# Diarizer diagnostics and the measured cause of the cluster-capacity failure

- Date: 2026-11-10
- Feature: speaker-turns
- Related code: crates/vt-diarize/src/diagnostics.rs, crates/vt-diarize/src/engine.rs (`Diarizer::diagnostics`), crates/vt-session/src/real_call_replay_tests.rs

## Context

After the 2026-11-09 live call, speaker separation reached a capacity limit and the refinement failed. The error did not say which limit fired or why clusters multiplied.

## Decisions

- `Diarizer::diagnostics()` returns fixed-size counters, available after an error: windows, turns, embeddings, which limit fired, track and approximate input time, and per track the assignments to existing clusters, clusters opened by reason (no candidate, best below the assign threshold, margin failure, all candidates removed by cannot-link), peak and singleton clusters, and score/margin histograms. No audio, embeddings or text are stored. Clustering behaviour is unchanged.
- `DiarizeError::Capacity` carries a `CapacityLimit` (constraints, turns, embeddings, clusters).
- An ignored replay test (`VT_REPLAY_ROOT`, `VT_REPLAY_MEETING`; must point at a copy of an app data directory) prints only these counters.

## Measurement

Replay of the 527.6 s call from a copy, echo cancellation on, system track (the microphone is not clustered in remote mode):

| Phase | Cluster limit (64) reached at | Embeddings | Assigned to existing | Opened | Opened by reason | Singleton clusters |
| --- | --- | ---: | ---: | ---: | --- | ---: |
| Refined | about 424.9 s | 160 | 96 | 64 | 1 no candidate, 3 below assign, 60 margin failure | 57 |
| Live | about 347.4 s | 134 | 70 | 64 | 1 no candidate, 5 below assign, 58 margin failure | 49 |

Of the 60 refined margin-failure opens, 35 had a best score of at least 0.9.

Interpretation: near-duplicate clusters make later embeddings ambiguous under the 0.10 second-best margin rule, which opens further clusters. The cause is not low similarity to existing voices.

Caveats: the replay omitted saved user constraints and the live speaker prior (they only affect finish, which was never reached). Capacity time is approximate to about 2 s.

## Alternatives Considered

- Change the margin rule now. Deferred; it needs its own decision and evaluation.

## Consequences

- Positive: the limit, track and reason for cluster growth are observable without exporting meeting content.
- Negative: the clustering rule is not changed yet. The margin-rule change was made afterwards; see [An ambiguous high-similarity window stays unattributed](2026-11-10-ambiguous-voice-stays-unattributed.md). This entry remains the record of the measurement before that change. Long calls can still reach the cluster limit (refinement continues, see the batch-refinement trail).
