# Inline speaker rename and a visible saving-audio state

- Date: 2026-11-11
- Feature: desktop-session (with speaker-turns)
- Related code: ui/src/App.svelte (`renameTarget`, `startRename`, `finishRename`, `savingAudio`, `announceSaved`), ui/src/style.css (`.speaker-label-button`, `.speaker-label-static`, `.speaker-label-input`), ui/src/lib/api.ts (`RecorderView`), tests/ui/desktop.spec.ts

## Context

The transcript speaker label opened the Speakers tab, which made naming a speaker a detour. Separately, the recorder thread ([trail](../../../recording/durable-recording/trail/2026-11-11-recorder-thread.md)) lets Stop return before queued audio is written, so the UI needed to say so rather than announce "Recording saved" early.

## Decisions

- Clicking a label edits it in place (Enter or blur saves, Escape cancels, an empty or unchanged name closes without a call) and renames that speaker on every line through the existing `rename_speaker` command. A label is a button only when the line maps to exactly one speaker identity of the current run and nothing is processing; every other label is static text with a hover reason. No new backend command.
- Renaming is allowed in the live transcript during a recording, as in the Speakers panel. This is recorded as an open owner decision, not a settled one. Edits made during a recording become searchable when it stops.
- After Stop the UI shows a "Saving audio" banner and defers "Recording saved to your library." until the recorder has finished; dropped audio is added to the gap banner and a backlog note to the live status line.

## Alternatives Considered

- Keep navigating to the Speakers tab: rejected as too indirect.
- Allow renaming overlap or channel-label lines: rejected, they have no single identity to rename.
- Block renaming during a recording: not chosen yet; left to the owner.

## Consequences

- Positive: one-step naming from the transcript; Stop no longer claims the audio is saved before it is.
- Negative: a rename during a live recording is not searchable until Stop.
- Not verified: browser tests in `tests/ui/desktop.spec.ts` cover the interactions against a mocked backend; no real recording has exercised the saving-audio state.
