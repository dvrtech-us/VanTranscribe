# Saved recording playback

`vt-playback` plays saved native FLAC from the Rust core through CPAL. It never uses WebView audio, creates a decoded WAV, changes recorded audio, or rewrites the recording database. Opening a meeting/player is silent: the output device is opened only after an explicit Play action.

## User flow and integration

1. The desktop service selects a saved meeting and opens `Playback` against its configured data root.
2. Explicit Play creates the current default native output stream and fills a bounded queue before starting it.
3. Mic and System audio share the manifest's session timeline. Recorded gaps and missing track intervals produce silence; one track's silence does not suppress the other.
4. Pause releases the output stream. Resume creates a fresh stream at the last consumed source-frame cursor. Seek discards the old stream/read-ahead queue and starts at the requested time, preserving whether playback was paused or running. Stop releases output and resets position to zero.
5. Independent gains from 0 to 1 apply to queued Mic/System frames; zero mutes a track. Replaying from the end restarts at zero.
6. End-of-timeline or device/decode failure stops output and remains visible in `PlaybackSnapshot`. A later Play resolves the default output route again instead of pretending that a disconnected stream still advances.
7. The caller drops the player on exit or before capture. The desktop integration must block all playback while any recording is active; this crate has no knowledge of another service's capture state.

## Technical flow

`Manifest::open` validates the meeting identity and opens SQLite with read-only/no-follow flags. It loads at most 100,000 chunk metadata rows, retaining independent, nonoverlapping Mic/System ranges. Pending audio is rejected until recorder recovery; explicit gap rows remain silent. Ready paths must exactly match `meetings/<meeting>/audio/<track>/<epoch>-<seq>.flac`, and hashes must be 64 hexadecimal characters. The timeline duration is the maximum chunk end and saved desktop meeting duration, retaining intentional leading/trailing silence.

Audio opens through a held `cap_std::fs::Dir` capability. Relative symlinks cannot escape the configured root, including path-resolution races. A regular encoded file is limited to 64 MiB and verified by SHA-256 before decoding. FLAC must match manifest rate, channels, 24-bit depth and frame count; each audio chunk is at most two seconds. Short, extra, malformed or mismatched samples fail visibly rather than being silently substituted with speech or a changed clock.

The decode worker caches at most one decoded two-second chunk per track. Mono is duplicated into stereo; stereo is preserved; sources with more channels are averaged to stereo mono. Native samples are linearly interpolated at output-frame session times, with integer/rational frame-to-time arithmetic preventing accumulated time drift. Chunk boundaries reset interpolation to the new range; no interpolation bridges a gap. This playback resampling is not a high-order anti-aliasing/archival conversion pipeline.

A fixed 8,192-frame `rtrb` SPSC queue stores separate track stereo frames and source positions. Its maximum queued source duration is 1.024 seconds at the minimum 8 kHz output rate (about 171 ms at 48 kHz). CPAL's callback fills output without allocations, file/DB access, decoding or locks. It applies current atomic track gains, sums/clamps to −1…1, converts to the device sample format, and advances the consumed source cursor. Mono output averages stereo; output channels above two remain silent. An underrun outputs silence, reports `buffering`, and does not advance/skip source audio. Position is measured at callback consumption, so hardware/device buffering can add audible latency; it is not a wall-clock estimate of the DAC position.

Pause/seek/stop release the prior native stream and queue. Resume/play/playing-seek query and rebuild the default output route, avoiding stale read-ahead frames after a control change. CPAL 0.18 streams are created paused; this implementation calls `play` only after prefill. Output formats are limited to 8–192 kHz and 1–8 channels. Device errors stop the source clock and surface through the snapshot. Dropping the player requests cancellation, silences callbacks and joins the worker; cancellation is observed between bounded render/decode operations.

## Waveforms

`waveform(root, meeting, bins)` reads/validates the same manifest and computes 1–2,048 peak bins per track with one decoded chunk at a time. Explicit gaps remain zero. It is an offline worker API with no output or disk cache; the desktop should run it outside its UI/audio callback and reject or defer it during active recording. It scans the recording's chunks, so long recordings still require proportional CPU/I/O even though PCM memory is bounded.

## Public types and entry points

| Entry | File | Role |
|---|---|---|
| `Playback`, `PlaybackSnapshot` | `crates/vt-playback/src/lib.rs` | Silent load, explicit playback/control, sample-driven state and bounded worker lifetime |
| `AudioSinkFactory`, `AudioSink`, `OpenedSink`, `OutputFormat` | `crates/vt-playback/src/lib.rs` | Injectable worker-owned native/mock output boundary |
| `AudioRenderer` | `crates/vt-playback/src/lib.rs` | Allocation-free queue consumption and track gain/mix |
| `waveform`, `Waveform` | `crates/vt-playback/src/lib.rs` | Bounded-memory per-track peak analysis |
| `Manifest`, `Timeline` | `crates/vt-playback/src/manifest.rs` | Read-only validation, capability file reads, bounded FLAC decode/cache and timeline interpolation |
| `CpalFactory` | `crates/vt-playback/src/output.rs` | Current default native output route, supported sample conversion and error reporting |
| Deterministic mock fixtures | `crates/vt-playback/src/tests.rs` | Generated FLAC, silence/gaps, controls, integrity/path and output tests |

`Playback::open(&Path, &str) -> Result<Playback>` loads metadata only. `play`, `pause`, `seek(position_ms)`, `stop`, and `set_volumes(mic, system)` return `Result<()>`; `snapshot()` returns serializable meeting/status/position/duration/gain/error fields. Status is `paused`, `playing`, `buffering`, `stopped`, `ended`, or `error`. Out-of-range seeks clamp to the timeline; invalid/nonfinite volumes are rejected.

## Database schema and SQL artifacts

The crate owns no tables or SQL artifacts and performs no database mutations. It reads recorder `meetings` and `audio_chunks`; when present, desktop `app_meetings.duration_ms` supplies the elapsed-duration tail. See `documentation/features/recording/durable-recording/sql/001-recording.sql` and `documentation/features/app/desktop-session/sql/001-desktop.sql`.

## Routes and access control

There are no network routes or account roles. The desktop service owns Tauri commands, the recording/playback exclusion lock and user intent. Only an explicitly selected local data root is read. A mock sink is opt-in injection for tests; production output uses CPAL. There are no network requests, automatic output, plaintext temporary audio or meeting-content logs in the crate.

## Primary API references

- [CPAL 0.18.2](https://docs.rs/cpal/0.18.2/cpal/): native output creation, supported formats and paused stream construction.
- [CPAL StreamTrait](https://docs.rs/cpal/0.18.2/cpal/traits/trait.StreamTrait.html): explicit stream start and device/route failures.
- [rtrb](https://docs.rs/rtrb/0.4.1/rtrb/): preallocated, wait-free single-producer/single-consumer queue.
- [cap-std directory API](https://docs.rs/cap-std/4.0.3/cap_std/fs/struct.Dir.html): capability-relative read-only file access.

## Verification limits

Deterministic tests use generated native-rate FLAC and a mock callback sink, never user recordings or actual output. They verify shared timeline/stereo/gap mixing, seek/pause/resume/stop/drop, track mute/gain, underrun cursor behavior, waveform gaps, resampling times, corrupt-data visibility, read-only preservation and symlink/path rejection. Native speaker output requires a separately authorized synthetic/public-audio smoke test; passing mocks does not prove route changes, physical latency, all sample formats or Windows/Linux runtime behavior. macOS Apple Silicon is the current runtime target; CPAL provides the platform output boundary for later Windows/Linux validation.


On 2026-11-06, `cargo test -p vt-playback --offline` passed all seven deterministic tests and `cargo clippy -p vt-playback --all-targets --offline -- -D warnings` passed. The separately authorized native check ran `cargo run -p vt-playback --example synthetic_output --offline -- --mute`. Both gains were set to zero before explicit Play; real CPAL callbacks advanced to 32 ms, resumed at 532 ms after a paused 500 ms seek, and reached 282 ms after a playing 250 ms seek. Pause position stayed stable and Stop returned to zero. The example created only its own temporary synthetic FLAC, never opened a user recording and emitted no audible signal. This proves callback/control integration on the current default route, not audible fidelity or broad device behavior.

The smoke example requires an explicit `--mute` or `--audible` argument and is never run on app launch. Use `--mute` for silent callback verification; audible tests require separate user intent. Worker panics become visible generic failures, and per-stream generation tokens prevent obsolete callbacks/errors from affecting a replacement stream.

## Shared saved-inference reader

`SavedAudioReader` in `crates/vt-playback/src/reader.rs` reuses the same capability-root path, hash, format and frame-count validation without opening an audio device. It holds one decoded native chunk per track and emits chronological mono slices of at most 20 ms, with System first on equal timestamps and explicit gap events. Session replay/refinement and offline speaker work use this reader through their echo-processing stage. The source archive, waveform and playback continue to use original samples. The native-rate, stereo/gap/tie-order/sample-total/progress regression verifies unchanged encoded bytes and hashes.
