use super::*;
use std::{fs, thread, time::Duration};
use tempfile::TempDir;

#[cfg(unix)]
fn test_service() -> (TempDir, Arc<AppService>) {
    use std::os::unix::fs::PermissionsExt;
    let temp = TempDir::new().unwrap();
    let helper = temp.path().join("capture-test-helper");
    fs::write(&helper,r#"#!/usr/bin/env python3
import sys,json,select,time
print(json.dumps({'type':'format','track':'mic','sample_rate':8000,'channels':1,'epoch':0,'start_ns':0}),flush=True)
i=0
while True:
    if select.select([sys.stdin],[],[],0)[0]:
        if sys.stdin.readline().strip()=='stop': break
    print(json.dumps({'type':'audio','track':'mic','epoch':0,'first_frame':i*80,'host_time_ns':i*10000000,'samples':[0.25]*80}),flush=True)
    print(json.dumps({'type':'level','track':'mic','value':0.25}),flush=True)
    i+=1
    time.sleep(0.01)
"#).unwrap();
    fs::set_permissions(&helper, fs::Permissions::from_mode(0o700)).unwrap();
    let service = AppService::new(temp.path().join("data"), helper).unwrap();
    (temp, service)
}

fn wait_until(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !condition() {
        assert!(
            Instant::now() < deadline,
            "session did not reach expected state"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(unix)]
#[test]
fn recording_pause_resume_notes_export_and_restart_use_persistent_state() {
    let (_temp, service) = test_service();
    assert!(
        service
            .start("No consent".into(), CaptureMode::InPerson, false, None)
            .is_err()
    );
    let started = service
        .start("Capture fixture".into(), CaptureMode::InPerson, true, None)
        .unwrap();
    let id = started.recording.unwrap().id;
    wait_until(|| {
        service
            .snapshot()
            .unwrap()
            .recording
            .as_ref()
            .is_some_and(|r| r.mic_level > 0.0)
    });
    service.save_note(&id, "A fixture bookmark").unwrap();
    service.pause().unwrap();
    thread::sleep(Duration::from_millis(100));
    service.resume().unwrap();
    thread::sleep(Duration::from_millis(100));
    service.stop().unwrap();
    wait_until(|| service.snapshot().unwrap().recording.is_none());
    let detail = service.detail(&id).unwrap();
    assert_eq!(detail.meeting.status, "ready");
    assert_eq!(detail.notes[0].text, "A fixture bookmark");
    let db = store::open(service.root()).unwrap();
    let gaps: i64 = db
        .query_row(
            "SELECT COUNT(*) FROM audio_chunks WHERE meeting_id=?1 AND state='gap'",
            [&id],
            |r| r.get(0),
        )
        .unwrap();
    assert!(gaps > 0, "pause must be an explicit gap");
    let clocks: i64 = db
        .query_row(
            "SELECT COUNT(*) FROM app_audio_clock WHERE meeting_id=?1",
            [&id],
            |r| r.get(0),
        )
        .unwrap();
    assert!(clocks > 0, "source hardware observations must be retained");
    for format in ["markdown", "txt", "json", "srt", "vtt"] {
        assert!(service.export(&id, format).unwrap().is_file());
    }
    service.rename(&id, "Renamed fixture").unwrap();
    let root = service.root.clone();
    let helper = service.helper.clone();
    wait_until(|| Arc::strong_count(&service) == 1);
    drop(service);
    let restarted = AppService::new(root, helper).unwrap();
    assert_eq!(
        restarted.detail(&id).unwrap().meeting.title,
        "Renamed fixture"
    );
    assert_eq!(restarted.detail(&id).unwrap().notes.len(), 1);
}

#[cfg(unix)]
#[test]
fn unexpected_capture_exit_is_visible_and_recoverable() {
    use std::os::unix::fs::PermissionsExt;
    let (temp, service) = test_service();
    fs::write(&service.helper,"#!/bin/sh\nprintf '%s\\n' '{\"type\":\"error\",\"message\":\"Fixture capture failure\",\"fatal\":true}'\n").unwrap();
    fs::set_permissions(&service.helper, fs::Permissions::from_mode(0o700)).unwrap();
    service
        .start("Failure fixture".into(), CaptureMode::Remote, true, None)
        .unwrap();
    wait_until(|| {
        service
            .snapshot()
            .unwrap()
            .recording
            .as_ref()
            .is_some_and(|r| r.status == "failed")
    });
    let view = service.snapshot().unwrap().recording.unwrap();
    assert!(view.error.unwrap().contains("Fixture capture failure"));
    assert_eq!(service.detail(&view.id).unwrap().meeting.status, "failed");
    drop(temp);
}

#[cfg(unix)]
#[test]
fn replay_staging_preserves_existing_transcript_until_successful_publish() {
    let (_temp, service) = test_service();
    let recording = vt_record::Recording::create(service.root(), "replay").unwrap();
    store::create_meeting(service.root(), "replay", "Replay fixture", "remote").unwrap();
    recording.finish().unwrap();
    let segment = |id: &str, text: &str| Segment {
        id: id.into(),
        track: "mic".into(),
        start_ms: 0,
        end_ms: 100,
        text: text.into(),
        speaker: "Me".into(),
        words: Vec::new(),
    };
    store::save_segment(
        service.root(),
        "replay",
        &segment("original", "Original partial"),
    )
    .unwrap();
    store::stage_segment(
        service.root(),
        "cancelled-job",
        "replay",
        &segment("cancelled", "Unpublished result"),
    )
    .unwrap();
    assert_eq!(
        service.detail("replay").unwrap().segments[0].text,
        "Original partial"
    );
    store::discard_staged(service.root(), "cancelled-job").unwrap();
    assert_eq!(
        service.detail("replay").unwrap().segments[0].text,
        "Original partial"
    );
    store::stage_segment(
        service.root(),
        "completed-job",
        "replay",
        &segment("replacement", "Complete replacement"),
    )
    .unwrap();
    store::publish_staged(service.root(), "completed-job", "replay").unwrap();
    let detail = service.detail("replay").unwrap();
    assert_eq!(detail.segments.len(), 1);
    assert_eq!(detail.segments[0].text, "Complete replacement");
    assert_eq!(detail.meeting.status, "ready");
}

#[cfg(unix)]
#[test]
fn shutdown_waits_for_saved_processing_and_signals_cancellation() {
    let (_temp, service) = test_service();
    service.runtime.lock().unwrap().processing = true;
    let worker = service.clone();
    let handle = thread::spawn(move || {
        wait_until(|| worker.cancel_processing.load(Ordering::Relaxed));
        thread::sleep(Duration::from_millis(100));
        worker.runtime.lock().unwrap().processing = false;
    });
    service.wait_for_shutdown();
    assert!(!service.runtime.lock().unwrap().processing);
    handle.join().unwrap();
}

#[cfg(unix)]
#[test]
fn second_app_instance_cannot_discard_a_running_replay_job() {
    let (_temp, service) = test_service();
    let recording = vt_record::Recording::create(service.root(), "instance-check").unwrap();
    store::create_meeting(
        service.root(),
        "instance-check",
        "Instance fixture",
        "remote",
    )
    .unwrap();
    recording.finish().unwrap();
    let segment = Segment {
        id: "staged".into(),
        track: "mic".into(),
        start_ms: 0,
        end_ms: 1,
        text: "Pending fixture".into(),
        speaker: "Me".into(),
        words: Vec::new(),
    };
    store::stage_segment(
        service.root(),
        "in-progress-job",
        "instance-check",
        &segment,
    )
    .unwrap();
    assert!(AppService::new(service.root.clone(), service.helper.clone()).is_err());
    let count: i64 = store::open(service.root())
        .unwrap()
        .query_row("SELECT COUNT(*) FROM app_transcription_staging", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(count, 1);
}
