# Live model choices, quiet-speech VAD and inference backlog

Date: 2026-11-07. This continues the accepted full design and the prior [retention/health checkpoint](2026-11-07-retention-health-and-corpus-validation.md). The previous turn installed that reviewed increment; this turn improves the measured speech failure and implements the remaining staged overload policy. No preimplementation design review was repeated. The full goal remains active.

## Current implemented behavior

- Two independently pinned English live acoustic models share the CPU streaming engine: June 26 Compact and June 21 Standard. Exact official archive/file bytes, SHA-256, licence/conversion metadata and safe explicit installation apply per catalog. Core `ModelManager::new` still selects Compact for compatibility.
- Fresh desktop setups choose Standard. Existing Compact directories and explicit saved choices remain respected. Selection is idle-only local metadata; it never downloads or removes weights. Explicit setup/cancel/verify-repair controls remain available. An operator can import an already cached official archive and pinned Silero file through the same checked installer.
- Desktop Standard requests live Silero threshold 0.2; Compact retains 0.5. Core legacy VAD defaults remain 0.5. Disabling/unavailable VAD runs the chosen model ungated. Input amplitude, resampling, context, endpointing and raw audio are unchanged. The selected manager is checked before the native full-hash constructor, including ungated fallback.
- Live creation and replay publication atomically retain requested model identity, pinned files/archive and VAD options with their new version. Earlier versions are not relabelled. STT availability is pinned at capture start; completing a download does not silently activate it mid-call. Shutdown guards prevent late capture/setup/processing starts.
- Backlog compares accepted raw-source endpoints with completed-input watermarks for configured DSP/STT/speaker workers, independently of silence/VAD text output. More than ten seconds of lag sustained for two monotonic seconds triggers ordered shedding: partials, speaker inference, live STT; at least five seconds separate continued overload stages. Absent stages skip; shed stages remain off for that capture. Pauses, gaps, source epochs and in-flight generations are guarded.
- Raw capture buffering/publication precedes inference enqueue and is never gated by this policy. Queue drops and unfinished contexts are recorded as inference evidence, distinct from native recording gaps. Bounded per-run audit publication runs outside the capture thread.
- Ledger `complete` means final audit publication, not complete transcription. Periodic reports are incomplete; final publication is mandatory even at the same revision. Missing expected or stale sealed reports explicitly disclose uncertainty without inventing source ranges. Final audit failures append warnings while preserving source warnings/raw audio. If warning persistence also fails, a ready Current retains the in-memory notice, skips automatic processing and still permits a new recording.
- Live notices and saved run-specific history explain reduced processing and source availability. A successful later refinement has its own history rather than inheriting another version's ledger.

Feature ownership: [live English](../../features/transcription/live-english/live-english.md), [recording health](../../features/capture/recording-health/recording-health.md), [desktop session](../../features/app/desktop-session/desktop-session.md), [public inference](../../features/quality/production-inference/production-inference.md). Each existing unmerged feature trail was updated in place and duplicate draft entries were consolidated.

## Production-path quality evidence

The original AMI first-300 baseline remains preserved. New comparisons execute actual `StreamingRecognizer` APIs, including production VAD and 20 ms input packets, against pinned unchanged PCM from the same development and next-300-second windows. No input gain is applied.

| Model / gate | Development WER | Held-out-window WER |
|---|---:|---:|
| Compact / 0.5 | 100.00% | 97.93% |
| Standard / ungated | 52.33% | 50.19% |
| Standard / 0.5 | 68.87% | 63.52% |
| Standard / 0.35 | 62.26% | 56.27% |
| Standard / 0.2 | 53.31% | 48.25% |

Default versus explicit threshold-0.5 events, finality, text and word-time projections match exactly on both AMI windows and the public clean control. Every tested clean-control configuration retains the complete expected sentence. The 60-second generated control (30 seconds silence, 30 seconds quiet coloured noise) produces zero ASR events/words at all three thresholds. This supports the selected configuration under these conditions, not universal noise/speech calibration.

The Standard/0.2 standalone phases have constructors around 0.676/0.703 seconds, real-time factors 0.025/0.027 and sampled RSS up to about 705 MB. These are sequential diagnostic observations with possible warm OS caches; they do not establish sustained two-track reference-machine CPU/RSS/battery or UI latency. Paced control delivery delays exclude capture and rendering. Adjacent windows of one AMI recording are not independent meetings; overlap serial word order is also ambiguous. Separate in-house diarization/gallery/hardware acceptance remains open.

Evidence: [catalog/VAD comparison](../../features/quality/production-inference/measurements/2026-11-07-production-catalog-vad.json), [threshold comparison](../../features/quality/production-inference/measurements/2026-11-07-production-vad-thresholds.json), [public live-worker profiles](../../features/quality/production-inference/measurements/2026-11-07-public-live-worker-profiles.json). Audio, weights and hypotheses stay in the temporary public-evidence cache. No private recording, OS capture/output or cloud service was used.

## Verification

- Core STT: 26 ordinary tests passed; six opt-in cases remain explicit. Separately, actual cached Standard import/native construction passed, and real threshold/default-parity/track/gap/time-bound tests passed. Model/option strict Clippy passed.
- Full workspace/all-target/all-feature checks: 302 Rust tests passed, zero failed, 18 explicit fixture/child cases ignored; formatting and strict Clippy passed. This includes atomic engine metadata/replay rollback, selection persistence/gating, archive safety, all backlog stages/generations and audit-failure readback.
- Backlog focused tests: 14 passed, including the real AppService/DSP/recording graph with injected workers, exact decoded FLAC frames/scalars/hashes, delayed in-flight generations, disk composition, raw preservation and missing/stale audit status. SQLite failures are injected; no disk is filled.
- Actual public Small/0.5 and Standard/0.2 live workers: both passed through a paced fake helper, real VAD/STT/speaker/DSP workers and durable recording. Each preserved 106,000 exact PCM24 frames on each track, produced 36 words/four speaker-turn results, and recorded zero shedding/gaps. The final rerun additionally asserts the ledger is complete. This is not a four-person identity or physical capture test.
- Browser: all 43 synthetic IPC flows passed, including selected-model persistence/error/cancel/repair controls, active-work gating, per-version notices and a ready post-recording audit warning that does not block New recording. Svelte/TypeScript reports zero errors/warnings.
- Final Opus implementation review found no blockers. Its three requested remedies were applied: pause-specific context notices, initial disk suspension in audit expectations, and in-app recovery for malformed/future saved model choices without silent fallback activation. The focused Opus re-review found no blockers or regressions. Eight focused selection/provenance tests and the invalid-choice browser flow pass.
- The reviewed Standard archive and pinned standalone VAD were imported offline through the verified installer; Compact remains installed. After a fresh idle-state check, the normal desktop lock and typed selection command configured Standard. The previous app and pre-configuration database were retained for recovery.
- The release bundle passes deep strict code-signature verification, contains 48,141,526 bytes (45.91 MiB), and opens the embedded frontend in 3.006 seconds in an isolated empty-data startup check. The installed replacement also connected its frontend and remains running with capture inactive. No OS audio input/output was started; this verifies launch/setup rather than physical recording quality. Release UI assets are bundled; the local development/test server is not a runtime requirement.

Two nonblocking review limits remain explicit: inactive candidate model pins still appear alongside `enabled=false`, a null requested model and a selection warning in invalid-choice provenance; a disk-state change between start preflight and worker creation can still produce an expected-but-unavailable audit notice. Model replacement rollback is best effort if both candidate publication and restoration renames fail; it is not claimed as a crash-atomic replacement.

## Remaining scope

Full application Lite/Balanced/Accurate packs, broad noise/dialect and meeting quality calibration, constrained diarization acceptance, staged-policy hardware soak, physical capture/route/permission/sleep/drift/crash-loss gates, encrypted voiceprints/consent/expiry/gallery testing, meeting detection, richer exports and Windows remain open. Improved public WER and passing software checks do not complete the full app objective or narrow the accepted design.

The next [refined speaker-context checkpoint](2026-11-07-refined-speaker-context-validation.md) integrates the previously measured offline context improvement and records exact settings with speaker-run results.
