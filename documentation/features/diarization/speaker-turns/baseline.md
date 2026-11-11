# Speaker turns baseline

## Invariants

- Turns are derived from independent track audio segmentation, never one embedding per STT final.
- Live source context remains five seconds; refined source context uses the canonical ten seconds. Both retain a 2.5-second normal ownership stride, with timestamps clipped to actual source and ownership bounds.
- Source-window metadata describes the configured maximum. Short final/gap/format tails are native-padded and own all remaining actual frames, which may exceed the normal stride; metadata never claims every turn had ten real seconds.
- Actual engine results include explicit phase-derived segmentation context; legacy/missing and synthetic context remains None. Retaining annotations copies original context without guessing from run kind.
- Per-track source buffers remain bounded; no whole-meeting audio concatenation occurs.
- Emitted turns below one second receive Unknown and no speaker ID; they never inherit a preceding voice.
- Overlap never updates a clean speaker exemplar. Two labels are retained only when supported by clean same-window speaker links; otherwise overlap remains explicit.
- Remote microphone Me is a channel prior, not voice recognition. A microphone cannot-link present at construction overrides it with actual embeddings/clustering. A late correction without microphone embeddings produces Unknown regions and a pending review; system clusters never receive the prior. In-person microphone voices are clustered.
- A margin failure (best at least 0.55 but not 0.10 ahead of the runner-up) with best at least 0.70 leaves the turn unattributed: no cluster is opened, the embedding joins no cluster, and the turn is emitted Unknown, uncertain, not provisional, with no speaker id or score. The embedding record is kept so `finish` replays the decision. With best below 0.70 a new cluster opens. Best below 0.55 or no candidate opens a cluster. A must-link or cannot-link endpoint always opens a cluster instead of being left unattributed.
- An ambiguous window is never assigned to an existing cluster and never contributes an exemplar, member or speech time.
- Scores are uncalibrated cosine similarities, not identity probabilities. Result calibrated is false.
- User must-links override similarity and cannot-links block assignments/merges using transitive must-link components.
- Missing, tied, under-covered, overlapping, contradictory or unavailable-evidence constraints produce individual reviews and mask affected connected regions Unknown; they do not abort the full transcript pipeline.
- Defensive replay recovery isolates only the implicated connected constraint component and retries at most the initial candidate count plus one; unrelated same-track and other-track corrections remain active.
- Dependent valid corrections wait under the original failed source review. Their anchors are included, but their valid source UUID is not presented as a removable failure.
- A review retains its exact constraint provenance and source anchors. Masked turns have no speaker IDs or scores and are excluded from final cluster evidence. Unrelated valid corrections continue to apply.
- Persisted constraints use track/time anchors, never serial turn IDs across processing runs.
- Unnamed diarization embeddings exist only in meeting-local memory. Result serialization contains no biometric vectors. Explicit named enrollment recomputes source vectors under the separate consent/encryption feature and never changes ordinary diarization persistence.
- No download, capture permission, cloud processing or cross-meeting memory access occurs during inference construction. The bounded enrollment extractor opens no gallery or secret store.
- Model setup is explicit, checks pinned sizes/hashes, preserves license notices, and never accepts a publisher account agreement.
- A refinement stopped by a turns, embeddings or (constraint-endpoint) clusters capacity limit never saves partial separation output; it carries the previous run's speaker annotations onto the new run and records the limit in the run's speaker warnings.
- A live speaker save that hits lock contention never stops the speaker worker; unsaved turns are retained and dropped turns become `persistence_failed` inference gaps. Engine errors and non-lock database errors still stop it, and the speaker worker is not restarted (only the speech worker recovers in place; see live-english).
- `Diarizer::diagnostics()` holds counters only and does not change clustering. It reports `margin_failures`, `margin_failure_best_at_least_merge`, `margin_failure_centroid_hist`, `ambiguous_unattributed` and the cap counters `cap_merges`, `cap_evictions`, `cap_evicted_ms`, `cap_unattributed`, `cap_reassigned`.
- At 64 clusters on a track a window that needs a new cluster is handled in this order: merge duplicate pairs (centroid cosine at least 0.70, at least one provisional, not cannot-linked) until at most 56 clusters remain; evict the oldest unanchored single-member cluster under 3,000 ms of speech; merge the most similar pair of established clusters (cosine at least 0.70); re-score the window if any merge happened and join a clear winner (0.55 / 0.10); otherwise leave the window unattributed (Unknown, uncertain, no speaker id). Merge ties keep the lowest indices, so replay is deterministic.
- `DiarizeError::Capacity(CapacityLimit::Clusters)` is returned at the cluster cap only for a must-link or cannot-link endpoint that cannot get a cluster (`capacity_refused`). No other turn fails at the cap.
- Merges and evictions are recorded (`record_merge`, `retired`); turns built in the same window follow them (merged id, or Unknown for an evicted id); `mic:me` is never rewritten. Merges made while streaming seed `refine`'s published merge history.
- `finish` adds `CAP_WARNING` ("Speaker separation reached its cluster limit; some similar voices were merged and some speech is shown as Unknown.") to `DiarizationResult.warnings` when any track has `cap_merges`, `cap_evictions` or `cap_unattributed` above zero.
- No replay measurement of cluster-cap handling on real recordings is recorded in the repository; label quality on single-microphone in-person audio is unvalidated.

## Defaults and bounds

| Configuration | Value |
| --- | --- |
| Default mode/phase | remote / refined |
| Inference | CPU, one thread per sequential native session |
| Sample rate / packet input | 16 kHz internal; 8–192 kHz mono, at most two seconds per accept |
| Configured maximum source window | Live 5 seconds / Refined 10 seconds |
| Ownership stride | 2.5 seconds for full windows; flush owns the shorter remaining tail |
| Native pyannote input | 10 seconds; live and short tails padded by sherpa |
| Source FIFO initial capacity | Live112,000 / Refined192,000 mono frames at16kHz, including max2s packet headroom |
| Explicit enrollment extractor | 1–5 seconds contiguous finite mono at 8–192 kHz; fresh resampler per span; 512 dimensions; no identification calibration |
| Result segmentation context | Some(actual phase configuration); legacy missing=None |
| Minimum eligible clean turn / visible cluster | 1000 ms / 3000 ms |
| Experimental assign / merge / margin | 0.55 / 0.70 / 0.10 |
| Speaker state | 64 clusters/track, 20 exemplars/cluster |
| Cluster-cap handling | `CAP_HEADROOM` 8 (merge pass targets 56), `ESTABLISHED_MS` 3000, merge threshold `MERGE` 0.70, re-score with `ASSIGN` 0.55 / `MARGIN` 0.10 |
| Live speaker save | IMMEDIATE transaction; 3 attempts, 300 ms busy timeout, 50/100 ms backoff (1.05 s worst case; shared `LiveSaveBudget::PRODUCTION` with the live transcript writer); at most 2048 unsaved turns carried |
| Replay embeddings / turn metadata | 8192 / 32768 per meeting |
| Constraint count | 4096 pairs and 4096 temporal constraints |
| Temporal matching | unique maximum overlap, at least 50% anchor coverage |
| Discontinuity tolerance | 5 ms |
| Download/connect timeout | 180 seconds / 10 seconds |
| Download cancellation polling | 50 ms |

## Flag rules

| Condition | Severity | Blocks complete publication? |
| --- | --- | --- |
| Missing/corrupt model hash/manifest | Error | Yes |
| Missing model-setup acknowledgement | Error | Setup only |
| Cluster cap reached for an ordinary window | Merge, evict or Unknown; `CAP_WARNING` on the result | No; raw audio and transcript unaffected, some speech Unknown |
| Capacity reached in `Diarizer` (turns, embeddings, constraints, or clusters for a constraint endpoint) | Error from `Diarizer`; never silent truncation | Constraints: yes. Turns/embeddings/clusters during refinement: no; the refined transcript is published, previous speaker labels are retained (or channel/Unknown labels) and a run warning names the limit |
| Constraint conflict/missing/ambiguous anchor | Needs review | No; publish timed transcript with affected voices Unknown and review metadata |
| Ambiguous window (margin failure, best at least 0.70) | Uncertain | No; emitted Unknown with no speaker id, decided again by the `finish` replay |
| Short/overlap/provisional speaker turn | Uncertain | No; explicit unknown/overlap metadata |
| Uncalibrated profile | Experimental | Always visible; no calibrated identity claim |

## Side effects and integration

1. Explicit setup downloads and verifies model assets in private staging, then atomically publishes the pack.
2. Speaker inference consumes supplied worker audio with independent per-track clocks.
3. The result returns turns, speaker metadata, merge history, version and explicit warnings.
4. The caller checks run/constraint revision and atomically publishes immutable run metadata.
5. The caller maps word midpoints and applies supported user identity names/reconciliation without rewriting raw transcript history.

The crate has no database or SQL artifacts. Session storage owns persistent turns/constraints and never persists unnamed embeddings. M2b remains open until calibrated English/channel DER, overlap, reconciliation and long-call performance gates pass.
