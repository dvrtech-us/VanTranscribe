# Playback, Live Speech Detection and Speaker Runtime Checkpoint

- Date: 2026-11-06 (Central)
- Objective: have a working app
- Goal status: active; all approved roadmap requirements remain in scope
- Previous checkpoint: [refinement and AI notes](2026-11-06-refinement-and-ai-notes-validation.md)
- Existing Claude Opus/Codex design reviews remain accepted; this increment implements that design without repeating its approval gate

## Current behavior

The Rust core now plays saved Mic/System FLAC on the shared recording timeline, with independent gains, silent gaps, pause/resume, seek, stop and waveform peaks. The UI supplies an Audio tab, waveform/slider seeking, transcript timestamp playback and projected speaker lanes. Playback opens no output stream until Play, and every recording state, including paused/stopping capture in another meeting, excludes output. Capture start stops and drops an existing player before launching its helper.

Optional Silero VAD gates live inference incrementally with bounded pre-roll/hangover. It never gates archived audio. Valid native online token times now supply acoustic word estimates, retained through replay; unavailable timing remains empty. The default preference is enabled only when a verified VAD is installed. Missing optional setup retains ungated recognition.

Experimental speaker models remain disabled by default, separately downloaded only after their source/license notice is acknowledged. Independent audio windows, one-thread speaker sessions and bounded worker/state separate speaker work from streaming STT and capture. Remote microphone Me is a channel prior; in-person voices are clustered. Short, unclear, provisional and overlapping turns remain explicit. No cross-meeting memory or unnamed embedding vector is persisted.

The Speakers tab supports meeting-local names, explicit source playback, same/different clean-excerpt corrections, reconciliation choices and correction removal. Persisted relationships use track/time anchors rather than serial IDs. Replacements supersede the prior relationship for the same excerpt pair; removal retains audit history and reverses linked manual alias/turn overrides. Names/relationships advance the speaker constraint and summary revision tokens. Processing publication rejects changed constraints. Reprocessing with speaker analysis disabled/unavailable retains previous timed annotations, names and unresolved reviews with an explicit reuse warning.

The implementation review identified two correction failures and an incremental-read cost. Saved microphone cannot-links now override the remote Me prior with actual clustering. Unresolvable, contradictory, overlapping or late corrections preserve recognized text, mark affected regions Unknown and create a linked review that can remove the original correction. Unrelated speaker evidence continues. Names survive Unknown masks for explicit later reconciliation; ambiguous microphone matches receive a fresh unnamed identity. Live turn publication avoids resolving the full transcript, and detail reads cache identity/alias resolution. Defensive replay failures isolate only the implicated connected component; unrelated Mic/System corrections remain effective. Dependent corrections defer under the original failed-source review, preserving its undo target. A 4,096-active-correction limit prevents storing relationships that the engine cannot process; replacements work at capacity.

## Inspected evidence

| Requirement | Evidence | Limit |
| --- | --- | --- |
| Native saved output | Generated FLAC plus a real, muted CPAL output smoke exercised callback cursor advance, pause, resume, playing seek and stop | No audible content or private recording; full device/route/physical-latency matrix remains open |
| Bounded faithful playback | Seven deterministic sink/FLAC tests cover gaps, stereo/downmix/resampling, underrun, gains, controls, SHA/path containment and unchanged source state | Source timeline correctness does not establish DAC latency |
| Recording exclusion | Five AppService tests include all active/paused/stopping states and a helper-execution marker proving output was dropped first | Simulated capture helper; native long-call transitions remain separate |
| Incremental VAD | Real public speech fixture retained tested onset/words, independent 16/48 kHz tracks, partials before final, bounded silence, gap/reset and forced endpoints | Full WER and end-to-end screen latency/corpus gates are not proven |
| Real speaker inference | Official public 56.86-second four-voice demo produced timed turns; separate 48 kHz/gap/mode checks passed | It produced six experimental clusters rather than four; this is not calibrated DER/count acceptance |
| Saved full service pipeline | Actual public two-track FLAC → diarization → Parakeet → word assignment/new complete run, retaining history and source hashes | Short fixture; no broad language/channel quality or hour-long throughput claim |
| Live full service pipeline | Paced public audio from a simulated NDJSON helper exercised real Silero, Zipformer and independent speaker workers; both ready flags, words/turns/provenance, channel prior, clean stop/seal and exact expected PCM24 archives passed | No OS input/output/network/private data; simulated source does not validate physical capture |
| Speaker continuity | Eight pure overlap/Hungarian tests cover conservative split/merge/ambiguity, track isolation and bounded identity matching | Temporal continuity does not verify a human identity |
| Storage and edits | Fourteen speaker-store regressions cover immutable raw text/turns, word mapping, summary revisions, anchors, conflicts/rollback, constraint version, replay, pending reviews, name retention and revocation/audit; a real pending-constraint refinement preserves the full public text and source hashes | Garbage collection, encryption, voiceprints and retention remain unimplemented |
| UI | Twenty-two Chrome IPC fixture flows pass, including Audio/Speakers controls, model notice gating, names, corrections/removal, review choices, historical state and global playback exclusion | Browser fixtures do not replace native click/manual or capture acceptance |
| Build | Full workspace tests, strict all-target lint, frontend diagnostics/production build and macOS release bundling pass; strict recursive signature and included license notices verified; isolated packaged frontend IPC initialization passed | Ad-hoc local signature; no distribution notarization claim |

The final ordinary Rust suites contain 159 passing tests; explicit model/device/public-fixture checks are selected separately. Test fixtures and runtime artifacts remain outside source control. This record excludes transcript content, private participant/device names and credentials.

The final recorder regression exposed a concurrent first-open WAL transition race despite an installed busy timeout. Initialization now retries only Busy/Locked on fresh connections within one shared five-second monotonic deadline and a rollback-safe schema transaction. Twenty independent stress invocations passed: 400 fresh manifests, 3,200 concurrent meeting writes and recoveries. Normal audio publication is not retried. Persistent-lock and unsupported-schema guards pass.

## Installed result

The final focused Claude Opus implementation check found no blocking defect after the remedies. The existing two-model design reviews remain accepted. Workspace tests (159 passed), strict all-target Clippy, formatting, frontend diagnostics (zero warnings/errors), 22 UI fixture flows and the production release build pass. The 43.75 MiB ad-hoc-signed macOS bundle includes the capture/summary helpers and required license notices; recursive strict signature verification passes. Its packaged frontend connects with isolated empty data and the review-link schema initializes.

The validated bundle is installed at `~/Applications/VanTranscribe.app` and running with its real packaged frontend connected. Installation checked the app was idle, retained the previous bundle for recovery and started no capture or playback. Optional speaker analysis remains off until explicitly configured.

## Completion audit

M1 now includes native playback and optional live VAD; full quality/latency acceptance remains open. M2b has a connected experimental core and user-correction workflow, with actual live/offline service evidence. Its uncalibrated thresholds, English/channel DER, overlap performance, speaker-count quality and long-session acceptance remain incomplete. This checkpoint does not complete M0 hardware/drift/benchmark gates, M2c AEC/bleed handling, M4 voiceprints/privacy/detection/retention or M5 Windows parity.

Native online word estimates have 40 ms model-frame granularity; batch end estimates can have 80 ms granularity. Neither is forced alignment. The model's canonical segmentation input is ten seconds; five-second source windows are padded internally, with 2.5-second ownership stride. The UI presents these labels as provisional suggestions, and similarity scores are not displayed as identity confidence.

Next checkpoint: [echo control, reversible bleed review and evaluation](2026-11-06-echo-and-evaluation-validation.md).
