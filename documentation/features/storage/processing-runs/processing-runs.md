# Processing runs and transcript revisions

A meeting keeps each transcript pass as an independent run. Live final segments are visible immediately; refinement/replay segments remain private to their running pass until publication. The previous pass and its segment IDs remain available for history and summary citations.

## User flow

1. Start a meeting. Its active live run receives final transcript segments.
2. Stop recording. The live run is sealed; subsequent refinement creates another run.
3. A successful replay or refinement publishes its complete transcript atomically. Cancellation or failure retains the previous active transcript.
4. Correct a transcript segment. An overlay changes displayed/exported/searched text without modifying the original row.
5. Reprocessing reapplies only corrections with one exact track/time/text-hash match. Unmatched or ambiguous corrections stay available for review; the user can attach one to a selected current segment or dismiss it.
6. Summaries cite IDs within their source run. Transcript corrections and newer publications make prior summaries stale. Title and personal-note edits refresh search without invalidating transcript-only AI notes.

## Technical flow

`store::initialize` executes both desktop migrations in a transaction. Meetings without revision metadata receive one live-kind migration run containing a copy of all legacy segments. Non-recording runs are sealed; `app_segments`, notes, audio, and meeting metadata are retained. Restart recovery abandons interrupted running passes while retaining viewable live finals.

`create_run` records engine metadata as JSON and captures the current persisted meeting-local speaker-constraint version (zero before user edits). `publish_run` rejects publication if those constraints changed while the pass was processing, preserving the earlier active transcript for a retry with the new constraints. `append_run_segments` inserts into unpublished running passes. `save_segment` accepts only the active running live pass and rebuilds search within its write transaction. `complete_live_run` seals normal capture. `publish_run` verifies meeting ownership and running status, resolves corrections, seals the run, moves the active pointer, marks summaries stale, and rebuilds search in one transaction. Legacy replay staging publishes a new replay pass with the same transaction semantics.

Correction anchors use the original segment's SHA-256 text hash, exact track, and exact start/end milliseconds. Matches must be unique in both directions. The conservative matcher does not guess across changed boundaries. Explicit resolution can re-anchor an overlay to a selected segment. Historical matches are retained. Replacement text is 1–32,000 bytes and must contain non-whitespace text.

`save_summary` requires the expected active run and correction revision token, a complete run, valid JSON, and source IDs present in that run. The token changes with transcript overlays and speaker edits that affect summary inputs: title and personal notes are not summary inputs, so those edits neither advance it nor invalidate summaries. Foreign keys retain source runs and cited segments. Summaries store provider/model/template, JSON and Markdown, time, revision and stale state. The summary provider owns semantic citation validation; storage enforces run ownership and existence.

`set_summary_error` stores or clears a summary-specific failure in `app_summary_failures`. `MeetingDetail.summary_error` loads that value independently of the meeting recording/transcript error. The caller controls summary lifecycle clearing/restoration; this store API does not alter meeting status, recording warnings, transcript revisions, summaries or search.

## Key types and entry points

| Type / entry point | Path | Role |
|---|---|---|
| `RunInfo`, `Segment`, `TranscriptWord` | `crates/vt-session/src/store.rs` | Version metadata and run-scoped text/word timestamps |
| `MeetingDetail`, `Correction`, `SummaryInfo` | `crates/vt-session/src/store.rs` | Active/historical detail, overlays and cited summaries |
| `create_run`, `append_run_segments`, `publish_run`, `abandon_run` | `crates/vt-session/src/store.rs` | Unpublished processing lifecycle |
| `correct_segment`, `resolve_correction` | `crates/vt-session/src/store.rs` | Immutable-original editing and review |
| `set_summary_error`, `MeetingDetail.summary_error` | `crates/vt-session/src/store.rs` | Persistent provider-failure metadata independent of recording/transcript health |
| `search_meetings`, `rebuild_fts` | `crates/vt-session/src/store.rs` | Literal query handling and effective-text indexing |
| Store regression tests | `crates/vt-session/src/store_tests.rs` | Temporary synthetic fixtures only |

## Database schema

| Table | Columns / purpose |
|---|---|
| `app_processing_runs` | UUID `id`, meeting FK, kind (live/replay/refine/legacy), status (running/complete/abandoned/superseded), engine JSON, captured persisted speaker-constraint version, created time |
| `app_meeting_revision` | Meeting FK, active run FK, monotonic correction/input version |
| `app_run_segments` | Composite run/segment key, track, ordered times, original text, speaker, word timestamp JSON; inserts require running state, updates/deletes rejected |
| `app_segment_corrections` | UUID, meeting, track/time/hash anchor, replacement, applied/needs_review/dismissed status, created time |
| `app_correction_matches` | Correction/run key and unique run/segment target, with composite segment FK |
| `app_summaries` | UUID, meeting/run FKs, input revision, provider/model/template, JSON/Markdown, stale boolean, created time |
| `app_summary_failures` | Meeting primary/foreign key with delete cascade, non-null error text; latest summary-only failure |
| `app_summary_sources` | Summary/source IDs plus source run; composite segment FK protects citations |
| `app_search_fts` | FTS5 meeting/source/reference/text rows for active effective segments, title, notes, and fresh active summaries |

## Search, integration and access

The local desktop service is the caller; there are no network routes or accounts. SQLite foreign keys and 5-second busy timeout are enabled per connection. User search input is split into whitespace tokens and quoted as literal FTS terms joined with AND; FTS operators cannot be injected. Empty searches return the meeting library. Search index writes occur inside the same store transaction as their source edits. Storage callers must use these APIs rather than modifying indexed source tables directly.

The store performs no model inference, cross-meeting voiceprint memory or run garbage collection. The desktop `speaker_store` now persists meeting-local timed speaker assignments, user names/temporal identity constraints and review records in migration 004; processing-run creation/publication captures and validates that constraint version. See [desktop session](../../app/desktop-session/desktop-session.md#native-saved-playback-and-live-speech-detection) for those annotations and their experimental limits. All runs are retained; future collection must preserve active runs and summary references. Effective exports use overlays. The full design roadmap remains in `design.md`.

## SQL artifacts

- `documentation/features/app/desktop-session/sql/001-desktop.sql`: original desktop tables, retained for safe migration.
- `documentation/features/app/desktop-session/sql/002-processing-runs.sql`: revision tables, immutability triggers, correction/summary relationships, independent summary failures and FTS5.
