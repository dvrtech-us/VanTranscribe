# Capture diagnostics, one rebuild per device change and coalesced overflow gaps

- Date: 2026-11-10
- Feature: macos-capture
- Related code: native/macos/CaptureHelper.swift (`ConfigurationChangeGate`, `OverflowCoalescer`, diagnostic emitters), native/macos/AudioRing.c, native/macos/AudioRing.h, native/macos/AudioRingTests.c, crates/vt-capture/src/lib.rs (`CaptureDiagnostic`, `InputTransport`, `read_event`, `CaptureEvent::{Gap, Status}`), crates/vt-session/src/capture_health.rs, native/macos/README.md

## Context

A real 95.5-minute in-person recording on 2026-11-10 (single microphone) recorded audio throughout while the live transcript's last segment ended at 47:41. Ten capture `overflow` gaps occurred at 47:31-47:36 (about 2.1 s lost). Two `device_change` outages lost 47.3 s (at 86:15) and 36.4 s (at 94:46), each followed by a second short gap. Clipped samples rose from 103 in the first 86 minutes to 3196 in the next 7.7 minutes.

Nothing recorded where the outage time went. A change of device could be slow in the helper (a Core Audio call that blocks), slow in the app (the consumer loop was late) or a repeated rebuild. The second short gap after each outage suggested the helper rebuilt twice for one change: its baseline fingerprint was read mid-transition, so the settled values differed from it. A burst of overflow runs produced one gap and one live-inference reset per run.

Also, an unknown event type from the helper ended the recording, so adding any new event was a compatibility break.

## Decisions

- Diagnostics are `status` events carrying a typed `diagnostic` object, published by capture sessions only (never by read-only probes whose stdout is one JSON document). An older parent reads them as plain status. The helper advertises capability `capture_diagnostics` in `capture_capabilities`; `helper_protocol` stays 4. Kinds: `heartbeat` (about once per second: IOProc callback count, control ticks, longest stdout write since the last heartbeat, queued ring packets), `rebuild` (reason, trigger, previous and new epoch, trigger age, fingerprint, stop and start durations, result), `slow_call` (any Core Audio call that blocks the control thread for more than 100 ms, API name and optional property selector code, plus phase), `epoch` (sample rate, channels, quantum, input transport type) and `settled`.
- Unknown event types and unknown or malformed diagnostics are ignored instead of ending the recording (`read_event` maps an unknown `type` to an ignorable status; a malformed event of a known type still fails). Previously an unknown event type was fatal.
- Privacy exception, owner-approved: the capture baseline forbids persisting a device UID or name. The `epoch` diagnostic carries the input TRANSPORT TYPE only, one of built_in, bluetooth, usb, aggregate, virtual, other or unknown, and it is persisted in the health report. The device identity, name and UID still never leave the helper. Slow-call names are validated to be API names (alphanumerics, `_`, `:`, `#`, at most 64 characters), otherwise replaced by `other`.
- One rebuild per device change: the baseline fingerprint is fixed only after the new stream settles (stable for 75 ms and at least 1 s, forced after 10 s). Changes to what the running ring was opened for are never absorbed; a consequence of the helper's own start (for example a Bluetooth profile change) is. `lastEnd` advances so a further rebuild before new audio continues from the earlier gap instead of re-reporting the old end.
- Overflow drop runs separated by less than 1 s of delivered audio are coalesced into one `overflow` gap whose length is dropped frames plus discarded frames. Audio between runs is discarded and counted, and following audio may be held up to 1 s (bounded to two seconds of frames). The gap event carries `dropped_frames`, `discarded_frames` and `overflow_runs`. The session therefore sees one gap and one reset instead of dozens.
- The C ring gains lock-free `vt_count_callback` / `vt_callback_count`, `vt_ring_queued` and rebuild-request source flags (`VT_REBUILD_NOTIFICATION`, `_POLL`, `_WAKE`) so the helper can report them without allocation in the IOProc.
- The session derives a capture-stall state from heartbeats (see the recording-health trail entry of the same date) and persists bounded lists in the health report.

## Alternatives Considered

- Kill and respawn the helper on a stall (watchdog). Rejected: a respawned helper restarts its epoch and clock, which the recorder rejects, and "no audio for N seconds" also fires when the app is the slow side.
- Add a new top-level event type. Rejected: an older parent treated unknown types as fatal; carrying diagnostics inside `status` is safe for every parent.
- Persist the device name or UID for diagnosis. Rejected by the privacy baseline; transport type is the owner-approved minimum.
- Move the recording loop's fsync work and helper pipe writes to other threads. Deferred until the new timings exist.

## Consequences

- Positive: the next long device-change outage can be attributed to a blocked call, a slow helper or a slow consumer, and a device change should no longer cost a second short gap.
- Negative: no watchdog. A Core Audio call that never returns is visible only as missing heartbeats. The long device-change outages are NOT fixed, only measurable now. Coalescing discards a little delivered audio between runs in exchange for fewer resets.
- Not verified: nothing here has been exercised by a real device change or a real recording yet. The native self-tests and Rust tests use synthetic data; the settle logic and coalescer are covered by pure self-tests.
