# A speaker-separation capacity limit does not fail refinement

- Date: 2026-11-10
- Feature: batch-refinement
- Related code: crates/vt-session/src/speaker_pipeline.rs (`refine`, `separate`, `settle`, `recoverable_capacity`, `capacity_warning`), crates/vt-session/src/store.rs (`retain_speakers_after_failed_separation`), crates/vt-diarize/src/lib.rs (`CapacityLimit`)

## Context

On 2026-11-09 the automatic refinement of the first real live call failed because speaker separation reached its cluster limit. The documentation described any capacity limit as a hard error that blocks publication, so a complete refined transcript was lost because of a speaker-labelling limit.

## Decisions

- `DiarizeError::Capacity` carries which limit fired (constraints, turns, embeddings, clusters).
- When turns, embeddings or clusters capacity is reached while processing audio or finishing, refinement continues. The refined transcript is produced and published and the meeting becomes ready.
- Nothing from the failed separation attempt is saved. The previous run's speaker keys, timed turns, user overrides and pending reviews are carried onto the new run, as when the speaker model is unavailable. With no previous turns, remote meetings use channel labels Me/Them and in-person meetings show Unknown.
- A warning is appended to the run's speaker warnings (Processing notes in the Speakers tab): "Speaker separation could not finish for this recording (limit: {limit}). The refined transcript is complete. " followed by "Speaker labels, names and corrections from the previous transcript were kept and applied by time." / "Speakers are shown as Unknown." / "The transcript is labelled by channel (Me / Them)."
- The constraints limit (raised when the diarizer is created), model errors, native-audio errors, invalid input and cancellation still fail the refinement.
- design.md's "partial runs are never published" still holds: the transcript is complete and no output of the failed separation is saved.

## Alternatives Considered

- Keep capacity as a hard error. The transcript is lost for a labelling limit.
- Publish the partial separation. Rejected; partial speaker output would look complete.

## Consequences

- Positive: refinement of long or acoustically ambiguous calls still delivers the refined transcript.
- Negative: when the previous run is a live run, its labels may be fragmented or stop early. The warning is not prominent in the UI. The underlying clustering cause is recorded in the speaker-turns trail entry of 2026-11-10 and is not fixed.
