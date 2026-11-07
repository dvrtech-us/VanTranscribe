use super::*;
fn eligible() -> RouteEligibility {
    RouteEligibility {
        eligible: true,
        epoch: 1,
        reason: None,
    }
}
fn processor() -> EchoProcessor {
    let mut p = EchoProcessor::new(DspConfig::default()).unwrap();
    p.configure_at_time(eligible(), 0).unwrap();
    p
}
fn flatten(output: Vec<ProcessedAudio>, track: TrackKind) -> Vec<f32> {
    output
        .into_iter()
        .filter(|a| a.track == track)
        .flat_map(|a| a.samples)
        .collect()
}
#[test]
fn default_bypass_never_modifies_caller_audio() {
    let mut p = EchoProcessor::new(DspConfig::default()).unwrap();
    let source = vec![0.25; 160];
    let before = source.clone();
    let result = p.accept(TrackKind::Mic, &source, 16000, 1234).unwrap();
    assert_eq!(source, before);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].samples, source);
    assert_eq!(result[0].start_ms, 1234);
    assert!(!result[0].aec_applied);
    assert!(result[0].reason.is_some());
}
#[test]
fn missing_reference_and_flush_preserve_every_microphone_sample() {
    let mut p = processor();
    let signal = (0..16000)
        .map(|n| ((n as f32) * 0.1).sin() * 0.1)
        .collect::<Vec<_>>();
    let mut output = p.accept(TrackKind::Mic, &signal, 16000, 0).unwrap();
    assert!(p.near.len() <= 20);
    output.extend(p.flush().unwrap());
    let actual = output
        .iter()
        .filter(|a| a.track == TrackKind::Mic)
        .flat_map(|a| a.samples.iter().copied())
        .collect::<Vec<_>>();
    assert_eq!(actual, signal);
    assert!(output.iter().all(|a|!a.aec_applied&&a.reason.as_deref().is_some_and(|s|s.contains("reference"))));
    assert!(p.accept(TrackKind::Mic, &[0.; 160], 16000, 1000).is_err());
}
#[test]
fn arbitrary_track_arrival_matches_reference_within_bound() {
    let mut p = processor();
    let source = vec![0.1; 1600];
    assert!(
        p.accept(TrackKind::Mic, &source, 16000, 1000)
            .unwrap()
            .is_empty()
    );
    let out = p.accept(TrackKind::System, &source, 16000, 1000).unwrap();
    assert_eq!(
        out.iter()
            .filter(|a| a.track == TrackKind::Mic && a.aec_applied)
            .count(),
        10
    );
    assert_eq!(flatten(out, TrackKind::System), source);
    assert!(p.near.is_empty());
}
#[test]
fn timestamp_alignment_supports_different_track_origins_without_inventing_reference() {
    let mut p = processor();
    let mut out = p.accept(TrackKind::System, &[0.2; 480], 16000, 4).unwrap();
    out.extend(p.accept(TrackKind::Mic, &[0.1; 480], 16000, 0).unwrap());
    out.extend(p.flush().unwrap());
    let mic = out
        .iter()
        .filter(|a| a.track == TrackKind::Mic)
        .collect::<Vec<_>>();
    assert_eq!(mic.len(), 3);
    assert_eq!(
        mic.iter().map(|a| a.start_ms).collect::<Vec<_>>(),
        vec![0, 10, 20]
    );
    assert!(!mic[0].aec_applied);
    assert!(mic[1..].iter().all(|a| a.aec_applied));
    assert_eq!(mic[0].samples, [0.1; 160]);
}
#[test]
fn route_changes_and_gaps_drain_held_short_replies_without_padding() {
    let mut p = processor();
    let reply = vec![0.3; 97];
    assert!(
        p.accept(TrackKind::Mic, &reply, 16000, 0)
            .unwrap()
            .is_empty()
    );
    let out = p
        .configure_at_time(
            RouteEligibility {
                eligible: false,
                epoch: 2,
                reason: Some("Headphones: echo cancellation is bypassed.".into()),
            },
            10,
        )
        .unwrap();
    assert_eq!(flatten(out, TrackKind::Mic), reply);
    let next = p.accept(TrackKind::Mic, &[0.4; 160], 16000, 10).unwrap();
    assert!(!next[0].aec_applied);
    assert_eq!(next[0].samples, [0.4; 160]);
    assert_eq!(next[0].start_ms, 10);
    p.reset_at_time("Explicit pause gap", 1000).unwrap();
    let resumed = p.accept(TrackKind::Mic, &[0.2; 160], 16000, 1000).unwrap();
    assert_eq!(resumed[0].start_ms, 1000);
}
#[test]
fn native_formats_resample_with_exact_duration_and_absolute_origins() {
    for rate in [8000, 16000, 44100, 48000, 96000, 192000] {
        let mut p = EchoProcessor::new(DspConfig::default()).unwrap();
        let source = (0..rate)
            .map(|n| (n as f32 * std::f32::consts::TAU * 440. / rate as f32).sin() * 0.1)
            .collect::<Vec<_>>();
        let mut out = Vec::new();
        for (index, packet) in source.chunks((rate / 100) as usize).enumerate() {
            let start = 5000 + index as u64 * 10;
            out.extend(p.accept(TrackKind::Mic, packet, rate, start).unwrap());
        }
        out.extend(p.flush().unwrap());
        let mic = out
            .iter()
            .filter(|a| a.track == TrackKind::Mic)
            .collect::<Vec<_>>();
        assert_eq!(
            mic.iter().map(|a| a.samples.len()).sum::<usize>(),
            16000,
            "rate{rate}"
        );
        assert_eq!(mic[0].start_ms, 5000);
        assert!(mic.windows(2).all(|p| p[1].start_ms == p[0].start_ms + 10));
        assert!(mic.iter().all(|a| a.rate == 16000
            && a.samples.len() <= 160
            && a.samples.iter().all(|s| s.is_finite())));
    }
}
#[test]
fn format_change_flushes_resampler_tails_before_starting_new_epoch() {
    let mut p = EchoProcessor::new(DspConfig::default()).unwrap();
    let mut out = p.accept(TrackKind::Mic, &[0.25; 441], 44100, 0).unwrap();
    out.extend(p.accept(TrackKind::Mic, &[0.5; 480], 48000, 10).unwrap());
    out.extend(p.flush().unwrap());
    let mic = out
        .iter()
        .filter(|a| a.track == TrackKind::Mic)
        .collect::<Vec<_>>();
    assert_eq!(mic.iter().map(|a| a.samples.len()).sum::<usize>(), 320);
    assert_eq!(mic[0].start_ms, 0);
    assert_eq!(mic.last().unwrap().start_ms, 10);
}
#[test]
fn sinc_output_is_chunk_equivalent_and_rejects_above_nyquist() {
    let run = |frequency: f32, chunk: usize| {
        let mut p = EchoProcessor::new(DspConfig::default()).unwrap();
        let source = (0..48000)
            .map(|n| (n as f32 * std::f32::consts::TAU * frequency / 48000.).sin())
            .collect::<Vec<_>>();
        let mut out = Vec::new();
        let mut offset = 0;
        for packet in source.chunks(chunk) {
            out.extend(
                p.accept(TrackKind::Mic, packet, 48000, offset * 1000 / 48000)
                    .unwrap(),
            );
            offset += packet.len() as u64;
        }
        out.extend(p.flush().unwrap());
        flatten(out, TrackKind::Mic)
    };
    let a = run(440., 997);
    let b = run(440., 48000);
    assert_eq!(a.len(), b.len());
    assert!(a.iter().zip(&b).all(|(a, b)| (a - b).abs() < 1e-5));
    let high = run(12000., 997);
    let rms = |x: &[f32]| (x[100..15900].iter().map(|v| v * v).sum::<f32>() / 15800.).sqrt();
    assert!(rms(&a) > 0.65);
    assert!(rms(&high) < 0.05);
}
#[test]
fn long_asymmetric_input_keeps_queues_bounded_and_reports_bypass() {
    let mut p = processor();
    let packet = [0.1; 32000];
    for second in 0..20 {
        let out = p
            .accept(TrackKind::Mic, &packet, 16000, second * 2000)
            .unwrap();
        assert!(p.near.iter().map(|f| f.samples.len()).sum::<usize>() <= 3200);
        assert!(out.iter().all(|a| !a.aec_applied));
    }
    p.flush().unwrap();
    let mut p = processor();
    for second in 0..20 {
        p.accept(TrackKind::System, &packet, 16000, second * 2000)
            .unwrap();
        assert!(p.far.iter().map(|f| f.samples.len()).sum::<usize>() <= 3200);
    }
}
#[test]
fn bad_input_and_overlapping_track_packets_are_rejected_before_state_changes() {
    let mut p = processor();
    for (rate, input, start) in [
        (0, vec![0.], 0),
        (192001, vec![0.], 0),
        (16000, vec![f32::NAN], 0),
        (16000, vec![0.; 32001], 0),
        (16000, vec![0.; 160], u64::MAX),
    ] {
        assert!(p.accept(TrackKind::Mic, &input, rate, start).is_err());
        assert!(p.near.is_empty());
    }
    p.accept(TrackKind::Mic, &[0.; 320], 16000, 0).unwrap();
    assert!(p.accept(TrackKind::Mic, &[0.; 320], 16000, 0).is_err());
    assert_eq!(p.near.len(), 2);
    assert!(EchoProcessor::new(DspConfig { max_jitter_ms: 201 }).is_err());
}
fn colored(seed: &mut u64, low: &mut f32) -> f32 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 7;
    *seed ^= *seed << 17;
    let noise = (*seed >> 40) as f32 / (1u32 << 23) as f32 - 1.;
    *low = 0.85 * *low + 0.15 * noise;
    *low
}
fn rms(x: &[f32]) -> f64 {
    (x.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>() / x.len() as f64).sqrt()
}
fn best_near_gain(output: &[f32], near: &[f32], start: usize, end: usize) -> f64 {
    let energy = near[start..end]
        .iter()
        .map(|x| f64::from(*x).powi(2))
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
        .fold(0., f64::max)
}
#[test]
fn actual_aec3_converges_and_retains_double_talk_and_short_near_reply() {
    let n = 24 * 16000;
    let mut far = vec![0.; n];
    let mut near = vec![0.; n];
    let (mut seed, mut seed2) = (0x123456789abcdefu64, 0x9876543210abcdefu64);
    let (mut low, mut low2) = (0., 0.);
    for i in 0..n {
        let t = i as f32 / 16000.;
        let envelope = 0.35 + 0.65 * (t * std::f32::consts::TAU * 2.3).sin().abs();
        far[i] = envelope
            * (0.32 * colored(&mut seed, &mut low)
                + 0.12 * (t * std::f32::consts::TAU * 173.).sin()
                + 0.07 * (t * std::f32::consts::TAU * 431.).sin());
        let talking = (12. * 16000.) as usize <= i && i < (16. * 16000.) as usize
            || (18. * 16000.) as usize <= i && i < (18.25 * 16000.) as usize;
        if talking {
            near[i] = 0.45 * colored(&mut seed2, &mut low2)
                + 0.13 * (t * std::f32::consts::TAU * 283.).sin();
        }
    }
    let mut mic = near.clone();
    for i in 640..n {
        mic[i] += 0.65 * far[i - 480] + 0.15 * far[i - 640];
    }
    let original = mic.clone();
    let mut p = processor();
    let mut output = Vec::new();
    let mut system = Vec::new();
    for frame in 0..n / 160 {
        let start = frame * 160;
        let at = frame as u64 * 10;
        system.extend(flatten(
            p.accept(TrackKind::System, &far[start..start + 160], 16000, at)
                .unwrap(),
            TrackKind::System,
        ));
        output.extend(flatten(
            p.accept(TrackKind::Mic, &mic[start..start + 160], 16000, at)
                .unwrap(),
            TrackKind::Mic,
        ));
    }
    output.extend(flatten(p.flush().unwrap(), TrackKind::Mic));
    assert_eq!(mic, original);
    assert_eq!(system, far);
    assert_eq!(output.len(), n);
    let suppression = rms(&output[8 * 16000..11 * 16000]) / rms(&mic[8 * 16000..11 * 16000]);
    let double_gain = best_near_gain(&output, &near, 13 * 16000, 15 * 16000);
    let short_gain = best_near_gain(&output, &near, 18 * 16000, 18 * 16000 + 4000);
    eprintln!(
        "Synthetic AEC RMS ratio={suppression:.4}, double-talk near gain={double_gain:.4}, short-reply near gain={short_gain:.4}"
    );
    assert!(suppression < 0.25, "AEC did not converge");
    assert!(double_gain > 0.35, "double-talk voice was suppressed");
    assert!(short_gain > 0.2, "short near reply was destroyed");
}

#[test]
fn route_epoch_reset_rejects_old_future_reference_but_preserves_mic() {
    let mut p = processor();
    p.accept(TrackKind::System, &[0.1; 3200], 16000, 0).unwrap();
    p.accept(TrackKind::Mic, &[0.2; 1600], 16000, 0).unwrap();
    assert!(p.engine.is_some());
    p.configure_at_time(
        RouteEligibility {
            eligible: true,
            epoch: 2,
            reason: None,
        },
        100,
    )
    .unwrap();
    assert!(p.engine.is_none());
    assert!(p.far.is_empty());
    let mut output = p.accept(TrackKind::Mic, &[0.25; 1600], 16000, 100).unwrap();
    output.extend(p.flush().unwrap());
    assert!(output.iter().all(|a| !a.aec_applied));
    assert_eq!(flatten(output, TrackKind::Mic), vec![0.25; 1600]);
}
#[test]
fn native_fractional_rate_pending_audio_stays_inside_two_hundred_ms() {
    let mut p = processor();
    let rate = 12345;
    let source = vec![0.1; rate as usize * 2];
    let mut offset = 0;
    for packet in source.chunks(997) {
        p.accept(
            TrackKind::Mic,
            packet,
            rate,
            offset * 1000 / u64::from(rate),
        )
        .unwrap();
        offset += packet.len() as u64;
        let state = &p.converters[0];
        let not_framed =
            (state.input_frames * 16000 / u64::from(rate)).saturating_sub(state.output_frames);
        let queued = p.near.iter().map(|f| f.samples.len() as u64).sum::<u64>();
        assert!(queued + not_framed <= 3200, "pending capture exceeded200ms");
    }
}
#[test]
fn terminal_partial_frame_bypasses_without_padding_or_voice_loss() {
    let mut p = processor();
    p.accept(TrackKind::System, &[0.1; 73], 16000, 1000)
        .unwrap();
    p.accept(TrackKind::Mic, &[0.25; 73], 16000, 1000).unwrap();
    let out = p.flush().unwrap();
    let mic = out.iter().find(|a| a.track == TrackKind::Mic).unwrap();
    assert_eq!(mic.start_ms, 1000);
    assert_eq!(mic.samples, [0.25; 73]);
    assert!(!mic.aec_applied);
    assert_eq!(mic.reason.as_deref(), Some(PARTIAL));
}
#[test]
fn zero_jitter_is_explicit_immediate_bypass_or_exact_reference() {
    let mut p = EchoProcessor::new(DspConfig { max_jitter_ms: 0 }).unwrap();
    p.configure_at_time(eligible(), 0).unwrap();
    let missing = p.accept(TrackKind::Mic, &[0.2; 160], 16000, 0).unwrap();
    assert_eq!(missing.len(), 1);
    assert!(!missing[0].aec_applied);
    assert!(p.near.is_empty());
    p.accept(TrackKind::System, &[0.1; 160], 16000, 10).unwrap();
    let aligned = p.accept(TrackKind::Mic, &[0.2; 160], 16000, 10).unwrap();
    assert!(aligned[0].aec_applied);
}
