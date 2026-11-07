//! Bounded PCM24 FLAC fallback, encoded directly from RFC 9639 sections 8.2,
//! 9.1, 9.2.4 and 9.3: https://www.rfc-editor.org/rfc/rfc9639.html
//! Independent verbatim channels avoid predictor/Rice expansion entirely.
use super::{RecordError, Result};
use vt_core::StreamFormat;

const BLOCK: usize = 4096;
const PCM_MIN: i32 = -8_388_608;
const PCM_MAX: i32 = 8_388_607;

/// Encode one native-rate, interleaved PCM24 recording chunk without resampling.
/// The output size is exactly 42 + 3*samples + (10+channels)*ceil(frames/4096).
pub(super) fn encode(samples: &[i32], format: StreamFormat) -> Result<Vec<u8>> {
    format.validate()?;
    let channels = usize::from(format.channels);
    if samples.is_empty() || !samples.len().is_multiple_of(channels) {
        return Err(RecordError::Encoding(
            "verbatim FLAC requires nonempty complete interleaved frames".into(),
        ));
    }
    let frames = samples.len() / channels;
    if frames > format.sample_rate as usize * 2
        || samples.iter().any(|s| !(PCM_MIN..=PCM_MAX).contains(s))
    {
        return Err(RecordError::Encoding(
            "verbatim FLAC requires PCM24 samples and at most two seconds".into(),
        ));
    }
    // At the maximum supported rate/duration there are only 94 frames, so
    // every UTF-8-coded fixed-block frame number is a single ASCII byte.
    let frame_count = frames.div_ceil(BLOCK);
    let capacity = 42 + samples.len() * 3 + frame_count * (10 + channels);
    let mut output = Vec::with_capacity(capacity);
    output.extend_from_slice(b"fLaC\x80\x00\x00\x22");
    // RFC 9639 permits a final block shorter than the fixed block size, even
    // when it is the only block (see its one-sample example in Appendix D.1).
    // Both STREAMINFO bounds remain >=16 and describe the fixed 4096 plan.
    output.extend_from_slice(&(BLOCK as u16).to_be_bytes());
    output.extend_from_slice(&(BLOCK as u16).to_be_bytes());
    let last_frames = (frames - 1) % BLOCK + 1;
    let min_frame_bytes = last_frames * channels * 3 + 10 + channels;
    let max_frame_bytes = frames.min(BLOCK) * channels * 3 + 10 + channels;
    push_u24(&mut output, min_frame_bytes);
    push_u24(&mut output, max_frame_bytes);
    let stream_fields = (u64::from(format.sample_rate) << 44)
        | (u64::from(format.channels - 1) << 41)
        | (23_u64 << 36)
        | frames as u64;
    output.extend_from_slice(&stream_fields.to_be_bytes());
    // A zero MD5 means unknown, explicitly permitted by STREAMINFO. Frame
    // CRCs still cover the header and complete encoded PCM data.
    output.extend_from_slice(&[0; 16]);
    for (number, interleaved) in samples.chunks(BLOCK * channels).enumerate() {
        let count = interleaved.len() / channels;
        let start = output.len();
        output.extend_from_slice(&[
            0xff,
            0xf8,                                          // fixed-block strategy
            0x70, // 16-bit block-size extension; rate from STREAMINFO
            ((format.channels - 1) as u8) << 4 | (6 << 1), // independent, 24-bit
            number as u8,
        ]);
        output.extend_from_slice(&((count - 1) as u16).to_be_bytes());
        let header_crc = crc8(&output[start..]);
        output.push(header_crc);
        for channel in 0..channels {
            output.push(0x02); // verbatim subframe, no wasted bits
            for frame in interleaved.chunks_exact(channels) {
                output.extend_from_slice(&frame[channel].to_be_bytes()[1..]);
            }
        }
        let frame_crc = crc16(&output[start..]);
        output.extend_from_slice(&frame_crc.to_be_bytes());
    }
    debug_assert_eq!(output.len(), capacity);
    Ok(output)
}

fn push_u24(output: &mut Vec<u8>, value: usize) {
    output.extend_from_slice(&(value as u32).to_be_bytes()[1..]);
}

fn crc8(bytes: &[u8]) -> u8 {
    let mut crc = 0_u8;
    for &byte in bytes {
        crc ^= byte;
        for _ in 0..8 {
            crc = (crc << 1) ^ if crc & 0x80 != 0 { 0x07 } else { 0 };
        }
    }
    crc
}

fn crc16(bytes: &[u8]) -> u16 {
    let mut crc = 0_u16;
    for &byte in bytes {
        crc ^= u16::from(byte) << 8;
        for _ in 0..8 {
            crc = (crc << 1) ^ if crc & 0x8000 != 0 { 0x8005 } else { 0 };
        }
    }
    crc
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn signal(frames: usize, channels: usize) -> Vec<i32> {
        (0..frames * channels)
            .map(|i| match i % 9 {
                0 => PCM_MIN,
                1 => PCM_MAX,
                2 => -1,
                3 => 0,
                4 => 1,
                _ => ((i as u32).wrapping_mul(2_654_435_761) & 0xffffff) as i32 - 0x800000,
            })
            .collect()
    }

    fn round_trip(rate: u32, channels: u16, frames: usize) -> Vec<u8> {
        let input = signal(frames, usize::from(channels));
        let bytes = encode(&input, StreamFormat::new(rate, channels).unwrap()).unwrap();
        let expected_len = 42 + input.len() * 3 + frames.div_ceil(BLOCK) * (10 + channels as usize);
        assert_eq!(bytes.len(), expected_len);
        let mut decoder = claxon::FlacReader::new(Cursor::new(&bytes)).unwrap();
        let info = decoder.streaminfo();
        assert_eq!(info.sample_rate, rate);
        assert_eq!(info.channels, u32::from(channels));
        assert_eq!(info.bits_per_sample, 24);
        assert_eq!(info.samples, Some(frames as u64));
        assert_eq!(
            decoder
                .samples()
                .collect::<std::result::Result<Vec<_>, _>>()
                .unwrap(),
            input
        );
        bytes
    }

    #[test]
    fn verbatim_round_trips_short_final_and_block_boundaries() {
        for frames in [1, 15, 16, 4095, 4096, 4097, 8192, 8193] {
            for channels in [1, 2, 8] {
                round_trip(192_000, channels, frames);
            }
        }
    }

    #[test]
    fn verbatim_native_rates_and_all_channel_counts_round_trip() {
        for rate in [
            8000, 12345, 16000, 44100, 48000, 88200, 96000, 176400, 192000,
        ] {
            for channels in 1..=8 {
                round_trip(rate, channels, 97);
            }
        }
    }

    #[test]
    fn verbatim_largest_chunk_is_bounded_and_bit_exact() {
        round_trip(192_000, 8, 384_000);
    }

    #[test]
    fn verbatim_metadata_has_real_frame_sizes_and_valid_fixed_block_bounds() {
        for frames in [1, 15, 16, 4095, 4096, 4097, 8192, 8193] {
            let bytes = round_trip(192_000, 8, frames);
            assert_eq!(&bytes[8..12], &[0x10, 0, 0x10, 0]);
            let u24 = |v: &[u8]| u32::from_be_bytes([0, v[0], v[1], v[2]]) as usize;
            let mut offset = 42;
            let mut frame_sizes = Vec::new();
            for (number, first) in (0..frames).step_by(BLOCK).enumerate() {
                let count = (frames - first).min(BLOCK);
                let size = 10 + 8 + count * 8 * 3;
                let frame = &bytes[offset..offset + size];
                assert_eq!(&frame[..5], &[0xff, 0xf8, 0x70, 0x7c, number as u8]);
                assert_eq!(u16::from_be_bytes([frame[5], frame[6]]) as usize + 1, count);
                assert_eq!(crc8(&frame[..8]), 0);
                assert_eq!(crc16(frame), 0);
                frame_sizes.push(size);
                offset += size;
            }
            assert_eq!(offset, bytes.len());
            assert_eq!(u24(&bytes[12..15]), *frame_sizes.iter().min().unwrap());
            assert_eq!(u24(&bytes[15..18]), *frame_sizes.iter().max().unwrap());
            assert!(bytes[26..42].iter().all(|b| *b == 0));
        }
    }

    #[test]
    fn verbatim_rejects_invalid_chunks_without_truncation() {
        let stereo = StreamFormat::new(192_000, 2).unwrap();
        for samples in [
            vec![],
            vec![0],
            vec![PCM_MIN - 1, 0],
            vec![PCM_MAX + 1, 0],
            vec![0; 768_002],
        ] {
            assert!(encode(&samples, stereo).is_err());
        }
        for format in [
            StreamFormat {
                sample_rate: 0,
                channels: 1,
            },
            StreamFormat {
                sample_rate: 192001,
                channels: 1,
            },
            StreamFormat {
                sample_rate: 48000,
                channels: 0,
            },
            StreamFormat {
                sample_rate: 48000,
                channels: 9,
            },
        ] {
            assert!(encode(&[0], format).is_err());
        }
    }

    #[test]
    #[ignore = "requires an existing ffmpeg executable; synthetic files only"]
    fn verbatim_independent_ffmpeg_decoder_is_bit_exact() {
        use std::{io::Write, process::Command};
        for frames in [1, 15, 16, 4095, 4096, 4097, 384_000] {
            let input = signal(frames, 8);
            let bytes = encode(&input, StreamFormat::new(192_000, 8).unwrap()).unwrap();
            let mut file = tempfile::NamedTempFile::new().unwrap();
            file.write_all(&bytes).unwrap();
            let decoded = Command::new("ffmpeg")
                .args([
                    "-nostdin",
                    "-hide_banner",
                    "-loglevel",
                    "error",
                    "-f",
                    "flac",
                    "-i",
                ])
                .arg(file.path())
                .args([
                    "-map",
                    "0:a:0",
                    "-c:a",
                    "pcm_s24le",
                    "-f",
                    "s24le",
                    "pipe:1",
                ])
                .output()
                .expect("the explicitly invoked reference smoke requires ffmpeg");
            assert!(
                decoded.status.success(),
                "{}",
                String::from_utf8_lossy(&decoded.stderr)
            );
            let expected: Vec<u8> = input
                .iter()
                .flat_map(|sample| sample.to_le_bytes()[..3].to_vec())
                .collect();
            assert_eq!(decoded.stdout.len(), expected.len());
            assert!(
                decoded.stdout == expected,
                "reference PCM mismatch at {frames} frames"
            );
        }
    }
}
