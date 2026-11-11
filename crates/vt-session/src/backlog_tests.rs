use super::*;
use crate::{CaptureMode, session};
use std::{
    fs,
    path::Path,
    sync::{
        Condvar, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Instant,
};
use tempfile::TempDir;

const SECOND: u64 = 1_000_000_000;
fn feed(c: &mut Controller, now: u64, end: u64) {
    for track in [TrackKind::Mic, TrackKind::System] {
        let start = c.tracks[index(track)].end.unwrap_or(0);
        c.raw(track, start, end, 0, now);
    }
}
fn stalled() -> Controller {
    let mut c = Controller::default();
    c.configure(true, true);
    feed(&mut c, 0, 12_000);
    c
}
#[test]
fn sustained_lag_sheds_in_order_and_never_recovers_mid_recording() {
    let mut c = stalled();
    assert_eq!(c.evaluate(0, false), None);
    for second in 1..=12 {
        feed(&mut c, second * SECOND, 12_000 + second * 20);
        let stage = c.evaluate(second * SECOND, false);
        assert_eq!(
            stage,
            match second {
                2 => Some(BacklogStage::PartialsDisabled),
                7 => Some(BacklogStage::SpeakersDisabled),
                12 => Some(BacklogStage::SttDisabled),
                _ => None,
            }
        );
        if second == 2 {
            assert!(c.allowed(Worker::Stt) && c.allowed(Worker::Speakers));
            assert!(c.report.gaps.is_empty());
        }
    }
    c.reset("source_gap", false);
    c.resume();
    feed(&mut c, 13 * SECOND, 100);
    assert_eq!(c.stage, BacklogStage::SttDisabled);
    assert!(!c.allowed(Worker::Stt));
    assert!(
        c.report
            .gaps
            .iter()
            .any(|g| g.worker == "stt" && g.reason == "sustained_backlog")
    );
    assert!(c.report.gaps.iter().any(|g| g.worker == "speakers"));
}
#[test]
fn per_track_origins_completed_silence_and_absent_workers_do_not_create_lag() {
    let mut c = Controller::default();
    c.configure(true, false);
    // Independent helper offsets never enter the lag subtraction across tracks.
    for (track, start) in [(TrackKind::Mic, 5_000_000), (TrackKind::System, 27)] {
        let generation = c.raw(track, start, start + 20, 42, 0);
        for worker in [Worker::Dsp, Worker::Stt] {
            c.complete(worker, track, generation, start + 20);
        }
    }
    assert_eq!(c.evaluate(0, false), None);
    assert_eq!(c.lag_ms, 0);
    assert!(!c.allowed(Worker::Speakers));
    // Completed input advances even when no speech events were emitted.
    for worker in [Worker::Dsp, Worker::Stt] {
        c.complete(worker, TrackKind::Mic, c.generation, 5_000_020);
    }
    assert_eq!(c.lag(), 0);
}
#[test]
fn pauses_stale_source_and_epochs_reset_confirmation_and_reject_old_completions() {
    let mut c = stalled();
    c.evaluate(0, false);
    assert_eq!(c.evaluate(3 * SECOND, false), None); // no fresh source
    feed(&mut c, 4 * SECOND, 12_020);
    assert_eq!(c.evaluate(4 * SECOND, false), None);
    let old = c.generation;
    c.reset("pause", true);
    assert!(
        c.report
            .warnings
            .iter()
            .any(|warning| warning.starts_with("Recording pause reset"))
    );
    assert!(
        !c.report
            .warnings
            .iter()
            .any(|warning| warning.contains("source discontinuity"))
    );
    c.complete(Worker::Stt, TrackKind::Mic, old, 99_999);
    assert_eq!(c.evaluate(6 * SECOND, false), None);
    c.resume();
    let new = c.raw(TrackKind::Mic, 90_000, 90_020, 1, 7 * SECOND);
    assert_ne!(old, new);
    assert_eq!(c.lag(), 20);
    c.complete(Worker::Stt, TrackKind::Mic, old, 100_000);
    assert_eq!(c.tracks[0].completed[1], None);
    let old = new;
    c.raw(TrackKind::Mic, 90_020, 90_040, 2, 8 * SECOND);
    assert_ne!(c.generation, old);
    assert_eq!(c.lag(), 20);
    assert!(
        c.report
            .warnings
            .iter()
            .any(|warning| warning.contains("source discontinuity"))
    );
    assert!(c.report.gaps.iter().any(|g| g.reason == "pause"));
    let mut gap = stalled();
    gap.reset("source_gap", false);
    assert!(
        gap.report
            .warnings
            .iter()
            .any(|warning| warning.contains("source discontinuity"))
    );
    assert!(
        !gap.report
            .warnings
            .iter()
            .any(|warning| warning.starts_with("Recording pause reset"))
    );
}
#[test]
fn optional_stages_skip_and_disk_suspension_is_stronger_and_sticky() {
    let mut c = Controller::default();
    c.configure(false, true);
    feed(&mut c, 0, 12_000);
    c.evaluate(0, false);
    feed(&mut c, 2 * SECOND, 12_020);
    assert_eq!(
        c.evaluate(2 * SECOND, false),
        Some(BacklogStage::SpeakersDisabled)
    );
    let mut c = stalled();
    c.evaluate(0, false);
    feed(&mut c, SECOND, 12_020);
    assert_eq!(c.evaluate(SECOND, true), None);
    assert!(!c.allowed(Worker::Dsp));
    feed(&mut c, 2 * SECOND, 12_040);
    assert_eq!(c.evaluate(2 * SECOND, false), None);
    assert!(
        c.report
            .gaps
            .iter()
            .any(|g| g.reason == "disk_pressure" && g.end_ms == 12_040)
    );
    assert_eq!(c.stage, BacklogStage::Normal);
}
#[test]
fn inference_gap_metadata_is_bounded_and_explicitly_marks_truncation() {
    let mut c = Controller::default();
    for worker in [Worker::Stt, Worker::Speakers, Worker::Dsp] {
        for n in 0..MAX_GAPS + 5 {
            // Separated intervals never merge.
            let start = n as u64 * 100;
            c.gap(
                worker,
                TrackKind::Mic,
                n as u64,
                start,
                start + 20,
                "queue_full",
            );
        }
    }
    assert_eq!(c.report.gaps.len(), 3 * MAX_GAPS_PER_WORKER);
    assert!(c.report.gaps.len() <= MAX_GAPS);
    assert!(c.report.truncated);
    assert_eq!(c.report.warnings.len(), 1);
}
#[test]
fn adjacent_gaps_from_one_worker_collapse_and_leave_room_for_other_workers() {
    let mut c = Controller::default();
    // One dead worker reports every 20 ms chunk, 1 ms apart, across several
    // generations: one entry, keeping the first generation.
    for n in 0..500_u64 {
        c.gap(
            Worker::Speakers,
            TrackKind::System,
            n / 50,
            n * 20,
            n * 20 + 19,
            "worker_unavailable",
        );
    }
    assert_eq!(c.report.gaps.len(), 1);
    let gap = &c.report.gaps[0];
    assert_eq!((gap.start_ms, gap.end_ms, gap.generation), (0, 9_999, 0));
    assert!(!c.report.truncated);
    // A different reason or track is a separate entry, even when adjacent.
    c.gap(
        Worker::Speakers,
        TrackKind::System,
        9,
        10_000,
        10_020,
        "queue_full",
    );
    c.gap(
        Worker::Speakers,
        TrackKind::Mic,
        9,
        0,
        20,
        "worker_unavailable",
    );
    assert_eq!(c.report.gaps.len(), 3);
    // A worker that floods non-adjacent gaps is capped at its share...
    for n in 0..200_u64 {
        c.gap(
            Worker::Speakers,
            TrackKind::Mic,
            0,
            50_000 + n * 100,
            50_020 + n * 100,
            "queue_full",
        );
    }
    assert_eq!(
        c.report
            .gaps
            .iter()
            .filter(|g| g.worker == "speakers")
            .count(),
        MAX_GAPS_PER_WORKER
    );
    assert!(c.report.truncated);
    // ...so the speech worker's gaps are still recorded.
    c.gap(
        Worker::Stt,
        TrackKind::Mic,
        3,
        70_000,
        71_000,
        "recognizer_error",
    );
    assert!(
        c.report
            .gaps
            .iter()
            .any(|g| g.worker == "stt" && g.reason == "recognizer_error" && g.end_ms == 71_000)
    );
}
#[test]
fn old_health_json_without_failures_or_capture_still_deserializes() {
    let report: BacklogReport = serde_json::from_str(
        r#"{"complete":true,"stages":[],"gaps":[{"worker":"stt","track":"mic","generation":4,"start_ms":0,"end_ms":20,"reason":"queue_full"}],"warnings":["w"],"truncated":false,"max_lag_ms":7}"#,
    )
    .unwrap();
    assert!(report.failures.is_empty());
    assert_eq!(
        report.capture,
        crate::capture_health::CaptureHealth::default()
    );
    assert_eq!(report.gaps[0].generation, 4);
    let failure: WorkerFailure =
        serde_json::from_str(r#"{"worker":"stt","source_ms":5,"cause":"c","terminal":true}"#)
            .unwrap();
    assert_eq!(failure.count, 0);
}
#[test]
fn failures_keep_the_first_and_latest_per_worker_with_bounded_causes() {
    let mut c = Controller::default();
    c.configure(true, true);
    for n in 0..10_u64 {
        c.failure(Worker::Stt, Some(n * 1_000), &format!("fault {n}"), false);
    }
    c.failure(Worker::Speakers, Some(3), &"x".repeat(1_000), true);
    let stt = c
        .report
        .failures
        .iter()
        .filter(|f| f.worker == "stt")
        .collect::<Vec<_>>();
    assert_eq!(stt.len(), 2);
    assert_eq!((stt[0].cause.as_str(), stt[0].count), ("fault 0", 1));
    assert_eq!(
        (stt[1].cause.as_str(), stt[1].count, stt[1].source_ms),
        ("fault 9", 10, 9_000)
    );
    let speakers = c
        .report
        .failures
        .iter()
        .find(|f| f.worker == "speakers")
        .unwrap();
    assert!(speakers.terminal && speakers.cause.chars().count() <= 301);
    // Failures are outside the deduplicated, capped warning list.
    assert!(c.report.warnings.is_empty());
}
#[test]
fn a_dead_speaker_worker_never_becomes_the_reason_the_audio_stage_or_speech_stopped() {
    let mut c = Controller::default();
    c.configure(true, true);
    feed(&mut c, 0, 1_000);
    c.worker_exit(
        Worker::Speakers,
        "speaker-inference",
        &Err(anyhow::anyhow!("speaker fault")),
    );
    assert!(c.allowed(Worker::Dsp) && c.allowed(Worker::Stt));
    assert_eq!(c.stop_reason(Worker::Dsp), None);
    assert_eq!(c.live_transcription().unwrap().state, LiveState::Active);
    // Much later the speech worker fails: that, not resource protection, is
    // why the audio stage stops.
    feed(&mut c, SECOND, 600_000);
    c.worker_exit(
        Worker::Stt,
        "speech-inference",
        &Err(anyhow::anyhow!("speech fault")),
    );
    assert!(!c.allowed(Worker::Dsp));
    assert_eq!(
        c.stop_reason(Worker::Dsp),
        Some(LiveStopReason::WorkerFailed)
    );
    let live = c.live_transcription().unwrap();
    assert_eq!(
        (live.state, live.reason, live.since_source_ms),
        (
            LiveState::Stopped,
            Some(LiveStopReason::WorkerFailed),
            Some(600_000)
        )
    );
    assert!(
        c.report
            .failures
            .iter()
            .any(|f| f.worker == "stt" && f.terminal)
    );
    // A later generic stop (the audio stage seeing a closed queue) keeps it.
    c.stopped(Worker::Stt);
    assert_eq!(
        c.live_transcription().unwrap().reason,
        Some(LiveStopReason::WorkerFailed)
    );
    // Shedding and disk protection are named as such.
    let mut shed = stalled();
    shed.worker_exit(
        Worker::Speakers,
        "speaker-inference",
        &Err(anyhow::anyhow!("speaker fault")),
    );
    for second in 1..=12 {
        feed(&mut shed, second * SECOND, 12_000 + second * 20);
        shed.evaluate(second * SECOND, false);
    }
    assert_eq!(
        shed.stop_reason(Worker::Dsp),
        Some(LiveStopReason::SustainedBacklog)
    );
    assert_eq!(
        shed.live_transcription().unwrap().reason,
        Some(LiveStopReason::SustainedBacklog)
    );
    let mut disk = stalled();
    disk.suspend_disk();
    assert_eq!(
        disk.stop_reason(Worker::Dsp),
        Some(LiveStopReason::DiskProtection)
    );
    assert_eq!(
        disk.live_transcription().unwrap().reason,
        Some(LiveStopReason::DiskProtection)
    );
    // Exits after the recording loop ends are shutdown, not a live stop.
    let mut closing = stalled();
    closing.closing();
    closing.worker_exit(Worker::Stt, "speech-inference", &Ok(()));
    assert_eq!(
        closing.live_transcription().unwrap().state,
        LiveState::Active
    );
}

struct Resources(AtomicU64);
impl crate::health::ResourceProbe for Resources {
    fn free_bytes(&self, _: &Path) -> Result<u64> {
        Ok(4_000_000_000)
    }
    fn uptime_ns(&self) -> Option<u64> {
        None
    }
    fn tick_ns(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
}
fn wait(mut condition: impl FnMut() -> bool) {
    let until = Instant::now() + Duration::from_secs(12);
    while !condition() {
        assert!(
            Instant::now() < until,
            "synthetic backlog fixture timed out"
        );
        thread::sleep(Duration::from_millis(5));
    }
}
#[cfg(unix)]
fn helper(temp: &TempDir) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = temp.path().join("fake-helper");
    fs::write(&path,r#"#!/usr/bin/env python3
import json,pathlib,select,sys,time
folder=pathlib.Path(__file__).parent
assert sys.argv[1:3]==['--mode','remote']
def emit(value): print(json.dumps(value),flush=True)
for track in ['mic','system']:
    emit({'type':'format','track':track,'sample_rate':16000,'channels':1,'epoch':0,'start_ns':0})
packets=0
while True:
    if select.select([sys.stdin],[],[],0)[0]:
        if sys.stdin.readline().strip()=='stop': break
    try: target=int((folder/'target').read_text())
    except (FileNotFoundError,ValueError): target=0
    if packets<target:
        for track in ['system','mic']:
            emit({'type':'audio','track':track,'epoch':0,'first_frame':packets*320,'host_time_ns':packets*20000000,'samples':[0.25 if track=='mic' else -0.125]*320})
        packets+=1
    else: time.sleep(0.002)
(folder/'stopped').write_text(str(packets))
"#).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    path
}
struct Release {
    gate: Arc<(Mutex<bool>, Condvar)>,
    app: Arc<AppService>,
}
impl Drop for Release {
    fn drop(&mut self) {
        *self.gate.0.lock().unwrap() = true;
        self.gate.1.notify_all();
        self.app.wait_for_shutdown();
    }
}
#[cfg(unix)]
#[test]
fn actual_session_sheds_slow_workers_but_persists_every_native_frame_and_run_audit() {
    let temp = TempDir::new().unwrap();
    let resources = Arc::new(Resources(AtomicU64::new(0)));
    let app =
        AppService::with_resources(temp.path().join("data"), helper(&temp), resources.clone())
            .unwrap();
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let blocked = gate.clone();
    *app.test_workers.lock().unwrap() = Some(Arc::new(TestWorkers {
        stt: true,
        speakers: true,
        accept: Arc::new(move |_, message| {
            if matches!(message, session::Inference::Audio { .. }) {
                let mut released = blocked.0.lock().unwrap();
                while !*released {
                    released = blocked.1.wait(released).unwrap();
                }
            }
            Ok(vec![])
        }),
        speaker_turns: None,
        live_save: store::LiveSaveBudget::PRODUCTION,
    }));
    let _release = Release {
        gate: gate.clone(),
        app: app.clone(),
    };
    let id = app
        .start(
            "Synthetic backlog test".into(),
            CaptureMode::Remote,
            true,
            None,
        )
        .unwrap();
    let id = id.recording.unwrap().id;
    let feed = |packets: u64| {
        fs::write(temp.path().join("target"), packets.to_string()).unwrap();
        wait(|| {
            app.backlog
                .lock()
                .unwrap()
                .tracks
                .iter()
                .all(|t| t.end == Some(packets * 20))
        });
    };
    feed(600);
    wait(|| app.backlog.lock().unwrap().overload_since.is_some());
    for second in 1..=12 {
        resources.0.store(second * SECOND, Ordering::Relaxed);
        feed(600 + second);
        let stage = match second {
            0..=1 => BacklogStage::Normal,
            2..=6 => BacklogStage::PartialsDisabled,
            7..=11 => BacklogStage::SpeakersDisabled,
            _ => BacklogStage::SttDisabled,
        };
        wait(|| {
            app.snapshot()
                .unwrap()
                .live_processing
                .as_ref()
                .unwrap()
                .backlog
                .stage
                == stage
        });
    }
    // Stage3 affects only inference. More raw audio is accepted after both model workers are disabled.
    feed(620);
    *gate.0.lock().unwrap() = true;
    gate.1.notify_all();
    app.stop().unwrap();
    // The live run is sealed and the meeting status set by separate writes,
    // so wait for both before asserting the outcome.
    wait(|| {
        let detail = app.detail(&id).unwrap();
        detail.meeting.status != "recording" && detail.runs.iter().all(|r| r.status != "running")
    });
    let detail = app.detail(&id).unwrap();
    assert_eq!(detail.meeting.status, "ready");
    let report = detail.inference_health.unwrap();
    assert!(report.complete);
    assert_eq!(
        report.stages.iter().map(|s| s.stage).collect::<Vec<_>>(),
        vec![
            BacklogStage::PartialsDisabled,
            BacklogStage::SpeakersDisabled,
            BacklogStage::SttDisabled
        ]
    );
    assert!(
        report
            .gaps
            .iter()
            .any(|g| g.worker == "stt" && g.reason == "sustained_backlog" && g.end_ms >= 12_400)
    );
    // The native archive decoder also verifies manifest SHA before returning PCM.
    let mut reader = vt_playback::SavedAudioReader::open(&app.root, &id).unwrap();
    let mut frames = [0usize; 2];
    while let Some(packet) = reader.next_packet().unwrap() {
        if let vt_playback::SavedAudioPacket::Audio {
            track,
            samples,
            rate,
            ..
        } = packet
        {
            assert_eq!(rate, 16000);
            assert!(samples.iter().all(|sample| *sample
                == if track == TrackKind::Mic {
                    0.25
                } else {
                    -0.125
                }));
            frames[index(track)] += samples.len();
        } else {
            panic!("inference drops must not create raw recording gaps");
        }
    }
    assert_eq!(frames, [620 * 320; 2]);
    let db = store::open(&app.root).unwrap();
    assert!(
        db.execute(
            "UPDATE app_run_inference_health SET info_json='{}' WHERE run_id=?1",
            [detail.run_id.unwrap()]
        )
        .is_err()
    );
    let next = store::create_run(&app.root, &id, "refine", "{}").unwrap();
    store::publish_run(&app.root, &id, &next.id).unwrap();
    assert!(app.detail(&id).unwrap().inference_health.is_none());
}

#[test]
fn completed_input_without_speech_events_advances_real_consumer_and_old_generations_do_not() {
    let temp = TempDir::new().unwrap();
    let app = AppService::new(temp.path().join("data"), "missing-helper".into()).unwrap();
    let generation = {
        let mut c = app.backlog.lock().unwrap();
        c.configure(true, false);
        c.raw(TrackKind::Mic, 9_000_000, 9_000_020, 1, 0)
    };
    let (tx, rx) = mpsc::channel();
    tx.send(session::Inference::Audio {
        track: TrackKind::Mic,
        samples: vec![0.; 320],
        rate: 16000,
        start_ms: 9_000_000,
        generation: generation.wrapping_sub(1),
    })
    .unwrap();
    tx.send(session::Inference::Audio {
        track: TrackKind::Mic,
        samples: vec![0.; 320],
        rate: 16000,
        start_ms: 9_000_000,
        generation,
    })
    .unwrap();
    drop(tx);
    let mut accepted = 0;
    session::consume_inference(&app, Worker::Stt, &rx, |message| {
        if matches!(message, session::Inference::Audio { .. }) {
            accepted += 1;
        }
        Ok(())
    })
    .unwrap();
    assert_eq!(accepted, 1);
    let c = app.backlog.lock().unwrap();
    assert_eq!(c.tracks[0].completed[1], Some(9_000_020));
}

#[test]
fn stage_one_suppresses_only_partial_publication_and_final_segments_still_commit() {
    let temp = TempDir::new().unwrap();
    let app = AppService::new(temp.path().join("data"), "missing-helper".into()).unwrap();
    let _recording = vt_record::Recording::create(&app.root, "fixture").unwrap();
    store::create_meeting(&app.root, "fixture", "Synthetic publication", "remote").unwrap();
    {
        let mut c = app.backlog.lock().unwrap();
        c.configure(true, true);
        feed(&mut c, 0, 12_000);
        c.evaluate(0, false);
        feed(&mut c, 2 * SECOND, 12_020);
        c.evaluate(2 * SECOND, false);
    }
    let run = store::detail(&app.root, "fixture")
        .unwrap()
        .active_run_id
        .unwrap();
    let writer = crate::live_transcript::TranscriptWriter::start(
        app.clone(),
        "fixture".into(),
        run,
        store::LiveSaveBudget::PRODUCTION,
    )
    .unwrap();
    let live = session::LiveTranscript {
        in_person: false,
        sink: writer.sink(),
    };
    for is_final in [false, true] {
        session::save_transcript(
            &app,
            &live,
            0,
            vt_stt::TranscriptEvent {
                track: TrackKind::Mic,
                start_ms: 0,
                end_ms: 100,
                text: "Public synthetic final".into(),
                is_final,
                words: vec![],
            },
        );
    }
    writer.finish();
    assert!(app.runtime.lock().unwrap().partials.is_empty());
    assert!(app.runtime.lock().unwrap().live_finals.is_empty());
    assert_eq!(
        store::detail(&app.root, "fixture").unwrap().segments.len(),
        1
    );
}

#[test]
fn in_flight_completion_cannot_cross_a_source_generation_and_next_input_resets_both_tracks() {
    let temp = TempDir::new().unwrap();
    let app = AppService::new(temp.path().join("data"), "missing-helper".into()).unwrap();
    let generation = {
        let mut c = app.backlog.lock().unwrap();
        c.configure(true, false);
        c.raw(TrackKind::Mic, 0, 20, 0, 0)
    };
    let (tx, rx) = mpsc::channel();
    let (started, waiting) = mpsc::channel();
    let (release, blocked) = mpsc::channel();
    let worker_app = app.clone();
    let worker = thread::spawn(move || {
        let mut reset_count = 0;
        let mut first = true;
        session::consume_inference(&worker_app, Worker::Stt, &rx, |message| {
            match message {
                session::Inference::Reset(_, _) => reset_count += 1,
                session::Inference::Audio { .. } if first => {
                    first = false;
                    started.send(()).unwrap();
                    blocked.recv_timeout(Duration::from_secs(10)).unwrap();
                }
                _ => (),
            }
            Ok(())
        })
        .unwrap();
        reset_count
    });
    tx.send(session::Inference::Audio {
        track: TrackKind::Mic,
        samples: vec![0.; 320],
        rate: 16000,
        start_ms: 0,
        generation,
    })
    .unwrap();
    waiting.recv_timeout(Duration::from_secs(10)).unwrap();
    let next = {
        let mut c = app.backlog.lock().unwrap();
        c.reset("source_gap", false);
        c.raw(TrackKind::Mic, 80_000, 80_020, 1, SECOND)
    };
    release.send(()).unwrap();
    tx.send(session::Inference::Audio {
        track: TrackKind::Mic,
        samples: vec![0.; 320],
        rate: 16000,
        start_ms: 80_000,
        generation: next,
    })
    .unwrap();
    drop(tx);
    assert_eq!(worker.join().unwrap(), 4);
    let c = app.backlog.lock().unwrap();
    assert_eq!(c.tracks[0].completed[1], Some(80_020));
    assert_ne!(generation, next);
    assert!(
        c.report
            .gaps
            .iter()
            .any(|gap| gap.generation == generation && gap.worker == "stt" && gap.end_ms == 20)
    );
}

#[cfg(unix)]
#[test]
fn actual_graph_audit_failure_preserves_raw_sealed_run_and_appends_a_persisted_warning() {
    audit_failure_graph(false);
}
#[cfg(unix)]
#[test]
fn actual_graph_keeps_ready_current_warning_when_both_audit_and_warning_writes_fail() {
    audit_failure_graph(true);
}
#[cfg(unix)]
fn audit_failure_graph(refuse_warning: bool) {
    let temp = TempDir::new().unwrap();
    let resources = Arc::new(Resources(AtomicU64::new(0)));
    let app =
        AppService::with_resources(temp.path().join("data"), helper(&temp), resources).unwrap();
    *app.test_workers.lock().unwrap() = Some(Arc::new(TestWorkers {
        stt: true,
        speakers: false,
        accept: Arc::new(|_, _| Ok(vec![])),
        speaker_turns: None,
        live_save: store::LiveSaveBudget::PRODUCTION,
    }));
    app.runtime.lock().unwrap().model.installed = true; // Test consumer replaces model setup.
    let gate = Arc::new((Mutex::new(true), Condvar::new()));
    let _shutdown = Release {
        gate,
        app: app.clone(),
    };
    let db = store::open(&app.root).unwrap();
    db.execute_batch("CREATE TRIGGER fail_audit_insert BEFORE INSERT ON app_run_inference_health BEGIN SELECT RAISE(ABORT,'injected ledger failure'); END; CREATE TRIGGER fail_audit_update BEFORE UPDATE ON app_run_inference_health BEGIN SELECT RAISE(ABORT,'injected ledger failure'); END;").unwrap();
    let started = app
        .start(
            "Synthetic ledger failure".into(),
            CaptureMode::Remote,
            true,
            None,
        )
        .unwrap();
    let id = started.recording.unwrap().id;
    fs::write(temp.path().join("target"), "100").unwrap();
    wait(|| {
        app.backlog
            .lock()
            .unwrap()
            .tracks
            .iter()
            .all(|track| track.end == Some(2_000))
    });
    let prior = "Existing capture/source warning must survive";
    let memory_prior = "Separate in-memory capture warning must survive";
    app.runtime
        .lock()
        .unwrap()
        .current
        .as_mut()
        .unwrap()
        .view
        .error = Some(memory_prior.into());
    store::set_status(&app.root, &id, "recording", Some(prior)).unwrap();
    if refuse_warning {
        db.execute_batch("CREATE TRIGGER fail_warning BEFORE UPDATE OF error ON app_meetings WHEN NEW.error LIKE '%Live inference history could not be saved%' BEGIN SELECT RAISE(ABORT,'injected warning persistence failure'); END;").unwrap();
    }
    app.stop().unwrap();
    wait(|| {
        store::detail(&app.root, &id)
            .unwrap()
            .runs
            .iter()
            .all(|run| run.status != "running")
    });
    // Run sealing follows audit join; wait until the ready transition is done.
    wait(|| {
        if refuse_warning {
            app.runtime
                .lock()
                .unwrap()
                .current
                .as_ref()
                .is_some_and(|current| current.view.status == "ready")
        } else {
            store::detail(&app.root, &id).unwrap().meeting.status == "ready"
        }
    });
    let detail = store::detail(&app.root, &id).unwrap();
    assert!(detail.runs.iter().any(|run| run.status == "complete"));
    let incomplete = detail
        .inference_health
        .as_ref()
        .expect("expected live audit absence must be explicit after sealing");
    assert!(!incomplete.complete && incomplete.truncated);
    assert!(
        incomplete
            .warnings
            .iter()
            .any(|warning| warning == AUDIT_WARNING)
    );
    assert!(incomplete.gaps.is_empty());
    assert!(detail.meeting.error.as_deref().unwrap().contains(prior));
    if refuse_warning {
        assert!(app.audit_warning_unpersisted());
        let snapshot = app.snapshot().unwrap();
        let error = snapshot.recording.unwrap().error.unwrap();
        assert!(
            error.contains(prior) && error.contains(memory_prior) && error.contains(AUDIT_WARNING)
        );
        assert!(
            snapshot
                .live_processing
                .unwrap()
                .backlog
                .warning
                .unwrap()
                .contains(AUDIT_WARNING)
        );
    } else {
        assert!(!app.audit_warning_unpersisted());
        let warning = detail.meeting.error.unwrap();
        assert!(warning.contains(AUDIT_WARNING) && warning.contains(memory_prior));
        assert_eq!(warning.matches(AUDIT_WARNING).count(), 1);
        // The append is durable and survives a fresh independent DB read.
        assert!(
            store::detail(&app.root, &id)
                .unwrap()
                .meeting
                .error
                .unwrap()
                .contains(AUDIT_WARNING)
        );
    }
    let mut reader = vt_playback::SavedAudioReader::open(&app.root, &id).unwrap();
    let mut frames = [0; 2];
    while let Some(packet) = reader.next_packet().unwrap() {
        let vt_playback::SavedAudioPacket::Audio { track, samples, .. } = packet else {
            panic!("audit failure must not fabricate raw gaps")
        };
        assert!(samples.iter().all(|sample| *sample
            == if track == TrackKind::Mic {
                0.25
            } else {
                -0.125
            }));
        frames[index(track)] += samples.len();
    }
    assert_eq!(frames, [32_000; 2]);
    let c = app.backlog.lock().unwrap();
    assert!(
        c.report
            .warnings
            .iter()
            .any(|warning| warning == AUDIT_WARNING)
    );
}

#[test]
fn sealed_legacy_ledger_without_completion_and_expected_missing_ledger_are_explicitly_incomplete() {
    let temp = TempDir::new().unwrap();
    let app = AppService::new(temp.path().join("data"), "missing-helper".into()).unwrap();
    let _raw = vt_record::Recording::create(&app.root, "fixture").unwrap();
    store::create_meeting_with_engines(
        &app.root,
        "fixture",
        "Synthetic old ledger",
        "remote",
        r#"{"capture_inference_health_expected":true}"#,
    )
    .unwrap();
    let run = store::detail(&app.root, "fixture")
        .unwrap()
        .active_run_id
        .unwrap();
    let db = store::open(&app.root).unwrap();
    // A periodic snapshot saved by an older build has no completion field.
    db.execute(
        "INSERT INTO app_run_inference_health VALUES(?1,'fixture',?2)",
        params![
            run,
            r#"{"stages":[],"gaps":[],"warnings":[],"truncated":false,"max_lag_ms":0}"#
        ],
    )
    .unwrap();
    store::complete_live_run(&app.root, "fixture").unwrap();
    for present in [true, false] {
        if !present {
            db.execute(
                "DELETE FROM app_run_inference_health WHERE run_id=?1",
                [&run],
            )
            .unwrap();
        }
        let report = info(&db, &run).unwrap().unwrap();
        assert!(!report.complete && report.truncated);
        assert!(report.gaps.is_empty());
        assert!(
            report
                .warnings
                .iter()
                .any(|warning| warning == AUDIT_WARNING)
        );
    }
    let _legacy = vt_record::Recording::create(&app.root, "legacy").unwrap();
    store::create_meeting(&app.root, "legacy", "Legacy unmarked", "remote").unwrap();
    store::complete_live_run(&app.root, "legacy").unwrap();
    assert!(
        store::detail(&app.root, "legacy")
            .unwrap()
            .inference_health
            .is_none()
    );
}
#[test]
fn final_audit_marks_complete_even_when_nothing_changed_since_the_periodic_write() {
    let temp = TempDir::new().unwrap();
    let app = AppService::new(temp.path().join("data"), "missing-helper".into()).unwrap();
    let _raw = vt_record::Recording::create(&app.root, "fixture").unwrap();
    store::create_meeting(
        &app.root,
        "fixture",
        "Synthetic audit finalization",
        "remote",
    )
    .unwrap();
    let run = store::detail(&app.root, "fixture")
        .unwrap()
        .active_run_id
        .unwrap();
    app.backlog.lock().unwrap().configure(true, false);
    let writer = AuditWriter::start(app.clone(), "fixture".into(), run.clone()).unwrap();
    wait(|| {
        info(&store::open(&app.root).unwrap(), &run)
            .unwrap()
            .is_some_and(|report| !report.complete)
    });
    assert_eq!(app.backlog.lock().unwrap().report().0, 0);
    writer.finish();
    store::complete_live_run(&app.root, "fixture").unwrap();
    assert!(
        info(&store::open(&app.root).unwrap(), &run)
            .unwrap()
            .unwrap()
            .complete
    );
}
