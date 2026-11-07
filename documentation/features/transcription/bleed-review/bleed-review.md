# Microphone Bleed Review

Bleed review makes microphone transcript lines reversible user annotations. The app can suggest a possible duplicate of nearby System speech; similarity alone never hides speech. User voiceprint comparison is unavailable, and these suggestions provide no acoustic or biometric proof.

## User flow

1. Read current transcript lines and visible possible-bleed candidates.
2. Explicitly confirm a microphone line as bleed, or keep it as genuine speech. Short replies can be handled manually but never become automatic candidates.
3. Confirmed bleed is excluded from current effective transcript consumers. Restore it by keeping the line genuine; original saved text and audio remain intact.
4. After a new transcription, exact unique raw anchors inherit the decision. Missing or ambiguous bleed matches show a pending review and remain visible. Genuine choices need no unmatched review; they still suppress suggestions if an exact source returns.
5. “Keep this speech visible” dismisses a pending annotation without guessing its new target. That decision lineage is retired and does not return on later processing.
6. Historical runs retain their own match snapshots, including pending reviews and prior confirmed choices.

## Technical flow

`store::bleed::details` receives the selected run's complete effective segment list before hidden filtering. It returns `BleedDetails { decisions, candidates }`. Candidates compare Mic lines of at least four normalized tokens with System lines whose start times are within two seconds. Unicode lowercase alphanumeric tokens preserve word order; punctuation becomes whitespace. Token edit similarity is `1 - distance / max(token_counts)` and must be at least 0.8. Lines over 128 tokens are skipped. Sorted timestamp lookup examines at most the nearest 64 System lines per Mic line. Highest similarity wins, followed by nearest start, earlier start, then stable segment ID. Dense-window limits can omit plausible matches; candidates are review suggestions only.

`set_decision` checks active-run membership and allows a completed run or a persisted final line in an active live run. It reads the raw Mic source anchor, appends a decision audit event, supersedes that lineage's current snapshot, inserts the applied match, increments transcript revision, invalidates summaries, and rebuilds FTS within one transaction. Any failure rolls back every change. The session service additionally excludes processing races.

`reapply` runs inside publication before the new run becomes complete. It uses the newest event of each nonretired lineage. Matching requires exact raw track, start, end and SHA-256 original text, with uniqueness in both directions: one annotation per one source line. Unmatched and ambiguous bleed events become `needs_review`, with no hidden segment. Genuine events without a unique match are omitted from the new snapshot because no speech should be hidden; their audit remains available for later exact matching. A dismissal appends a genuine retired event; it never deletes the old decision.

`hidden_ids` returns only applied confirmed-bleed segment IDs. The session integration uses this set for search, effective transcript, summary inputs and exports. It must still pass the full segment list to `details` so confirmed choices can be restored. Raw completed transcript rows and recorded audio are never changed.

| Class or entry point | File | Purpose |
|---|---|---|
| `BleedDecision`, `BleedCandidate`, `BleedDetails` | `crates/vt-session/src/bleed_store.rs` | Serializable review state |
| `set_decision`, `dismiss_pending` | Same | Transactional explicit choices |
| `reapply`, `hidden_ids`, `details` | Same | Publication and effective-read integration |
| Synthetic regressions | `crates/vt-session/src/bleed_store_tests.rs` | Restore, rollback, matching, historical isolation and bounded work |

## Commands and access

The desktop session service owns `set_bleed_decision` and `resolve_bleed_review` IPC wiring and capture/processing guards. There is no HTTP route or remote service. Editing requires the active meeting run; historical annotation snapshots are read-only. SQLite triggers reject edits to completed historical snapshots and updates/deletes of audit events. The local OS account retains access to the database and audio.

## Schema

| Table | Columns and meaning |
|---|---|
| `app_bleed_decisions` | `seq` event order; `id` event UUID; `lineage_id` choice history; `meeting_id`; `source_run_id`, `source_segment_id` immutable raw source; `track` Mic; `start_ms`, `end_ms`, `original_text_hash` exact anchor; `decision` bleed/genuine; `reason` explicit user action; `retired` prevents future reapplication; `created_at` audit time |
| `app_bleed_run_matches` | `run_id`, `decision_id` composite key; nullable `segment_id`; `status` applied/needs_review/superseded/dismissed; `reason` run-specific explanation |

SQL artifact: `documentation/features/app/desktop-session/sql/007-bleed-annotations.sql`. The migration is additive and idempotent. It adds no embedding, voiceprint or audio payload.

## Validation and limits

Synthetic tests exercise confirm/restore effects on FTS, revisions and summaries, transaction rollback on search failure, immutable raw history, exact matching, collisions in both directions, retired pending reviews, unmatched genuine choices, short replies, genuine repetition, textual export exclusion with JSON retention, and a 10,000-segment dense candidate set. This establishes workflow behavior; it does not establish bleed precision, recall, acoustic speaker identity or quality acceptance on real meetings. No automatic annotation or deletion is implemented.
