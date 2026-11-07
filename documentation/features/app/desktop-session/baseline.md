# Desktop Session Baseline

## Flow invariants

- Manual recordings begin only after a `consent_confirmed` acknowledgment. Fresh detected AlwaysRecord starts additionally require an explicit per-app participant-notice attestation and saved input; detection defaults off.
- Only one recording, replay, refinement or summary job runs at a time. Failed sessions permit a new recording attempt.
- A per-data-root OS file lock is acquired before initialization or recovery. A second app instance cannot recover or discard another instance's active work.
- Microphone selection uses an explicit device UID or the system default. A Bluetooth system default is not opened without an explicit device choice.
- Remote mode requests independent Mic/System tracks. Tap creation failure warns and degrades to microphone-only; unexpected channel layouts stop capture instead of relabeling samples.
- Native object ownership and JSON publication stay outside the Core Audio IOProc. The callback copies through preallocated C SPSC storage with lock-free atomics.
- Native host-clock observations remain in `app_audio_clock` separately from exact adjacent writer ranges. No callback-arrival timestamp substitutes for observed source time.
- Audio publication remains independent from inference. The inference queue uses nonblocking submission and can drop recognition work while recording continues.
- Pause flushes accepted audio and resets recognition streams while retaining the loaded model. Native gaps reset only the affected stream. Resume creates a discontinuity before accepting subsequent audio. Elapsed meeting/note time includes paused time.
- Stop requests helper shutdown, continues draining emitted events through EOF, flushes accepted track data and finalizes inference before marking a normal meeting ready.
- Startup attempts recovery of interrupted `recording`, `paused` and `processing` desktop meetings. Recovery failures are marked failed rather than hidden.
- Partials live in memory, render italic/muted and cannot be edited; final transcript segments append only to the active running live run. Remote fallback labels are Me/Them channel labels; in-person fallback is Unknown in live recognition, replay and refinement. Optional speaker annotations derive labels separately from STT.
- Model installation is explicit, source/hash pinned, cancellable and performed outside audio callbacks. No model or cloud provider is silently enabled.
- Saved transcription reads ready audio only. Replay finals remain staged until one transaction creates/publishes a new replay run; failure/cancellation preserves the previous transcript and historical runs.
- A normal stop seals its live run before automatic post-processing. Installed-pack automatic refinement can chain into explicitly configured automatic AI notes; absent prerequisites do not trigger model downloads or provider selection.
- Refined transcript segments and word times remain private to their processing run until successful atomic publication. The active pointer, completion state, overlay matching, stale-summary marking and FTS refresh commit together.
- Original run segments are immutable. Legacy `app_segments`, notes and audio survive migration; historical runs remain addressable and are not garbage-collected by this increment.
- Corrections are overlays on the current transcript. Reapplication requires unique exact track/time/original-text-hash anchors. Unmatched/ambiguous edits remain visible for explicit review; dismissal is an explicit action.
- Search uses the backend FTS index over title, current effective transcript, notes and fresh active summaries. Raw user text cannot introduce FTS query operators.
- Historical transcript views stay selected during polling, expose no correction controls, and disable export because the export command uses the current run.
- Summary publication requires the same complete active run and input revision used for generation. Transcript/speaker-annotation edits that change summary inputs and new transcript publication make older notes stale. Title/personal-note edits preserve transcript revision and summary freshness because they are not summary inputs.
- Summary-only failures persist in `MeetingDetail.summary_error` separately from recording/transcript status and warnings; failure storage never overwrites source-health metadata.
- Every stored summary source is an existing segment in that summary's run. Stale/partial summaries and known processing gaps are visibly marked; source links load/focus the exact cited historical segment.
- Structured AI notes and Markdown fallbacks render escaped text; model output never becomes executable HTML.
- Provider discovery only checks actual Apple availability and numeric-loopback local servers. It never enables a provider or downloads a model.
- External summary providers require explicit text-processing consent, HTTPS, a chosen model and a scoped Keychain key; failures never silently switch providers.
- A model installed after recording starts is offered for saved transcription; the UI does not pretend a live recognizer was attached to that session.
- The UI displays real backend state and reports lack of a Tauri connection. It contains no sample meetings or simulated capture bridge.
- Saved audio is played only by the Rust-core CPAL engine. Loading a meeting/player or computing a waveform never starts output; a user action is required.
- The runtime reservation blocks playback while any meeting is recording, paused or stopping; capture start stops/drops existing output before opening native inputs.
- Playback validates read-only manifest geometry, exact paths, SHA-256 and FLAC frame bounds through root-scoped capability file reads. It neither modifies source audio/database nor creates plaintext decoded temporary files.
- Playback caches at most one two-second chunk per track and queues 8,192 output frames. Mic/System retain the common session timeline, independent 0–1 gains and explicit-gap silence.
- Pause/seek/stop drop prior output/read-ahead. Position follows consumed source frames; underruns insert silence without skipping speech. Device/decode errors stop the cursor and remain visible.
- Optional live Silero VAD gates inference only, retaining onset/endpoint context and absolute times. It never gates raw capture, and absent optional setup does not automatically download a model.
- At most 4,096 active speaker relationship corrections are stored per meeting. Replacing an existing excerpt pair remains possible at capacity; revocation frees capacity without deleting audit history.
- Speaker models require explicit acquisition-notice acknowledgment and inference is off by default. Cluster scores/labels remain experimental and uncalibrated; overlap/unclear/provisional evidence never verifies a human identity.
- Pure speaker continuity is per-track: named ≥30% splits and conflicting named merges require review; a sole named merge parent can retain its stable label. Remaining Hungarian pairs require ≥50% of the larger cluster duration, without tied optimal assignments. Ordinary diarization never persists unnamed embedding vectors. Explicit named voice enrollment uses the separate encrypted voice-memory store.

## Flag and failure rules

| Condition | Severity | Blocks? |
|---|---|---|
| Recording acknowledgment absent | Error | Start |
| Missing helper or unavailable selected microphone | Error | The recording attempt |
| Bluetooth system default without explicit UID | Action required | Opening that microphone |
| Microphone authorization not granted | Error with System Settings guidance | Microphone capture |
| System process tap unavailable | Warning | System track; microphone may continue |
| Silence alone | Uncertain signal state | Does not prove permission denial |
| Unexpected aggregate channel/stream layout | Error | Capture, to avoid track corruption |
| Native queue overflow/device transition | Explicit gap | Affected observed range |
| Inference queue full or engine failure | Warning | Live recognition; archive recording continues |
| Missing/unverified speech model | Setup required | Recognition, not audio recording |
| Download cancellation/hash mismatch/extraction failure | Setup error | Model publication |
| Healthy saved transcript, refinement pack installed | Available | New refinement run can replace the active pointer only on success |
| Failed/recovered/errored transcript or canceled processing | Retry/review available | Previous text and notes remain visible until successful new publication |
| Unmatched or ambiguous correction anchor | Review | Automatic overlay application only |
| Summary run/input revision changed | Error/stale | Fresh summary publication and stale-summary search indexing |
| External provider lacks explicit consent/key or secure endpoint | Setup required | External generation |
| Apple/local provider unavailable | Action required | Generation with that provider; no implicit cloud fallback |
| Recording active/paused/stopping | Exclusion | All native in-app playback; output is dropped before capture starts |
| Playback integrity/path/FLAC or native output failure | Error | Output clock stops; original audio/DB remains unchanged |
| Playback queue underflow | Buffering | Silence without source-cursor advance |
| Missing optional VAD | Ungated available | No model auto-download; raw recording unchanged |
| Speaker labels uncalibrated/provisional or reconciliation ambiguous | Experimental/review | Confirmed identity claims / automatic conflicting name inheritance |
| Unknown export format or invalid title/note | Error | Requested mutation |
| Interrupted meeting on startup | Recovery attempt | Failure becomes visible failed status |

## Side effects in order

1. Startup locks the data root, initializes/migrates recorder and desktop tables without deleting legacy transcripts, discards unpublished replay staging, abandons interrupted runs, recovers desktop sessions, checks both model catalogs and loads non-secret preferences.
2. Start validates acknowledgment/helper/concurrency, stops/drops existing playback under the runtime reservation, creates recording/desktop meeting rows and an active live run, then launches the session worker and helper.
3. Accepted capture packets update bounded track buffers. Before each chunk publishes, its original clock observation is saved; durable FLAC publication follows the recording feature's protocol.
4. Optional inference receives mono copies on a separate bounded channel. Finals append `app_run_segments` and refresh active FTS transactionally; partials only update in-memory state.
5. Notes insert `app_notes` and rebuild search; active notes receive elapsed session time and saved-meeting notes receive zero. Titles also rebuild search. Neither affects transcript revision or summary freshness. Correction edits advance the transcript revision and stale summaries before the transactional search refresh.
6. Stop transitions to processing while helper/event/recorder/inference completion drains. Success seals the live run, marks the meeting ready and considers configured automatic refinement/summary work; an error preserves failure text and attempts recorder recovery.
7. Refinement publishes new run completion, conservative overlay matches, active pointer, stale summaries and FTS together; any error rolls back publication. Summary storage validates active run/revision and run-scoped citations before committing.
8. Export creates a unique file under `Exports`, writes the requested representation and synchronizes it before returning the path.
9. Explicit playback validates/loads metadata silently, creates and prefills native output only for Play, and drops prior streams/queues for pause/seek/stop. Waveform analysis never creates output.
10. App exit cancels all model setup/capture/saved processing, drops/joins native playback and waits for bounded worker completion before exiting.

## Defaults and limits

| Contract | Value |
|---|---|
| Desktop target | macOS 14.4+, Apple Silicon current build target |
| Rust toolchain floor | 1.91 |
| Window | 1240×840 default, 860×620 minimum |
| Default data root | `~/Library/Application Support/com.dvrtech.vantranscribe/` |
| Diagnostic data override | `VANTRANSCRIBE_DATA_DIR` |
| Helper event cap | 4 MiB per JSON line |
| Native callback buffer | At least 10 s at configured callback quantum |
| Capture-reader channel | 128 events |
| Inference channel | 600 messages |
| FLAC worker block | At most 2 s, per track |
| Native saved playback | CPAL 0.18.2, 8–192 kHz / 1–8 output channels, callback-driven cursor |
| Playback bounds | 100,000 metadata rows, 64 MiB encoded chunk, 2 s decoded source per track, 8,192 queued output frames |
| Waveform bins | 1–2,048 per track; desktop default 512 |
| Audio preferences | Live VAD enabled when installed; experimental diarization disabled |
| Standalone VAD | 643,854-byte Silero v4 ONNX; MIT; explicit install or verified refinement-pack reuse |
| Speaker segmentation source window / ownership stride / native window | Live: 5,000 / 2,500 / 10,000 ms; Refined: 10,000 / 2,500 / 10,000 ms |
| Speaker reconciliation capacity | 128 identities per track; capacity produces review rather than partial inheritance |
| CPU speech engines | English streaming Zipformer; separate offline Parakeet v3 INT8 + Silero VAD, two model threads |
| Refinement download / installation | About 465 MiB download / 640 MiB installed; CC BY 4.0 model and MIT VAD notices retained |
| Processing preferences | `auto_refine=true`, `auto_summarize=true`; act only when required pack/provider is configured |
| Summary defaults | Provider Off (`none`), `general` template, 4,096 context tokens, no cloud consent |
| Summary context setting | 3,072–131,072 tokens; Apple constrained to its effective runtime budget |
| Summary templates | General, One-on-one, Standup, Client call, Interview |
| Local discovery | Numeric-loopback `127.0.0.1:11434` and `127.0.0.1:1234`; no provider auto-enable |
| Live model archive / selected weights | Standard: 506,956,414 / 190,180,941 bytes; Compact: 310,414,022 / 72,466,360 bytes |
| Model setup progress | Fraction 0–1; final 5% reserved for verification/extraction |
| UI polling | 500 ms recording/processing/either model setup, 2 s idle |
| Library search debounce | 250 ms, stale responses discarded |
| Correction text limit | 1–32,000 UTF-8 bytes containing non-whitespace text |
| Speaker constraints version | Per-meeting user-constraint version captured by processing runs; zero before edits |
| Process exclusion refresh | 2 s |
| Duration persistence | About every 2 s while active |
| Graceful helper drain before forced stop | 5 s |
| `CaptureProcess::stop` wait | 3 s before process kill |
| App shutdown wait | Up to 35 s |
| Title limit | 1–200 characters |
| Backend note limit | 1–32,000 UTF-8 bytes after trimming |
| Export formats | Markdown, TXT, JSON, SRT, VTT |

These are implementation contracts, not proof that the hardware capture, accuracy, drift, thermal or long-session exit criteria have passed.

## Access control and privacy

- Commands are local Tauri IPC, with no HTTP service or application account model. macOS privacy authorization is enforced by the OS.
- Automatic capture diagnostics are excluded from release builds. Debug diagnostics require an explicit environment opt-in plus command-line acknowledgment, wait for frontend initialization and write only a new report under the data root; stdout contains metadata only.
- Audio, transcripts, notes, summaries, clock metadata and exports are stored locally and unencrypted under ordinary filesystem permissions. At-rest encryption remains planned. Local deletion/retention is previewed and crash-recoverable; exports and backups remain outside that operation.
- Only explicit model setup downloads model data. Live/refinement recognition and Apple/local-loopback inference do not upload audio or transcript text externally. An explicitly configured/consented cloud provider receives transcript text for AI notes, governed by that provider’s data handling.
- Credentials are stored only in provider/origin-scoped macOS Keychain accounts; they are never returned to the WebView or saved in SQLite preferences. The password draft is cleared after save or leaving Settings. Changing provider/origin revokes draft cloud consent and clears draft key input; stored-key removal requires an explicit action for the selected provider/origin.
- UI theme and selected device UID are remembered in local WebView storage; meeting content is served by the Rust store.
- Recording-awareness acknowledgment is distinct from biometric enrollment. Speaker memory is globally off by default; enabling it requires the biometric explanation. Each enrollment additionally requires person-specific consent, retained clean refined speech, an explicit name and a separate protected key. Experimental diarization itself persists only timed assignments/user constraints.
- Documentation and fixtures must not include personal meeting content or credentials.

- Speaker relationships are revocable audit records. Replacing an exact excerpt pair supersedes its prior active relation; removal reverses linked manual aliases/turn overrides and advances the constraint revision. A conflicting user-name merge rolls back all changes.
- Disabled or unavailable optional speaker models retain previous timed labels, names, constraints and pending reviews on reprocessing, with an explicit reuse warning.


## Echo and reversible bleed invariants

- Default echo preference is enabled, but processing requires positive capture-time built-in-speaker/System-reference evidence. Unknown/headphone/external/mixed routes never enable AEC.
- Native-rate raw recording and playback retain the source. AEC applies only to inference copies; both ASR and independent speaker work consume the same processed track data.
- Pending inference packet times start from the first buffered source observation before the first FLAC flush; new callback time is never added again to already buffered duration.
- Capture-time route observations are immutable. Saved jobs use only cleanly sealed histories; interrupted, missing or unsuccessfully persisted histories bypass AEC rather than assuming today's output route.
- Saved processing verifies exact source paths, hashes, PCM24 format/frame geometry and bounded chunks, then interleaves <=20 ms native mono slices, System first on equal timestamps. No audio device is opened by decoding.
- Pause resets use the captured audio clock, never app elapsed time or permission/setup delay. Saved provenance distinguishes the user preference from missing route evidence.
- Echo state and resampling are reset on route/format/gap changes, while held Mic/tails are preserved through bypass. Consumer overflow cannot block raw capture.
- Text similarity only suggests visible Mic lines with >=4 tokens. It never supplies the missing user-identity/voiceprint evidence for automatic hiding.
- Explicit bleed decisions append audit events and change only current-run overlays. Confirmed hidden IDs are excluded from FTS, summary input and text exports. JSON/detail, source audio/text and old run snapshots retain them.
- Restore is explicit and reversible. Exact unique anchors can reapply a decision to a new run; ambiguous/unmatched bleed remains visible for review. Genuine/missing anchors do not create repeated reviews; dismissed pending lineages never revive.
- Bleed changes increment the summary input revision and stale old notes. With all speech hidden, summary generation is rejected before provider access. Historical citation navigation reveals hidden sources without changing the decision.

## Storage and health integration

- Audio retention defaults to keep; unreferenced obsolete transcript runs default to seven days. Active/running and cited/audit-referenced runs remain protected.
- Confirmed maintenance reserves the same work slot as capture/processing; audio output drops before files move. Automatic cleanup defers while saved audio plays.
- An interrupted confirmed deletion is recovered before ordinary recording recovery. Pending deletion blocks playback/replay/refinement without changing ready source rows into gaps.
- Live notes retain source timing provenance; saved notes without matching playback retain explicit untimed provenance. UI does not present them as precise source positions.
- Disk checks use decimal 2 GB warning, 500 MB inference suspension and 100 MB clean-stop thresholds. Unknown availability blocks new capture/processing.

- Automatic retention attempts are hourly even after errors; a newly saved policy is eligible at the next idle check. Manual cleanup and startup recovery remain available.

## Live-model and backlog integration

- Fresh setup chooses Standard/0.2 VAD; explicit preferences and legacy Compact directories remain unchanged. Core model/VAD defaults remain Compact/0.5.
- Model choice cannot change active capture, processing or model setup; selection never downloads. Verify/repair and setup are explicit actions.
- Global inference lag uses raw/completed-input source watermarks, configured workers and monotonic freshness. Beyond ten seconds sustained for two seconds, shedding proceeds partials, speakers, STT at least five seconds apart; disabled stages do not reactivate within that capture.
- Backlog never gates raw storage. Inference gaps and stage events belong to their run. Ledger `complete` means finalized audit, not complete transcription.
- Missing/incomplete expected sealed audits are visibly unknown. If neither final audit nor fallback warning can persist, ready Current/error remains visible and autojobs are skipped; a new recording remains permitted.
- Closing guards reserve runtime before new capture/model setup/processing can start.

- Invalid saved live-model settings leave startup and raw recording available but require explicit supported-choice confirmation before live STT/setup. The UI does not silently overwrite the stored choice.
- A normal pause is described as a pause reset. Deliberate disk suspension at capture start is not presented as a failed inference audit.

## Measured speaker segmentation provenance

- `DiarizationInfo.segmentation_context` records the actual engine source-window, ownership-stride and native-window durations for that run. It does not establish diarization quality or participant identity.
- `app_speaker_run_state.segmentation_context_json` is nullable. Startup checks the existing columns before adding it; old and warning-only rows remain unknown, and run kind never substitutes for measured context.
- Result context and timed annotations commit together. Failure rolls both back. Retaining previous annotations copies their exact original JSON, including a Live context on a later Refine-kind run.
- Completed-run speaker-result/copy API calls reject mutation. Existing direct-SQL run-state guard behavior remains unchanged. Retention fingerprints and deletes the existing state table normally.

## Opt-in meeting detection integration

- Detection is off by default. Disabled detection invokes no native process probe and never starts capture.
- The backend owns capture mode and UI setup readiness. Startup/WebView reload cannot authorize an unattended action before a fresh context handshake.
- Prompt recording still requires ordinary participant acknowledgment and an explicit device choice when Bluetooth requires it. AlwaysRecord additionally requires a per-app attestation and saved input.
- Only fresh verified future idle activity edges can authorize AlwaysRecord. Busy/setup episodes and ongoing microphone use after Stop do not become queued recordings.
- Native meeting apps and browser microphone use have different wording. Unknown evidence cannot prove that a call ended.
- More than sixty seconds of confirmed source-app absence can suggest Stop for its associated recording; no timer stops recording.
- Notifications contain canonical reminder text, run outside controller locks and only focus the app when clicked. In-app controls retain capture/stop authorization.
- Process observations stay in memory. Settings persist; canonical detected source provenance belongs to the actual initial run. No PID/path/activity-history table is created.
- Context confirmation is fenced by the page's first authoritative `context_generation`. Reload invalidates older queued context setters and settings saves before state or SQLite mutation; later polls cannot grant the old page a new generation.

## Consented speaker memory integration

- Voice-memory status, settings, expiry and deletion never read or create an OS key. Keys are accessed only for an explicit eligible enrollment; missing/locked keys cannot fall back to plaintext.
- A page generation fences enabling/settings and the final enrollment commit, so an old WebView request cannot restore authority after reload.
- Shared-room flags survive merges conservatively. Flagged speakers cannot enroll or update a voiceprint.
- Person deletion clears explicit links in the same transaction as prints/attestations. Optional name removal clears linked speaker-name overlays and invalidates summaries/search; spoken text, manual notes, immutable source history and exports remain.
- Biometric expiry runs at startup and in hourly idle maintenance even while speaker memory is off; no readable key is required.
- Database/audio/exports are still unencrypted. Only named biometric vectors use the separate encrypted store; automatic recognition remains unavailable pending real calibration.
