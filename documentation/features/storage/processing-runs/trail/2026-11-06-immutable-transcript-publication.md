# Immutable transcript publication and conservative overlays

- Date: 2026-11-06
- Feature: processing-runs
- Related code: `crates/vt-session/src/store.rs`, `crates/vt-session/src/store_tests.rs`

## Context

The initial desktop store replaced `app_segments` during successful replay. This prevented historical references and gave future text corrections no durable source anchor. Existing recordings and transcript rows must survive the upgrade.

## Decisions

Keep the original desktop tables as an archive and introduce run-scoped immutable segment rows plus a meeting revision pointer. Migrate existing transcripts transactionally and idempotently. Publish run completion, effective correction matching, pointer movement, summary invalidation and FTS updates together. Require a complete current run and monotonic input revision for summaries. Retain explicit run-scoped summary source foreign keys.

Use exact track/time/SHA-256 anchors for automatic overlay matching. Preserve ambiguous or unmatched overlays for explicit review. Track per-run matches to preserve historical source references. Run garbage collection is deferred; no historical pass is deleted by this implementation.

The final adversarial review identified that titles/personal notes are not current summarizer inputs. Their edits therefore refresh FTS without advancing transcript correction revisions or staling existing notes. A dedicated `app_summary_failures` row persists provider errors independently from recording/transcript warnings, so a failed summary request cannot overwrite source-health metadata.

## Alternatives Considered

- Destructively replace legacy segments: rejected because it loses history and invalidates citations.
- Fuzzy correction application: deferred because incorrect replacement can silently alter unrelated speech.
- Separate search refresh after commit: rejected because searches could disagree with the visible transcript.

## Consequences

- Positive: legacy data remains intact; publication and search agree; failed work remains isolated; summaries cannot silently become fresh against outdated input.
- Negative: transcripts occupy more disk space, exact anchors can require review after changed segmentation, and index rebuilding scans all active text for the edited meeting.
