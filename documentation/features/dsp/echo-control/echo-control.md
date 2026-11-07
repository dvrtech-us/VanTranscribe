# Echo control

`vt-dsp` provides local, inference-only acoustic echo cancellation. It uses the System track as the far-end reference and processes only the Mic track. Archived source audio is never rewritten. The processor starts with AEC bypassed and requires an explicit caller decision that the meeting application's actual loudspeaker route and the available System reference are suitable.

## User flow

1. The application records and archives its original tracks through the existing recording pipeline.
2. The application supplies its conservative output-route eligibility and a route epoch to `EchoProcessor::configure_at_time`. Unknown, headphone, mismatched, or unverified routes remain ineligible.
3. An inference worker submits timestamped mono packets through `accept`. Every returned packet goes to inference; Mic packets report whether AEC ran and provide a bypass reason when appropriate.
4. A route change, format change, or discontinuity drains pending speech before resetting acoustic state. A missing reference produces preserved Mic audio and an explicit reason.
5. At the end of inference, `flush` returns all remaining samples, including short replies. The processor is then finished.

The crate does not discover devices, record audio, play audio, change operating-system settings, download models, or make network requests. The application's route policy, status display, inference consumers, and recording controls are separate integration responsibilities.

## Technical flow

`EchoProcessor::new(DspConfig)` validates a jitter allowance of 0–200 ms; the default is 200 ms. `RouteEligibility` contains `eligible: bool`, `epoch: u64`, and an optional bounded reason. All fields default to an ineligible route. Changing eligibility, epoch, or reason drains the previous state before adopting the new route. `reset_at_time(reason, start_ms)` conservatively resets both track resamplers and AEC, returning queued audio first.

`accept(track, samples, rate, start_ms)` borrows the input. It accepts mono 8–192 kHz packets of at most two seconds, rejects non-finite samples and magnitudes above 16, and checks timestamp overflow. The caller downmixes channels. Each track must arrive chronologically; backwards or overlapping packets exceeding the 5 ms timestamp tolerance fail before changing state. A rate change or a discrepancy greater than 5 ms from the expected per-track time resets both tracks. Cross-track arrival order may differ.

Each native packet is processed internally in approximately 10 ms slices. The existing pinned sherpa-onnx `LinearResampler` supplies streaming sinc low-pass resampling when the source rate differs from 16 kHz. A native 16 kHz stream bypasses the resampler. Both output tracks always use 16 kHz mono, including when AEC is ineligible. Output timestamps retain the absolute source origin and sample counts rather than accumulating rounded packet durations; public timestamps have integer-millisecond precision.

Complete 160-sample frames enter Sonora's actual WebRTC AEC3 engine. Timestamped System samples are assembled across reference frames to cover the exact Mic interval. Reference frames are never invented. Sonora receives the matched render frame followed by the capture frame with zero additional declared stream delay; the engine estimates the acoustic echo path. Only the echo canceller is configured. Sonora noise suppression and automatic gain control remain disabled; the echo canceller's default high-pass behavior remains active.

Mic waits for a matching reference until its source-time deadline, within the configured allowance of at most 200 ms including frame formation. Pending reference and Mic queues are bounded; asymmetric track arrival cannot retain a whole meeting. A missing reference, a frame before the route epoch, a final partial frame, or a runtime AEC failure preserves the resampled Mic samples and records the reason. Missing reference or engine failure also resets AEC history. Partial frames are never padded into AEC. System output is the resampled source and is never echo-cancelled.

The caller must feed packets regularly to advance the source-time watermark. No wall-clock timer runs inside the crate; a stopped producer requires `flush` or an explicit reset to release held audio. Offline callers should interleave chronological source slices no longer than 20 ms, System before Mic at equal timestamps. Feeding one entire track first will intentionally exceed the reference history bound and cause bypass; it is not full-meeting buffering.

| Type or function | File | Responsibility |
| --- | --- | --- |
| `EchoProcessor` | `crates/vt-dsp/src/lib.rs` | Track clocks, bounded queues, route epochs, AEC, reset and terminal flush |
| `Converter` | `crates/vt-dsp/src/lib.rs` | Native-rate resampling, source origin and 160-sample framing |
| `DspConfig` / `RouteEligibility` | `crates/vt-dsp/src/lib.rs` | Explicit bounded timing and caller route policy |
| `ProcessedAudio` | `crates/vt-dsp/src/lib.rs` | Track, samples, rate, absolute start, AEC flag and reason |
| `new_engine` | `crates/vt-dsp/src/lib.rs` | Sonora AEC-only 16 kHz mono configuration |
| Regression tests | `crates/vt-dsp/src/tests.rs` | Real synthetic AEC, voice retention, clocks, bounds and immutable source checks |

## Integration and access

Run the processor on an inference worker, never the real-time capture callback. Use `vt_core::TrackKind` consistently on input and output. Forward every returned packet from `configure_at_time`, `reset_at_time`, `accept`, and `flush`; these methods may return previously held Mic speech. Route epochs must change whenever the acoustic path becomes uncertain. The global render mix alone does not establish which application or physical route produced audible sound; callers must not infer eligibility from the presence of System samples.

There are no HTTP routes, Tauri commands, permissions, authentication decisions, or OS capture APIs in this crate. The application owns access to recordings and devices. The default ineligible route cannot enable AEC through audio content. Reasons contain fixed diagnostics or bounded caller-provided route/reset text; the processor does not log audio.

## Database and SQL artifacts

The crate owns no tables, columns, migrations, or SQL artifacts. It does not persist processed audio, route metadata, credentials, or source files. Application-level inference-run provenance and raw-archive preservation remain caller responsibilities.

## Dependencies and distribution

`sonora` and `sonora-aec3` are pinned to 0.2.0. The engine identity is `AEC_ENGINE = "sonora-aec3@0.2.0"`. Sonora ports WebRTC M145 and requires Rust 1.91. Resampling reuses `sherpa-onnx = 2.0.1`, already used by transcription. The feature requires no external model download.

Bundle `crates/vt-dsp/resources/*`. `NOTICE.txt` inventories the BSD Sonora crates, WebRTC notice and patent grant, Ooura FFT, PFFFT/FFTPACK, RNNoise-derived code/weights, selected MIT dependency notices, and shared sherpa/ONNX Runtime notices. NS and AGC2 notices are included even though those features are disabled. The inspected locked Sonora subtree contains no identified GPL/LGPL dependency; this statement is not an audit of all application dependencies.

Primary upstream references: [Sonora source](https://github.com/dignifiedquire/sonora), [Sonora 0.2.0 crate documentation](https://docs.rs/sonora/0.2.0/sonora/), and [WebRTC M145 license](https://webrtc.googlesource.com/src/+/refs/branch-heads/7632/LICENSE). Exact notice sources are retained in `resources/NOTICE.txt`.

## Validation and remaining gates

Fifteen local tests cover actual AEC inference, delayed echo suppression, double-talk, a 250 ms near-end reply, missing reference, arbitrary cross-track order, different clock origins, route epochs, gaps, short tails, fractional-rate pending bounds, 8–192 kHz duration, sinc alias rejection and chunk equivalence, long asymmetric input, invalid input, and immutable caller audio. The deterministic 24-second synthetic test measured echo output/input RMS ratio 0.0096, double-talk near-signal gain 0.6226, and short-reply near-signal gain 0.4720 in the recorded development run. Test assertions allow variation and enforce suppression below 0.25 and near gains above 0.35/0.20 respectively.

These synthetic checks establish a working inference path and regression bounds. Real-meeting echo/double-talk corpus acceptance, hardware route changes, long-call behavior, and product-level transcription accuracy remain separate acceptance gates. No private meeting audio was used for these tests.
