//! A local-only recorder harness. Generates tones; never opens an audio device.
use std::{error::Error, path::PathBuf};
use vt_core::{AudioBlock, FramePosition, StreamFormat, TrackKind};
use vt_record::Recording;

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--recover") && args.len() == 3 {
        let report = Recording::recover(&args[1], &args[2])?;
        println!(
            "Verified {}, adopted {}, recorded {} new gaps",
            report.verified_chunks, report.adopted_chunks, report.gaps
        );
        return Ok(());
    }
    if !(2..=3).contains(&args.len()) {
        return Err("usage: synthetic <root> <meeting-id> [seconds:1..60]\n       synthetic --recover <root> <meeting-id>".into());
    }
    let seconds = args
        .get(2)
        .map(|v| v.parse::<u32>())
        .transpose()?
        .unwrap_or(4);
    if !(1..=60).contains(&seconds) {
        return Err("seconds must be 1..60".into());
    }
    let mut recording = Recording::create(PathBuf::from(&args[0]), &args[1])?;
    let mut mic_end_ns = 0;
    let mut system_end_ns = 0;
    for second in 0..seconds {
        for (track, channels, hz) in [(TrackKind::Mic, 1, 440.0), (TrackKind::System, 2, 660.0)] {
            let format = StreamFormat::new(48_000, channels)?;
            let mut samples = Vec::with_capacity(48_000 * channels as usize);
            for frame in 0..48_000 {
                let sample = (std::f32::consts::TAU * hz * frame as f32 / 48_000.0).sin() * 0.25;
                for channel in 0..channels {
                    samples.push(if channel == 0 { sample } else { -sample });
                }
            }
            let block = AudioBlock::new(
                format,
                FramePosition {
                    epoch: 0,
                    first_frame: u64::from(second) * 48_000,
                    host_time_ns: if track == TrackKind::Mic {
                        mic_end_ns
                    } else {
                        system_end_ns
                    },
                },
                samples,
            )?;
            let chunk = recording.write_chunk(track, &block)?;
            if track == TrackKind::Mic {
                mic_end_ns = chunk.end_ns;
            } else {
                system_end_ns = chunk.end_ns;
            }
        }
    }
    println!(
        "Recorded {seconds}s of synthetic Mic/System audio under {}",
        recording
            .root()
            .join("meetings")
            .join(recording.meeting_id())
            .display()
    );
    println!(
        "{} durable chunks; no audio devices were opened",
        recording.chunks()?.len()
    );
    recording.finish()?;
    Ok(())
}
