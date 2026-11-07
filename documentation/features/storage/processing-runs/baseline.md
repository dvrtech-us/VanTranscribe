# Processing-runs baseline

## Invariants

- Original run segments are immutable; completed passes accept no new segments.
- Final live segments are visible and searchable while recording continues.
- Unpublished, failed and abandoned passes do not replace the active transcript.
- Publication seals the selected run, resolves overlays, moves the active pointer, invalidates old summaries and rebuilds search within one transaction.
- Legacy migration retains original `app_segments` rows, notes and recordings, and copies every segment into one meeting-owned migration pass exactly once.
- Corrections never rewrite original text. Automatic matching requires one exact track/time/hash match, with no competing correction targeting that segment.
- Unmatched corrections remain reviewable; only explicit user resolution can dismiss or re-anchor them.
- Summary citation segment IDs are interpreted within the summary's run.
- A fresh summary is accepted only for the current complete run and expected input revision.
- Editing transcript overlays or speaker metadata that changes summary inputs invalidates summaries and increments the input revision. Title and personal-note edits refresh search but preserve the transcript revision and summary freshness because they are not summary inputs.
- Summary-only failures persist independently in `app_summary_failures` and are returned through `MeetingDetail.summary_error`; setting/clearing them does not alter recording status, warnings, transcript revisions or existing summaries.
- Only active effective transcript text, title, notes and fresh active summaries are searchable.
- Restart recovery abandons interrupted passes and retains existing active transcript finals.
- Synthetic tests use temporary roots; application-support databases and installed apps are never test fixtures.

## Flag rules

| Condition | Severity | Blocks? |
|---|---|---|
| Correction no longer uniquely matches | Review | Automatic overlay application only |
| Summary source revision is old | Stale | Fresh summary acceptance / summary search indexing |
| Processing failure or interruption | Error | Publication; active transcript remains visible |
| Invalid segment or word timing | Error | Segment insertion |
| Edit targets historical run | Error | Overlay write until current transcript reload |

## Side effects in order

Publication: validate ownership/running state → resolve corrections → seal run → move active pointer → set meeting ready → stale summaries → rebuild search → commit. Any error rolls back all steps. Correction edit: mutate overlay → bump transcript correction revision and stale summaries → rebuild search → commit. Note/title edit: mutate metadata → rebuild search → commit, preserving the transcript revision and summaries. Summary failure: upsert/delete only the separate failure row; preserve transcript health.

## Defaults and access

Engine metadata must be valid JSON. Each new run captures the persisted speaker-constraint version, which is zero before user edits. Publication rejects a changed constraint version rather than silently using stale names/relationships. Acoustic live word times are retained when valid and may be empty when unavailable; all persisted live/batch word times must lie within their segment. Correction text permits 1–32,000 bytes containing non-whitespace content. SQLite connections enable foreign keys and wait at most five seconds for a busy database. Local desktop callers have access to their configured data root; there are no HTTP routes, roles or remote authorization rules. No garbage collection currently removes historical passes.
