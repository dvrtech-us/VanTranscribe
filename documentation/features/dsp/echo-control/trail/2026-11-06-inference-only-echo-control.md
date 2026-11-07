# Inference-only echo control with bounded source clocks

- Date: 2026-11-06
- Feature: echo-control
- Related code: `crates/vt-dsp/src/lib.rs`, `crates/vt-dsp/src/tests.rs`, `crates/vt-dsp/resources/`

## Context

The approved audio design calls for actual local WebRTC AEC3 on Mic inference, using the System track as reference while preserving raw recordings. Capture tracks can have different formats, origins, arrival order, and gaps. A global render mix does not by itself prove that the microphone is hearing the meeting application's speakers.

## Decisions

Use exact Sonora/AEC3 0.2.0, a BSD Rust WebRTC M145 port with MSRV 1.91. Configure AEC only at 16 kHz mono; leave NS and AGC disabled. Reuse the existing pinned sherpa-onnx sinc resampler so inference conversion has the same tested low-pass and chunk behavior as transcription.

Require explicit caller eligibility and route epochs. Start in bypass. Match System reference by absolute sample clock rather than arrival position. Use 10 ms frames and a configurable 0–200 ms source-time allowance, small internal native slices, and bounded queues. Require chronological interleaving for offline processing instead of retaining an entire source track.

Drain speech on route/format/gap reset. Preserve Mic on absent reference, engine failure, and incomplete terminal frames. Do not zero-pad short replies into the echo engine. Return all drained output to the caller. Keep device policy, capture, archives, persistence, and inference consumers outside this crate.

Include licenses for all compiled Sonora components, including disabled NS/AGC2 code, and notices inherited from WebRTC/Ooura/PFFFT/RNNoise. Source and license details are recorded in `resources/NOTICE.txt`.

## Alternatives considered

- AEC without verified routing: rejected because the global System mix may be unrelated to the acoustic path and can suppress genuine near-end speech.
- Full-meeting reference buffering: rejected because live memory and latency must remain bounded. Offline chronological interleaving uses the same engine.
- A new Rubato conversion path: the existing verified sherpa sinc path was selected to avoid separate resampler semantics and another dependency.
- OS voice processing as the sole path: not used here; routing/capture capabilities are separate, and saved two-track inference needs the same explicit reference engine.
- Mock-only DSP validation: rejected. Tests invoke the actual Sonora AEC3 engine on deterministic delayed synthetic echo with double-talk and a short reply.

## Consequences

- Positive: raw tracks stay immutable; inference uses real local AEC with explicit provenance and conservative failure behavior.
- Positive: live and offline callers share clocks, bounds, resampling, and short-speech preservation.
- Negative: source-time waiting requires producer progress or explicit reset/flush; this library does not run a wall-clock timer.
- Negative: the application must distinguish a verified meeting loudspeaker route from an unrelated global render mix and must forward all drain outputs.
- Validation: 15 tests pass, covering actual echo suppression/near-end retention, missing reference, resets, clocks, rates, queue bounds, sinc properties, and source immutability. Clippy runs with warnings denied.
- Remaining acceptance: real-meeting corpus quality, hardware route transitions, long-call stability, and end-to-end transcription effects are not established by synthetic DSP tests.
