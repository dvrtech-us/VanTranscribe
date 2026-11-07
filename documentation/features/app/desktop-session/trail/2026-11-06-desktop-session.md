# Join Capture, Recording and Recognition in a Desktop App

- Date: 2026-11-06
- Feature: desktop-session
- Related code: `src-tauri/`, `crates/vt-session/`, `crates/vt-capture/`, `crates/vt-stt/`, `native/macos/`, `ui/`
- State: Working draft for this unmerged implementation change

## Context

The approved design and previous architecture reviews already establish the full roadmap. The user asked to continue until there is a working app. The initial durable recorder could be exercised through a synthetic command but could not capture a real meeting or present a desktop workflow. This change connects real native inputs, durable storage, local speech recognition and the interface without redefining the complete app objective around a synthetic demonstration.

## Decisions

- Introduce a Tauri 2 application backed by one `AppService`, with separate session, capture-reader, inference and model-download workers. The Svelte interface consumes local commands and never substitutes seeded meetings for backend state.
- Build native macOS capture as a bundled Swift helper plus a preallocated C SPSC callback ring. This uses the design's permitted Swift fallback for platform ownership while keeping Rust recording/inference/session orchestration. The helper owns the microphone/tap aggregate and publishes worker-side events over bounded line transport.
- Keep hardware source epoch/frame/time observations in `app_audio_clock` before publishing each recorder chunk. The recorder's contiguous worker timeline must not replace the evidence needed to evaluate actual capture-clock drift.
- Require explicit recording acknowledgment and an explicit Bluetooth microphone choice. Display system-audio uncertainty and tap fallback honestly instead of inferring denial from silence.
- Use the official sherpa-onnx Rust crate and a pinned public English Zipformer model, downloaded only by explicit user action. Verify the archive and selected file hashes, preserve license/attribution and recognize on a separate CPU worker.
- Preserve audio capture when live inference falls behind. Allow saved-audio transcription after model installation and safe retry of failed/recovered/errored partial text. Stage replay segments and publish them as a new immutable run after success; preserve existing text/history on failure/cancellation. Add separate Parakeet/Silero offline refinement so healthy transcripts can be reprocessed without destructive replacement.
- Persist desktop metadata in separate `app_*` tables alongside the recording manifest. Extend them with run-scoped immutable segments, active revisions, conservative correction overlays, cited summaries and transactional FTS. Retain all legacy transcript rows during migration. Cross-meeting people/voiceprint and encryption models remain planned; local retention and deletion have separate feature ownership; meeting-local identity/constraint annotations are part of this increment.
- Stop by requesting native shutdown and draining events to EOF before final recorder/inference flush. Exit also requests orderly shutdown rather than abandoning an active session immediately.
- Provide library, live/detail, notes/bookmarks, export, microphone/model setup and local appearance preferences. Add backend FTS search, run history, explicit correction review and escaped grounded AI notes with historical source navigation. Remote fallback channel labels remain Me/Them; in-person fallback remains Unknown through live recognition, replay and refinement; optional independent speaker inference is experimental, with full integration/calibration acceptance still open.

- Reserve one saved-processing slot for replay/refinement/AI notes. A normal stop seals its live pass, then automatic refinement can run with an installed pack and chain into configured AI notes. Automation defaults do not download models or select a provider.
- Use exact track/time/SHA-256 correction anchors with unique matching in both directions. Preserve unmatched/ambiguous edits for explicit user review instead of guessing or discarding them. Keep historical segments and summary citations run-scoped.
- Store summary results only when the expected complete active run and correction/input revision remain current. Mark old notes stale on transcript edits and new run publication; title/personal-note edits only refresh search because they are not summary inputs. Display partial outputs and known recording gaps honestly; citation validity is distinct from semantic entailment.
- Expose Apple, numeric-loopback local and explicitly consented external providers. Capability discovery enables nothing. Keep keys in provider/origin-scoped Keychain accounts, outside SQLite/WebView storage and readback. Revoke draft consent/credentials on origin changes and require explicit scoped removal.

- Add Rust-core CPAL saved playback with read-only manifest/integrity validation, capability-relative file reads, bounded per-track decode caches and an SPSC output queue. Loading is silent. The same runtime reservation stops output before capture and prohibits playback during active/paused/stopping recording. Drop old streams/queues on pause/seek/stop to prevent stale buffered audio.
- Expose optional incremental live Silero VAD independently of Parakeet setup, preserving raw audio and native recognition context/times. Retain real word alignment when available instead of synthesizing word times from text.
- Add pure per-track duration-overlap/Hungarian reconciliation and meeting-local speaker annotation/constraint storage, with explicit split/merge/tie/capacity review. User names are not inferred from acoustic similarity, and no embedding vectors enter storage. The experimental core's full annotation/calibrated quality gates remain open.

- Put pinned AEC3 on a separate inference worker, with explicit capture-time route evidence and conservative bypass. Replay the same bounded path from verified source chunks; store per-run application metadata and keep raw audio unchanged. Snapshot settings once per job so diarization and transcription agree.
- Treat similar text as a review suggestion until the user confirms bleed. Use append-only decision lineages and run-local match snapshots, exact unique reapplication, current-run guards and reversible visibility. Filter only confirmed decisions from search, AI inputs and text exports; retain full JSON/source/history. Automatic non-user voiceprint evidence remains a later independent requirement.
- Require explicit native capture mode and reject unknown/ambiguous helper arguments before permission or source setup. Advertise optional read-only probes through the existing safe device-list protocol so older helpers never receive an unknown diagnostic flag.

- Expose storage deletion as preview then explicit confirmation, with crash-recoverable intents and shared runtime exclusion. Start idle retention checks only from the desktop lifecycle, retain audio by default and protect cited/audit-linked history.
- Expose resource health and source-clock provenance without blocking initial library loading on device enumeration. Keep filesystem-heavy commands off the UI thread and present saved untimed notes honestly.

## Alternatives Considered

- **Mock interface with fabricated meeting data:** rejected because it would not prove the requested desktop workflow is connected to capture and storage.
- **Inference on the capture/recording path:** rejected because model latency must not govern the durability of meeting audio.
- **Normalize hardware clocks without retaining observations:** rejected because synthetic adjacency would hide drift and make M0 measurements unverifiable.
- **Open the Bluetooth default automatically:** rejected because it can switch a headset into call mode and degrade meeting audio without an explicit choice.
- **Publish replay results incrementally over existing partial text:** rejected because another failure could leave the user with less text. Replay uses transactional staging into a new run; refinement uses an unpublished run with the same atomic active-pointer/search boundary.

## Consequences

- Positive: the repository contains an app that can be built with a real native backend, local storage and model-driven recognition, ready for end-to-end runtime verification.
- Positive: callback work, archive publication and inference are separated; failure and setup states are exposed instead of simulated.
- Negative: macOS permission prompts, native channel layout, drift, device transitions, stop/drain timing and real transcript accuracy require runtime verification. A compile or synthetic recording does not prove those gates.
- Negative: data remains unencrypted, retained runs increase disk use, exact correction matching may need manual review after changed segmentation, and source citations do not prove model claims. Baseline labels remain channel-based and optional speaker labels are provisional/uncalibrated. Native playback is implemented with hardware/route/latency gates open; AEC/bleed handling, calibrated diarization acceptance, voiceprints, encryption and Windows remain open.

## Validation status

The earlier signed release bundle built and launched with the embedded interface connected, an installed local model and no automatic recording. Short real Mic/System capture and System-fixture recognition checks passed, while revealing an actual configuration transition with explicit gaps; notification filtering reduced repeated rebuilds to one source-epoch change. Full transcript quality and M0 long-call/device gates remain open. The earlier foundation passed 41 Rust tests and three Chrome UI flows. This expanded increment separately passes storage regression tests and ten Chrome UI flows, plus Svelte type checking and production UI bundling; real Parakeet public-audio and Apple fictional-text checks are recorded in their feature docs. Expanded workspace/release integration verification is tracked separately and remains in progress. Claude Opus reviewed the earlier desktop foundation and verified remedies to its six reported findings. The expanded processing/provider/UI increment receives its own final integration review; that earlier review is not a blanket approval of new code. No full-roadmap completion is asserted.

Seven generated-FLAC/mock playback tests and strict crate clippy pass after final worker-panic/late-callback safety changes. An authorized real CPAL test emitted only digital silence from temporary synthetic FLAC: callbacks advanced, pause held position, resume/playing seek worked and Stop reset the cursor. Eight pure reconciliation tests pass. No private recording was opened for these checks and no audible output was requested.

## Review remedies

- Final processing review narrowed summary invalidation to actual transcript inputs; metadata edits no longer stale AI notes. Persist summary failures separately from recording/transcript warnings through `app_summary_failures` and `MeetingDetail.summary_error`.

- Keep automatic capture diagnostics out of release builds. Debug checks require an environment opt-in, wait for interface initialization, constrain new report files to the data root and log only metadata.
- Reset only affected STT streams, retaining the loaded model across gaps and pauses.
- Wait for or cancel saved transcription during shutdown; stage retry results until atomic publication.
- Derive writer epochs from the last persisted epoch and preserve native gap provenance.
- Lock the canonical app data root before recovery, protecting an active replay from a second app instance.
- Embed the frontend with Tauri custom-protocol and verify its native IPC readiness; use an ad-hoc local macOS signature for repeatable local builds.

The final implementation review added hourly failed-attempt backoff, bounded/compacted retention receipts, truthful known-meeting maintenance status and explicit separation of paced core traces from product latency observations.

The integrated English chooser preserves explicit/legacy selections, recommends Standard/0.2 on fresh setup from bounded production-path comparisons and records requested model/configuration with each live/replay version. Source-watermark overload stages and finalized per-run health ledgers preserve raw audio and disclose incomplete history; full profiles and hardware/quality acceptance remain separate.

Final implementation remedies keep malformed/unsupported model settings repairable without enabling a fallback, distinguish pause notices from source discontinuities, and account for the initial disk gate when declaring expected inference-audit evidence. Focused failure-path tests cover each remedy.

### Measured segmentation context follow-up

The core now uses ten seconds of real source context for saved/refined diarization while Live retains five seconds and its existing 2.5-second ownership stride. Both phases record their actual source/stride/native-window values. Desktop storage adds nullable `segmentation_context_json` to the existing speaker run-state table through a guarded startup column check, so old runs and warning-only rows stay explicitly unknown. This is provenance rather than a new quality claim; calibrated English/channel acceptance remains open.

Speaker context commits with its turns and is copied verbatim when annotations are retained. A copied Live context does not become Refined merely because its new transcript run is Refine-kind. Tests cover save/copy/restart, legacy `NULL`, warning-only unknown values, rollback of result and copy publication, and completed-run API rejection. Existing direct-SQL run-state guard semantics and retention ownership remain unchanged. Opt-in public live/refined service tests assert actual phase context separately from synthetic legacy metadata.

## Opt-in meeting reminder integration

Implement the approved advisory detection flow with a protocol-advertised read-only native snapshot, bounded polling worker and explicit UI readiness/mode handshake. A separate per-app AlwaysRecord choice requires participant-notice attestation and saved input, and is limited to future verified idle activity. Source provenance is attached during initial run creation; no background activity history is stored. Native notifications only bring the existing window forward. Manual consent and Stop actions retain their existing capture authority, and false/unknown/stale evidence cannot cause an unattended stop. Settings and reload/busy gates make the automatic policy explicit and testable without actual audio devices.

A per-page context generation also fences queued WebView commands after reload: the old page cannot adopt a later poll's generation or release a fresh Settings context. The service checks the generation before any mode/setup mutation.
The same generation fence protects policy saves: an older AlwaysRecord draft cannot overwrite a newer opt-out or suppression choice and acquire authority for future activity.

### Consented encrypted voice memory

The desktop connects explicit saved-speaker enrollment and Settings → People to a separate protected biometric store. A named person needs individual consent and eligible retained refined speech; no ordinary diarization vector is persisted. Separate Keychain-backed column encryption leaves the existing database/audio encryption scope open. Shared-room flags prohibit enrollment and survive merging. Page-generation checks protect queued opt-in/enrollment requests; deletion and expiry are keyless. Optional forgetting of names removes linked name overlays and invalidates derived summaries/search without rewriting immutable spoken content or claiming exported copies were erased. See the voice-memory feature for the complete storage/consent contract.
