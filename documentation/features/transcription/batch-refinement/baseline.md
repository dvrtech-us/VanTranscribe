# Batch refinement behavior baseline

- Offline inference uses pinned Parakeet v3 INT8 with the CPU provider; it never silently substitutes the live model.
- Each track owns its VAD, input clock, sinc resampler and waveform state.
- Native VAD calls contain at most 512 samples. Hard epochs contain at most 29 seconds; offline decoding adds 200ms of context only. Full meetings are never concatenated in inference memory.
- Epochs do not replay audio. A forced cut can affect words crossing that cut.
- Input contains at most two seconds of finite mono samples at 8–192kHz per call.
- Gaps/overlaps over 5ms and sample-rate changes finalize the prior clock. Explicit resets affect one track only.
- Word start/end estimates derive from acoustic token timestamps/durations or the 80ms encoder-frame extent; sentence duration is never divided uniformly into word times.
- Segment/word times stay within real input bounds; decoder padding never extends meeting time.
- A second `finish` returns no additional segments; accepting after finish fails.
- `installed` performs no large weight hash pass. Full verification precedes native model construction.

| Condition | Severity | Blocks? |
|---|---|---|
| Missing/corrupt/pin-mismatched model | Error | Yes, refinement |
| Invalid input or token alignment | Error | Yes, current call |
| Explicit model setup cancellation | Cancelled | Yes, publication |
| Concurrent model installation | Busy | Yes, second installer |
| Unexpected download host, archive above 1 GiB, or extracted weights outside the accepted catalogs | Error | Yes, publication |

Setup side effects occur in order: take lock; create private temporary work directory; verified downloads; safe selected extraction; VAD copy and full manifest; full file verification; sync candidate; atomic replacement with rollback on rename failure; sync model root. Failure/cancellation leaves temporary work automatically removed and does not publish a partial model.

Defaults: `REFINEMENT_MODEL_ID=sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8`, Parakeet CC-BY-4.0, Silero MIT, VAD 643,854 bytes, Parakeet archive safety cap 1 GiB. The compressed archive size is not an acceptance pin. Accepted weight catalogs are the current official files and the pre-2026-11-07 files. Two ASR threads, one thread per VAD, 0.5 VAD threshold, 0.2s minimum speech, 0.35s minimum silence, 29s hard epoch. Only explicit setup accesses the network; model and meeting authorization remain the session/UI layer's responsibility.
- A speaker-separation capacity limit (turns, embeddings or clusters) does not block publication of the refined transcript. Nothing from the failed separation is saved; previous speaker annotations are carried forward and the run's speaker warnings state the limit. Other separation errors still fail the refinement.
