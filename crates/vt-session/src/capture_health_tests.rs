use super::*;

const MS: u64 = 1_000_000;
const SECOND: u64 = 1_000 * MS;

fn heartbeat(callbacks: u64) -> CaptureDiagnostic {
    CaptureDiagnostic::Heartbeat {
        epoch: 1,
        callbacks,
        control_ticks: 0,
        queued_packets: 3,
        max_emit_ns: 7 * MS,
        at_ns: 0,
    }
}

/// Drives the monitor like the session loop: an iteration every 50 ms and a
/// heartbeat every second while `heartbeats` says so. Returns the final time.
fn run(
    monitor: &mut CaptureMonitor,
    health: &mut CaptureHealth,
    from: u64,
    until: u64,
    mut callbacks: impl FnMut(u64) -> Option<u64>,
) -> u64 {
    let mut now = from;
    while now < until {
        monitor.iteration(now, now / MS, health);
        if now.is_multiple_of(SECOND)
            && let Some(count) = callbacks(now)
        {
            monitor.heartbeat(now, now / MS, &heartbeat(count), health);
        }
        monitor.evaluate(now, true, health);
        now += 50 * MS;
    }
    now
}

#[test]
fn frozen_callbacks_stall_after_five_seconds_and_clear_when_audio_resumes() {
    let mut monitor = CaptureMonitor::default();
    let mut health = CaptureHealth::default();
    // Healthy for 10 s, then the IOProc stops while heartbeats continue.
    let now = run(&mut monitor, &mut health, 0, 10 * SECOND, |t| Some(t / MS));
    assert_eq!(monitor.view().state, CaptureState::Ok);
    let frozen = 9 * SECOND / MS;
    let now = run(&mut monitor, &mut health, now, 14 * SECOND, |_| {
        Some(frozen)
    });
    assert_eq!(
        monitor.view().state,
        CaptureState::Ok,
        "under 5 s is not a stall"
    );
    let now = run(&mut monitor, &mut health, now, 16 * SECOND, |_| {
        Some(frozen)
    });
    assert_eq!(monitor.view().state, CaptureState::Stalled);
    assert_eq!(monitor.view().kind, Some(StallKind::NoAudioCallbacks));
    // `since` is the last heartbeat that showed callbacks advancing.
    assert_eq!(monitor.view().since_source_ms, Some(9_000));
    assert_eq!(health.stalls.len(), 1);
    assert_eq!(health.stalls[0].duration_ms, None);
    run(&mut monitor, &mut health, now, 20 * SECOND, |t| {
        Some(t / MS)
    });
    assert_eq!(monitor.view(), &CaptureView::default());
    assert_eq!(health.stalls_total, 1);
    assert_eq!(health.stalls[0].kind, StallKind::NoAudioCallbacks);
    assert_eq!(health.stalls[0].since_source_ms, 9_000);
    assert!(health.stalls[0].duration_ms.is_some_and(|d| d >= 1_000));
    assert_eq!(health.slow_iterations, 0);
    assert_eq!(health.heartbeats, 20);
    assert_eq!((health.max_emit_ms, health.max_queued_packets), (7, 3));
}

#[test]
fn missing_heartbeats_while_the_loop_runs_are_helper_silent() {
    let mut monitor = CaptureMonitor::default();
    let mut health = CaptureHealth::default();
    let now = run(&mut monitor, &mut health, 0, 5 * SECOND, |t| Some(t / MS));
    let now = run(&mut monitor, &mut health, now, 10 * SECOND, |_| None);
    assert_eq!(monitor.view().state, CaptureState::Stalled);
    assert_eq!(monitor.view().kind, Some(StallKind::HelperSilent));
    assert_eq!(monitor.view().since_source_ms, Some(4_000));
    // A heartbeat ends `helper_silent`, even before callbacks are judged.
    run(&mut monitor, &mut health, now, 12 * SECOND, |t| {
        Some(t / MS)
    });
    assert_eq!(monitor.view().state, CaptureState::Ok);
    assert_eq!(health.stalls[0].kind, StallKind::HelperSilent);
    assert!(health.stalls[0].duration_ms.is_some());
}

#[test]
fn a_slow_session_loop_is_consumer_slow_and_never_blames_the_helper() {
    let mut monitor = CaptureMonitor::default();
    let mut health = CaptureHealth::default();
    let now = run(&mut monitor, &mut health, 0, 3 * SECOND, |t| Some(t / MS));
    // One iteration blocked for 7 s (e.g. a database write); every heartbeat
    // sent meanwhile waits in the queue.
    let late = now - 50 * MS + 7 * SECOND;
    assert!(monitor.iteration(late, late / MS, &mut health));
    assert!(!monitor.evaluate(late, true, &mut health));
    assert_eq!(monitor.view().state, CaptureState::ConsumerSlow);
    assert_eq!(monitor.view().kind, Some(StallKind::ConsumerSlow));
    assert_eq!(health.slow_iterations, 1);
    assert_eq!(health.max_iteration_ms, 7_000);
    assert_eq!(health.stalls.len(), 1);
    assert_eq!(health.stalls[0].kind, StallKind::ConsumerSlow);
    assert_eq!(health.stalls[0].duration_ms, Some(7_000));
    // The queued heartbeats arrive with advancing callbacks: back to normal,
    // never `stalled`.
    let end = run(
        &mut monitor,
        &mut health,
        late + 50 * MS,
        late + 6 * SECOND,
        |t| Some(t / MS),
    );
    assert_eq!(monitor.view(), &CaptureView::default());
    assert!(
        health
            .stalls
            .iter()
            .all(|s| s.kind == StallKind::ConsumerSlow)
    );
    // Shorter slow iterations are counted, not recorded as stalls.
    monitor.iteration(end + 600 * MS, 0, &mut health);
    assert_eq!(health.slow_iterations, 2);
    assert_eq!(health.stalls_total, 1);
}

#[test]
fn pauses_and_helpers_without_heartbeats_never_stall() {
    let mut monitor = CaptureMonitor::default();
    let mut health = CaptureHealth::default();
    // An older helper sends no heartbeats at all.
    run(&mut monitor, &mut health, 0, 30 * SECOND, |_| None);
    assert_eq!(monitor.view().state, CaptureState::Ok);
    let mut monitor = CaptureMonitor::default();
    let now = run(&mut monitor, &mut health, 0, 2 * SECOND, |t| Some(t / MS));
    let mut t = now;
    while t < now + 20 * SECOND {
        monitor.iteration(t, 0, &mut health);
        monitor.evaluate(t, false, &mut health);
        t += 50 * MS;
    }
    assert_eq!(monitor.view().state, CaptureState::Ok);
    assert!(health.stalls.is_empty());
}

#[test]
fn rebuild_slow_call_and_epoch_diagnostics_are_recorded_and_bounded() {
    let mut health = CaptureHealth::default();
    assert!(health.record(
        1_000,
        &CaptureDiagnostic::Epoch {
            epoch: 2,
            sample_rate: 48_000,
            mic_channels: 1,
            system_channels: 2,
            quantum: 512,
            input_transport: InputTransport::Bluetooth,
        }
    ));
    assert!(health.record(
        1_500,
        &CaptureDiagnostic::Settled {
            epoch: 2,
            settle_ns: 1_200 * MS,
            absorbed: 1,
            stable: true,
        }
    ));
    for n in 0..40 {
        assert!(health.record(
            2_000 + n,
            &CaptureDiagnostic::Rebuild {
                reason: RebuildReason::DeviceChange,
                trigger: RebuildTrigger::Poll,
                previous_epoch: 1,
                epoch: 2,
                trigger_age_ns: 2_100 * MS,
                fingerprint_ns: 30 * MS,
                stop_ns: 45_000 * MS,
                start_ns: 900 * MS,
                result: RebuildResult::Ok,
            }
        ));
        assert!(health.record(
            3_000,
            &CaptureDiagnostic::SlowCall {
                call: "AudioDeviceStop".into(),
                duration_ns: 44_000 * MS,
                phase: DiagnosticPhase::Stop,
            }
        ));
    }
    assert!(!health.record(0, &CaptureDiagnostic::Unknown));
    assert!(!health.record(0, &heartbeat(1)));
    let epoch = &health.epochs[0];
    assert_eq!(
        (epoch.sample_rate, epoch.input_transport, epoch.settle_ms),
        (48_000, InputTransport::Bluetooth, Some(1_200))
    );
    assert_eq!(health.rebuilds.len(), MAX_RECORDS);
    assert_eq!(health.rebuilds_total, 40);
    assert_eq!(health.slow_calls.len(), MAX_RECORDS);
    assert_eq!(health.slow_calls_total, 40);
    assert!(health.truncated);
    let rebuild = &health.rebuilds[0];
    assert_eq!(
        (
            rebuild.trigger_age_ms,
            rebuild.stop_ms,
            rebuild.start_ms,
            rebuild.result
        ),
        (2_100, 45_000, 900, RebuildResult::Ok)
    );
    assert_eq!(health.slow_calls[0].duration_ms, 44_000);
    // Persisted JSON carries no device names or UIDs, only these fields.
    let json = serde_json::to_value(&health).unwrap();
    assert_eq!(json["epochs"][0]["input_transport"], "bluetooth");
    assert_eq!(json["rebuilds"][0]["trigger"], "poll");
    assert_eq!(
        serde_json::from_value::<CaptureHealth>(json).unwrap(),
        health
    );
    assert_eq!(
        serde_json::from_str::<CaptureHealth>("{}").unwrap(),
        CaptureHealth::default()
    );
}
