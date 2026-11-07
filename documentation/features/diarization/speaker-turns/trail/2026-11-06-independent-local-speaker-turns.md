# Independent local turns with bounded experimental clustering

- Date: 2026-11-06
- Feature: speaker-turns
- Related code: `crates/vt-diarize/`
- State: Working draft for this unmerged feature, consolidated through 2026-11-07

## Context

Approved architecture §7 requires audio-derived speaker changes independent of STT segment length, explicit short/overlap uncertainty, independent track clusters and durable user constraints. The native Rust API exposes a combined offline diarizer and a separate embedding extractor, but not raw segmentation matrices or embeddings from that combined pipeline.

## Decisions

Use sherpa's one-window segmentation path with phase-specific source context: Live preserves five seconds, internally padded to the canonical ten-second input; Refined supplies ten seconds of real source for full windows. The steady ownership stride remains2.5seconds. On finish/gap/format change, a shorter remaining tail is padded and clipped and owns all remaining actual frames, potentially more than one stride. Typed Phase::segmentation_context reports configured maximum source window, stride and native window; actual engine results carry Some(context), while legacy and synthetic metadata remains None and retained annotations copy old context unchanged. Re-extract clean turn embeddings independently and implement meeting-local clustering/constraints in Rust. Clip all acoustic timestamps to real source samples and owned window intervals. Keep short turns Unknown and overlap explicit. Remote microphone Me remains a channel prior.

Retain capped exemplar sets, provisional speech thresholds and bounded metadata/replay storage. Resolve durable temporal anchors to current run-local turns before replaying constrained clustering; missing, tied, under-covered, overlapping or contradictory anchors produce per-constraint review metadata and Unknown affected regions while unrelated transcript processing continues. Constrained agglomeration evaluates a bounded exemplar-sample silhouette hierarchy with a soft live-count prior. No unnamed vector is serialized or persisted. Explicit named enrollment uses a separate bounded `VoiceEmbedder` to recompute retained clean source spans under session-owned participant consent, generation/source checks and encrypted storage. It verifies pinned weights and has a distinct version for its resampling/normalization contract and unavailable identification calibration; ordinary diarization does not consult that store. Defensive replay failures identify implicated turn endpoints and isolate only the connected correction component, with bounded retries; unrelated corrections remain applied. Valid dependent relationships wait under the original failed-source review, preserving a truthful undo target instead of suggesting that the valid neighbor should be removed. Must-link closure is computed before seed assignment so transitive user constraints cannot be defeated by greedy acoustic seeds. A remote microphone cannot-link overrides the Me prior when present at construction; a late correction without microphone embeddings masks its regions and requests a fresh refinement rather than aborting the transcript.

Pin actual official release hashes and byte lengths. Preserve the MIT CNRS notice from the segmentation archive and Apache-2.0 model-card evidence for ERes2Net. Model acquisition is explicit; no publisher gated agreement is accepted. The manifest declares the profile experimental_uncalibrated, records original provenance and notes that release assets do not identify original weight/conversion revisions.

## Alternatives considered

- Embedding an entire STT final was rejected because it merges independent voices and violates §7.
- The native combined global clustering was rejected as the identity authority because it does not expose our must/cannot constraints.
- A separate direct ONNX segmentation binding could avoid duplicate native embedding sessions, but introduces another runtime boundary. The existing pinned sherpa API supports a working local path now; direct segmentation remains an optimization/quality option.
- Pretending the canonical model natively uses five seconds or calling placeholder cosine thresholds calibrated was rejected. Live five-second padding remains explicit; refined full context now matches the native ten-second input. No existing threshold or clustering policy is changed.
- Inferring context from a historical run kind was rejected: older refined runs may have used five-second context, so only actual engine provenance can claim the new ten-second configuration.

## Consequences

- Positive: actual local model inference, independent timestamps, bounded memory, explicit uncertainty and temporal user-constraint replay are testable without private recordings.
- Negative: live/terminal padding, larger refined context and capped seed clustering retain corpus-dependent accuracy effects; unconstrained wrong seeds cannot always split automatically. Native construction keeps two embedding-model sessions. Full identity reconciliation, calibrated English/channel DER and long-call resource acceptance remain open.
- Validation: synthetic tests cover clustering and temporal/word boundaries; actual official public multi-speaker audio verifies segmentation/embedding, separate modes and clock bounds. Native regressions also verify remote microphone prior override and successful finalization with late/unmatched correction reviews. Public demonstration audio is temporary research input only and is not redistributed or treated as a calibrated evaluation corpus.

## Phase context evidence and validation

The existing matched ten-second context audit used the same2.5second ownership and product Unknown/overlap/constraint policies and motivated this narrow refined-only change. It is diagnostic evidence, not calibrated DER or full M2b acceptance. Native regression captures live pre-change times/labels/flags from the pinned public short control and confirms parity after implementation. Refined checks cover ten-second wait, real-source ownership, long tail/short flush, gaps, rate changes, bounded storage and context provenance. Original public four-speaker/48kHz/gap/user-constraint regressions are retained; the constraint input expands from8to16seconds to supply two clean owned turns under the larger context without weakening its assertions. No installed models, app settings, user audio, clustering thresholds or vector-persistence policy are changed.
