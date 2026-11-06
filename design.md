# VanTranscribe: Design Document

| | |
|---|---|
| **Status** | Draft v0.2, revised after adversarial review by Claude Opus and OpenAI Codex (see `documentation/planning/Architecture/2026-11-06-design-review-revisions.md`) |
| **Date** | 2026-11-06 |
| **Author** | DVRTech (with Claude) |
| **License (planned)** | MIT |
| **Platforms** | macOS (Apple Silicon) first, then Windows; Linux is a stretch goal |
| **Stack** | Tauri 2, Rust core, TypeScript, Svelte, Bootstrap 5 UI |

---

## 1. Overview

VanTranscribe is a free, open-source desktop meeting recorder and transcriber. It is meant to replace Zoom Workplace's **"My Notes"** feature.

- **No bot joins the meeting.** The app captures the computer's **system audio output**, which carries the remote participants, and the **local microphone**, which carries you, directly on the device.
- **Near real-time transcription** with **speaker labels**, shown live while the meeting runs.
- **Works with any meeting app**: Zoom, Teams, Google Meet in a browser, Slack huddles, Webex, phone calls through the computer.
- **In-person meetings** use the microphone alone, with speaker diarization.
- **Summaries, action items and decisions** come from a pluggable model: Apple Intelligence (on-device), a local LLM, or a cloud API.
- **Local-first.** By default, no audio, transcript or voiceprint leaves the machine.

### 1.1 Goals

1. Match the core Zoom My Notes workflow: start, live transcript, speakers, summary, and search, with zero per-minute cost.
2. Show live transcript text within about 1.5 s of speech, and produce a high-accuracy refined transcript within minutes of the meeting ending.
3. Label speakers out of the box. The mic/system channel split gives "Me" and "Them" for free, and diarization separates the individual remote speakers.
4. Remember speakers across meetings. This is opt-in voiceprint matching, so "Speaker 2" becomes "Jane" automatically next time.
5. Run comfortably on a 16 GB Apple Silicon laptop for a 1 to 2 hour meeting without hurting call quality.
6. Use a pluggable engine architecture, so the speech-to-text (STT), diarization and summary backends can be swapped without touching the pipeline.

### 1.2 Non-goals for v1

- Joining meetings as a bot, or integrating with meeting platform APIs (Zoom, Graph, Meet REST).
- Mobile apps.
- Multi-user cloud sync or team workspaces.
- Real-time translation.
- Video capture or screen OCR.

### 1.3 Guiding principles

- **Privacy by default.** Cloud is always an explicit, per-provider opt-in that uses the user's own API key.
- **Degrade gracefully.** If a permission or model is missing, keep recording audio and transcribe later rather than failing.
- **Never lose audio.** Recording is isolated from inference. Per-track audio is written to disk in small, independently recoverable chunks (§5.6), so a crash, a stall or a slow model never costs more than about 2 s of audio, and the transcript can always be regenerated.
- **Two passes beat one.** A fast live pass gives instant feedback. A slower refinement pass afterward gives accuracy.

---

## 2. Feature parity matrix

| Capability | Zoom My Notes | VanTranscribe v1 | v1.x | v2 |
|---|---|---|---|---|
| Record without a bot | ✅ | ✅ | | |
| Any meeting app (Zoom/Teams/Meet/etc.) | ✅ | ✅ | | |
| In-person meetings (mic only) | ✅ (mobile/web) | ✅ (desktop) | | Mobile companion |
| Live transcript | ✅ | ✅ | | |
| Speaker separation | ✅ | ✅ Me/Them live, plus diarized remote speakers | Better live diarization (Sortformer-class) | |
| Name speakers | ✅ | ✅ manual rename | ✅ voiceprint memory (opt-in) | |
| Summary / action items | ✅ (AI Companion) | ✅ pluggable LLM | Custom templates | Ask questions across meetings |
| Search past meetings | ✅ | ✅ full-text | Semantic search | |
| Export | ✅ | ✅ MD/TXT/SRT/VTT/JSON | DOCX, Confluence, Notion | |
| Auto-detect meeting start | Partial | ✅ prompt when a meeting app uses the mic | Calendar integration | |
| Cost | Paid Workplace plan (free tier capped) | Free | Free | Free |
| Data location | Zoom cloud | Local disk | | Optional self-hosted sync |

---

## 3. User flows

### 3.1 Remote meeting (primary)
1. The user joins a Zoom, Teams or Meet call as normal.
2. `meeting_detect` notices that a known meeting app has started using the microphone and shows a notification: *"Meeting detected in Zoom. Start VanTranscribe?"* The user can also click **Record** in the menu bar or press the global hotkey `⌥⌘R`.
3. On first use, the app shows a **consent reminder** with a one-click "Copy disclosure message" for the meeting chat. The reminder can be dismissed per session and can be turned off.
4. Recording starts. The menu bar icon turns red. The **Live window** opens, docked to the side and optionally always on top, and streams the transcript:
   - Lines from the mic track are labeled **Me**.
   - Lines from the system track are labeled **Speaker 1, Speaker 2, …**. If voiceprint memory is on, known voices show the stored name with a confidence dot.
   - Partial (in-progress) text appears grey and italic, then settles into final text.
5. During the meeting the user can:
   - Rename a speaker by clicking the label. The rename applies to every line from that speaker.
   - Bookmark a moment (`⌥⌘B`).
   - Type private notes, which are interleaved by timestamp.
   - Pause and resume.
6. The user clicks **Stop**. The meeting moves to the *Processing* state, and the **refinement pass** runs (§8). The target time for a 1 hour meeting is set by M0 benchmarks (provisionally ≤ 10 min). The live transcript stays usable meanwhile.
7. The meeting detail view shows the refined transcript, speakers, summary, action items, decisions, and audio playback with click-to-seek.
8. The user can export the meeting or copy the summary to the clipboard.

### 3.2 In-person meeting
1. The user chooses **Record → In-person** (or the app auto-selects it when no meeting app is active).
2. Only the microphone is captured, with no system tap and no echo cancellation.
3. Diarization runs on the mic track, so every voice, including the user's, becomes Speaker N. If the user's own voiceprint is enrolled, their lines are labeled **Me**.
4. Everything else matches §3.1.

### 3.3 Library and search
- The library lists meetings by date, with title, duration, participants and app source.
- Full-text search covers transcripts, summaries and notes. Results jump to the matching timestamp.
- Filters: date range, speaker or person, meeting app.

### 3.4 First run / onboarding
1. Welcome screen and privacy statement.
2. Permission walkthrough: Microphone, then System Audio Recording, with a level meter for each. System audio is verified by a chirp played from a separate process, because our own process is excluded from capture (§10.1).
3. Model download. The default "Balanced" pack is about 1 GB. A "Lite" pack is offered for 8 GB machines.
4. Summary provider choice: Apple Intelligence (if available), local Ollama/LM Studio (if detected), cloud (enter a key), or none.
5. Optional: enroll your own voice by reading a 20-second prompt. This improves the in-person "Me" label.

---

## 4. Architecture

### 4.1 High-level

```
┌───────────────────────────── Tauri 2 app ─────────────────────────────┐
│  Web UI (TypeScript + Svelte + Bootstrap 5)                           │
│   Live window · Library · Meeting detail · Settings · Onboarding      │
│        ▲ Tauri events (transcript deltas, levels, status)             │
│        │ Tauri commands (start/stop, rename, export, settings)        │
├────────┴──────────────────────────────────────────────────────────────┤
│  Rust core (workspace crates)                                         │
│                                                                       │
│  meeting_detect ──► session (orchestrator / state machine)            │
│                        │                                              │
│   capture ──► dsp ──► stt ──► diarize ──► speakers ──► transcript bus  │
│   (mic,       (resample, (live +   (online +  (voiceprint   │         │
│    system)    AEC, VAD)  refine)   offline)   matching)     ▼         │
│                                                    store (SQLite+FTS, │
│   summarize ◄──────────── refinement job ◄──────── audio files)       │
│   (Apple FM bridge / OpenAI-compatible local / cloud)                 │
│                                                                       │
│  models (download, checksum, license registry) · secrets (Keychain)   │
├───────────────────────────────────────────────────────────────────────┤
│  Platform layer                                                       │
│   macOS: Core Audio process tap, AVFoundation/cpal mic, Swift bridge  │
│          for FoundationModels, CoreML EP for ONNX Runtime             │
│   Windows: WASAPI process loopback, DirectML/CPU EP, Credential Mgr   │
└───────────────────────────────────────────────────────────────────────┘
```

### 4.2 Crate layout

```
vantranscribe/
  src-tauri/             # Tauri app: commands, events, tray, windows
  crates/
    vt-core/             # shared types: Segment, SpeakerId, TrackKind, clock, errors
    vt-capture/          # AudioSource trait + per-OS implementations, device/route watcher
    vt-record/           # durable chunked audio writer + manifest (isolated from inference)
    vt-dsp/              # resampler, AEC, VAD, level metering, drift correction
    vt-stt/              # SttEngine trait + engines
    vt-diarize/          # online + offline diarization
    vt-speakers/         # voiceprint store + matching
    vt-summarize/        # Summarizer trait + providers, prompt templates
    vt-store/            # SQLite schema/migrations, FTS, audio file IO, export
    vt-models/           # model catalog, downloader, checksum, license + provenance registry
    vt-detect/           # meeting app detection
    vt-session/          # orchestrator: wires the pipeline, state machine
  swift/
    VTAppleBridge/       # Swift package: FoundationModels (weak-linked) + (fallback) SCK capture
  ui/                    # Svelte app
  documentation/
```

### 4.3 Key abstractions

```rust
// vt-capture
pub enum TrackKind { Mic, System }

pub trait AudioSource: Send {
    fn kind(&self) -> TrackKind;
    fn start(&mut self, sink: AudioSink) -> Result<()>;   // sink is a lock-free ring buffer producer
    fn stop(&mut self) -> Result<()>;
    fn format(&self) -> StreamFormat;                       // native rate/channels (can change on route change)
    fn health(&self) -> SourceHealth;                       // Ok | NoSignal(duration) | DeviceLost | Rebuilding | PermissionUncertain
}

// Every buffer carries hardware timing, not callback-arrival time.
pub struct AudioFrame<'a> {
    pub samples: &'a [f32],
    pub host_time: u64,          // AudioTimeStamp.mHostTime (macOS) / QPC position (Windows)
    pub sample_index: u64,       // monotonically increasing per track, gaps recorded explicitly
    pub epoch: u32,              // incremented on every device/format rebuild
}

// vt-stt
pub trait StreamingStt: Send {
    fn accept(&mut self, pcm16k: &[f32], t0: ClockTime);
    fn poll(&mut self) -> Vec<SttEvent>;                    // Partial{text,..} | Final{text, words[], t0, t1}
    fn reset(&mut self);
}
pub trait BatchStt: Send + Sync {
    fn transcribe(&self, pcm16k: &[f32], opts: &BatchOpts) -> Result<Vec<WordSeg>>;
}

// vt-diarize
pub trait OnlineDiarizer: Send {
    fn assign(&mut self, seg: &SpeechSegment, embedding: &Embedding) -> LocalSpeakerId;
}
pub trait OfflineDiarizer: Send + Sync {
    fn diarize(&self, pcm16k: &[f32], hints: &DiarizeHints) -> Result<Vec<Turn>>;
}

// vt-summarize
#[async_trait]
pub trait Summarizer: Send + Sync {
    fn caps(&self) -> SummarizerCaps;                       // context_tokens, local/cloud, structured output support
    async fn complete(&self, req: LlmRequest) -> Result<LlmResponse>;
}
```

Engines are chosen at runtime from Settings. The default engine set ships as a named **profile**: Lite, Balanced or Accurate.

---

## 5. Audio capture and real-time pipeline

### 5.1 Two-track capture: the core idea

The app records **two independent tracks** and never mixes them for processing:

| Track | Source | Default label | Diarized? |
|---|---|---|---|
| `Mic` | The input device **the meeting app is actually using** (resolved per §5.2), else the user's chosen device | **Me** in remote mode | Only in in-person mode |
| `System` | All system output except VanTranscribe's own processes | **Speaker N** | Yes, always |

This channel split yields a strong *Me vs. Them* prior with no ML at all, and it halves the diarization problem.

It is a **prior, not a guarantee**:
- The mic can contain other people in the room, and speaker bleed (§5.4).
- The system track can contain non-meeting audio, such as a YouTube video or notification sounds.

Labels therefore stay correctable, and the system track is VAD- and diarization-gated rather than assumed to be all speech.

### 5.2 Capture backends

**macOS (primary target: macOS 14.4+)**

Process taps exist from macOS 14.2. The design uses **14.4** as the floor because early tap releases had stability fixes, and AudioCap and other implementations target 14.4 for the same reason.

| Backend | When | Notes |
|---|---|---|
| **Core Audio process tap** in a **private aggregate device** together with the mic | Default, macOS ≥ 14.4 | See the notes below this table. |
| `cpal` 0.17 loopback | M0 spike only | Uses a process tap internally but cannot express our exclusion list. Not shipped. |
| ScreenCaptureKit audio | macOS 13.x–14.3 fallback | Needs Screen Recording permission. `excludesCurrentProcessAudio = true` covers only our main process, so in-app playback is still blocked while recording (see below). |
| Mic via the same aggregate device (preferred) or `cpal` | Always | Standard Microphone permission. |

Notes on the process tap backend:
- **Tap.** A global stereo tap built with `CATapDescription(stereoGlobalTapButExcludeProcesses:)` and `muteBehavior = unmuted`, so the user still hears the meeting.
- **Exclusion uses process AudioObjectIDs, not PIDs.** Translate with `kAudioHardwarePropertyTranslatePIDToProcessObject`. The exclusion list must cover our main process **and every WebKit helper (WebContent/GPU) process** spawned by the Tauri WKWebView, because WebView `<audio>` plays from those processes. The list is refreshed when the process list changes.
- **Belt and braces.** All recording playback is performed by the Rust core (`vt-store` player via CoreAudio), never by WebView `<audio>`. In-app playback is disabled while a recording is active.
- **Permission.** Needs **System Audio Recording** (not Screen Recording).
- **Clock.** The mic and the tap share one aggregate device with `kAudioSubDeviceDriftCompensationKey` enabled on the non-clock sub-device. This gives one hardware clock domain for both tracks, as in Apple's sample and AudioCap.
- **Bindings.** Implemented with `objc2-core-audio` (which already binds `CATapDescription`). Those types are `!Send`, so all Core Audio object ownership is confined to one dedicated capture-control thread, and audio data leaves only through the ring buffer.

**Mic device selection (macOS):**
- When a meeting app is detected, read its process object's `kAudioProcessPropertyDevices` to find the input device it is actually using, and follow that device.
- Otherwise use the user's choice in Settings, defaulting to the system default input.
- **Never open a Bluetooth headset microphone unless the user explicitly picks it.** Opening an AirPods or headset mic forces the HFP profile and degrades everyone's audio to narrowband.
- When the meeting app is itself already on HFP, follow it. Record the actual sample rate and codec class per track in `meta.json` so the evaluation and voiceprint logic know the channel quality.

**Windows (M5): Windows 11 only.** Process loopback requires build 20348+. Consumer Windows 10 tops out at 19045 and is end-of-life.

| Backend | When | Notes |
|---|---|---|
| WASAPI **process loopback, exclude-tree** (own PID) | Windows 11, after a successful **feature probe** (activation attempt at startup) | `ActivateAudioInterfaceAsync` + `AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK`. Exclude-tree also excludes our WebView2 child processes. The Rust `wasapi` crate's application-loopback client is `!Send`, and several endpoint methods are unavailable in this mode (no device period or mix format). COM objects are therefore owned on a dedicated MTA thread, and the capture format is specified explicitly (48 kHz float stereo). |
| WASAPI shared loopback on default render endpoint | Probe failed | Through `cpal`. It also captures our own playback, so in-app playback is disabled while recording. |
| Mic via WASAPI | Always | Subject to Windows mic privacy settings. Prefer the **communications** default device (the device the meeting app uses), not the console default. |

Windows test matrix: endpoint fallback, communications vs. console device, exclusive-mode and DRM-protected playback (which may be silent in loopback, and that is expected).

**Linux (stretch):** the default sink's `.monitor` source through PipeWire/Pulse (`capture-sink=true`).

### 5.3 Pipeline per track

The design has **two independent paths**. Inference can stall, crash or fall behind without ever affecting recording.

```
                    ┌──────────── RECORDING PATH (highest priority) ─────────────┐
AudioSource ─► SPSC ring A ─► [Record thread]  native-rate frames → FLAC chunk writer
 (RT callback:                 (per track)       2 s chunks, fsync per chunk, manifest row
  copy only,                                     per chunk (track, epoch, sample range, host_time)
  never blocks) ─► SPSC ring B ─┐
                    └────────────┼───────────────────────────────────────────────┘
                                 ▼  INFERENCE PATH (may lag or shed load)
                         [DSP thread] resample → 16 kHz mono (rubato, low-pass)
                                 ├─► AEC (mic only, reference = system, when enabled)
                                 ├─► Silero VAD (32 ms frames) → speech/non-speech flags
                                 └─► incremental 20–100 ms frames → streaming STT
                                                   │ Partial / Final (word timestamps)
                                                   ▼
                         [Speaker-seg worker] speaker-change detection on speech regions
                                 → speaker turns → embeddings → online clustering
                                                   ▼
                         [Speakers] map cluster → known person (if enabled)
                                                   ▼
                         transcript bus ─► store (segments, pass='live') + Tauri event to UI
```

- **Two rings per track.**
  - Ring A (recording) is sized for ≥ 10 s and drained by a thread that does nothing but encode and write.
  - Ring B (inference) is sized for 30 s. Overflowing it drops inference input only, and the drop is logged as a gap in the live transcript. Recording continues.
  - If Ring A ever overflows (disk stall), the gap is written to the manifest as an explicit `gap` row, so the timeline stays correct.
- **Threading:** STT, speaker segmentation and diarization run on a bounded worker pool. ONNX Runtime intra-op threads are **budgeted globally**: live STT 2 threads, speaker work 1 thread, refinement uses the remainder after the meeting. This stops workers oversubscribing cores during a call.
- **Streaming feed:** audio is fed to the streaming recognizer **incrementally** (every 20–100 ms), not as completed VAD regions. VAD gates which frames are fed and signals endpoints. This is what makes ≤ 1.5 s partials possible.
- **Timeline:**
  - Every frame carries `host_time` and `sample_index` (§4.3). Both tracks are mapped onto one session timeline from their hardware timestamps, never from callback-arrival time.
  - On macOS, both tracks share the aggregate device's clock domain (§5.2), so residual drift is handled by Core Audio drift compensation.
  - On Windows, inter-device drift is estimated from QPC-stamped device positions over a 30 s sliding window. It is corrected in the resampler for the inference path only. The recording path stores native-rate audio plus timestamps.
- **Segmenting for STT finals:** endpoints come from the recognizer and VAD, and are force-cut at 15 s on the lowest-energy frame. **STT segments are not speaker turns.** Speaker attribution is computed separately (§7.1) and words are assigned to turns by timestamp.
- **Latency targets** (M1 Pro, Balanced profile):

| Metric | Target |
|---|---|
| Speech → partial text on screen | ≤ 1.5 s |
| End of utterance → final text | ≤ 4 s |
| Final text → live speaker label | ≤ 1 s after final |
| Stop → refined transcript (1 h meeting) | ≤ 10 min (provisional; replaced by M0 benchmark results) |

### 5.4 Echo and bleed handling

When the user is on laptop speakers, the mic hears the remote participants. Without mitigation, their words appear twice: on System as "Speaker 1" and on Mic as "Me".

1. **Output-route detection.** Read the transport type of the output device the meeting app renders to: built-in speakers vs. headphones, Bluetooth or USB headset. With headphones, AEC is skipped. The route is re-evaluated on every route change (§5.5).
2. **AEC (speakers only).** WebRTC AEC3 through a Rust port (`aec3` or `sonora-aec3`, BSD-3, pinned version).
   - The near end is the mic and the far-end reference is the System track.
   - **Caveat:** the global tap is a mix of everything rendered, possibly to several output devices. It is therefore only a good reference when the meeting app renders to the same device the mic hears. When the meeting app's output device differs from the built-in speakers (for example external speakers, or HDMI), AEC is disabled and the UI recommends headphones.
   - AEC state is **reset on every route or format change**, and the delay estimator is re-converged.
   - Double-talk (both sides speaking at once) is an explicit test case (§19).
   - The raw mic audio is always what gets recorded. AEC applies only to the inference path, so the refinement pass can **replay AEC offline** with full look-ahead and better delay alignment.
   - macOS Voice-Processing I/O is **not** used: opening VPIO ourselves would duck other audio and could interfere with the call.
3. **Residual gate.** If a Mic segment's speech overlaps a System segment in time *and* its embedding is closer to the active remote speaker than to the user's voiceprint, the segment is down-weighted.
4. **Bleed annotation, not deletion.**
   - Mark a Final Mic segment as probable bleed when all of these hold:
     - it has ≥ 4 tokens;
     - its text has ≥ 0.8 token similarity to a System segment within ±2 s;
     - its speaker embedding does not match the user.
   - Bleed is stored as a **reversible annotation** (`is_bleed` + reason). It is hidden by default and restorable with one click.
   - Short replies ("yes", "okay"), genuine repetition and simultaneous speech are therefore never silently destroyed.

The meeting app's own echo cancellation does not help here. It processes the app's *outgoing* signal, not the raw mic stream that VanTranscribe sees.

### 5.5 Lifecycle, device changes and discontinuities

Real meetings include unplugged headsets, AirPods auto-switching, lids closing and permissions being revoked. Every such event is a **discontinuity** with an explicit state transition. Silent corruption is never acceptable.

| Event | Detection | Behavior |
|---|---|---|
| Default input/output device change, headset plug/unplug, AirPods auto-switch | Core Audio property listeners (`kAudioHardwarePropertyDefaultInputDevice`/`…Output…`, `kAudioDevicePropertyDeviceIsAlive`) · Windows `IMMNotificationClient` | Source → `Rebuilding`: tear down and rebuild the tap and aggregate device (or WASAPI client), the resamplers and AEC. Increment `epoch`. Write a `gap` manifest row with start and end host times. Resume. UI shows "Audio device changed, recording continued". |
| Bluetooth A2DP → HFP switch | Sample-rate / format change on the device | Treated as a device change. Channel quality is logged per epoch. |
| Sleep / lid close | `NSWorkspace.willSleepNotification` · `WM_POWERBROADCAST` | While recording, hold an `IOPMAssertionCreateWithName(kIOPMAssertionTypePreventUserIdleSystemSleep)` assertion (Windows: `SetThreadExecutionState`). On forced sleep (lid close), flush the current chunk, mark a gap, and resume on wake. |
| Permission revoked mid-meeting / sustained no-signal | Health monitor | `PermissionUncertain` banner. Mic-only recording continues. |
| Pause / resume | User | Explicit `pause` gap rows. Timeline continues. |
| App crash / force quit | Manifest has an unclosed session on next launch | Recover: every fsynced chunk is valid on its own. Close the session at the last good chunk, mark the meeting `recovered`, and offer to run refinement. |
| Disk nearly full | Free-space check every 30 s | Warn at < 2 GB free. At < 500 MB, stop inference, keep recording. At < 100 MB, stop recording cleanly with a notification. |

Long-meeting memory is bounded: live clustering state, partial transcripts and ring buffers have fixed caps. Multi-hour soak tests with injected transitions are part of §19.

### 5.6 Durable recording format

- **Format:** per-track **FLAC** chunks at native sample rate and channel count, 2 s per chunk. This is lossless and needs no resampling on the recording path. Opus was rejected for the archive because it is lossy and was resampled.
- **Location:** chunks are written to `meetings/<id>/audio/<track>/<epoch>-<seq>.flac`.
- **Durability:** each chunk is written to a temp name, fsynced, then renamed. Its manifest row (`audio_chunks`) is committed only after that rename.
- **Compaction:** after successful refinement, chunks are concatenated into one compressed **Opus 32 kbps** file per track for long-term storage (≈ 14 MB/hour/track). FLAC is kept only if "Keep lossless audio" is enabled. Compaction is itself crash-safe: the new file is written, verified and published before the chunks are deleted.
- **Encryption:** when at-rest encryption is on, chunks are encrypted as they are written (age, streaming). Refinement decodes into memory or into encrypted temp files. **No plaintext WAV is ever written to disk.**
- **Accounting:** each chunk records which sample ranges it covers. Gaps are explicit, so "audio lost" is always measurable, never assumed zero.

---

## 6. Speech-to-text engines

| Role | Default engine | Runtime | Why | Alternatives |
|---|---|---|---|---|
| **Live (streaming)** | **Streaming Zipformer (English)** via **sherpa-onnx** `OnlineRecognizer` | ONNX Runtime, **CPU EP by default** | True streaming, low CPU, cross-platform, one runtime for STT, VAD and embeddings | Parakeet EOU 120M (FluidAudio, Swift); Kyutai STT (heavier); multilingual streaming Zipformer models where available. Moonshine streaming is a separate integration, because sherpa-onnx currently exposes Moonshine v2 only as an *offline* recognizer. |
| **Refinement (batch)** | **NVIDIA Parakeet TDT 0.6B v3** INT8, sherpa-onnx's own `nemo_transducer` export | ONNX Runtime | 25 European languages, CC-BY-4.0. The ~2.5% EN WER figure is on clean read speech; meeting audio will be worse, and our own eval set (§19) is the source of truth. | Whisper large-v3-turbo via `whisper-rs` for non-European languages |
| **Multilingual fallback** | `whisper.cpp` (`whisper-rs`) large-v3-turbo | Metal / CPU | ~99 languages | |
| **Cloud (opt-in)** | Deepgram Nova (streaming WS), AssemblyAI Universal-Streaming | HTTPS/WSS | Best accuracy and built-in diarization for users who want it | OpenAI transcription API (batch) |

**Bindings.**
- Use the **official upstream `sherpa-onnx` Rust crate** (k2-fsa, v1.13.x, static linking by default). It covers the online and offline recognizers, VAD, speaker embeddings and offline speaker diarization.
- The community `sherpa-rs` crate is deprecated and read-only (June 2026), so it is not used.
- Pin the crate version and the native library version together. Every model must be **sherpa-onnx's own export** of that model. Third-party ONNX splits (for example the four-graph Parakeet export) are not drop-in compatible.

**Execution providers.**
- Default to the CPU EP.
- The CoreML EP (macOS) and DirectML EP (Windows) are opt-in per model, and only after M0 benchmarks show a win. Transducer models with dynamic shapes often fall back to CPU or run slower on CoreML.
- Published throughput numbers, such as FluidAudio's ~200x real time, were measured on CoreML/ANE with different code and do **not** transfer to this stack.

**Cloud STT behavior:** when a cloud provider is selected, the per-track 16 kHz PCM is streamed to it. Provider diarization labels are mapped into our `LocalSpeakerId` space, and local VAD still gates sending to reduce cost.

---

## 7. Speaker identification strategy

### 7.1 Live (online) diarization
- **Speaker turns are computed independently of STT segments.**
  - A 15 s STT final can contain several speakers, so it is never embedded as one unit.
  - On each track's speech regions, run **speaker-change detection**. The default is pyannote segmentation-3.0 (ONNX) on a sliding 5 s window, which also flags **overlapped speech**.
  - Each resulting turn ≥ 1.0 s is embedded with sherpa-onnx (default 3D-Speaker ERes2Net).
  - STT words are assigned to turns by timestamp midpoint.
- **Short or unclear turns** (< 1.0 s, or overlap-flagged) get the label **`unknown`** or **`overlap`**. They do *not* inherit the previous speaker, which would systematically mislabel interruptions and back-channel replies. The refinement pass resolves most of them.
- **Online clustering** runs per track:
  - Assign to the nearest cluster if similarity ≥ τ_assign *and* it beats the second-best cluster by ≥ margin m.
  - Otherwise open a provisional cluster, which becomes visible after ≥ 3 s of speech.
  - Clusters are represented by a **capped exemplar set** (up to 20 embeddings, cosine-to-mean pruning) instead of an EMA centroid. This avoids slow drift and chain-merging in long meetings.
  - Clusters are merged when exemplar-set similarity exceeds τ_merge. Labels already shown are re-keyed in the UI.
- **Threshold calibration:** τ_assign, τ_merge and m are **calibrated per embedding version** (§7.4) on the eval set and shipped in the model manifest. The values 0.55 and 0.70 are placeholders until calibration. Thresholds are never shared across embedding models.
- **Remote mode:** the System track is clustered and the Mic track is labeled "Me".
- **In-person mode:** the Mic track is clustered. If the user enrolled their voice, the cluster matching their voiceprint is labeled "Me".
- **Expectation:** live diarization error is substantially worse than offline (published streaming DER of about 30% vs. about 15% offline). The UI states that labels are refined after the meeting.
- **v1.x upgrade path:** an `OnlineDiarizer` implementation for NVIDIA Streaming Sortformer (4 speakers) once its license and the CoreML/ONNX port are validated.

### 7.2 Offline diarization (refinement pass)
- **Pipeline:** pyannote segmentation-3.0 (ONNX), then embeddings, then clustering.
  - sherpa-onnx's `OfflineSpeakerDiarization` provides segmentation and embedding extraction. Its clustering config exposes only a fixed `num_clusters` or a distance `threshold`.
  - Our extra logic is therefore implemented **in `vt-diarize`** on top of the embeddings sherpa-onnx returns:
    - priors from the live speaker count;
    - must-link and cannot-link constraints from user corrections;
    - exclusion of "Me" from the System track in remote mode.
  - The clustering is agglomerative with constraints, plus a speaker-count search scored by silhouette.
- **Overlap:** overlap-flagged regions keep **both** speaker labels when segmentation assigns two speakers. Otherwise they keep `overlap`.

### 7.3 Reconciling live labels with refined labels
The user may rename "Speaker 2" to "Jane" during the meeting. Renames, merges and splits are stored as **identity constraints** (§12) with provenance, not as edits to cluster IDs. After refinement:
1. Build a duration-weighted overlap matrix between live clusters and refined clusters.
2. Handle the **non-one-to-one** cases explicitly:
   - **Split:** one live cluster maps to two refined clusters, each with ≥ 30% of its time. If the live cluster was named, ask the user, showing a sample clip from each refined cluster. Otherwise create new labels.
   - **Merge:** two live clusters map mostly to one refined cluster. Keep a user name if exactly one of them was named. If both were named differently, raise a conflict for review.
   - **One-to-one:** solve the remaining clusters with the Hungarian algorithm, accepting a pair only if overlap is ≥ 50%.
3. User constraints always win over clustering. A must-link or cannot-link the user created is fed back into §7.2 and re-applied on every future re-processing run.
4. Every speaker assignment records its **provenance**: `user`, `voiceprint`, `refined-cluster` or `live-cluster`, plus a score.

### 7.4 Cross-meeting speaker memory (opt-in)
**Who is enrolled.** A voiceprint belongs to the *person being recorded*, who is usually **not** the app user. Enrollment therefore requires the participant-consent steps in §11.

**Enrollment.**
- When the user names a speaker, they can tick **"Remember this voice"** and must confirm the consent attestation for that person.
- The app stores the mean of that person's refined turn embeddings, requiring at least 20 s of clean, non-overlapped speech, in `voiceprints`, linked to a `person`.
- Voiceprints are stored **per channel type**:
  - `remote_wideband`: meeting codec, ≥ 16 kHz
  - `remote_narrowband`: HFP or phone
  - `in_person`: room mic
- A voice heard through a meeting codec matches poorly against the same person in a room, so matching only compares voiceprints of the same channel type.

**Embedding versioning.**
- Each voiceprint records an `embedding_version`: a hash over the model weights, the preprocessing (sample rate, feature config), the normalization, and the calibration-file version.
- Comparisons happen **only within the same version**. When the embedding model changes, existing voiceprints are re-computed from stored audio where retained. Otherwise they are marked stale and re-learned on the next confirmed match.

**Matching (open-set identification).** Each refined cluster is compared with the voiceprints of the same channel type and version:
- **Auto-label** ("✓ recognized") requires all of these:
  - score ≥ τ_auto;
  - a **margin ≥ m_id over the second-best person**;
  - at least 15 s of speech in the cluster.
- **Suggest** (one-click confirm) when the score is ≥ τ_suggest, or above τ_auto with an insufficient margin.
- Otherwise, no suggestion.

**Calibration.**
- Thresholds are calibrated per `embedding_version` to a target **false-identification rate** that accounts for gallery size, because the false-accept rate grows with the number of people enrolled.
- When the gallery has more than 50 people, τ_auto is raised automatically according to the calibration curve.
- Each speaker carries a "shared room" flag, for a conference room on one remote laptop. Flagged speakers are never used to create or update voiceprints, so a room of several voices cannot poison one person's voiceprint.

**Learning and expiry.**
- **Confirm-to-learn:** a user-confirmed match adds the cluster embedding to the person's exemplar set, capped at 10 exemplars per channel type with oldest-first eviction.
- **Auto-expiry:** voiceprints not matched in 12 months (configurable) are deleted automatically, per the published retention schedule in §11.

**Storage.** Voiceprints are biometric data. They are opt-in, stored only locally, encrypted at rest with a separate key (§11), listed in Settings → People, and deletable individually or all at once. Embeddings of **unnamed** speakers exist only in memory during processing and are **never persisted**.

---

## 8. Post-meeting refinement pass

Triggered by **Stop**, or resumable on next launch after a crash.

Every refinement is a new, immutable **processing run**. Re-running refinement later, for example with a better model or after the user fixes speakers, creates another run. It never mutates an old one.

1. **Seal audio.** Close the final chunk and verify the manifest (sample ranges, gaps). No audio is copied or decoded to plaintext on disk (§5.6).
2. **Create run.** Insert a `processing_runs` row (`status = running`) recording the engine versions, model hashes and the constraint-set version.
3. **Offline AEC replay** (speakers route only) on the mic track, in memory, using the system track as reference with full look-ahead.
4. **Batch STT** on each track's VAD speech regions only, with word timestamps. Parakeet v3 is the default, with Whisper for unsupported languages. Only speech is transcribed, which typically cuts compute by 30–50%.
5. **Offline diarization** (§7.2) on the System track (remote mode) or the Mic track (in-person mode), applying all user identity constraints.
6. **Align** words to speaker turns by midpoint. Split segments at speaker changes and keep `overlap`/`unknown` labels.
7. **Reconcile** live → refined speakers (§7.3) and **match voiceprints** (§7.4).
8. **Bleed annotation** across tracks (§5.4 step 4).
9. **Publish atomically.** In **one SQLite transaction**:
   - insert all run segments;
   - set `processing_runs.status = complete`;
   - point `meetings.active_run_id` at the new run;
   - update the FTS rows for that meeting.
   Audio files are never part of this transaction: audio is immutable and referenced by the manifest, so database and file consistency only requires that manifest chunks exist (checked in step 1).
10. **Re-apply corrections.**
    - The user's text corrections are stored as **overlays** anchored to time ranges (`segment_corrections`: track, start/end ms, original text hash, replacement), not as edits to segment rows.
    - Overlays are re-applied to the new run when their anchor still matches.
    - Overlays that no longer match are shown as "needs review", never dropped silently.
11. **Summarize** (§9) against the active run, if a provider is configured. Summary items cite segments **by run-scoped IDs**. When the active run changes, older summaries are marked `stale (based on an earlier transcript)` and can be regenerated.
12. **Compact** audio (§5.6) after a successful publish.

**Recovery and progress:**
- Progress is reported to the UI per stage.
- Stages run inside their run. A crash leaves a `running` run, which is marked `abandoned` on next launch, and refinement restarts in a fresh run. Partial runs are never published.
- The **live** pass is itself stored as run 0 (`kind = live`), so the meeting always has a viewable transcript.
- Abandoned and superseded runs are garbage-collected after 7 days.

**Encryption caveat:** when encryption at rest is on, intermediate artifacts (decoded PCM, embeddings, model outputs) stay in memory, or go to encrypted temp files under the meeting folder that are deleted at run end. If the encryption key is unavailable (for example the Keychain is locked), refinement waits rather than falling back to plaintext.

---

## 9. Summaries and AI notes

### 9.1 Providers (`Summarizer` trait)

| Provider | Availability | Context | Notes |
|---|---|---|---|
| **Apple Intelligence** (Foundation Models framework) | macOS 26+, Apple Intelligence enabled, supported hardware and region | **4,096 tokens per session**, covering instructions, schema, input and output together | On-device and free. Called through `swift/VTAppleBridge` (see §9.3). |
| **Local OpenAI-compatible server** | Ollama, LM Studio, llama.cpp server, MLX server | Model-dependent (8K–128K) | Auto-detected on `localhost:11434` / `:1234`. Recommended: Qwen3 8B/14B or Gemma 3 12B class on 16 GB+ machines. |
| **Cloud** | Anthropic (default model: latest Claude Sonnet), OpenAI, any OpenAI-compatible endpoint | Large | The user supplies a key, stored in the Keychain or Credential Manager. Only **transcript text** is sent, never audio, unless cloud STT is separately enabled. |
| **None** | | | Transcript only. |

### 9.2 Strategy
- **Token budgeting:** every request is budgeted as `instructions + schema + input + reserved output ≤ context`.
  - The budget is measured with the provider's tokenizer: `tokenCount(for:)` and `contextSize` on Foundation Models (macOS 26.4+), with a conservative estimate (chars ÷ 3) on earlier versions.
  - Other providers use their own token counting or an estimate.
- **Map-reduce for small contexts.** When the transcript plus overhead exceeds the budget:
  1. Chunk by speaker turns. For the 4K Apple model, chunk input is about **1.2–1.5K tokens**, leaving room for instructions, the `@Generable` schema and about 800 tokens of output.
  2. Each **map** call uses a **fresh session** and extracts compact notes as structured output: key points, action items (owner, task, due), decisions, open questions. Each note cites segments by **short integer IDs**, which a per-run lookup table maps to real segment IDs. UUIDs would waste tokens.
  3. **Recursive reduce:** if the accumulated notes still exceed the budget, reduce them in groups, repeating until one reduce fits.
- **Guardrails and refusals:** if Foundation Models refuses or hits a guardrail on a chunk, that chunk is retried once with a neutral prompt. After that, the meeting falls back to the next configured provider, or the summary is shown as partial with the gap flagged.
- **Single pass** when the transcript fits the context.
- **Templates** (user-editable, stored in `settings`): General, 1:1, Stand-up, Client call, Interview. Output sections: TL;DR, Key points, Decisions, Action items (owner, task, due), Open questions.
- **Grounding:** every action item and decision carries the `segment_id`s it came from. The UI links them to the transcript, which makes errors and hallucinations easy to spot.
- **Ask about this meeting** (v1.x): chat over one transcript. Cross-meeting Q&A comes in v2 with local embeddings.

### 9.3 Apple Foundation Models bridge
- **What `swift-rs` does and doesn't do:** it provides C-compatible calls only. It does not bridge Swift `async`, generics or `@Generable` types. The bridge is therefore a **job API**:
  - `vt_fm_availability() -> enum` (available, notEnabled, notSupportedDevice, modelNotReady)
  - `vt_fm_start(request_json, callback_ctx) -> job_id`
  - `vt_fm_cancel(job_id)`
  - `callback(ctx, job_id, status, result_json)`, invoked from a Swift `Task`
- All `@Generable` structs, prompts and session handling stay **inside Swift**. Rust sends and receives JSON only.
- **Linking:** FoundationModels is **weak-linked**, and every call is wrapped in `if #available(macOS 26, *)`, so the app still launches on macOS 14.4–15. The build uses the macOS 26 SDK.
- **Evaluate first:** the existing `fm-rs` crate. Adopt it if it covers structured output and cancellation. Otherwise build the bridge above.
- **Testing:** availability is tested in CI on a macOS 26 runner, and also on older macOS to confirm the app degrades gracefully.

---

## 10. Permissions and platform specifics

### 10.1 macOS
| Permission | Needed for | Info.plist key | Detection |
|---|---|---|---|
| Microphone | Mic track | `NSMicrophoneUsageDescription` | `AVCaptureDevice.authorizationStatus(for: .audio)` |
| **System Audio Recording** | Process tap | `NSAudioCaptureUsageDescription` (must be added manually, because Xcode omits it from its key list) | **No public API to query it.** A denied permission produces all-zero buffers, but so do muted, DRM-protected or genuinely silent output. Silence can't prove denial, so the app reports **`NoSignal` / "permission uncertain"**, never "denied". See the onboarding check below. |
| Screen Recording | Only for the ScreenCaptureKit fallback on macOS < 14.4 | | `CGPreflightScreenCaptureAccess()` |
| Notifications | Meeting-detected prompt | | `UNUserNotificationCenter` |

- **Onboarding capture check:**
  - Our own process is excluded from the tap, so a tone played *by VanTranscribe* can never verify capture.
  - Instead, onboarding plays a short known chirp through a **separate process** (`/usr/bin/afplay` on a bundled file) and cross-correlates it on the system track. This confirms permission, tap and routing in one step. On failure, the app deep-links to *System Settings → Privacy & Security → Screen & System Audio Recording*.
  - During meetings, `NoSignal` for > 30 s while a meeting app is active raises a non-blocking banner offering the same check.
- **Distribution:** Developer ID signed and **notarized** DMG, with auto-update through the Tauri updater (signed). Hardened runtime with the `com.apple.security.device.audio-input` entitlement. Not sandboxed in v1, because a private aggregate device plus a tap is simplest outside the App Store sandbox. Revisit for the Mac App Store later.
- **Homebrew cask** as a secondary channel.

### 10.2 Windows
- Microphone privacy setting (*Settings → Privacy → Microphone → Let desktop apps access*). Detected by the capture failing with `E_ACCESSDENIED`.
- Loopback capture needs no permission.
- Distribution as a signed MSIX or NSIS installer through the Tauri bundler. SmartScreen reputation requires a code-signing certificate. Azure Trusted Signing is the low-cost option.

---

## 11. Privacy, security and consent

- **No telemetry.** No analytics SDKs. Crash reports go to a local file. Optional, explicit "Send report" opens a prefilled GitHub issue.
- **Local storage** under the app data directory. Optional **at-rest encryption** of the database and audio uses SQLCipher plus age-encrypted audio. The key lives in the Keychain or Credential Manager. Voiceprints are always encrypted, using a separate column-level key.
- **Secrets** (cloud API keys) live only in the OS keychain, never in SQLite or config files.
- **Cloud transparency:** a persistent badge in the Live window shows when *any* data is leaving the device ("Cloud STT: Deepgram", "Summary: Anthropic").
- **Consent:**
  - A reminder appears at record start. It explains that some jurisdictions require all participants to consent (for example California, Florida, Illinois, Maryland, Massachusetts, Montana, New Hampshire, Pennsylvania and Washington) and that cross-state calls should be treated as all-party.
  - A one-click "copy disclosure" puts text on the clipboard to paste into the meeting chat. The text is editable in Settings.
  - The app is not legal advice, and the UI says so.
- **Recording consent and biometric consent are separate.**
  - Recording consent covers everyone in the meeting being recorded and transcribed.
  - Biometric consent covers a specific person's voiceprint being stored.
  - The person enrolled is usually **not** the app user, so the user's own opt-in does not cover other participants. BIPA §15, where it applies, requires informed **written** consent from the subject, a publicly available retention and destruction schedule, and no disclosure without consent. AI note-taker vendors faced suits on exactly this point in 2025–26.
- **Biometric data (voiceprints):**
  - Off by default. Turning it on requires an explicit toggle and an explanation (Illinois BIPA, Texas CUBI, Washington, GDPR Art. 9).
  - **Per-person attestation:** before saving a voiceprint for anyone other than the user, the user must confirm that the person consented in writing. The app records the attestation date, the method (free text, such as "email 2026-11-06") and the user who attested.
  - "Send consent request" generates a short, editable consent message to email or paste to the person.
  - A **published retention schedule** (shown in-app and in the repo's PRIVACY.md) states that voiceprints auto-expire after 12 months without a match, or immediately on request.
  - **Deletion propagation:** deleting a person deletes their voiceprints, attestation records and person links. Past transcripts keep the plain-text name unless the user also chooses "remove name from past meetings".
  - Embeddings of unnamed speakers are never persisted (§7.4).
  - The user's own voiceprint is enrolled with the user's own consent at onboarding (§3.4).
  - Settings → People has "Forget this person" and "Delete all voiceprints".
- **Logs:** never contain transcript text, speaker names or audio. Logs record IDs, timings and errors only. Debug logging of text needs an explicit, time-limited toggle.
- **Retention:** configurable auto-delete of audio after N days (default: keep), with the transcript kept separately. Deleting a meeting deletes its audio chunks, runs, segments, corrections, summaries and FTS rows.
- **Visible indicator:** the menu bar icon is red while recording and cannot be hidden.

---

## 12. Data model

### 12.1 Filesystem
```
~/Library/Application Support/VanTranscribe/        (%APPDATA%\VanTranscribe on Windows)
  vantranscribe.db                     SQLite (WAL), optionally SQLCipher
  meetings/<meeting_uuid>/
    audio/mic/<epoch>-<seq>.flac       durable 2 s chunks while recording (§5.6)
    audio/system/<epoch>-<seq>.flac
    mic.opus, system.opus              compacted archive after refinement
    meta.json                          capture devices per epoch, sample rates/codec class, app versions, engine profile
  models/<model_id>/...                downloaded models + manifest.json (sha256, license)
  logs/
```

### 12.2 SQLite schema (summary)

| Table | Key columns | Purpose |
|---|---|---|
| `meetings` | `id` (uuid), `title`, `started_at`, `ended_at`, `mode` (`remote`/`in_person`), `source_app`, `status` (`recording`/`processing`/`ready`/`recovered`/`failed`), `active_run_id`, `engine_profile` | One row per recording |
| `tracks` | `id`, `meeting_id`, `kind` (`mic`/`system`), `archive_path`, `duration_ms` | Logical audio track |
| `audio_chunks` | `id`, `track_id`, `epoch`, `seq`, `path`, `sample_rate`, `channels`, `first_sample`, `n_samples`, `host_time_start`, `kind` (`audio`/`gap`/`pause`), `gap_reason`, `device_uid`, `codec_class` | Durable recording manifest (§5.6). Gaps are explicit. |
| `processing_runs` | `id`, `meeting_id`, `kind` (`live`/`refine`), `status` (`running`/`complete`/`abandoned`/`superseded`), `engines_json` (names + weight hashes), `constraints_version`, `created_at` | Immutable transcript revisions (§8) |
| `segments` | `id`, `run_id`, `track_id`, `start_ms`, `end_ms`, `speaker_ref`, `text`, `words_json`, `confidence`, `is_bleed`, `bleed_reason` | Transcript lines, scoped to a run |
| `segment_corrections` | `id`, `meeting_id`, `track_id`, `start_ms`, `end_ms`, `original_text_hash`, `replacement_text`, `status` (`applied`/`needs_review`), `created_at` | Text-correction overlays that survive re-processing |
| `meeting_speakers` | `id`, `meeting_id`, `run_id`, `label` ("Speaker 2"), `person_id` (nullable), `assignment_source` (`user`/`voiceprint`/`refined-cluster`/`live-cluster`), `match_score`, `is_me`, `is_shared_room`, `color` | Speakers within a run, with provenance |
| `identity_constraints` | `id`, `meeting_id`, `kind` (`name`/`must_link`/`cannot_link`), `anchor_json` (track + time ranges), `person_id`, `created_at` | User speaker corrections, re-applied on every run (§7.3) |
| `people` | `id`, `display_name`, `email` (optional), `created_at` | Named people across meetings |
| `voiceprints` | `id`, `person_id`, `channel_type`, `embedding_version` (hash), `exemplars` (encrypted blob), `n_exemplars`, `last_matched_at`, `expires_at`, `updated_at` | Opt-in speaker memory (§7.4). Compared only within the same `channel_type` + `embedding_version`. |
| `biometric_consents` | `id`, `person_id`, `attested_at`, `method`, `attested_by`, `revoked_at` | Participant consent records (§11) |
| `notes` | `id`, `meeting_id`, `t_ms`, `text`, `kind` (`note`/`bookmark`) | User notes and bookmarks |
| `summaries` | `id`, `meeting_id`, `run_id`, `provider`, `model`, `template`, `content_md`, `items_json`, `is_stale`, `created_at` | AI output, possibly several per meeting. Becomes stale when `active_run_id` moves. |
| `settings` | `key`, `value_json` | Preferences, templates |
| `search_fts` | FTS5 (external-content), rows of (`meeting_id`, `source` = segment/summary/note, `ref_id`, `text`) | Search |

**FTS synchronization:**
- Segments are indexed **only for the active run**, inside the same transaction that moves `active_run_id`.
- Summaries and notes are indexed by triggers on insert, update and delete.
- A `rebuild-search-index` maintenance command regenerates the index from source tables, and also runs after migrations that touch indexed columns.

Migrations are managed with `refinery` or `sqlx` migrations, with `rusqlite` as the driver. The schema DDL will live in `documentation/features/storage/.../sql/` once implemented, per the documentation system.

---

## 13. UI

- **Menu bar / tray:** Record (remote) · Record (in-person) · Pause · Stop · Open library · recording timer · input level meters for both tracks.
- **Live window** (compact, dockable, optional always-on-top):
  - Speaker-colored lines, with partials in italic grey.
  - Auto-scroll that pauses when the user scrolls up.
  - Inline speaker rename.
  - Bookmark and note box.
  - Status chips: AEC on/off, models in use, cloud badge, dropped-frame warning.
- **Library:** list with search, filters and status.
- **Meeting detail** tabs:
  - **Transcript:** the refined transcript, editable text with corrections kept, click to seek.
  - **Summary:** the template picker, regenerate, copy, and links from summary items to source segments.
  - **Speakers:** merge or split speakers, assign people, "remember voice".
  - **Audio:** a waveform per track with speaker lanes. Playback is done by the Rust core, not WebView `<audio>`, and is **disabled while any recording is active** (§5.2).
- **Export:** Markdown (default, with front-matter), TXT, SRT, VTT, JSON (full fidelity). DOCX and Confluence come in v1.x.
- **Settings:** General · Capture (devices, AEC mode, meeting detection apps) · Engines and models (profile, downloads, disk use) · AI summaries (provider, model, templates) · People and voiceprints · Privacy and storage (encryption, retention, consent text) · Advanced (debug panel, logs).
- **Styling:** Bootstrap 5, light and dark themes, keyboard-first (`⌥⌘R` record, `⌥⌘B` bookmark, `⌘F` search).

---

## 14. Meeting detection

Detection is **advisory only**. It suggests; it never decides on its own.

- **macOS:**
  - Listen to the Core Audio process list (`kAudioHardwarePropertyProcessObjectList`, with `kAudioProcessPropertyIsRunningInput` per process, macOS 14+). Fall back to polling every 2 s.
  - Browser and Electron audio runs in **helper processes** (Chrome Helper, WebKit WebContent, Teams/Slack helpers). Each process object's PID is mapped to its **responsible parent app** (`responsibility_get_pid_responsible_for_pid`, falling back to the parent-PID walk) before matching bundle IDs (`us.zoom.xos`, `com.microsoft.teams2`, `com.tinyspeck.slackmacgap`, `com.cisco.webexmeetingsapp`, browsers).
- **Windows:** enumerate `IAudioSessionManager2` capture sessions and map them to process names, applying the same parent mapping for browser child processes.
- **Confidence:**
  - Native meeting apps using the mic → "Meeting detected in Zoom".
  - A **browser** using the mic is *not* evidence of a meeting, because it could be dictation or a voice memo. The prompt is phrased "Chrome is using the microphone. Record?" and can be suppressed per app.
- **Behavior:**
  - A notification prompt appears, never auto-recording by default. An optional per-app "always record" setting is available.
  - **No unattended auto-stop.** When the meeting app releases the mic for more than 60 s, the app shows a non-blocking "Meeting may have ended. Stop recording?" prompt. Recording continues until the user acts.
  - Never prompt in **in-person mode**, where there is no meeting app.

---

## 15. Performance budgets and model packs

| Profile | Live STT | Refinement STT | Embeddings | Approx. download | Target machine |
|---|---|---|---|---|---|
| **Lite** | Streaming Zipformer EN (small) | Whisper small/base (`whisper-rs`) or a small sherpa-onnx offline model | 3D-Speaker small | ~300–500 MB | 8 GB Apple Silicon / 4-core x86 |
| **Balanced** (default) | Streaming Zipformer EN | Parakeet v3 INT8 (encoder alone ≈ 620 MB) | 3D-Speaker ERes2Net | ~1.1 GB | 16 GB M1+ |
| **Accurate** | Streaming Zipformer EN, then Parakeet re-decode of each final (3 s windows) | Parakeet v3 FP16 + Whisper large-v3-turbo fallback | WeSpeaker ResNet34 | ~3 GB | 32 GB M-Pro/Max |

Every pack also includes Silero VAD and pyannote segmentation-3.0 (a few MB each). Sizes are estimates, to be replaced by the actual model manifest sums.

**Budgets on an M1 with 16 GB, Balanced profile, during a live call:**

| Metric | Budget |
|---|---|
| Sustained CPU, live | ≤ 25% of total |
| RSS | ≤ 1.2 GB |
| Battery impact vs. idle call | ≤ +15% |
| Refinement pass, 1 h meeting (≈ 2 h of track audio) | ≤ 10 min provisional, CPU EP. Re-set from M0 benchmarks. |
| Disk while recording (FLAC, 48 kHz stereo system + mono mic) | ≈ 1–1.5 GB per hour, temporary until compaction |
| Disk after compaction (Opus 32 kbps per track) | ≈ 14 MB per hour per track (24 kbps would be ≈ 10.8 MB) |

All figures in this table are **targets to be measured in M0** on the reference machine: cold start, steady-state CPU for two tracks, thermal throttling after 60 min, and the full refinement time. Figures borrowed from other projects are not used as commitments.

If the live pipeline falls behind real time for more than 10 s, it **sheds load** in this order: stop partials, then stop live diarization (labels arrive after the meeting), then stop live STT (audio-only, refined later). The UI says what is happening. Audio is never dropped.

---

## 16. Licensing

- **VanTranscribe source:** MIT.
- **Models are never bundled in the installer.** They are downloaded on first run from their canonical source (Hugging Face / GitHub releases) with a sha256 manifest. Each model's license and attribution is shown in Settings → Models and in the About box.

| Component | License | Notes |
|---|---|---|
| sherpa-onnx, ONNX Runtime | Apache-2.0 / MIT | |
| Parakeet TDT 0.6B v3 | CC-BY-4.0 | Attribution required (About box) |
| Silero VAD | MIT | |
| Moonshine | English models MIT. **Non-English models reportedly under a non-commercial community license** (reviewers disagree; verify). | Not in default packs |
| pyannote segmentation-3.0 | MIT (© CNRS). The Hugging Face gate is an access form, not a license term. | Keep the CNRS copyright notice. Download the sherpa-onnx ONNX export. |
| 3D-Speaker / WeSpeaker embeddings | Apache-2.0 | |
| whisper.cpp / Whisper weights | MIT | |
| NVIDIA Streaming Sortformer v2.1 | NVIDIA Open Model License per reviewers (some mirrors say CC-BY-4.0). Confirm on the official `nvidia/diar_streaming_sortformer_4spk-v2.1` card. | Optional v1.x engine, downloaded only on opt-in. Consider the `parakeet-rs` crate, which already runs Sortformer. |
| WebRTC AEC3 ports | BSD-3 | |
| Tauri, Svelte, Bootstrap | MIT/Apache-2.0 | |

**Artifact-level provenance.** The model manifest records, for every downloaded file:
- source URL;
- sha256;
- exact license text and version;
- required notices;
- conversion provenance: who exported it to ONNX, from which upstream weights, with which tool version;
- the acquisition terms accepted (for example HF access forms).

Native libraries (ONNX Runtime, sherpa-onnx, whisper.cpp) ship their license files in the app bundle. Test fixtures in §19 must have documented redistribution rights, or they stay out of the public repo.

---

## 17. Windows plan (M5)

| Concern | macOS | Windows |
|---|---|---|
| Supported OS | macOS 14.4+ | **Windows 11** (process loopback needs build 20348+) |
| System audio | Core Audio process tap | WASAPI process loopback (exclude own process tree) after a feature probe, falling back to shared loopback |
| Mic | cpal / CoreAudio | cpal / WASAPI |
| Output-route detection | Device transport type | `IMMDevice` form factor (`Headphones`/`Headset` vs. `Speakers`) |
| ONNX acceleration | CoreML EP | DirectML EP, falling back to CPU (AVX2) |
| On-device LLM | Apple Foundation Models | Ollama/LM Studio by default. Phi Silica (Windows AI APIs) requires **package identity (MSIX)** and a Limited Access Feature unlock, so it is offered only in the MSIX build on Copilot+ PCs. |
| Secrets | Keychain | Credential Manager (`keyring` crate) |
| Meeting detection | Core Audio process list | `IAudioSessionManager2` |
| Signing | Developer ID + notarization | Authenticode (Azure Trusted Signing) |

Everything above the platform layer (§4.1) is shared code.

---

## 18. Roadmap

| Milestone | Scope | Exit criteria |
|---|---|---|
| **M0: Reliable capture + benchmarks** (macOS) | Tauri skeleton. Aggregate device (mic + tap) excluding our process **and WebKit helpers**, with drift compensation. Durable FLAC chunk writer and manifest. Crash recovery. Device-change, sleep/wake and HFP handling (§5.5). Level meters. External-chirp onboarding check. **Benchmark spike:** official `sherpa-onnx` crate running streaming Zipformer, Parakeet v3, segmentation and embeddings on the CPU EP vs. CoreML EP; cold start, two-track CPU, refinement time. | A 30-min Zoom call yields two in-sync tracks (< 50 ms drift) with no self-capture. Killing the app mid-call loses ≤ 2 s. Unplugging a headset mid-call produces a gap row and continued recording. Benchmark numbers replace the provisional targets in §5.3 and §15. |
| **M1: Live transcript, Me/Them** | Streaming STT fed incrementally, Silero VAD per track, live window, SQLite store with runs, Markdown export, Rust-core playback | Partials ≤ 1.5 s. Live WER within 1.5x of the batch engine on a test set. |
| **M2a: Refinement** | Parakeet refinement on VAD speech, processing runs, active revision, correction overlays, FTS sync | A refined transcript is published atomically. Corrections survive re-runs. |
| **M2b: Diarization** | Speaker-change detection, online exemplar clustering, offline constrained clustering, split/merge reconciliation, in-person mode, threshold calibration | DER ≤ 20% on a 4-speaker in-house test set after refinement (collar 0.25 s, overlap scored; definition fixed in the eval harness) |
| **M2c: Echo** | AEC3 live and offline replay, route detection, bleed annotation | Probable-bleed lines < 2% on speakers. Zero lost genuine short replies on the double-talk set. |
| **M3: Summaries** | `Summarizer` trait, Ollama/OpenAI-compatible, Anthropic/OpenAI cloud, Apple Foundation Models bridge, map-reduce, templates, grounded action items | A 1 h meeting summarized on Apple FM in < 60 s. Action items link to sources. |
| **M4: Memory and search** | Voiceprints (opt-in, encrypted, per channel type, versioned), participant consent attestation and expiry, people management, search UI, advisory meeting detection, encryption at rest, retention | Recognition ≥ 90% for enrolled voices with false-identification ≤ 2% **at a gallery of 50 people** on the test set |
| **M5: Windows 11** | WASAPI process loopback with feature probe, dedicated COM thread, device-change handling, CPU/DirectML EP, Credential Manager, NSIS + optional MSIX, signing | Parity with M4 on a Win 11 x64 laptop |

Each milestone ships its feature docs, baselines and trail entries under `documentation/features/` as its behavior lands (development-documentation-system). AEC (M2c), diarization (M2b) and voiceprints (M4) are gated **independently**: each can slip without blocking the others.
| **v1.x** | Sortformer live diarization, DOCX/Confluence export, calendar titles, "ask this meeting" | |
| **v2** | Cross-meeting Q&A, semantic search, mobile companion for in-person, optional self-hosted sync | |

---

## 19. Testing strategy

- **Audio fixtures:** a curated set of recordings with consent: 1:1, 4-person remote, in-person round table, speakers vs. headphones, and accents. Ground-truth RTTM and transcripts are stored with Git LFS.
- **Metrics harness** (`cargo xtask eval`): WER (STT), DER/JER (diarization), bleed-duplicate rate, live latency percentiles, CPU and RSS. It runs on every PR for a 5-min subset and nightly on the full set.
- **Capture tests:**
  - A chirp played by an **external process** must appear on the System track.
  - A chirp played by the app's Rust core **and by its WebView** must be absent (self-exclusion, including WebKit helpers).
  - Measure drift over 60 min.
- **Durability and fault injection:**
  - SIGKILL during recording: at most 2 s lost, and the session recovers.
  - Slow storage, simulated with a throttled disk image.
  - Disk full.
  - Keychain locked during refinement.
  - Abandoned processing runs.
- **Lifecycle soak tests:** 3-hour sessions with scripted device switches (headset plug/unplug, AirPods A2DP↔HFP, default-output change), sleep/wake, pause/resume and permission revocation. Assert bounded memory, continuous timelines and explicit gaps.
- **Double-talk and bleed set:** speakers-route recordings with simultaneous speech and short replies, to verify that AEC and bleed annotation never hide genuine speech.
- **Diarization metric definition:** DER with a 0.25 s collar, overlap scored, computed with `pyannote.metrics`-equivalent logic in the harness. The definition is fixed so numbers stay comparable over time.
- **Unit tests:** clustering, reconciliation (Hungarian mapping), map-reduce chunking, FTS, migrations.
- **Manual QA matrix:** Zoom / Teams desktop / Meet in Chrome and Safari × headphones / speakers / Bluetooth × macOS 14, 15, 26.

---

## 20. Risks and open questions

| # | Risk / question | Impact | Mitigation / next step |
|---|---|---|---|
| R1 | Official `sherpa-onnx` Rust crate coverage and stability (it replaced the deprecated `sherpa-rs`) | Medium | M0 benchmark spike exercises every API we need. Pin crate and native versions together. |
| R2 | Process taps from Rust via `objc2-core-audio` (`!Send` types) vs. a Swift helper | Medium | Confine Core Audio objects to one control thread. The Swift bridge exists for Apple FM, so capture can move there if needed. |
| R3 | No API to query the System Audio Recording permission; silence is ambiguous | High (UX) | Report "permission uncertain", never "denied". Mandatory onboarding check with an **external-process** chirp (§10.1). |
| R12 | Self-capture through WebKit helper processes | Medium | Exclude helper process objects, play audio from the Rust core, disable playback while recording, and an automated test (§19). |
| R13 | Recording durability (disk stalls, crashes, device changes) | High | Separate recording path, fsynced chunks, explicit gaps (§5.5–5.6), fault-injection tests. |
| R14 | Re-processing invalidating user corrections and summary references | Medium | Immutable runs, correction overlays, identity constraints, stale-summary marking (§8). |
| R15 | Scope: full roadmap kept by owner decision despite reviewer advice to cut | High (schedule) | Independent gates for M2a/b/c and M4. Every milestone ships usable value on its own. |
| R4 | Live diarization quality (about 30% DER) disappoints users | Medium | Set expectations in the UI. The refinement pass fixes it. Sortformer in v1.x. |
| R5 | AEC quality on speakers, and interaction with Bluetooth latency | Medium | AEC3 with delay estimation, plus text de-duplication. Recommend headphones in onboarding. |
| R6 | Apple Foundation Models context size, rate limits, availability (region, language, hardware) | Low–Medium | Map-reduce. Detect availability at runtime and hide the option when unavailable. |
| R7 | Model license changes or gated downloads (pyannote, Sortformer) | Medium | License registry, download-on-demand, prefer Apache/MIT/CC-BY models for defaults. |
| R8 | Windows process-loopback build floor; fully unsupported on older Win 10 | Low | Fall back to shared loopback and mute in-app playback while recording. |
| R9 | Legal exposure from recording without consent | High (user) | Consent reminder, disclosure text, visible indicator, local-only default. Not legal advice. |
| R10 | Thermal and battery impact on long meetings | Medium | Load-shedding ladder (§15), Lite profile, defer refinement while on battery (setting). |
| R11 | Competitive overlap with Meetily / Anarlog | Low | Our differentiators are free diarization, cross-meeting speaker memory, Apple Intelligence summaries, and a strong in-person mode. Evaluate reusing MIT-licensed code from Meetily or Anarlog where it makes sense. |

**Verified in review:**
- `NSAudioCaptureUsageDescription` exists, and there is no public permission preflight.
- `stereoGlobalTapButExcludeProcesses` and `muteBehavior` exist.
- Parakeet v3 INT8 ships in sherpa-onnx as `nemo_transducer` (CC-BY-4.0).
- The Apple FM session context is 4,096 tokens.
- Windows process loopback requires build 20348.
- `sherpa-rs` is deprecated; the official crate supports diarization and embeddings.
- `objc2-core-audio` binds `CATapDescription`.

**Still to verify before or in M0:**
- Whether macOS 26 changes System Audio Recording permission behavior.
- Moonshine streaming availability and the license for non-English models.
- The FluidAudio license (a possible Swift alternative for macOS).
- Whether `fm-rs` is good enough to adopt.
- The official Sortformer v2.1 license.
- Whether `responsibility_get_pid_responsible_for_pid` is still usable (it is a private SPI) or the parent-walk fallback is needed.

---

## 21. Prior art and references

**Existing projects**
- Meetily (MIT, Tauri/Rust; diarization in paid Pro): https://github.com/Zackriya-Solutions/meetily
- Anarlog, formerly Hyprnote/Char (MIT, Tauri): https://github.com/fastrepl/anarlog · https://char.com/docs/about/what-is-hyprnote
- Scriberr (self-hosted batch transcription with diarization): https://github.com/rishikanthc/Scriberr
- Screenpipe: https://screenpi.pe/about
- OpenWhispr (WebRTC AEC sidecar): https://openwhispr.com/changelog
- Zoom My Notes: https://www.zoom.com/en/products/ai-assistant/features/ai-note-taking

**Audio capture**
- Apple, Capturing system audio with Core Audio taps: https://developer.apple.com/documentation/coreaudio/capturing-system-audio-with-core-audio-taps
- Apple forums, taps vs. ScreenCaptureKit and permission behavior: https://developer.apple.com/forums/thread/756428 · https://developer.apple.com/forums/thread/771864
- insidegui/AudioCap: https://github.com/insidegui/AudioCap
- Recall.ai, system audio access overview: https://www.recall.ai/blog/how-to-get-access-to-system-audio
- cpal macOS loopback source: https://docs.rs/crate/cpal/latest/source/src/host/coreaudio/macos/loopback.rs
- screencapturekit-rs: https://github.com/svtlabs/screencapturekit-rs
- Microsoft, loopback recording: https://learn.microsoft.com/windows/win32/coreaudio/loopback-recording
- Microsoft, Application Loopback sample: https://learn.microsoft.com/en-us/samples/microsoft/windows-classic-samples/applicationloopbackaudio-sample/
- `wasapi` crate: https://docs.rs/wasapi/latest/wasapi/struct.AudioClient.html
- Two-track capture write-up (cpal + WASAPI): https://dev.to/madnessbrains_i/capturing-the-mic-and-the-call-as-two-separate-tracks-on-windows-with-cpal-and-wasapi-loopback-3do6
- AEC3 Rust ports: https://github.com/rubybit/aec3-rs · https://docs.rs/crate/sonora-aec3/latest/source/README.md

**Speech and diarization**
- FluidAudio benchmarks (Parakeet, Sortformer, DER figures): https://docs.fluidinference.com/reference/benchmarks.md · https://github.com/fluidinference/fluidaudio
- Parakeet v3 ONNX: https://huggingface.co/talatapp/parakeet-tdt-0.6b-v3-onnx-split
- Moonshine v2 paper: https://arxiv.org/html/2602.12241
- sherpa-onnx: https://github.com/k2-fsa/sherpa-onnx · official Rust crate: https://docs.rs/sherpa-onnx · Moonshine v2 Rust example (offline recognizer): https://github.com/k2-fsa/sherpa-onnx/blob/master/rust-api-examples/examples/moonshine_v2.rs
- sherpa-onnx NeMo transducer models (Parakeet v3): https://k2-fsa.github.io/sherpa/onnx/pretrained_models/offline-transducer/nemo-transducer-models.html
- `sherpa-rs` (deprecated): https://github.com/thewh1teagle/sherpa-rs
- `objc2-core-audio` CATapDescription: https://docs.rs/objc2-core-audio/latest/objc2_core_audio/struct.CATapDescription.html
- Apple, CATapDescription.muteBehavior: https://developer.apple.com/documentation/coreaudio/catapdescription/mutebehavior
- Apple, Foundation Models context window: https://developer.apple.com/documentation/foundationmodels/managing-the-context-window
- `swift-rs`: https://github.com/Brendonovich/swift-rs · `fm-rs`: https://docs.rs/crate/fm-rs
- Windows process-loopback activation type (build 20348): https://learn.microsoft.com/windows/win32/api/audioclientactivationparams/ne-audioclientactivationparams-audioclient_activation_type
- Illinois BIPA (740 ILCS 14): https://www.ilga.gov/Legislation/ILCS/Articles?ActID=3004&ChapterID=5
- pyannote community-1: https://pyannote.ai/blog/community-1
- Streaming Sortformer GGUF port: https://huggingface.co/handy-computer/diar_streaming_sortformer_4spk-v2.1-gguf
- WhisperKit / SpeakerKit: https://github.com/argmaxinc/WhisperKit · https://www.argmaxinc.com/blog/speakerkit

**Summaries**
- Meetily on summary quality (local vs. cloud): https://meetily.ai/blog/our-quest-for-meeting-summary-accuracy

**Legal**
- Bloomberg Law, AI notetaking pitfalls: https://news.bloomberglaw.com/us-law-week/ai-notetaking-poses-wiretapping-discovery-and-ethical-pitfalls
- AI notetaker lawsuits 2026: https://blog.buildbetter.ai/ai-notetaker-lawsuits-2026-how-to-record-meetings-legally/
