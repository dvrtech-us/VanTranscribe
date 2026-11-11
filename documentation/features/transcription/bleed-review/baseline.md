# Bleed Review Baseline

## Invariants

- Text similarity creates visible candidates only; speech is never automatically hidden, deleted or identified biometrically.
- Only explicit user-confirmed applied bleed hides a Mic line from effective consumers. Raw transcript and audio remain intact.
- Every choice appends an immutable audit event. A restore records genuine speech and preserves the earlier event.
- Active completed transcripts and persisted live final lines permit decisions; historical runs do not.
- Decisions anchor to raw Mic track, start/end and SHA-256 original text, unaffected by corrected display text.
- New-run reapplication requires a unique exact match in both directions. Missing or ambiguous bleed anchors remain visible and pending. Unmatched genuine choices do not require another review; future exact matches still suppress duplicate suggestions.
- Each run owns its match snapshot. Changing the active run never rewrites historical annotations.
- A pending dismissal records a retired genuine event, selects no current line, and prevents that lineage recurring in future runs.
- Mutation, revision increment, summary invalidation and search refresh (`refresh_search_after_edit`) are atomic. Search failure rolls back the complete operation. During an active recording the whole-meeting FTS rebuild is deferred (only non-current summary rows are dropped), so a bleed decision made then becomes searchable when the recording stops.
- Existing applied bleed or genuine decisions suppress candidate suggestions for that line.
- No embeddings, speaker voiceprints or private fixtures are stored or fetched by this feature.

## Defaults

| Setting | Value |
|---|---|
| Candidate Mic minimum | 4 normalized tokens |
| Candidate maximum per line | 128 tokens; oversized lines skipped |
| System start-time window | ±2,000 ms from Mic start |
| Comparison cap | Nearest 64 System lines per Mic line |
| Ordered edit similarity | At least 0.8 |
| Normalization | Unicode lowercase alphanumeric tokens; punctuation becomes spaces |
| Match priority | Similarity, nearest start, earlier start, segment ID |
| Automatic annotation | Disabled; no acoustic evidence implemented |

## Flag rules

| Condition | Severity | Blocks? |
|---|---|---|
| Similar nearby text | Visible suggestion | No |
| Short reply or oversized text | Candidate omitted | No; module API accepts an explicit manual decision; the current UI offers actions through candidate and existing-decision review cards |
| Missing or ambiguous raw match | Needs review, speech visible | Hiding only |
| Noncurrent run, non-Mic source or invalid decision | Error | Mutation |
| Missing/previously resolved pending decision | Error | Dismissal |
| Raw/audit or historical snapshot mutation | SQLite integrity error | Mutation |
| Revision, summary or FTS failure | Transaction error | Entire choice; prior state preserved |

## Side effects in order

1. Validate active run and persisted Mic source, or pending review.
2. Append the immutable event and update only current-run match snapshots.
3. Increment transcript revision and mark summaries stale.
4. Rebuild effective search and commit; failure restores all earlier state.
5. On publication, match latest nonretired events to new raw lines before effective consumers are rebuilt.

## Access

Only the local application's current-run editing path mutates decisions. The session service supplies processing/capture guards. Historical snapshots are read-only. No network, model setup, biometric enrollment, audio rewrite or capture occurs in this module.
