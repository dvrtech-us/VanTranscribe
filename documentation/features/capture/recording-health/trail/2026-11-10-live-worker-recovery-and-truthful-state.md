# The speech worker recovers in place and the recording reports the truth

- Date: 2026-11-10
- Feature: recording-health
- Related code: crates/vt-session/src/session.rs (`LiveRecognizer`, `SpeechWorker`, `STT_RECOVERY_BUDGET`, `RECOGNIZER_ERROR_GAP`, `infer_worker`), crates/vt-session/src/backlog.rs (`LiveStopReason`, `LiveTranscriptionView`, `WorkerFailure`, `Controller::{failure, worker_exit, stop_reason, latch_live, gap}`, `live_stopped_warning`, `audio_stage_stopped_warning`, `MAX_GAPS_PER_WORKER`), crates/vt-session/src/capture_health.rs, crates/vt-session/src/lib.rs (`RecordingView`), crates/vt-session/src/echo_pipeline.rs

## Context

A real 95.5-minute in-person recording on 2026-11-10 (single microphone) recorded audio throughout while the live transcript's last segment ended at 47:41. The live STT worker exited and nothing restarted it. The most likely trigger is a database lock on the transcript save during a roughly 10 s stall; this is inferred, not proven, because the error text saved at stop was erased by the automatic refine.

The recording could not say what had happened:

- The live speaker worker had failed at 6:12 with the cluster capacity error. Its per-chunk `worker_unavailable` gap entries exhausted the 128-entry gap list within 20 s, hiding every later gap.
- The stored health report showed no shed stage (stages empty, maximum lag 10.46 s). The worker failures were warnings only, deduplicated and bounded.
- The audio-stage exit warning blamed "recording resource protection", which was not the reason.
- The recording screen showed "Recording" throughout.
- Capture had ten `overflow` gaps at 47:31-47:36 (about 2.1 s lost) and two `device_change` outages that lost 47.3 s (at 86:15) and 36.4 s (at 94:46), each followed by a second short gap. Clipped samples rose from 103 in the first 86 minutes to 3196 in the next 7.7 minutes. Nothing in the report could separate a helper stall from a slow app.

## Decisions

Recovery (speech worker only):

- `LiveRecognizer` (`accept`, `reset_track`, `finish`, `rebuild`) abstracts the engine; `SherpaRecognizer` is the real one and a fake is used in tests. `SpeechWorker::recover` handles a per-message error: it records a `recognizer_error` STT gap for the failing packet and a non-terminal failure entry, adds the warning "Live transcription recovered from a speech engine error; a short stretch of live text may be missing. Raw audio is preserved; refine saved audio for complete coverage.", and resets the affected track.
- The engine is rebuilt only when the error is `SttError::Engine` or the track reset itself fails; before a rebuild both tracks' pending text is finalized and saved.
- The same cause twice in a row with no successful message in between is terminal. A budget of 20 recoveries per recording (`STT_RECOVERY_BUDGET`) ends the worker with the gap reason `recovery_budget_exhausted`. Each recovery loses at most the failing packet (at most 2 s) plus one track's recognizer context.
- The hypothesis returned by a source-gap reset (and by `finish`) is now saved: capped at the audio already fed so it never runs past a gap start, skipped if identical to the last final already published for that track, and saved once. Before, a reset discarded it.
- Recovery runs on the speech thread because it owns the only receiver of the speech queue.
- The speaker worker is still not restarted. Sustained-backlog shedding is unchanged and still sticky for the recording. This narrows the earlier "ordered sticky live degradation, no stage reactivates within that capture" statements: they still hold for backlog shedding and disk suspension, but a speech engine error no longer ends live transcription on its own.

Truthful state and health:

- `RecordingView.live_transcription` (`state` active/stopped, `since_source_ms`, `reason`) is `None` when live transcription was not configured. `reason` is a `LiveStopReason`: `worker_failed`, `recovery_budget_exhausted`, `sustained_backlog`, `disk_protection`, `audio_pipeline_stopped`, `worker_unavailable`. Once stopped it never returns to active within the recording; a generic reason seen first is replaced by the specific one recorded moments later by the stopping worker.
- `RecordingView.capture` (`state` ok/stalled/consumer_slow, `since_source_ms`, `kind` no_audio_callbacks/helper_silent/consumer_slow) shows a capture stall.
- The audio-stage exit warning now names the real reason (`audio_stage_stopped_warning`).
- The health report gains `failures[]`: the first and the latest failure per worker (`FAILURES_PER_WORKER` = 2), each with `worker`, `source_ms`, `cause` capped at 300 characters (including the SQLite extended code when present), `terminal` and an ordinal `count`. It also gains `capture` (see the macos-capture trail entry of the same date).
- Gaps of the same worker, track and reason merge when adjacent within 1 ms regardless of generation, and each worker may use at most `MAX_GAPS_PER_WORKER` = 42 of the 128 entries, so one failing worker cannot hide another's gaps. A worker that stopped on its own error records later audio under `worker_failed` or `recovery_budget_exhausted` rather than `worker_unavailable`.
- Every recording now has a health report (it also carries capture diagnostics). Side effect: a recording without live models can show inference-health warnings, for example a busy-database warning.
- The housekeeping and final-echo lock handling described in the live-english trail entry of the same date is part of the same change.

## Alternatives Considered

- Restart workers as new threads. Rejected: channel ownership makes it impossible without restructuring; the speech thread owns the only receiver of its queue and the sender side lives in the audio stage.
- Retry on the recognizer thread. Rejected: it would overflow its roughly six-second queue.

## Consequences

- Positive: a transient speech engine fault costs a short stretch of text instead of the rest of live transcription, and when live transcription does stop, the screen and the stored report say when and why.
- Negative: the speaker worker can still end for the rest of a recording. Shedding is unchanged and sticky.
- Not verified: nothing here has run in a real recording yet. The engine rebuild path is covered only through a fake recognizer. Several "fails before the fix" cases were reasoned rather than observed.
