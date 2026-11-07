//! Synthetic archive-to-inference checks. No output device or capture helper is opened.
use super::*;
use crate::{AppService, echo_store, store};
use rusqlite::Connection;
use sha2::{Digest, Sha256};
use std::{fs, sync::Arc};
use tempfile::TempDir;
use vt_capture::{OutputRoute, OutputRouteKind, OutputRouteReason, OutputTransport};
use vt_core::{AudioBlock, FramePosition, GapReason, StreamFormat};

struct TrackSource {
    rate: u32,
    samples: Vec<f32>,
    gaps: Vec<(u64, u64)>,
}
struct Fixture {
    _temp: TempDir,
    app: Arc<AppService>,
    id: &'static str,
    proof: Proof,
}
#[derive(Debug, PartialEq)]
struct Proof {
    manifest: Vec<String>,
    files: Vec<(String, String, Vec<u8>)>,
}
fn proof(root: &Path, id: &str) -> Proof {
    let db = Connection::open(root.join("vantranscribe.db")).unwrap();
    let mut query = db.prepare("SELECT json_object('track',track,'seq',seq,'epoch',epoch,'first_frame',first_frame,'frame_count',frame_count,'sample_rate',sample_rate,'channels',channels,'start_ns',start_ns,'end_ns',end_ns,'state',state,'path',path,'sha256',sha256,'kind',kind,'gap_reason',gap_reason) FROM audio_chunks WHERE meeting_id=?1 ORDER BY track,seq").unwrap();
    let manifest = query
        .query_map([id], |r| r.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    let mut query = db.prepare("SELECT path,sha256 FROM audio_chunks WHERE meeting_id=?1 AND state='ready' ORDER BY track,seq").unwrap();
    let files = query
        .query_map([id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })
        .unwrap()
        .map(|r| {
            let (path, hash) = r.unwrap();
            let bytes = fs::read(root.join(&path)).unwrap();
            assert_eq!(format!("{:x}", Sha256::digest(&bytes)), hash);
            (path, hash, bytes)
        })
        .collect();
    Proof { manifest, files }
}
fn eligible() -> OutputRoute {
    OutputRoute {
        kind: OutputRouteKind::BuiltInSpeakers,
        transport: OutputTransport::BuiltIn,
        default_output_known: true,
        data_source_known: true,
        speaker_terminal: true,
        headphone_jack_connected: Some(false),
        active_output_processes: 1,
        process_devices_known: true,
        all_processes_same_output: true,
        global_tap: true,
        reference_available: true,
        reason: OutputRouteReason::Eligible,
    }
}
fn fixture(
    mic: TrackSource,
    system: TrackSource,
    origin_ms: u64,
    routes: Vec<(u64, OutputRoute)>,
    sealed: bool,
) -> Fixture {
    let temp = TempDir::new_in("/private/tmp").unwrap();
    let app = AppService::new(
        temp.path().join("data"),
        temp.path().join("never-started-helper"),
    )
    .unwrap();
    let id = "synthetic-saved-echo";
    let mut recording = vt_record::Recording::create(app.root(), id).unwrap();
    store::create_meeting(app.root(), id, "Synthetic saved echo fixture", "remote").unwrap();
    for (track, source) in [(TrackKind::Mic, mic), (TrackKind::System, system)] {
        let channels = if track == TrackKind::System { 2 } else { 1 };
        let format = StreamFormat::new(source.rate, channels).unwrap();
        let mut cursor = 0;
        while cursor < source.samples.len() {
            let ms = cursor as u64 * 1000 / u64::from(source.rate);
            if let Some((_, end)) = source.gaps.iter().find(|(start, _)| *start == ms) {
                let end_frame = (*end * u64::from(source.rate) / 1000) as usize;
                recording
                    .write_gap(
                        track,
                        format,
                        vt_record::Gap {
                            epoch: 0,
                            first_frame: cursor as u64,
                            frame_count: (end_frame - cursor) as u64,
                            start_ns: (origin_ms + ms) * 1_000_000,
                            end_ns: (origin_ms + end) * 1_000_000,
                            reason: GapReason::Pause,
                        },
                    )
                    .unwrap();
                cursor = end_frame;
                continue;
            }
            let next_gap = source
                .gaps
                .iter()
                .filter(|(start, _)| *start > ms)
                .map(|(start, _)| (*start * u64::from(source.rate) / 1000) as usize)
                .min()
                .unwrap_or(source.samples.len());
            let end = (cursor + source.rate as usize * 2)
                .min(next_gap)
                .min(source.samples.len());
            let samples = if channels == 2 {
                source.samples[cursor..end]
                    .iter()
                    .flat_map(|v| [*v * 0.75, *v * 1.25])
                    .collect()
            } else {
                source.samples[cursor..end].to_vec()
            };
            recording
                .write_chunk(
                    track,
                    &AudioBlock::new(
                        format,
                        FramePosition {
                            epoch: 0,
                            first_frame: cursor as u64,
                            host_time_ns: origin_ms * 1_000_000
                                + cursor as u64 * 1_000_000_000 / u64::from(source.rate),
                        },
                        samples,
                    )
                    .unwrap(),
                )
                .unwrap();
            cursor = end;
        }
    }
    for (epoch, (relative_ms, route)) in routes.into_iter().enumerate() {
        echo_store::record_route(
            app.root(),
            id,
            &RouteObservation {
                epoch: epoch as u32,
                start_ns: (origin_ms + relative_ms) * 1_000_000,
                route,
            },
        )
        .unwrap();
    }
    recording.finish().unwrap();
    if sealed {
        echo_store::seal_routes(app.root(), id).unwrap();
    }
    store::complete_live_run(app.root(), id).unwrap();
    let proof = proof(app.root(), id);
    Fixture {
        _temp: temp,
        app,
        id,
        proof,
    }
}
fn source(rate: u32, samples: Vec<f32>) -> TrackSource {
    TrackSource {
        rate,
        samples,
        gaps: vec![],
    }
}
#[derive(Default)]
struct Output {
    mic: Vec<(u64, Vec<f32>)>,
    system: Vec<(u64, Vec<f32>)>,
    gaps: Vec<(TrackKind, u64)>,
}
fn process(fixture: &Fixture) -> (EchoInfo, Output) {
    process_with_preference(fixture, true)
}
fn process_with_preference(fixture: &Fixture, enabled: bool) -> (EchoInfo, Output) {
    let mut output = Output::default();
    let mut last_fraction = 0.0;
    let info = saved(&fixture.app, fixture.id, enabled, |packet, fraction| {
        assert!(fraction >= last_fraction && fraction <= 1.0);
        last_fraction = fraction;
        match packet {
            processing::AudioPacket::Audio {
                track,
                samples,
                rate,
                start_ms,
            } => {
                assert_eq!(rate, 16000);
                assert!(samples.iter().all(|x| x.is_finite()));
                let target = if track == TrackKind::Mic {
                    &mut output.mic
                } else {
                    &mut output.system
                };
                if let Some((previous, values)) = target.last() {
                    let expected = *previous + values.len() as u64 / 16;
                    if start_ms != expected {
                        assert!(
                            start_ms > expected && output.gaps.contains(&(track, expected)),
                            "unannounced track discontinuity"
                        );
                    }
                }
                target.push((start_ms, samples));
            }
            processing::AudioPacket::Gap { track, start_ms } => output.gaps.push((track, start_ms)),
        }
        Ok(())
    })
    .unwrap();
    assert_eq!(last_fraction, 1.0);
    assert_eq!(proof(fixture.app.root(), fixture.id), fixture.proof);
    (info, output)
}
fn flatten(packets: &[(u64, Vec<f32>)]) -> Vec<f32> {
    packets
        .iter()
        .flat_map(|(_, s)| s.iter().copied())
        .collect()
}
fn pcm(signal: &[f32]) -> Vec<f32> {
    vt_core::pcm24(signal)
        .unwrap()
        .into_iter()
        .map(|v| v as f32 / 8_388_608.0)
        .collect()
}
fn rms(signal: &[f32]) -> f64 {
    (signal.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / signal.len() as f64).sqrt()
}
fn color(seed: &mut u64, low: &mut f32) -> f32 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 7;
    *seed ^= *seed << 17;
    let white = (*seed >> 40) as f32 / (1u32 << 23) as f32 - 1.0;
    *low = 0.85 * *low + 0.15 * white;
    *low
}
fn near_gain(output: &[f32], near: &[f32], start: usize, end: usize) -> f64 {
    let energy = near[start..end]
        .iter()
        .map(|v| f64::from(*v).powi(2))
        .sum::<f64>();
    (0..=800)
        .step_by(8)
        .map(|lag| {
            (start..end)
                .filter(|n| n + lag < output.len())
                .map(|n| f64::from(output[n + lag]) * f64::from(near[n]))
                .sum::<f64>()
                / energy
        })
        .fold(0.0, f64::max)
}

#[test]
fn real_saved_flac_path_reduces_delayed_echo_and_retains_double_talk_and_short_reply() {
    let n = 14 * 16000;
    let (mut seed, mut seed2) = (0x123456789abcdefu64, 0x9876543210abcdefu64);
    let (mut low, mut low2) = (0.0, 0.0);
    let mut far = vec![0.0; n];
    let mut near = vec![0.0; n];
    for i in 0..n {
        let t = i as f32 / 16000.0;
        let envelope = 0.35 + 0.65 * (t * std::f32::consts::TAU * 2.3).sin().abs();
        far[i] = envelope
            * (0.32 * color(&mut seed, &mut low)
                + 0.12 * (t * std::f32::consts::TAU * 173.0).sin()
                + 0.07 * (t * std::f32::consts::TAU * 431.0).sin());
        if (8 * 16000..10 * 16000).contains(&i) || (11 * 16000..11 * 16000 + 4000).contains(&i) {
            near[i] = 0.45 * color(&mut seed2, &mut low2)
                + 0.13 * (t * std::f32::consts::TAU * 283.0).sin();
        }
    }
    let mut mic = near.clone();
    for i in 640..n {
        mic[i] += 0.65 * far[i - 480] + 0.15 * far[i - 640];
    }
    let far48 = sherpa_onnx::LinearResampler::create(16000, 48000)
        .unwrap()
        .resample(&far, true);
    assert_eq!(far48.len(), 14 * 48000);
    let fixture = fixture(
        source(16000, mic.clone()),
        source(48000, far48),
        2500,
        vec![(0, eligible())],
        true,
    );
    for (path, _, bytes) in &fixture.proof.files {
        if let Err(error) = claxon::FlacReader::new(std::io::Cursor::new(bytes)) {
            let artifact = Path::new("/private/tmp/vt-echo-invalid-synthetic.flac");
            fs::write(artifact, bytes).unwrap();
            fs::write(
                "/private/tmp/vt-echo-invalid-synthetic-manifest.json",
                fixture.proof.manifest.join("\n"),
            )
            .unwrap();
            panic!(
                "Recorder emitted invalid synthetic FLAC {path}: {error}; header={:?}; preserved={}",
                &bytes[..42.min(bytes.len())],
                artifact.display()
            );
        }
    }
    let (info, out) = process(&fixture);
    let clean = flatten(&out.mic);
    assert_eq!(clean.len(), n);
    assert_eq!(flatten(&out.system).len(), n);
    assert_eq!(out.mic.first().unwrap().0, 2500);
    assert_eq!(out.system.first().unwrap().0, 2500);
    assert_eq!(out.mic.last().unwrap().0, 16490);
    assert!(out.gaps.is_empty());
    assert!(info.enabled && info.applied_mic_ms > 13000);
    assert_eq!(info.applied_mic_ms + info.bypassed_mic_ms, 14000);
    let suppression = rms(&clean[6 * 16000..8 * 16000]) / rms(&mic[6 * 16000..8 * 16000]);
    let double_gain = near_gain(&clean, &near, 9 * 16000, 10 * 16000);
    let short_gain = near_gain(&clean, &near, 11 * 16000, 11 * 16000 + 4000);
    eprintln!(
        "Saved synthetic FLAC echo RMS ratio={suppression:.4}; double-talk gain={double_gain:.4}; short-reply gain={short_gain:.4}"
    );
    assert!(
        suppression < 0.6,
        "saved echo path did not reduce steady echo"
    );
    assert!(double_gain > 0.2, "double-talk content was removed");
    assert!(short_gain > 0.15, "short near reply was removed");
    assert!(rms(&clean[11 * 16000..11 * 16000 + 4000]) > 0.01);
}

#[test]
fn saved_ineligible_and_unsealed_routes_preserve_exact_native_16k_microphone_pcm() {
    let mic: Vec<f32> = (0..6400).map(|n| (n as f32 * 0.071).sin() * 0.2).collect();
    let far: Vec<f32> = (0..6400).map(|n| (n as f32 * 0.11).sin() * 0.15).collect();
    let mut mixed = eligible();
    mixed.all_processes_same_output = false;
    mixed.reason = OutputRouteReason::MixedProcessOutputs;
    let mut headphones = eligible();
    headphones.kind = OutputRouteKind::Headphones;
    headphones.headphone_jack_connected = Some(true);
    headphones.reason = OutputRouteReason::Headphones;
    for (route, sealed) in [
        (OutputRoute::default(), true),
        (mixed, true),
        (headphones, true),
        (eligible(), false),
    ] {
        let fixture = fixture(
            source(16000, mic.clone()),
            source(16000, far.clone()),
            1234,
            vec![(0, route)],
            sealed,
        );
        let (info, out) = process(&fixture);
        assert_eq!(flatten(&out.mic), pcm(&mic));
        assert_eq!(out.mic.first().unwrap().0, 1234);
        assert_eq!(out.mic.last().unwrap().0, 1624);
        assert_eq!(info.applied_mic_ms, 0);
        assert_eq!(info.bypassed_mic_ms, 400);
        assert!(!info.warnings.is_empty());
        if !sealed {
            assert!(info.enabled, "stored preference must remain enabled");
            assert!(info.warnings.iter().any(|s| s.contains("incomplete")));
            assert!(
                !info
                    .warnings
                    .iter()
                    .any(|s| s.contains("disabled in audio processing preferences")),
                "an incomplete capture history must not be reported as a disabled preference"
            );
        }
    }
}

#[test]
fn saved_disabled_preference_preserves_enabled_false_and_exact_source_audio() {
    let mic: Vec<f32> = (0..6400).map(|n| (n as f32 * 0.071).sin() * 0.2).collect();
    let far: Vec<f32> = (0..6400).map(|n| (n as f32 * 0.11).sin() * 0.15).collect();
    let fixture = fixture(
        source(16000, mic.clone()),
        source(16000, far),
        1234,
        vec![(0, eligible())],
        true,
    );
    let (info, out) = process_with_preference(&fixture, false);
    assert!(!info.enabled);
    assert_eq!(info.applied_mic_ms, 0);
    assert_eq!(info.bypassed_mic_ms, 400);
    assert_eq!(flatten(&out.mic), pcm(&mic));
    assert!(
        info.warnings
            .iter()
            .any(|s| s == "Echo cancellation was disabled in audio processing preferences.")
    );
    assert!(!info.warnings.iter().any(|s| s.contains("incomplete")));
}

#[test]
fn saved_8k_microphone_and_48k_system_keep_absolute_time_and_exact_duration() {
    let mic: Vec<f32> = (0..3200)
        .map(|n| (n as f32 * std::f32::consts::TAU * 317.0 / 8000.0).sin() * 0.2)
        .collect();
    let far: Vec<f32> = (0..19200)
        .map(|n| (n as f32 * std::f32::consts::TAU * 181.0 / 48000.0).sin() * 0.1)
        .collect();
    let fixture = fixture(
        source(8000, mic),
        source(48000, far),
        9000,
        vec![(0, OutputRoute::default())],
        true,
    );
    let (info, out) = process(&fixture);
    for packets in [&out.mic, &out.system] {
        assert_eq!(packets.first().unwrap().0, 9000);
        assert_eq!(packets.last().unwrap().0, 9390);
        assert_eq!(flatten(packets).len(), 6400);
        assert!(rms(&flatten(packets)) > 0.05);
    }
    assert_eq!(info.applied_mic_ms, 0);
    assert_eq!(info.bypassed_mic_ms, 400);
}

#[test]
fn saved_route_change_and_gap_preserve_bypass_replies_without_reusing_old_reference() {
    let mic: Vec<f32> = (0..16000).map(|n| 0.2 * (n as f32 * 0.071).sin()).collect();
    let far: Vec<f32> = (0..8000).map(|n| 0.15 * (n as f32 * 0.11).sin()).collect();
    let mut headphones = eligible();
    headphones.kind = OutputRouteKind::Headphones;
    headphones.headphone_jack_connected = Some(true);
    headphones.reason = OutputRouteReason::Headphones;
    let fixture = fixture(
        TrackSource {
            rate: 16000,
            samples: mic.clone(),
            gaps: vec![(500, 800)],
        },
        source(16000, far),
        3000,
        vec![(0, eligible()), (400, headphones), (800, eligible())],
        true,
    );
    let (info, out) = process(&fixture);
    let clean = flatten(&out.mic);
    let original = pcm(&mic);
    assert_eq!(clean.len(), 11200);
    assert_eq!(
        &clean[6400..8000],
        &original[6400..8000],
        "headphone route must bypass"
    );
    assert_eq!(
        &clean[8000..],
        &original[12800..],
        "post-gap missing reference must preserve the reply"
    );
    assert_eq!(flatten(&out.system).len(), 8000);
    assert!(out.gaps.contains(&(TrackKind::Mic, 3500)));
    assert!(out.mic.iter().all(|(at, _)| *at < 3500 || *at >= 3800));
    assert_eq!(out.mic.first().unwrap().0, 3000);
    assert_eq!(out.mic.last().unwrap().0, 3990);
    assert!(out.mic.iter().any(|(at, _)| *at == 3800));
    assert_eq!(info.applied_mic_ms + info.bypassed_mic_ms, 700);
    assert!(info.bypassed_mic_ms >= 300);
    assert!(info.warnings.iter().any(|s| s.contains("Headphones")));
    assert!(info.warnings.iter().any(|s| s.contains("reference")));
}
