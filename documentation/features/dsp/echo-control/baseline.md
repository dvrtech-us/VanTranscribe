# Echo control baseline

## Pipeline invariants

- Raw caller audio is borrowed immutably; echo control never writes an archive.
- A new processor bypasses AEC until the caller explicitly supplies an eligible route.
- System is the far-end reference and remains unaffected by AEC; only Mic may receive AEC.
- Every returned packet is 16 kHz mono and retains its absolute source clock at integer-millisecond public precision.
- Complete AEC frames contain exactly 160 samples. Incomplete final frames bypass without padding or discarded speech.
- AEC requires complete timestamp-matching System reference coverage for each Mic frame.
- Missing reference preserves the resampled Mic signal and reports a reason.
- Pending audio is bounded by the configured source-time allowance, at most 200 ms, with a minimum one-frame storage unit. Caller progress, reset, or flush advances delivery; the crate owns no wall-clock timer.
- Per-track order is chronological. Cross-track arrival may differ; an offline source interleaves at most 20 ms slices to keep reference coverage within the bound.
- A route epoch, timing discontinuity, or rate change drains old audio before resetting both resamplers and AEC history.
- Consumers forward reset/configuration return values as well as normal accept/flush outputs.
- A terminal flush returns every remaining sample and prevents subsequent use.
- Noise suppression and automatic gain control stay disabled.

## Flag rules

| Condition | Severity | Blocks inference? | Behavior |
| --- | --- | --- | --- |
| Ineligible or unknown route | Status | No | Mic bypass with supplied/default reason |
| Missing reference within jitter limit | Warning | No | Mic bypass; AEC history reset |
| Route, rate, or timing changes | Warning/status | No | Drain held samples, reset, continue new epoch |
| Frame predates route epoch | Warning | No | Mic bypass |
| Final partial frame | Status | No | Preserve exact samples without AEC or padding |
| AEC processing failure/non-finite result | Warning | No | Preserve original resampled Mic and reset engine |
| Invalid rate, excessive packet, non-finite/extreme input, backward time, overflow | Error | Rejects packet | Return `DspError`; caller decides run failure policy |
| Resampler failure | Error | Yes for operation | Return `DspError::Resampler`; never fabricate audio |
| Operation after terminal flush | Error | Yes | Reject operation |

## Side effects in order

1. Validate the packet or configuration before mutation where validation is possible.
2. Drain old speech on an epoch/format discontinuity.
3. Resample into new inference buffers and assemble bounded frames.
4. Return unchanged resampled System and eligible AEC-processed or bypassed Mic.
5. Return reason/provenance fields for the application to display or retain.

There is no network access, device configuration, archive mutation, database write, playback, model download, or transcript logging.

## Configuration defaults

| Setting | Value |
| --- | --- |
| Engine | Exact `sonora-aec3@0.2.0`, via `sonora = 0.2.0` |
| Rust MSRV | 1.91 |
| `max_jitter_ms` | 200; valid 0–200 |
| Route | Ineligible, epoch 0, no caller reason |
| Inference format | 16 kHz mono |
| AEC frame | 10 ms / 160 samples |
| Accepted native format | Mono, 8–192 kHz, at most 2 seconds per packet |
| Timestamp tolerance | 5 ms within a track |
| Maximum absolute input sample | 16; all samples finite |
| Maximum route/reset reason | 512 bytes |
| Resampler | sherpa-onnx 2.0.1 streaming sinc; bypass at native 16 kHz |
| NS / AGC | Disabled |

## Access control

The caller owns recording authorization, route eligibility, source-file access, and worker scheduling. `vt-dsp` has no authentication surface and cannot enable itself from transcript/audio contents. No route is presumed eligible from System-track availability alone. Runtime diagnostics never include audio samples.
