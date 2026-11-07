# Local Retention and Deletion

Retention provides explicit meeting deletion, audio-only purging, optional age-based audio removal and collection of unreferenced obsolete transcript runs. Audio defaults to keeping everything. Obsolete nonactive runs become eligible after seven days by default; users can choose to keep every transcript version, and referenced history remains protected. This feature does not implement encryption, secure erasure, voiceprint retention or deletion from backups/exported copies.

## User flow

1. Request a deletion preview for one meeting or preview the configured maintenance policy.
2. Inspect file/byte/run counts and protected history counts. A preview alone changes no meeting data or files.
3. Explicitly confirm a manual deletion token, or apply a previously authorized retention policy while the desktop is idle.
4. The session service reserves maintenance, rejects capture/processing activity, and stops/drops playback before deletion. The store also rejects busy meeting state, running jobs and a held recorder lease.
5. The operation confirms a durable intent, moves owned content into a same-root trash directory, commits database changes, then removes trash.
6. A failed database operation restores original paths. A committed database deletion whose trash cleanup fails is reported as pending cleanup and retried on recovery.
7. Audio-only purge leaves the transcript, annotations, summaries, source citations, notes and search intact, with explicit audio-unavailable metadata. Playback and processing must consult that metadata.

Exported copies remain wherever they were saved, including the root `Exports` folder. Meeting deletion never selects files by an export-name pattern. Backups and copies made outside the data root are outside this operation.

## Technical flow

`retention::plan(root, meeting, DeletionKind)` writes an unconfirmed token with a fingerprint of all owned database rows, foreign-key inventory, bounded filesystem inventory, counts and exact owned move paths. `commit(root, token)` is the manual confirmation boundary. It rechecks the active snapshot and inventory before writing `confirmed` durably. Stale previews fail without touching source content.

All filesystem operations use a `cap_std::fs::Dir` rooted at the data directory. Meeting IDs permit only 1–64 ASCII letters, digits, hyphens and underscores. Database audio paths must equal the recorder's expected meeting/track/epoch/sequence path. Traversal rejects symlinks, special files, non-UTF8 filenames, depth over 16 and more than 100,000 entries. Only `meetings/<owned-id>` or its `audio` child is selected; unknown root/trash entries are never scavenged.

Confirmed intents move each selected source by rename to `.retention-trash/<token>/<index>`, with directory synchronization. On recovery, source/trash presence makes partial moves resumable; simultaneous source and trash paths fail rather than overwrite. Database deletion uses an immediate transaction and deferred foreign-key checking. A deletion gate tied to that confirmed intent permits only the required immutable DELETE operations. The gate is removed before commit; ordinary transcript/turn/audit update and insert guards remain unchanged.

`has_pending(root, meeting)` reads confirmed/cleanup intents directly, including receipts whose meeting rows have already been deleted. `audio_available` returns false for those pending states, explicit purges and absent meetings; an existing legacy meeting without purge metadata remains available. Prepared previews leave audio available. Before any recorder recovery, playback or inference, session callers must block an affected meeting while `has_pending` is true: temporarily moved ready chunks must never be reclassified as missing-audio gaps.

`recover(root)` processes only `confirmed` and `cleanup` intents. It must run after schema initialization, before ordinary interrupted-recording recovery, and under the desktop/runtime maintenance reservation. Unconfirmed previews never authorize recovery deletion. A database failure rolls back the gate and data changes, restores moved files, and marks the operation failed. If restoration itself fails, the confirmed intent remains available for recovery and the combined pending-restoration error is persisted when storage permits; failure to save status never replaces the primary error or discards the recovery payload. A filesystem failure after database commit leaves state `cleanup` and an explicit pending token; restored audio is never falsely claimed.

## Bounded preview and receipt metadata

Unconfirmed previews expire after 24 hours. Confirmation checks the deadline explicitly and returns a fresh-preview instruction without touching source files. Successful planning and recovery sweep expired unconfirmed rows; a new plan keeps at most four ordinary unconfirmed previews for that meeting, pruning the oldest before insertion. A token evicted by either bound also returns an actionable fresh-preview error through the progress-metadata lookup.

Completed and restored-failure receipts immediately retain only their valid versioned payload shape and preview metadata: `entries`, `moves` and `run_ids` become empty arrays. At most the 128 newest ordinary terminal receipts remain globally. Housekeeping skips malformed terminal payloads rather than making startup recovery fail, while terminal count pruning still applies. A successful database deletion invalidates other unconfirmed previews for that meeting because their source snapshot is stale.

Confirmed, cleanup and deletion-gated intents are exempt from every expiry, count and compaction rule, even when ancient. Their complete filesystem inventory and selected-run capabilities remain unchanged until recovery reaches a terminal state. Housekeeping only changes disposable SQLite intent metadata; it never traverses or deletes source files. `meeting_for_token` reads a UUID-validated persisted meeting identifier for progress display without deserializing the full inventory; commit retains full payload/snapshot validation.

## Policy and protected history

`MaintenancePolicy { audio_keep_days: None, obsolete_run_days: 7, cleanup_obsolete_runs: true }` is the default. A positive audio age policy enables purging by the saved meeting start date; `None` retains audio. `cleanup_obsolete_runs=false` keeps all transcript versions while leaving the independent audio-age policy active. The cleanup flag defaults to true when reading older policy JSON without that field. Zero days is rejected even when cleanup is disabled. Dates and arithmetic are validated. The session integration applies policies only while safely idle. Policy previews contain per-meeting actions; due audio purging and eligible run collection share one transaction to avoid self-invalidating previews.

Run collection preserves current/active runs, running runs, recent runs, every run referenced by a summary or summary citation, every source run referenced by immutable bleed decisions, and every correction-match run. Protected totals include active, reference, recent and explicitly retained-by-policy runs. Collection deletes run-owned raw segments, annotation snapshots, speaker turns/keys/state, review rows and echo-processing metadata, while retaining meeting-level names/constraints. It does not claim that every historical run is removable after seven days.

The schema is inspected through `sqlite_master`, `PRAGMA table_info` and `PRAGMA foreign_key_list`. A new unknown FK descendant of an owned table blocks retention until ownership handling is added. Optional health tables are handled when initialized: note timing, source timing and meeting health. Global resource-health and application settings survive meeting deletion.

| Entry point/type | File | Purpose |
|---|---|---|
| `plan`, `commit`, `DeletionPreview`, `DeletionKind` | `crates/vt-session/src/retention.rs` | Manual preview/confirmation |
| `preview_policy`, `apply_policy`, `MaintenancePolicy`, `PolicyPreview` | Same | Authorized age-based maintenance |
| `recover`, `MaintenanceReport` | Same | Confirmed recovery, bounded receipts and truthful cleanup reporting |
| `meeting_for_token` | Same | Cheap validated progress-display meeting lookup; no deletion authorization |
| `audio_available`, `has_pending` | Same | Audio availability and pending-recovery guard before any reader/recorder recovery |
| Synthetic FLAC, failure and process-kill regressions | `crates/vt-session/src/retention_tests.rs` | Temporary-only validation |

## Desktop controls and commands

Settings → Privacy and retention loads the saved policy and last maintenance report. Independent controls keep all audio or set an audio age, and clean up unreferenced transcript versions or keep every version. Seven days remains the positive version-age default even when cleanup is disabled. Saving authorizes background policy execution. The scheduler checks every 60 seconds, skips recording/processing and active saved-audio playback, and throttles automatic attempts to hourly even after failure. Saving retention preferences resets the attempt/success backoff so it can be checked at the next idle tick.

Meeting detail → Stored meeting data offers **Review audio removal** and **Review meeting deletion**. These buttons request previews only. A native modal shows owned file/byte/version totals, protected history and the export/backup limitation. A separate acknowledgement enables confirmation. Stale previews require a new review; a returned commit result is retained even if refreshing the view fails, and that token cannot be resubmitted. A restored failure report does not clear the meeting selection. Pending cleanup and errors remain visible in the maintenance report.

Unavailable audio disables transcript timestamp playback, speaker/bleed excerpts, waveform requests and replay/refinement. Text, notes, summaries, historical versions and source citations remain readable. The maintenance progress banner has no cancel action. A disk/source-clock strip distinguishes verified source timing from approximate observations, and note timing labels use persisted provenance rather than invented synchronization. Legacy notes without timing metadata say “Timing unverified.”

| Tauri command | Arguments | Result |
|---|---|---|
| `maintenance_policy` | None | `MaintenancePolicy` |
| `save_maintenance_policy` | `policy` | Saved policy |
| `preview_deletion` | `id`, `kind` meeting/audio | `DeletionPreview` |
| `commit_deletion` | `token` | `MaintenanceReport` |
| `preview_cleanup` | None | `PolicyPreview` |
| `apply_cleanup` | `preview` | `MaintenanceReport`; server rechecks all policy fields |
| `last_maintenance_report` | None | Last persisted report |

| Frontend component | File | Purpose |
|---|---|---|
| Privacy controls | `ui/src/lib/PrivacyPanel.svelte` | Saved policy, manual/automatic previews and report display |
| Confirmation modal | `ui/src/lib/DeletionDialog.svelte` | Separate acknowledgement, one-shot commit and refresh-failure handling |
| Resource status | `ui/src/lib/ResourceStatus.svelte` | Recording disk and source-clock status |
| API adapter/types | `ui/src/lib/api.ts` | Typed maintenance commands, health and timing metadata |
| Application integration | `ui/src/App.svelte` | Meeting controls, guarded audio readers, retained text/citations and maintenance progress |

## Schema and integration

| Table | Columns and purpose |
|---|---|
| `app_retention_intents` | `token` UUID; `meeting_id`; `kind` meeting/audio/runs; `state` preview/confirmed/cleanup/done/failed; `payload_json` exact plan/inventory/selected runs while actionable, then a minimal valid terminal payload; `created_at`, nullable `confirmed_at`; nullable `error`. No FK to removed meetings; a minimal receipt survives deletion. |
| `app_retention_gate` | Transaction-local-use `token` FK to a confirmed intent; removed before commit. It authorizes scoped immutable DELETE guards, never UPDATE. |
| `app_audio_retention` | `meeting_id` FK; `audio_available`; `purged_at`; `original_chunks`, `original_bytes`. Purging deletes chunk rows instead of inventing recording gaps. |

SQL artifacts: `documentation/features/app/desktop-session/sql/008-retention.sql` and `009-retention-delete-guards.sql`, applied after schemas001/002/004/006/007. The second artifact replaces only DELETE guards and preserves normal raw/audit immutability.

The desktop adapter owns IPC commands, policy persistence, explicit confirmation UI, capture/processing exclusion and playback teardown. There is no HTTP route, network request or cloud deletion. Local filesystem/database access is controlled by the OS account and desktop instance lock; the gate is an application integrity boundary, not protection against an OS user manually editing SQLite.

Deletion is ordinary unlink after rename, not guaranteed secure erasure from SSDs, snapshots or backups. Actionable intents contain identifiers, counts, hashes and the required filesystem inventory; terminal receipts clear inventory/move/run-ID arrays immediately and retain only bounded preview metadata. No transcript text or audio payload is stored in these receipts. Storage/privacy UI must state the export-copy limitation and pending-cleanup state accurately.

## Validation

Temporary-only regressions encode real synthetic FLAC and construct completed transcripts with speaker names, bleed audits, corrections, citations, notes, route evidence and health rows. They check audio-only preservation, full owned-row deletion with retained exports/global health, stale previews, running jobs/recorder locks, unknown FK descendants, invalid policy dates, referenced history, a combined audio/GC policy action, backward-compatible policy decoding and keep-all transcript versions alongside active audio purging. Injected filesystem/database failures prove restoration and unchanged immutability guards. Child test processes are actually killed after durable confirmation, after rename and after database commit; recovery completes only confirmed work. A moved confirmed intent with an obstructed restore remains pending, reports audio unavailable, and preserves ready chunk hashes/states. Pending lookup also survives complete meeting-row deletion. A postcommit trash symlink produces pending cleanup while an outside-root sentinel survives, then recovery succeeds after the obstruction is removed. Mocked desktop UI regressions cover policy defaults/keep-all, preview cancellation, acknowledgement, stale tokens, result/refresh failures, capture exclusions, unavailable audio, retained citations and nonblocking device discovery. No native audio is captured or played by those tests. Receipt stress tests cover preview count/expiry without source changes, terminal count and immediate compaction, malformed terminal skip, ancient confirmed/cleanup/gated payload preservation, and recovery reporting when a restored-failure receipt is legitimately pruned. These tests establish local workflow behavior, not secure-erasure or platform-wide filesystem certification.
