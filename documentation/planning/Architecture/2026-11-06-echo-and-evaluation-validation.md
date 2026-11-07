# Echo Control, Bleed Review and Evaluation Checkpoint

- Date: 2026-11-06 (Central)
- Objective: have a working app
- Goal status: active; the full approved roadmap remains in scope
- Previous checkpoint: [playback and speaker runtime](2026-11-06-playback-and-speaker-validation.md)
- The existing Claude Opus/Codex design reviews remain accepted. This checkpoint records implementation and its final higher-model review, without another plan-approval gate.

## Current behavior

The desktop now connects pinned Sonora AEC3 to live transcription and independent speaker work, and replays the same inference-only path during saved transcription/refinement. Native source-rate PCM24 FLAC and playback remain original. Positive capture-time built-in-speaker, jack, terminal, per-process output-device and System-reference evidence is required. Headphones, unknown/mixed/external routes and incomplete saved route histories bypass AEC. Metadata contains no process/device names or identifiers. User preference and eligibility reasons remain distinct in run provenance.

`SavedAudioReader` shares playback's capability-root path/SHA/format/frame verification and holds at most one decoded native chunk per track. It interleaves <=20 ms mono slices, System first on ties, without opening an output device. Route/format/gap changes preserve held audio and reset adaptive state. Pause resets use observed source-audio time rather than app startup elapsed time.

Transcript review suggests similar Mic/System text but never automatically hides it. A user can confirm probable bleed, keep genuine speech, restore hidden speech or dismiss an unmatched earlier decision. Append-only decision lineages and per-run match snapshots preserve history. Only explicit applied bleed is excluded from FTS, summary inputs and text exports. JSON/detail retain all speech and annotations. Exact unique anchors can reapply decisions; ambiguous speech remains visible. Restoring changes the summary revision, and fully hidden input is rejected before provider access. Historical citations reveal their hidden source line without editing its decision. Review lists are paginated.

The `cargo xtask eval` harness scores explicit reference/hypothesis files for WER, DER/JER, bleed/short-reply errors, latency and supplied resource observations. Its fixed DER/JER collar is 250 ms total boundary width (±125 ms), with overlap scored and global Hungarian mapping. Missing evidence stays unavailable. The included corpus is fictional, and reports explicitly say `acceptance: not_claimed`.

## Inspected evidence

| Requirement | Evidence | Limit |
| --- | --- | --- |
| Real inference AEC | 15 `vt-dsp` tests include actual AEC3, delayed echo, double-talk, a 250 ms near reply, missing references, 8–192 kHz resampling, route/gap/tail accounting and exact queue bounds | Synthetic signals are not a meeting-corpus quality gate |
| Source → AEC integration | Five actual saved-FLAC pipeline tests verify source bytes/hashes/manifest, chronological timing/gaps, stereo/native rates, route changes and truthful preference/history provenance | The strong 14-second synthetic case measured echo RMS ratio 0.0123, double-talk gain 0.6121 and short-reply gain 0.5170; these are not speech-recognition acceptance scores |
| Actual speech integration | Public cached-model live workers and saved Parakeet refinement passed, retaining words, turns, history and exact raw archives | Fake paced source, no OS capture/output; no full WER/DER/latency or long-call claim |
| Inference clock | 105-packet regression covers source times before/across the first two-second flush; actual-engine pause regression contrasts the repaired source clock with the old +5-second startup offset | App elapsed/bookmark cross-process anchoring and broad hardware timing still require validation |
| Valid native FLAC | 23 recorder tests; pathological oversized Rice frames, 1–15-frame final blocks and native 192 kHz use bounded standards-conformant verbatim fallback. Claxon and independent FFmpeg decoded 192 kHz/8-channel and boundary cases bit exactly | Existing ordinary compression is retained; broad hardware/disk/long-call acceptance remains open |
| Safe helper entry points | Seven capture Rust tests, native self-tests and invalid-argument subprocess cases; explicit capture mode required, unknown/ambiguous commands rejected before permission/capture; safe device-list handshake advertises protocol 2 | Anonymous route probe could not prove a usable speaker/reference route on this machine; no positive hardware AEC acceptance claim |
| Bleed decisions | Seven storage tests cover confirm/restore, FTS rollback, exact/ambiguous reapplication, historical snapshots, retirement, 10k-segment bounded comparisons and text-vs-JSON export behavior | Automatic non-user/voiceprint evidence remains unimplemented |
| Summary filtering | Real loopback-fixture service test inspects provider input, current source citations, restore/stale-note behavior, processing guards and all-hidden rejection | Fictional text/local test server, no real provider upload |
| Evaluation arithmetic | 20 metric/manifest cases plus an actual CLI integration pass; standalone fictional command generated a fingerprinted report with intentional errors and no acceptance claim | A consented/public multi-speaker ground-truth corpus, production inference runner and automated PR/nightly jobs remain to be supplied |
| UI | 26 browser IPC fixture flows pass, including isolated-track listening controls, explicit preferences, confirm/restore, short-reply visibility, pending decisions, pagination, historical hidden citations and read-only guards; new review panel visually inspected | Browser fixtures do not replace the native application/manual matrix |
| Final ordinary gate | 224 Rust tests passed, strict workspace all-target Clippy and formatting passed, frontend diagnostics zero warnings/errors | 13 explicit fixture/helper checks are excluded from the ordinary gate; selected public/model/codec checks above were run separately |

The final Opus review identified a pause reset using the wrong clock and saved provenance conflating incomplete history with user preference. Both remedies have real before/after regressions and received a focused verification with no blocking defect found. A test-only completion-marker race was also corrected with atomic publication; it had observed an empty file before the helper finished writing it, rather than a playback-ordering violation.

## Packaged and installed result

The 44.68 MiB local macOS bundle builds, verifies with strict recursive ad-hoc signing, advertises helper protocol 2 and includes all 22 echo-notice files with matching source hashes. The new route/processing/bleed schema initializes successfully. One initial 25-second isolated launch did not report frontend readiness during concurrent validation; a later sole-instance check and three consecutive isolated launches connected successfully (0.76, 0.43 and 0.44 seconds). This is not a cold-start-under-load SLO claim.

The validated bundle is installed at `~/Applications/VanTranscribe.app`, running with its packaged frontend connected. Installation checked no recording/processing was active and retained the previous app for recovery. Installed schema/preferences were checked using metadata only: no meetings or active capture, speaker analysis off, echo requested with route eligibility still required. No capture or playback was started by installation.

During validation, an old helper binary treated a new diagnostic flag as a capture request and briefly opened capture before stopping. It emitted only format/status metadata, no audio samples; no recording was saved. This was disclosed immediately. Strict argument validation and a known-safe `--list-devices` capability handshake now prevent that path, and the final reviewer confirmed no further probe-to-capture path in the current code. Subsequent probes used the known-fresh helper and remained read only.

## Completion audit

This increment advances M0 recording validity/timing, M1 inference, M2c route-gated AEC/reversible review, and the shared evaluation harness. It does not complete the roadmap. M0 physical capture/drift/lifecycle/disk/benchmark gates, M1 WER/screen-latency acceptance, M2a throughput/language profiles, M2b calibrated DER/speaker-count quality, M2c real-speaker echo/double-talk acceptance and automatic identity evidence, M3 hour-long provider timing, M4 consented encrypted voice memory/privacy/retention/detection, richer exports and M5 Windows parity remain open. Requirements are not narrowed to the implemented increments.

The subsequent [retention, recording-health and public-corpus checkpoint](2026-11-07-retention-health-and-corpus-validation.md) records the next implementation increment without superseding these existing hardware/quality limits.
