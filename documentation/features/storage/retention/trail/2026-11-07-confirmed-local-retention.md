# Confirmed Local Retention

- Date: 2026-11-07
- Feature: retention
- Related code: `crates/vt-session/src/retention.rs`, `retention_tests.rs`, desktop SQL008/009

## Context

The immutable transcript and annotation schemas must prevent ordinary history mutation while allowing explicit privacy deletion. Filesystem removal and SQLite transactions cannot commit atomically together. History also contains citations and raw-anchor audit references that must not dangle during automatic collection.

## Decisions

Use an explicit preview/confirmation boundary, durable confirmed intents, capability-relative same-root trash rename, database cleanup under a scoped transaction gate, then physical cleanup. Recover only confirmed intents and distinguish rollback/restoration from postcommit cleanup. Audio-only purge retains transcript/citation history and records unavailable audio rather than synthesizing gaps. Default audio policy keeps everything; seven-day run collection preserves all active/recent/referenced sources. Unknown FK descendants fail closed. Exported copies are deliberately retained, without filename-pattern deletion.

## Alternatives Considered

- Unlink files first: rejected because database failure would leave an intact manifest pointing to lost audio.
- Delete database rows first: rejected because a crash would orphan untracked private files without a recoverable intent.
- Disable all immutability triggers during deletion: rejected because it would weaken ordinary transcript and audit protection.
- Collect every nonactive old run: rejected because summary citations, corrections and bleed audit source anchors must remain valid.
- Delete matching export filenames: rejected because exported copies can be user-managed and no explicit tracked-copy selection exists.

## Consequences

- Positive: explicit deletion, audio retention and bounded history maintenance can resume after interruption while keeping references valid and errors visible.
- Negative: filesystem/database failures can require later cleanup; referenced historical sources remain stored, and export/backups need separate user action.
- Ordinary deletion does not promise secure erasure from SSDs, filesystem snapshots or copies.

## Desktop integration in this increment

Privacy settings expose independent audio-age and transcript-version controls, preview/report commands and per-meeting deletion previews. A native modal requires acknowledgement before a one-shot confirmation; refresh failure preserves the returned report and cannot retry an already committed token. The background scheduler checks each minute and limits automatic attempts to hourly even after failure (policy save resets the backoff) while idle, skipping saved-audio playback. Unavailable audio blocks waveform/playback/reprocessing while source text and citations remain available. UI tests use mocked IPC and synthetic data only.

## Bounded receipt remediation

Implementation review found unbounded retention-intent inventory growth from repeated previews and terminal receipts. Unconfirmed previews now expire at 24 hours and are capped at four per meeting before insertion. Done/failed receipts immediately clear entries/moves/run IDs while retaining a valid payload and preview metadata; only the 128 newest ordinary terminal receipts remain globally. Planning/recovery sweeps disposable rows, and successful deletion invalidates other meeting previews. Confirmed, cleanup and gated recovery capabilities are exempt regardless of age/count. Malformed terminal payloads are skipped safely, and a pruned restored-failure receipt cannot suppress its reported error. Pending restoration errors are saved best-effort without modifying confirmed state or the recovery payload. Stress and actual process-kill recovery regressions verify those boundaries.
