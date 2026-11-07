# Refined speaker context and per-run provenance

Date: 2026-11-07. This continues the accepted full design and the [live-model/backlog checkpoint](2026-11-07-live-models-and-backlog-validation.md). The owner accepted the independent Anthropic and Codex design reviews; this implementation does not restart those reviews or narrow M0–M5.

## Problem and intended behavior

The existing native pyannote segmentation model expects ten seconds of audio. Before this increment both application phases provided five-second source windows, which the native wrapper padded. The earlier [matched context audit](../../features/quality/production-inference/measurements/2026-11-07-config-model-context-audit.json) found lower offline DER using ten seconds while keeping the ownership stride, Unknown/short/overlap rules, embedding/clustering thresholds and user constraints. That source-copy experiment did not activate product behavior.

This increment integrates the measured ten-second source context for saved refinement while retaining live five-second context and 2.5-second ownership stride. The source stream remains bounded and independently clocked per track. Final tails and discontinuities still clip native output to available audio. Labels remain experimental; a longer context is not a calibrated speaker-identity system.

Actual engine results carry typed segmentation-context metadata. The session stores it with the run's speaker state, in the same publication transaction. Historical absent metadata remains unknown; reusing previous labels copies their context rather than inferring today's settings from the new run kind. No model, speaker preference or installed user recording is changed by the evidence runner.

## Production-path observations

The actual product engine reproduces the earlier context-only experiment on the unchanged public AMI development and adjacent held-out windows. The comparison keeps the 2.5-second normal ownership stride, embedding and clustering policy, user constraints and unchanged PCM. Finalized Live-phase five-second clustering is a retrospective control, distinct from the actual emitted live assignments.

| Fixed legacy segment-reference protocol | Finalized five-second control DER | Refined ten-second DER |
| --- | ---: | ---: |
| Development [0,300) seconds | 54.54% | 46.13% |
| Held-out window [300,600) seconds | 68.81% | 50.70% |

The improvement is not uniform speaker discrimination: on the development window missed reference speech falls from 102.41 to 82.87 seconds while confusion rises from 3.05 to 6.54 seconds; scored hypothesis speakers increase from four to six. On the held-out window misses fall from 149.35 to 125.72 seconds and confusion from 21.27 to 0.38 seconds, with scored speakers decreasing from six to three.

The separate secondary only-words protocol gives 47.53% → 38.02% and 65.31% → 43.73%. These scores use the same hypotheses with a different explicit reference protocol; they do not replace the original segment-reference results. Actual emitted live assignments score 56.74%/77.51% under the legacy protocol and are not relabelled as offline results. Four assignment projections match the earlier five- and ten-second artifacts exactly, excluding only the explicit phase provenance string.

The refined runs still contain 19/13 distinct assigned IDs when provisional and uncertain turn identities are included; only 6/3 survive the unchanged Unknown/provisional masking into scored RTTM. The held-out only-words protocol has three lexical speakers. No in-house four-speaker, DER≤20%, independent-meeting or gallery-of-50 acceptance is claimed. The clean public control produces a bounded single assigned voice in both phases; it has no scored speaker reference.

Constructors were about 159–176 ms. Processing the two 300-second windows took 10.91/13.40 seconds for five-second context and 16.63/17.39 seconds for ten-second context. The maximum sampled process RSS was about 484.5 MB. These are sequential diagnostic observations with possible warm caches, not sustained hardware/latency or peak-memory acceptance. The longer context costs more computation and is confined to saved refinement.

Evidence: [actual production context comparison](../../features/quality/production-inference/measurements/2026-11-07-production-diarization-context.json). Original measurement artifacts remain unchanged.

## Verification status

- Core: 25 ordinary tests and five opt-in native cases passed, including a pre-change live-output baseline, refined boundary/short-tail/gap/rate behavior, the four-speaker smoke fixture, resampling and correction handling. The old correction fixture grew from eight to sixteen seconds to provide multiple clean ownership turns under the new context; its assertions remain intact.
- Storage: three new context tests passed, including legacy/idempotent migration, exact save/copy/restart, finalized API rejection and transactional rollback. The wider speaker filter passed 33 tests and retention passed 17. Strict session Clippy passed.
- The optional browser API type passes Svelte/TypeScript with zero errors/warnings. No browser interaction or visual flow changed in this increment.
- All four opt-in public AppService cases passed: Small and Standard live workers retain five-second speaker context, exact 106,000 PCM24 frames/track and 36 words; normal and pending-correction saved refinements store ten-second context and preserve source audio/history/recognized speech. Native app integration uses temporary data and fake/saved capture only; no device or private source is opened.
- Final locked workspace/all-target/all-feature gate passes 309 Rust tests with zero failures and 20 intentional opt-in/child cases ignored. Formatting and strict all-target Clippy pass. The first stopped gate is retained as failure evidence rather than erased; the passing rerun includes the lock remedy.
- The signed release passes deep strict signature verification and opens its bundled frontend in 1.089 seconds with isolated empty data. It initializes the nullable speaker-context column and creates no meetings/capture. Bundle size is 48,127,542 bytes (45.90 MiB). This single possibly warm-cache launch is not a general startup-performance claim.
- Final Opus implementation review found no blockers. It confirmed clipped source ownership, gap/rate flushes, actual phase wiring, nullable/idempotent/atomic metadata, retained source/experimental constraints and the descriptor-alias regression. The quality decomposition above addresses its reporting note. The idle installed app was gracefully replaced after that review. Its prior bundle and current pre-migration database were retained. The new installed frontend connected and remains running with capture/processing inactive; Standard selection is preserved and the new nullable column is present. Zero meetings were created by this update.

Review assessment: both phases deliberately share a fixed 2.5-second stride and the geometry test pins that contract; a future per-phase stride requires an engine/test change. Full result replacement with absent context deliberately yields unknown metadata instead of retaining a potentially stale context from different output; actual native publishers always supply their phase, while warning-only updates preserve known metadata. A post-commit unlock error still propagates: audio/status may already be durably finalized, and the session can report failure/recovery rather than silently claim a successful lock handoff. That rare error-classification limitation is not covered by injected syscall-failure tests. No Drop semantics or recovery retry was added.

## Successful recording-lock handoff

The first full workspace gate stopped at the existing generated recording round-trip test: immediate recovery after successful `finish()` returned the recording file-lock Busy error. It was not a SQLite error and did not touch an installed recording. A focused investigation identified duplicated/inherited descriptor retention as a reproducible lock mechanism; the particular parallel spawn behind the initial event is not proven. The regression retains a safe `File::try_clone` descriptor across the consumed writer's successful finish. It failed with Busy before the remedy and verified all three independently decoded chunks after explicit unlock. Active recovery remains Busy while the writer owns the lock.

`Recording::finish` now explicitly unlocks after the committed final status and `writable=false`, with no recorder operation afterward. It does not alter Drop, delay/retry recovery or weaken active-writer exclusion. The full recorder suite passes 23 tests with two intentional child helpers ignored, including SIGKILL publication recovery and 20 fresh-root rounds × 8 concurrent writers. Strict Clippy passes. [Rust's File locking contract](https://doc.rust-lang.org/std/fs/struct.File.html#method.lock) documents why merely closing one of several shared descriptors does not release their common lock. The [durable recording feature](../../features/recording/durable-recording/durable-recording.md) and its original unmerged trail record this narrow remedy.

## Remaining scope

The full accepted roadmap remains active. Calibrated English/channel speaker thresholds, global constrained clustering beyond the current bounded seed hierarchy, in-house four-speaker DER, gallery-of-50 identification, independent-meeting calibration, physical capture and long-call gates, encrypted speaker memory/consent/expiry, complete engine packs, meeting detection and Windows remain unproven or unimplemented. Adjacent windows from one public meeting cannot establish those gates.

The next [meeting-detection checkpoint](2026-11-07-meeting-detection-validation.md) implements the remaining opt-in app reminders and explicit per-app recording rules.
