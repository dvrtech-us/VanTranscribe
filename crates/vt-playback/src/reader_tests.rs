use crate::{SavedAudioPacket, SavedAudioReader};
use rusqlite::Connection;
use sha2::{Digest, Sha256};
use std::fs;
use tempfile::TempDir;
use vt_core::{AudioBlock, FramePosition, GapReason, StreamFormat, TrackKind};

fn block(
    first_frame: u64,
    start_ms: u64,
    rate: u32,
    channels: u16,
    samples: Vec<f32>,
) -> AudioBlock {
    AudioBlock::new(
        StreamFormat::new(rate, channels).unwrap(),
        FramePosition {
            epoch: 0,
            first_frame,
            host_time_ns: start_ms * 1_000_000,
        },
        samples,
    )
    .unwrap()
}

#[test]
fn archive_reader_orders_native_packets_preserves_gap_bounds_and_every_pcm_frame() {
    let root = TempDir::new().unwrap();
    let id = "reader-fixture";
    let mut recorder = vt_record::Recording::create(root.path(), id).unwrap();
    recorder
        .write_chunk(TrackKind::Mic, &block(0, 0, 16_000, 1, vec![0.25; 720]))
        .unwrap();
    recorder
        .write_gap(
            TrackKind::Mic,
            StreamFormat::new(16_000, 1).unwrap(),
            vt_record::Gap {
                epoch: 0,
                first_frame: 720,
                frame_count: 400,
                start_ns: 45_000_000,
                end_ns: 70_000_000,
                reason: GapReason::DeviceChange,
            },
        )
        .unwrap();
    recorder
        .write_chunk(TrackKind::Mic, &block(1120, 70, 16_000, 1, vec![0.5; 560]))
        .unwrap();
    recorder
        .write_chunk(
            TrackKind::System,
            &block(
                0,
                0,
                48_000,
                2,
                (0..2880).flat_map(|_| [0.125, 0.375]).collect(),
            ),
        )
        .unwrap();
    recorder
        .write_gap(
            TrackKind::System,
            StreamFormat::new(48_000, 2).unwrap(),
            vt_record::Gap {
                epoch: 0,
                first_frame: 2880,
                frame_count: 960,
                start_ns: 60_000_000,
                end_ns: 80_000_000,
                reason: GapReason::Overflow,
            },
        )
        .unwrap();
    recorder
        .write_chunk(
            TrackKind::System,
            &block(
                3840,
                80,
                48_000,
                2,
                (0..1200).flat_map(|_| [-0.5, 0.25]).collect(),
            ),
        )
        .unwrap();
    recorder.finish().unwrap();
    let db = Connection::open(root.path().join("vantranscribe.db")).unwrap();
    let mut q = db.prepare("SELECT path,sha256 FROM audio_chunks WHERE meeting_id=?1 AND state='ready' ORDER BY track,seq").unwrap();
    let originals: Vec<(String, String, Vec<u8>)> = q
        .query_map([id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .unwrap()
        .map(|r| {
            let (path, hash) = r.unwrap();
            let bytes = fs::read(root.path().join(&path)).unwrap();
            (path, hash, bytes)
        })
        .collect();
    assert_eq!(originals.len(), 4);

    // This reader has no output sink: decoded frames remain in test memory.
    let mut reader = SavedAudioReader::open(root.path(), id).unwrap();
    assert_eq!(reader.progress(), 0.0);
    let mut order = Vec::new();
    let mut gaps = Vec::new();
    let mut mic = Vec::new();
    let mut system = Vec::new();
    let mut progress = 0.0;
    while let Some(packet) = reader.next_packet().unwrap() {
        match packet {
            SavedAudioPacket::Audio {
                track,
                samples,
                rate,
                start_ms,
            } => {
                assert!(!samples.is_empty());
                assert!(samples.len() <= rate as usize / 50, "packet exceeds 20 ms");
                assert!(samples.iter().all(|v| v.is_finite()));
                order.push((start_ms, track));
                match track {
                    TrackKind::Mic => {
                        assert_eq!(rate, 16_000);
                        mic.extend(samples);
                    }
                    TrackKind::System => {
                        assert_eq!(rate, 48_000);
                        system.extend(samples);
                    }
                }
            }
            SavedAudioPacket::Gap {
                track,
                start_ms,
                end_ms,
            } => {
                order.push((start_ms, track));
                gaps.push((track, start_ms, end_ms));
            }
        }
        assert!(reader.progress() >= progress);
        progress = reader.progress();
    }
    assert_eq!(
        order,
        vec![
            (0, TrackKind::System),
            (0, TrackKind::Mic),
            (20, TrackKind::System),
            (20, TrackKind::Mic),
            (40, TrackKind::System),
            (40, TrackKind::Mic),
            (45, TrackKind::Mic),
            (60, TrackKind::System),
            (70, TrackKind::Mic),
            (80, TrackKind::System),
            (90, TrackKind::Mic),
            (100, TrackKind::System),
        ]
    );
    assert_eq!(
        gaps,
        vec![(TrackKind::Mic, 45, 70), (TrackKind::System, 60, 80)]
    );
    assert_eq!(mic.len(), 1280);
    assert_eq!(system.len(), 4080);
    for (samples, value) in [
        (&mic[..720], 0.25),
        (&mic[720..], 0.5),
        (&system[..2880], 0.25),
        (&system[2880..], -0.125),
    ] {
        assert!(samples.iter().all(|s| (*s - value).abs() < 0.000001));
    }
    let mic_sum: f64 = mic.iter().map(|s| f64::from(*s)).sum();
    let system_sum: f64 = system.iter().map(|s| f64::from(*s)).sum();
    assert!((mic_sum - (720.0 * 0.25 + 560.0 * 0.5)).abs() < 0.001);
    assert!((system_sum - (2880.0 * 0.25 - 1200.0 * 0.125)).abs() < 0.001);
    assert_eq!(reader.progress(), 1.0);
    assert!(reader.next_packet().unwrap().is_none());
    for (path, hash, original) in originals {
        let after = fs::read(root.path().join(path)).unwrap();
        assert_eq!(after, original);
        assert_eq!(format!("{:x}", Sha256::digest(&after)), hash);
    }
}
