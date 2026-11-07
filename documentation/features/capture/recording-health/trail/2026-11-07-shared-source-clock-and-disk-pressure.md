# Align Source Timing and Protect Raw Recording Under Disk Pressure

- Date: 2026-11-07
- Feature: recording-health
- Related code: `crates/vt-session/src/{health,source_timing,backlog,session,lib}.rs`, `crates/vt-capture/src/lib.rs`, `native/macos/CaptureHelper.swift`
- State: Working draft for this unmerged implementation change

## Context

The final echo review found a pause reset that mixed app startup elapsed time with the helper's hardware epoch. Correcting that reset exposed the same origin mismatch in notes and displayed/persisted durations. The approved roadmap also requires measured resource controls that protect raw capture before inference, plus responsive device discovery under a hung driver.

## Decisions

- Announce the helper startup Mach epoch in raw-uptime nanoseconds and read the same OS clock through safe Rust APIs. Validate anchors and retain explicit provenance for older/unknown helpers; never replace hardware timestamps with callback arrivals.
- Attach every note's timing provenance within the existing note/FTS transaction. Use paused playback cursor for saved bookmarks and explicit untimed metadata for ordinary saved notes.
- Introduce injected disk/clock readings, exact decimal thresholds and a thirty-second poll. Refresh before every chunk publication as well, since high-rate multichannel capture can consume meaningful space inside thirty seconds.
- Suspend inference below 500 MB and keep raw audio, then stop/drain below 100 MB. Freeze durations at source endpoints and refuse saved processing until space is available.
- Split valid native packets into twenty-millisecond slices before bounded queues while preserving PCM/frame/host origins. Use actual completed-input source watermarks for sequential backlog shedding, independently of speech events and raw recording.
- Keep device inventory on the established read-only command with a three-second deadline and a 256 KiB stdout cap. A hung inventory is not evidence for the earlier isolated frontend timeout; this change independently prevents that class of GUI stall.

- Integrate retention authority before saved-media access or recorder recovery. Recover confirmed intents before interrupted staging cleanup at startup, expose audio availability in detail, and keep note timing readback compatible with store-only legacy fixtures.
- Protect waveform scans with a reader lease instead of holding the runtime mutex across decoding, and preserve note timestamp honesty in Markdown/TXT exports.

## Alternatives considered

- App `Instant` elapsed as the source clock: rejected because helper startup/permission delay shifts note and DSP timestamps.
- Callback-arrival origin: rejected because pipe/worker backlog is not a hardware timestamp.
- Disable raw capture below 500 MB: rejected; inference yields first and buffered raw data drains at the critical threshold.
- Poll only every thirty seconds: rejected because 192 kHz/eight-channel chunks could outgrow the critical reserve between polls.
- Keep a 600-message queue without packet-duration bounds: rejected because a valid two-second packet could make that minutes of queued audio.

## Consequences and verification

The source epoch is explicit, legacy behavior is honest, and low-space controls preserve raw frames. Additional metadata is local and owned by meeting/note FKs. Injected tests cover thresholds/cadence, no-start critical preflight, continued raw capture after shedding, clean critical drain, atomic note metadata/FTS failure and output-prohibited paused bookmarks. Pure native tests bracket converted Mach clocks with raw uptime without capture. Queue tests preserve every scalar/frame/host offset. Long-call drift and the remaining hardware/lifecycle gates remain roadmap work; no new hardware-capture or crash-loss claim is made.

Retention integration tests cover pending/purged guards, an actual confirmed-move subprocess crash/restart, no fabricated gaps, atomic note readback/export and a reader lease that blocks maintenance while snapshots remain responsive. Error/panic release is deterministic. All source files are generated in temporary roots; output factories explicitly prohibit device opening.

The follow-up backlog implementation uses per-track source/completion watermarks, two seconds of confirmed `>10s` lag and five seconds between the ordered partial/speaker/STT actions. Disabled stages stay off; absent stages are skipped, source discontinuities advance a generation and paused/stale input cannot escalate. Silent input still acknowledges completion. Disk suspension remains stronger. Inference queue drops/deferrals are separate from raw gaps and stored as bounded run-owned evidence by a separate writer, joined before sealing. Retention fingerprints and deletes the new FK descendant with its run.

A deliberately blocked synthetic STT/speaker fixture drives the real AppService relay/DSP path without audio devices. All stages fire in order while subsequent raw PCM still persists; decoding both FLAC tracks verifies manifest SHA and exact native scalars/frame totals with no raw gaps. Pure tests cover unequal source origins, quiet input completion, optional workers, source generations, pause/freshness, disk composition and bounded audit truncation. Publication tests retain finals after partial-only shedding, reject sealed audit mutation and show later refined runs do not inherit live warnings. Existing pause/AEC and source-packet clock regressions remain required.

Final audit publication always writes an explicit completion flag, including unchanged revisions. That flag describes ledger sealing, never transcript/label completeness. Expected missing and previously unsealed ledgers read back as explicitly unavailable after restart. Audit-only SQLite aborts prove recording/run sealing and exact native PCM survive while distinct DB and in-memory capture warnings are preserved. If the fallback warning write also fails, ready in-memory status remains visible and automatic processing is skipped until a later user action; no raw capture is failed for this optional ledger.

Final review remedies make ordinary pause notices specific to pause and carry initial disk suspension into provenance/audit expectations. An intentionally disabled inference graph no longer appears to have lost an expected ledger. These change inference notices only; raw recording, generations and gap evidence retain their existing semantics.

The meeting-detection integration adds a separate continuous observation clock: Apple `Instant` excludes sleep, so detector freshness uses `CLOCK_MONOTONIC_RAW` instead. Hardware/source timing keeps its existing raw-uptime namespace. Unavailable continuous time leaves detection unknown; injected clock/probe-duration checks prevent pre-suspend evidence from authorizing a post-wake capture. This does not change raw audio clocks or resource/retention cadence.
