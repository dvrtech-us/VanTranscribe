use super::*;
use std::sync::atomic::AtomicUsize;
use tempfile::TempDir;
use vt_core::{AudioBlock, FramePosition, GapReason, StreamFormat, TrackKind};

#[derive(Default)]
struct MockState {
    renderer: Mutex<Option<AudioRenderer>>,
    opens: AtomicUsize,
    plays: AtomicUsize,
    drops: AtomicUsize,
}
#[derive(Clone, Default)]
struct Mock(Arc<MockState>);
struct MockSink(Arc<MockState>);
impl AudioSink for MockSink {
    fn play(&self) -> Result<()> {
        self.0.plays.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}
impl Drop for MockSink {
    fn drop(&mut self) {
        *self.0.renderer.lock().unwrap() = None;
        self.0.drops.fetch_add(1, Ordering::Relaxed);
    }
}
impl AudioSinkFactory for Mock {
    fn open(&mut self, renderer: AudioRenderer) -> Result<OpenedSink> {
        self.0.opens.fetch_add(1, Ordering::Relaxed);
        *self.0.renderer.lock().unwrap() = Some(renderer);
        Ok(OpenedSink {
            format: OutputFormat {
                sample_rate: 8000,
                channels: 2,
            },
            sink: Box::new(MockSink(self.0.clone())),
        })
    }
}
impl Mock {
    fn render(&self, frames: usize) -> Vec<f32> {
        let mut output = vec![0.0; frames * 2];
        if let Some(renderer) = self.0.renderer.lock().unwrap().as_mut() {
            renderer.render(&mut output, 2);
        }
        output
    }
}
fn block(
    epoch: u32,
    first_frame: u64,
    start_ns: u64,
    rate: u32,
    channels: u16,
    samples: Vec<f32>,
) -> AudioBlock {
    AudioBlock::new(
        StreamFormat::new(rate, channels).unwrap(),
        FramePosition {
            epoch,
            first_frame,
            host_time_ns: start_ns,
        },
        samples,
    )
    .unwrap()
}
fn fixture() -> TempDir {
    let root = TempDir::new().unwrap();
    let mut recording = vt_record::Recording::create(root.path(), "playback-fixture").unwrap();
    recording
        .write_chunk(TrackKind::Mic, &block(0, 0, 0, 8000, 1, vec![0.25; 800]))
        .unwrap();
    recording
        .write_gap(
            TrackKind::Mic,
            StreamFormat::new(8000, 1).unwrap(),
            vt_record::Gap {
                epoch: 0,
                first_frame: 800,
                frame_count: 0,
                start_ns: 100_000_000,
                end_ns: 200_000_000,
                reason: GapReason::DeviceChange,
            },
        )
        .unwrap();
    recording
        .write_chunk(
            TrackKind::Mic,
            &block(1, 0, 200_000_000, 16000, 1, vec![0.5; 1600]),
        )
        .unwrap();
    recording
        .write_chunk(
            TrackKind::System,
            &block(
                0,
                0,
                0,
                48000,
                2,
                (0..14400).flat_map(|_| [0.125, -0.125]).collect(),
            ),
        )
        .unwrap();
    recording.finish().unwrap();
    root
}
fn close(a: f32, b: f32) {
    assert!((a - b).abs() < 0.00001, "{a} != {b}");
}

#[test]
fn load_is_silent_and_native_track_mixing_preserves_gaps_and_stereo_timeline() {
    let root = fixture();
    let mock = Mock::default();
    let playback = Playback::with_sink(root.path(), "playback-fixture", mock.clone()).unwrap();
    assert_eq!(mock.0.opens.load(Ordering::Relaxed), 0);
    assert_eq!(playback.snapshot().status, "paused");
    assert_eq!(playback.snapshot().duration_ms, 300);
    assert!(mock.render(16).iter().all(|s| *s == 0.0));
    playback.play().unwrap();
    let samples = mock.render(2400);
    close(samples[0], 0.375);
    close(samples[1], 0.125);
    close(samples[800 * 2], 0.125);
    close(samples[800 * 2 + 1], -0.125);
    close(samples[1600 * 2], 0.625);
    close(samples[1600 * 2 + 1], 0.375);
    assert_eq!(playback.snapshot().position_ms, 300);
    mock.render(1);
    assert_eq!(playback.snapshot().status, "ended");
}

#[test]
fn pause_resume_seek_stop_and_drop_have_sample_driven_positions_and_fresh_queues() {
    let root = fixture();
    let mock = Mock::default();
    let playback = Playback::with_sink(root.path(), "playback-fixture", mock.clone()).unwrap();
    playback.play().unwrap();
    mock.render(160);
    playback.pause().unwrap();
    assert_eq!(playback.snapshot().position_ms, 20);
    assert_eq!(playback.snapshot().status, "paused");
    assert!(mock.render(800).iter().all(|s| *s == 0.0));
    assert_eq!(playback.snapshot().position_ms, 20);
    playback.seek(250).unwrap();
    assert_eq!(playback.snapshot().position_ms, 250);
    playback.play().unwrap();
    close(mock.render(1)[0], 0.625);
    playback.seek(50).unwrap();
    close(mock.render(1)[0], 0.375);
    assert!(mock.0.opens.load(Ordering::Relaxed) >= 3);
    playback.stop().unwrap();
    assert_eq!(playback.snapshot().position_ms, 0);
    assert_eq!(playback.snapshot().status, "stopped");
    assert!(mock.render(20).iter().all(|s| *s == 0.0));
    playback.seek(u64::MAX).unwrap();
    assert_eq!(playback.snapshot().position_ms, 300);
    playback.play().unwrap();
    close(mock.render(1)[0], 0.375);
    drop(playback);
    assert!(mock.0.renderer.lock().unwrap().is_none());
    assert_eq!(
        mock.0.opens.load(Ordering::Relaxed),
        mock.0.drops.load(Ordering::Relaxed)
    );
}

#[test]
fn track_volume_and_mute_apply_to_queued_frames_without_seeking() {
    let root = fixture();
    let mock = Mock::default();
    let playback = Playback::with_sink(root.path(), "playback-fixture", mock.clone()).unwrap();
    playback.play().unwrap();
    playback.set_volumes(0.0, 1.0).unwrap();
    let system = mock.render(1);
    close(system[0], 0.125);
    close(system[1], -0.125);
    playback.set_volumes(0.5, 0.0).unwrap();
    let mic = mock.render(1);
    close(mic[0], 0.125);
    close(mic[1], 0.125);
    playback.set_volumes(0.0, 0.0).unwrap();
    assert!(mock.render(4).iter().all(|s| *s == 0.0));
    assert_eq!(playback.snapshot().mic_volume, 0.0);
    assert!(playback.set_volumes(f32::NAN, 1.0).is_err());
    assert!(playback.set_volumes(1.1, 1.0).is_err());
}

#[test]
fn underflow_outputs_silence_without_advancing_or_skipping_source_cursor_and_mix_is_clamped() {
    let shared = Arc::new(Shared::new());
    shared.playing.store(true, Ordering::Release);
    shared.generation.store(1, Ordering::Release);
    let (mut producer, consumer) = RingBuffer::new(2);
    let mut renderer = AudioRenderer {
        queue: consumer,
        shared: shared.clone(),
        generation: 1,
    };
    producer
        .push(Frame {
            generation: 1,
            position_ns: 125000,
            mic: [0.75; 2],
            system: [0.75; 2],
            end: false,
        })
        .unwrap();
    let mut samples = [9.0; 6];
    renderer.render(&mut samples, 2);
    assert_eq!(samples, [1.0, 1.0, 0.0, 0.0, 0.0, 0.0]);
    assert_eq!(shared.position_ns.load(Ordering::Acquire), 125000);
    assert_eq!(shared.status.load(Ordering::Acquire), BUFFERING);
    producer
        .push(Frame {
            generation: 1,
            position_ns: 250000,
            mic: [0.25; 2],
            system: [0.0; 2],
            end: false,
        })
        .unwrap();
    renderer.render(&mut samples[..2], 2);
    assert_eq!(samples[..2], [0.25, 0.25]);
    assert_eq!(shared.position_ns.load(Ordering::Acquire), 250000);
    renderer.report_error("Synthetic device disconnected");
    renderer.render(&mut samples, 2);
    assert!(samples.iter().all(|s| *s == 0.0));
    assert_eq!(shared.status.load(Ordering::Acquire), FAILED);
}

#[test]
fn linear_resampling_uses_native_sample_times_and_waveforms_include_gap_silence() {
    let root = fixture();
    let wave = waveform(root.path(), "playback-fixture", 3).unwrap();
    assert_eq!(wave.duration_ms, 300);
    assert_eq!(wave.mic.len(), 3);
    close(wave.mic[0], 0.25);
    close(wave.mic[1], 0.0);
    close(wave.mic[2], 0.5);
    for peak in wave.system {
        close(peak, 0.125);
    }
    assert!(waveform(root.path(), "playback-fixture", 2049).is_err());
    let other = TempDir::new().unwrap();
    let mut recording = vt_record::Recording::create(other.path(), "ramp").unwrap();
    recording
        .write_chunk(
            TrackKind::Mic,
            &block(
                0,
                0,
                0,
                8000,
                1,
                (0..32)
                    .map(|i| if i % 2 == 0 { 0.0 } else { 0.5 })
                    .collect(),
            ),
        )
        .unwrap();
    recording.finish().unwrap();
    let mut timeline = Timeline::new(Manifest::open(other.path(), "ramp").unwrap());
    close(timeline.frame(62_500).unwrap()[0][0], 0.25);
    close(timeline.frame(125_000).unwrap()[0][0], 0.5);
}

#[test]
fn corrupt_audio_fails_visibly_without_changing_recordings_or_database() {
    let root = fixture();
    let db_path = root.path().join("vantranscribe.db");
    let before = std::fs::read(&db_path).unwrap();
    let path = root
        .path()
        .join("meetings/playback-fixture/audio/mic/0-0.flac");
    let mut bytes = std::fs::read(&path).unwrap();
    bytes[0] ^= 1;
    std::fs::write(&path, &bytes).unwrap();
    let mock = Mock::default();
    let playback = Playback::with_sink(root.path(), "playback-fixture", mock.clone()).unwrap();
    assert!(playback.play().is_err());
    assert_eq!(playback.snapshot().status, "error");
    assert!(playback.snapshot().error.unwrap().contains("integrity"));
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    assert_eq!(std::fs::read(&db_path).unwrap(), before);
    assert!(waveform(root.path(), "playback-fixture", 5).is_err());
}

#[cfg(unix)]
#[test]
fn manifest_identity_and_symlink_escape_are_rejected() {
    let root = fixture();
    let outside = TempDir::new().unwrap();
    let path = root
        .path()
        .join("meetings/playback-fixture/audio/mic/0-0.flac");
    let copy = outside.path().join("outside.flac");
    std::fs::copy(&path, &copy).unwrap();
    std::fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink(&copy, &path).unwrap();
    assert!(waveform(root.path(), "playback-fixture", 5).is_err());
    assert!(Manifest::open(root.path(), "../escape").is_err());
    let db = ConnectionForTests::open(root.path());
    db.execute(
        "UPDATE audio_chunks SET path='../../escape.flac' WHERE track='mic' AND seq=0",
        [],
    )
    .unwrap();
    assert!(Manifest::open(root.path(), "playback-fixture").is_err());
}
struct ConnectionForTests;
impl ConnectionForTests {
    fn open(root: &Path) -> rusqlite::Connection {
        rusqlite::Connection::open(root.join("vantranscribe.db")).unwrap()
    }
}
