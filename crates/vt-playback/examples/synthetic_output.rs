//! Explicit native-output smoke test. --mute exercises real callbacks silently.
use std::{
    thread,
    time::{Duration, Instant},
};
use vt_core::{AudioBlock, FramePosition, StreamFormat, TrackKind};
use vt_playback::Playback;
fn wait_at(player: &Playback, position: u64) -> Result<u64, Box<dyn std::error::Error>> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let state = player.snapshot();
        if let Some(error) = state.error {
            return Err(error.into());
        }
        if state.position_ms >= position {
            return Ok(state.position_ms);
        }
        if Instant::now() >= deadline {
            return Err("Native output callback did not consume source frames.".into());
        }
        thread::sleep(Duration::from_millis(5));
    }
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let muted=match args.as_slice(){[flag]if flag=="--mute"=>true,[flag]if flag=="--audible"=>false,_=>return Err("Use --mute for silent native-output verification, or explicitly choose --audible for a synthetic tone.".into())};
    let root = tempfile::TempDir::new()?;
    let mut recording = vt_record::Recording::create(root.path(), "synthetic-output")?;
    let rate = 48000;
    let samples = (0..rate)
        .map(|i| (i as f32 * 440.0 * std::f32::consts::TAU / rate as f32).sin() * 0.05)
        .collect();
    recording.write_chunk(
        TrackKind::Mic,
        &AudioBlock::new(
            StreamFormat::new(rate, 1)?,
            FramePosition {
                epoch: 0,
                first_frame: 0,
                host_time_ns: 0,
            },
            samples,
        )?,
    )?;
    recording.finish()?;
    let player = Playback::open(root.path(), "synthetic-output")?;
    player.set_volumes(if muted { 0.0 } else { 1.0 }, 0.0)?;
    assert_eq!(player.snapshot().status, "paused");
    player.play()?;
    let first = wait_at(&player, 25)?;
    player.pause()?;
    let paused = player.snapshot().position_ms;
    thread::sleep(Duration::from_millis(40));
    assert_eq!(player.snapshot().position_ms, paused);
    assert_eq!(player.snapshot().status, "paused");
    player.seek(500)?;
    assert_eq!(player.snapshot().position_ms, 500);
    player.play()?;
    let resumed = wait_at(&player, 525)?;
    player.seek(250)?;
    let sought = wait_at(&player, 275)?;
    player.stop()?;
    assert_eq!(player.snapshot().position_ms, 0);
    assert_eq!(player.snapshot().status, "stopped");
    println!(
        "Native CPAL callbacks consumed synthetic frames: first={first}ms, resume={resumed}ms, seek={sought}ms; paused cursor stable; stop=0ms; muted={muted}."
    );
    Ok(())
}
