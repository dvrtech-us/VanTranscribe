# Retention Baseline

## Invariants

- Audio is retained by default (`audio_keep_days=None`); obsolete run eligibility defaults to seven days with `cleanup_obsolete_runs=true`. A false cleanup flag retains every transcript run independently of audio retention.
- Manual deletion requires a preview token and a distinct explicit commit. Preview and unconfirmed recovery never delete meeting content.
- Active, paused/stopping capture, processing and playback leases are excluded by the session reservation; the store also rejects busy saved meeting state, running jobs and held recorder locks.
- Stale database or owned-file snapshots fail before confirmation and source moves. Unconfirmed tokens expire after 24 hours and require a fresh preview; evicted tokens return the same actionable instruction.
- Ordinary unconfirmed previews are capped at four per meeting; done/failed receipts are capped at the 128 newest globally and immediately clear inventory/move/run-ID arrays. Expired previews and old terminal receipts are pruned during planning/recovery.
- Confirmed, cleanup and gated intents are never expired, count-pruned or compacted; malformed terminal payloads cannot make recovery fail during housekeeping. Successful database deletion invalidates other meeting previews.
- Confirmed intent is durable before same-root rename. Database commit precedes physical trash cleanup.
- Database failure rolls back deletion and the trusted gate, then restores original files. Unrestorable files keep a recoverable confirmed intent, with its pending error persisted when possible and its full payload intact.
- Postcommit filesystem failure reports pending cleanup; audio/transcript state never claims a failed restoration or available purged audio.
- Pending confirmed/cleanup intents make audio unavailable and block recorder recovery, playback and inference for that meeting, including cleanup after its database rows were removed. Prepared previews do not block audio. Existing legacy meetings without purge metadata remain available; absent meetings do not.
- Recovery resumes confirmed or cleanup intents only. It never treats arbitrary trash contents or previews as authorization.
- Audio purge removes encoded owned audio and chunk rows, preserves transcripts/citations/notes/annotations/search/summaries and records explicit audio-unavailable metadata without fabricated gaps.
- Meeting deletion removes owned DB descendants, FTS and the owned meeting directory; exported copies, global settings and global resource health remain untouched.
- History collection preserves active/running/recent runs, all runs when cleanup is disabled, and every summary, citation, bleed-source audit and correction-match referenced run.
- Raw transcript and audit UPDATE guards remain absolute; scoped DELETE exceptions require a confirmed transaction gate whose removal precedes commit.
- All current FK descendants are enumerated; unknown descendants block deletion until reviewed.
- Capability-relative operations reject symlinks, unsafe IDs, manifest path mismatches, special files, excessive depth/entries and source/trash collisions.

- Settings provide independent audio-age and transcript keep-all choices; meeting removal always uses a separate preview and acknowledged confirmation.
- A returned commit result cannot be submitted again if UI refresh fails; the result stays visible, and a restored failure leaves the selected meeting intact.
- Unavailable audio blocks waveform calls, playback excerpts and reprocessing while text/citations remain usable.
- Maintenance progress has no cancel action. Disk/source-clock and note labels reflect supplied metadata only.

## Defaults and bounds

| Setting | Value |
|---|---|
| Audio age policy | None; keep audio |
| Background scheduling | Check every 60 seconds; automatic attempt no more than hourly even on failure; idle only, skip active saved-audio playback; save-policy resets backoff |
| Unconfirmed preview TTL / per-meeting count | 24 hours / 4 ordinary previews |
| Terminal receipt count | 128 newest ordinary receipts globally; entries/moves/run_ids empty |
| Protected receipt exceptions | Confirmed, cleanup and gated intents retain full payload regardless of age/count |
| Obsolete run cleanup | Enabled; legacy policy JSON defaults the missing flag to true |
| Obsolete run age | 7 days; remains positive when cleanup is disabled |
| Minimum configured age | Positive days |
| Owned ID | 1–64 ASCII alphanumeric, hyphen, underscore |
| Tree traversal | At most 100,000 entries; depth at most 16 |
| Move staging | `.retention-trash/<UUID>/<index>` beneath the data root |
| Source identity | Exact recorder track/epoch/sequence relative path |
| Export copies | Always retained |

## Flag rules

| Condition | Severity | Blocks? |
|---|---|---|
| Capture, processing or recorder lease active | Busy | Mutation |
| Preview snapshot changed, expired or evicted | Stale/expired preview | Confirmation; new preview required |
| Unknown FK descendant | Integrity error | Retention |
| Symlink, outside/unowned path, special file or traversal bound | Safety error | Source move or cleanup |
| Active/referenced/recent history | Protected count | That run's collection |
| Database failure after moves | Failure with restoration | Deletion; source files restored when possible |
| Restore failure | Recovery pending | Completion; intent remains confirmed |
| Physical cleanup failure after DB commit | Cleanup pending | Final receipt; data state remains committed |
| Unconfirmed intent on recovery | No action | No |

## Ordered side effects

1. Preview validates owned identity, idle state, recorder lease, FK graph, raw manifest and inventory (database reads and file inspection alternate, never nested); persists only an unconfirmed plan in one write.
2. Confirmation reads and checks the preview, takes the recording lease and inventories files outside the database, then in one confirming write revalidates the snapshot and records durable user/policy intent; the lease is held until that write commits.
3. Move selected owned sources to same-volume trash and synchronize directories.
4. In one transaction, open the scoped gate, delete owned DB rows or mark audio unavailable, close the gate and record cleanup state.
5. Remove only the confirmed intent's trash, synchronize and record done; retain pending cleanup on failure.
6. Startup recovery runs before ordinary recorder recovery under the same exclusion rules.

## Access and limitations

The local desktop instance and runtime reservation own maintenance. No agent validation targets installed app data; fixtures use temporary roots and synthetic public content. No network, enrollment, secure-erase guarantee or backup/export deletion is included. Referenced history can outlive the seven-day threshold by design.
