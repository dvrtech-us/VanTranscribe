# Saved playback baseline

## Flow invariants

- Player construction does not open/start native output; only explicit Play creates a stream.
- All audio output originates in the Rust core. No WebView audio or plaintext decoded file is created.
- The caller prohibits playback while any recording is active and drops playback before capture/exit.
- Manifest/database access is read only; playback never recovers, repairs or changes recorded chunks.
- Ready audio paths match their meeting/track/epoch/sequence identities; capability-relative opens cannot escape the data root through symlinks.
- Encoded file SHA-256, 24-bit FLAC geometry, exact frame count and two-second source bounds are checked before frames enter output.
- Mic/System ranges remain on the common session timeline. Explicit gaps, leading/trailing silence and absent track ranges remain silence.
- Decoded PCM cache holds at most one two-second chunk per track; queued output is limited to 8,192 frames.
- Output callbacks perform no allocation, locks, decoding, file I/O or SQLite work.
- Position follows consumed source frames; underruns output silence without advancing or skipping source speech.
- Pause/seek/stop discard the old stream and queue. Paused seeks remain silent; playing seeks resume from the selected source position.
- Volume updates apply to queued frames, are finite in 0…1, and zero mutes a track. Summed output is clamped to −1…1.
- Device/decode failures stop the source clock and remain visible. Explicit retry rebuilds the current default route.
- Drop cancels, silences and joins the playback worker; no detached output continues after disposal.
- Waveform analysis uses bounded PCM memory and never starts audio or writes a cache/database.

## Failure and flag rules

| Condition | Severity | Blocks? |
|---|---|---|
| Recording is active | Caller-enforced exclusion | All in-app playback |
| Unknown meeting, invalid identity or overlapping same-track ranges | Error | Load |
| Pending/unrecovered audio or malformed manifest publication state | Error | Load until recorder recovery |
| Escaping path/symlink, nonregular/oversized file, bad SHA-256 or invalid FLAC | Error | Playback/analysis; original data remains unchanged |
| Missing/disconnected/unsupported output route | Error | Output; clock stops and explicit retry is available |
| Queue underflow | Buffering | Emits silence without skipping transcript/audio time |
| Nonfinite/out-of-range gain | Error | Gain mutation |
| Seek outside duration | Clamped | No error; selected position stays within timeline |

## Side effects in order

Load: validate identity → read manifest with read-only/no-follow SQLite flags → validate time/identity metadata → start idle worker. Play: resolve native output → create paused stream/queue → verify/decode bounded source chunks → prefill → explicitly start output. Seek/pause/stop: disable consumption → drop old stream/queue → update/preserve consumed position → optionally create a fresh output for playing seek. Exit/drop: disable callbacks → set cancellation → join worker. No step mutates audio/database files.

## Defaults, bounds and access

| Contract | Value |
|---|---|
| Initial state | Paused, position 0, both track gains 1 |
| Manifest maximum | 100,000 chunk rows |
| Encoded chunk maximum | 64 MiB |
| Source chunk maximum | 2 seconds, 8–192 kHz, 1–8 channels, PCM24 |
| Decoded cache | One stereo chunk per track |
| Output queue | 8,192 frames, fixed allocation |
| Worker generation batch | At most 512 output frames |
| Native output initialization timeout | 3 seconds |
| Worker idle poll | At most 5 ms; full queue/end poll 2 ms |
| Waveform bins | 1–2,048 per track |
| Timeline source | Manifest start/end nanoseconds, plus optional saved desktop duration |

The local desktop service owns authorization and recording exclusion. CPAL is a native output boundary; cross-platform runtime behavior still requires platform validation. Playback position reflects callback consumption and can lead physical speaker output by device buffering latency. Linear interpolation is used for playback-rate conversion; no high-order anti-aliasing or archival export claim is made.
