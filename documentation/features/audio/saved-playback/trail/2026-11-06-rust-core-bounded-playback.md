# Rust-core bounded saved playback

- Date: 2026-11-06
- Feature: saved-playback
- Related code: `crates/vt-playback/`
- State: Working draft for the current unmerged increment

## Context

The approved design requires Rust-core playback and prohibits playback during recording to prevent self-capture. Saved audio already exists as native-rate per-track FLAC with explicit timeline gaps; decoding a complete meeting or creating a temporary WAV would violate bounded-memory/privacy expectations.

## Decisions

Use CPAL 0.18 for native output, selected only by explicit Play. Load metadata through read-only SQLite, read encoded audio through a root-scoped directory capability, validate chunk identity/SHA-256/FLAC bounds, and cache at most one decoded two-second chunk per track. Interpolate each independent track at the common output time. Queue separate track frames in a preallocated SPSC ring; apply current gains, stereo mix and clamping only in the output callback.

Pause/seek/stop drop the previous native stream and queue. Resuming creates a paused replacement at the consumed sample cursor, prefills verified frames, then starts it. This trades some control latency for reliable removal of old buffered audio and fresh default-route resolution. Underflow inserts output silence without moving the source cursor. Drop requests cancellation and joins the worker.

Keep the capture exclusion policy in the desktop service's shared lock. Expose an injectable sink for deterministic tests and a bounded-memory offline waveform API. No playback, smoke sound or user recording access is automatic.

## Alternatives considered

- WebView `<audio>`: rejected because it weakens the approved self-capture/output boundary.
- Full-meeting decode or plaintext WAV: rejected for memory, privacy and temporary-file cleanup concerns.
- Reuse a queue across seeks: rejected because old queued/hardware frames can leak after a timeline change.
- Advance from wall time during underruns: rejected because it can silently skip speech and diverge from source citations.

## Consequences

- Positive: timeline gaps, track mixing, source cursor, cancellation and path/integrity behavior can be tested without speakers or private recordings.
- Negative: control changes rebuild native output; physical latency depends on the device; linear resampling has lower fidelity than a dedicated anti-aliasing converter. Waveforms scan source audio and require CPU/I/O proportional to duration, despite bounded memory.
