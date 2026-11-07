use super::*;
use crate::health::ResourceProbe;
use std::{
    fs,
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
    time::Instant,
};
use tempfile::TempDir;
const SECOND: u64 = 1_000_000_000;
fn app(kind: MeetingAppKind) -> MeetingApp {
    vt_capture::known_meeting_apps()
        .iter()
        .find(|app| serde_json::to_value(app.kind).unwrap() == serde_json::to_value(kind).unwrap())
        .unwrap()
        .clone()
}
fn observed(apps: &[MeetingApp]) -> MeetingAppSnapshot {
    MeetingAppSnapshot {
        complete: true,
        apps: apps.to_vec(),
        error: None,
    }
}
fn state(temp: &TempDir) -> DetectionState {
    store::initialize(temp.path()).unwrap();
    preferences::load(temp.path()).unwrap();
    let mut state = DetectionState::load(temp.path());
    state.settings.enabled = true;
    state.ready = true;
    state.setup_open = false;
    state
}
fn always(app: &MeetingApp) -> DetectionRule {
    DetectionRule {
        app_id: app.id.clone(),
        policy: DetectionPolicy::AlwaysRecord,
        participant_notice_attested: true,
        input: Some(DetectionInput::Default),
    }
}
fn absence(state: &mut DetectionState, start: u64, end: u64, current: Option<&str>) {
    for second in start..=end {
        state.observe(observed(&[]), second * SECOND, current, current.is_some());
    }
}
#[test]
fn prompt_wording_freshness_and_dismissed_episode_rearm_require_confirmed_absence() {
    let temp = TempDir::new().unwrap();
    let mut state = state(&temp);
    let meeting = app(MeetingAppKind::Meeting);
    let browser = app(MeetingAppKind::Browser);
    state.observe(
        observed(&[meeting.clone(), browser.clone()]),
        0,
        None,
        false,
    );
    let snapshot = state.snapshot(0, false, None, false);
    assert_eq!(snapshot.prompts.len(), 2);
    assert!(
        snapshot
            .prompts
            .iter()
            .find(|prompt| prompt.app.id == browser.id)
            .unwrap()
            .message
            .contains("is using the microphone")
    );
    assert!(
        snapshot
            .prompts
            .iter()
            .find(|prompt| prompt.app.id == meeting.id)
            .unwrap()
            .message
            .contains("Meeting detected")
    );
    let original = state.episodes[&meeting.id].id.clone();
    state.episodes.get_mut(&meeting.id).unwrap().dismissed = true;
    assert!(
        state
            .snapshot(7 * SECOND, false, None, false)
            .prompts
            .is_empty()
    );
    absence(&mut state, 8, 68, None); // exactly60 seconds does not end/rearm
    assert_eq!(state.episodes[&meeting.id].id, original);
    state.observe(
        MeetingAppSnapshot {
            complete: false,
            apps: vec![],
            error: None,
        },
        69 * SECOND,
        None,
        false,
    );
    absence(&mut state, 70, 100, None);
    state.observe(
        observed(std::slice::from_ref(&meeting)),
        101 * SECOND,
        None,
        false,
    );
    assert_eq!(state.episodes[&meeting.id].id, original);
    assert!(state.episodes[&meeting.id].dismissed);
    absence(&mut state, 102, 163, None);
    state.observe(
        observed(std::slice::from_ref(&meeting)),
        164 * SECOND,
        None,
        false,
    );
    assert_ne!(state.episodes[&meeting.id].id, original);
    assert!(!state.episodes[&meeting.id].dismissed);
}
#[test]
fn ended_reminder_binds_only_detected_recording_and_unknown_or_stale_breaks_release_timer() {
    let temp = TempDir::new().unwrap();
    let mut state = state(&temp);
    let meeting = app(MeetingAppKind::Meeting);
    state.observe(observed(std::slice::from_ref(&meeting)), 0, None, false);
    let episode = state.episodes[&meeting.id].id.clone();
    state.started(
        "recorded",
        CaptureMode::Remote,
        Some(&(meeting.clone(), episode)),
        false,
    );
    absence(&mut state, 1, 61, Some("recorded"));
    assert!(
        state
            .snapshot(61 * SECOND, true, Some("recorded"), false)
            .ended
            .is_none()
    );
    state.observe(
        MeetingAppSnapshot {
            complete: false,
            apps: vec![],
            error: Some("unverified".into()),
        },
        62 * SECOND,
        Some("recorded"),
        true,
    );
    absence(&mut state, 63, 123, Some("recorded"));
    assert!(
        state
            .snapshot(123 * SECOND, true, Some("recorded"), false)
            .ended
            .is_none()
    );
    state.observe(observed(&[]), 124 * SECOND, Some("recorded"), true);
    assert!(
        state
            .snapshot(124 * SECOND, true, Some("recorded"), false)
            .ended
            .is_some()
    );
    assert!(
        state
            .snapshot(131 * SECOND, true, Some("recorded"), false)
            .ended
            .is_none()
    );
    state.observe(
        observed(&[]),
        140 * SECOND,
        Some("different-manual-recording"),
        true,
    );
    assert!(state.association.is_none());
    state.started("manual", CaptureMode::Remote, None, false);
    absence(&mut state, 141, 205, Some("manual"));
    assert!(
        state
            .snapshot(205 * SECOND, true, Some("manual"), false)
            .ended
            .is_none()
    );
}
#[test]
fn always_record_requires_explicit_policy_and_future_idle_edges_not_queued_active_apps() {
    let temp = TempDir::new().unwrap();
    let mut state = state(&temp);
    let apps = vt_capture::known_meeting_apps()
        .iter()
        .filter(|app| matches!(&app.kind, MeetingAppKind::Meeting))
        .take(2)
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(apps.len(), 2);
    state.settings.rules = apps.iter().map(always).collect();
    state.observe(observed(&apps), 0, None, false);
    assert!(state.auto_candidate(0, false).is_none()); // already-active baseline
    absence(&mut state, 1, 62, None);
    state.observe(observed(&apps), 63 * SECOND, None, false);
    assert!(state.auto_candidate(63 * SECOND, false).is_some());
    let episode = state.episodes[&apps[0].id].id.clone();
    state.started(
        "meeting",
        CaptureMode::Remote,
        Some(&(apps[0].clone(), episode)),
        false,
    );
    assert!(state.auto_candidate(63 * SECOND, false).is_none()); // Stop immediately must not auto-start the other app
    absence(&mut state, 64, 125, Some("meeting"));
    state.observe(observed(&apps), 126 * SECOND, Some("meeting"), true);
    assert!(state.auto_candidate(126 * SECOND, false).is_none()); // source resumed during recording
    state.observe(observed(&apps), 127 * SECOND, None, false);
    assert!(state.auto_candidate(127 * SECOND, false).is_none());
    absence(&mut state, 128, 189, None);
    state.observe(observed(&apps), 190 * SECOND, None, false);
    assert!(state.auto_candidate(190 * SECOND, false).is_some());
    state.setup_open = true;
    assert!(state.auto_candidate(190 * SECOND, false).is_none());
    state.mode = CaptureMode::InPerson;
    assert!(
        state
            .snapshot(190 * SECOND, false, None, false)
            .prompts
            .is_empty()
    );
    let mut invalid = always(&apps[0]);
    invalid.participant_notice_attested = false;
    assert!(
        validate_settings(DetectionSettings {
            enabled: true,
            rules: vec![invalid]
        })
        .is_err()
    );
    let mut invalid = always(&apps[0]);
    invalid.input = None;
    assert!(
        validate_settings(DetectionSettings {
            enabled: true,
            rules: vec![invalid]
        })
        .is_err()
    );
}
#[test]
fn notifications_are_episode_deduped_policy_gated_and_never_confirm_recording_or_people() {
    let temp = TempDir::new().unwrap();
    let mut state = state(&temp);
    let meeting = app(MeetingAppKind::Meeting);
    state.setup_open = true;
    state.observe(observed(std::slice::from_ref(&meeting)), 0, None, false);
    assert!(state.notifications(0, false, None, None).is_empty());
    state.setup_open = false;
    assert_eq!(state.notifications(0, false, None, None).len(), 1);
    assert!(state.notifications(0, false, None, None).is_empty());
    state.settings.rules = vec![DetectionRule {
        app_id: meeting.id.clone(),
        policy: DetectionPolicy::Suppress,
        participant_notice_attested: false,
        input: None,
    }];
    assert!(state.snapshot(0, false, None, false).prompts.is_empty());
    state.settings.rules.clear();
    let episode = state.episodes[&meeting.id].id.clone();
    state.started(
        "recorded",
        CaptureMode::Remote,
        Some(&(meeting, episode)),
        false,
    );
    absence(&mut state, 1, 62, Some("recorded"));
    let notifications = state.notifications(62 * SECOND, true, Some("recorded"), None);
    assert_eq!(notifications.len(), 1);
    assert!(notifications[0].1.contains("Recording continues"));
    assert!(
        state
            .notifications(62 * SECOND, true, Some("recorded"), None)
            .is_empty()
    );
    state.association.as_mut().unwrap().dismissed = true;
    assert!(
        state
            .snapshot(62 * SECOND, true, Some("recorded"), false)
            .ended
            .is_none()
    );
}
struct Resources(AtomicU64);
impl health::ResourceProbe for Resources {
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
    let deadline = Instant::now() + Duration::from_secs(12);
    while !condition() {
        assert!(
            Instant::now() < deadline,
            "synthetic detection fixture timed out"
        );
        thread::sleep(Duration::from_millis(5));
    }
}
#[cfg(unix)]
fn helper(temp: &TempDir) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = temp.path().join("fake-helper");
    let canonical = serde_json::to_string(&app(MeetingAppKind::Meeting)).unwrap();
    fs::write(&path,format!(r#"#!/usr/bin/env python3
import json,pathlib,select,sys,time
folder=pathlib.Path(__file__).parent
args=sys.argv[1:]
if args==['--list-devices']:
    (folder/'probed').open('a').write('readonly\n')
    while (folder/'hold-inventory').exists() and not (folder/'release-inventory').exists(): time.sleep(.005)
    print(json.dumps({{'devices':[{{'id':'synthetic-input','name':'Synthetic input','is_default':True,'bluetooth':(folder/'bluetooth').exists()}}],'system_supported':True,'helper_protocol':4,'supported_commands':['list_devices','meeting_apps']}}))
    sys.exit(0)
if args==['--meeting-apps']:
    print(json.dumps({{'complete':True,'apps':[{canonical}],'error':None}}))
    sys.exit(0)
assert args[:2]==['--mode','remote']
(folder/'capture-started').write_text(json.dumps(args))
def emit(value): print(json.dumps(value),flush=True)
if (folder/'fatal').exists():
    emit({{'type':'error','message':'Synthetic capture permission failure','fatal':True}})
    sys.exit(0)
if (folder/'wait-no-audio').exists():
    while True:
        if select.select([sys.stdin],[],[],0)[0] and sys.stdin.readline().strip()=='stop': sys.exit(0)
        time.sleep(.005)
for track in ['mic','system']: emit({{'type':'format','track':track,'sample_rate':16000,'channels':1,'epoch':0,'start_ns':0}})
for first in range(0,35200,320):
    for track in ['mic','system']: emit({{'type':'audio','track':track,'epoch':0,'first_frame':first,'host_time_ns':first*1000000000//16000,'samples':[0.25]*320}})
(folder/'fed').write_text('35200')
while True:
    if select.select([sys.stdin],[],[],0)[0] and sys.stdin.readline().strip()=='stop': break
    time.sleep(.005)
"#)).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    path
}
fn service(temp: &TempDir, helper: PathBuf) -> (Arc<AppService>, Arc<Resources>) {
    let clock = Arc::new(Resources(AtomicU64::new(0)));
    let service =
        AppService::with_resources(temp.path().join("data"), helper, clock.clone()).unwrap();
    (service, clock)
}
struct Shutdown(Arc<AppService>);
impl Drop for Shutdown {
    fn drop(&mut self) {
        self.0.wait_for_shutdown();
    }
}
#[cfg(unix)]
#[test]
fn default_off_worker_never_probes_and_restart_preserves_suppression_without_activity_history() {
    let temp = TempDir::new().unwrap();
    let (service, _) = self::service(&temp, helper(&temp));
    let _stop = Shutdown(service.clone());
    service.start_meeting_detection().unwrap();
    thread::sleep(Duration::from_millis(100));
    assert!(!temp.path().join("probed").exists());
    assert!(!temp.path().join("capture-started").exists());
    service
        .update_meeting_detection(DetectionSettings {
            enabled: true,
            rules: vec![],
        })
        .unwrap();
    service
        .set_detection_context(CaptureMode::Remote, false)
        .unwrap();
    service.apply_detection(observed(&[app(MeetingAppKind::Meeting)]));
    let episode = service.snapshot().unwrap().meeting_detection.prompts[0]
        .episode_id
        .clone();
    service.dismiss_detection(&episode, true).unwrap();
    let restored = DetectionState::load(&service.root);
    assert_eq!(restored.settings.rules[0].policy, DetectionPolicy::Suppress);
    assert!(restored.observation.is_none() && restored.episodes.is_empty() && !restored.ready);
    let keys = store::open(&service.root)
        .unwrap()
        .prepare("SELECT key FROM app_settings WHERE key LIKE '%detection%'")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(keys, vec![SETTINGS_KEY]);
}
#[cfg(unix)]
#[test]
fn fresh_prompt_setup_handoff_preserves_consent_freshness_and_atomic_source_provenance() {
    let temp = TempDir::new().unwrap();
    let (service, clock) = self::service(&temp, helper(&temp));
    let _stop = Shutdown(service.clone());
    service
        .update_meeting_detection(DetectionSettings {
            enabled: true,
            rules: vec![],
        })
        .unwrap();
    service
        .set_detection_context(CaptureMode::Remote, false)
        .unwrap();
    let observed_app = app(MeetingAppKind::Meeting);
    service.apply_detection(observed(std::slice::from_ref(&observed_app)));
    let episode = service.snapshot().unwrap().meeting_detection.prompts[0]
        .episode_id
        .clone();
    let opened = service
        .set_detection_context(CaptureMode::Remote, true)
        .unwrap();
    assert_eq!(opened.meeting_detection.prompts[0].episode_id, episode);
    assert!(
        service
            .start_detected_recording(&episode, "Synthetic detected meeting".into(), false, None)
            .is_err()
    );
    assert!(!temp.path().join("capture-started").exists());
    clock.0.store(7 * SECOND, Ordering::Relaxed);
    assert!(
        service
            .start_detected_recording(&episode, String::new(), true, None)
            .is_err()
    );
    assert!(!temp.path().join("capture-started").exists());
    service.apply_detection(observed(std::slice::from_ref(&observed_app)));
    let started = service
        .start_detected_recording(&episode, "Synthetic detected meeting".into(), true, None)
        .unwrap();
    let id = started.recording.unwrap().id;
    wait(|| temp.path().join("fed").exists());
    service.stop().unwrap();
    wait(|| {
        store::detail(&service.root, &id)
            .unwrap()
            .runs
            .iter()
            .all(|run| run.status != "running")
    });
    let detail = store::detail(&service.root, &id).unwrap();
    let engines: serde_json::Value = serde_json::from_str(&detail.runs[0].engines_json).unwrap();
    assert_eq!(engines["recording_trigger"], "detected_prompt");
    assert_eq!(
        engines["detected_source_app"],
        serde_json::to_value(observed_app).unwrap()
    );
    let mut reader = vt_playback::SavedAudioReader::open(&service.root, &id).unwrap();
    let mut frames = [0; 2];
    while let Some(packet) = reader.next_packet().unwrap() {
        let vt_playback::SavedAudioPacket::Audio { track, samples, .. } = packet else {
            panic!("detection must not invent raw gaps")
        };
        assert!(samples.iter().all(|sample| *sample == 0.25));
        frames[usize::from(track == TrackKind::System)] += samples.len();
    }
    assert_eq!(frames, [35200; 2]);
}
#[cfg(unix)]
#[test]
fn always_record_uses_saved_input_only_after_idle_edge_not_during_setup_and_notifies_outside_locks()
{
    let temp = TempDir::new().unwrap();
    let (service, clock) = self::service(&temp, helper(&temp));
    let _stop = Shutdown(service.clone());
    let observed_app = app(MeetingAppKind::Meeting);
    service
        .update_meeting_detection(DetectionSettings {
            enabled: true,
            rules: vec![always(&observed_app)],
        })
        .unwrap();
    service
        .set_detection_context(CaptureMode::Remote, false)
        .unwrap();
    let notifications = Arc::new(Mutex::new(Vec::new()));
    let received = notifications.clone();
    let weak = Arc::downgrade(&service);
    service.set_detection_notifier(Arc::new(move |title, body| {
        weak.upgrade().unwrap().snapshot().unwrap();
        received
            .lock()
            .unwrap()
            .push((title.to_owned(), body.to_owned()));
    }));
    service.apply_detection(observed(std::slice::from_ref(&observed_app)));
    assert!(!temp.path().join("capture-started").exists()); // no verified idle edge
    for second in 1..=62 {
        clock.0.store(second * SECOND, Ordering::Relaxed);
        service.apply_detection(observed(&[]));
    }
    service
        .set_detection_context(CaptureMode::Remote, true)
        .unwrap();
    service.apply_detection(observed(std::slice::from_ref(&observed_app)));
    assert!(!temp.path().join("capture-started").exists());
    service
        .set_detection_context(CaptureMode::Remote, false)
        .unwrap();
    for second in 63..=124 {
        clock.0.store(second * SECOND, Ordering::Relaxed);
        service.apply_detection(observed(&[]));
    }
    clock.0.store(125 * SECOND, Ordering::Relaxed);
    service.apply_detection(observed(std::slice::from_ref(&observed_app)));
    assert!(
        !notifications
            .lock()
            .unwrap()
            .iter()
            .any(|(_, body)| body.contains("Recording started"))
    );
    wait(|| temp.path().join("fed").exists());
    wait(|| {
        let current = service.snapshot().unwrap().recording.unwrap();
        store::open(&service.root)
            .unwrap()
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM audio_chunks WHERE meeting_id=?1 AND state='ready')",
                [current.id],
                |row| row.get::<_, bool>(0),
            )
            .unwrap()
    });
    service.apply_detection(observed(std::slice::from_ref(&observed_app)));
    let snapshot = service.snapshot().unwrap();
    let id = snapshot.recording.unwrap().id;
    let detail = store::detail(&service.root, &id).unwrap();
    let engines: serde_json::Value = serde_json::from_str(&detail.runs[0].engines_json).unwrap();
    assert_eq!(engines["recording_trigger"], "always_record");
    assert_eq!(engines["participant_notice_attested"], true);
    assert_eq!(
        notifications
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, body)| body.contains("Recording started"))
            .count(),
        1
    );
    assert!(
        !fs::read_to_string(temp.path().join("capture-started"))
            .unwrap()
            .contains("--input-device")
    );
    service.stop().unwrap();
    wait(|| {
        store::detail(&service.root, &id)
            .unwrap()
            .runs
            .iter()
            .all(|run| run.status != "running")
    });
    service.apply_detection(observed(std::slice::from_ref(&observed_app)));
    assert_eq!(store::meetings(&service.root).unwrap().len(), 1);
}
#[test]
fn corrupted_mode_stays_disabled_until_explicit_configuration_repairs_restart() {
    let temp = TempDir::new().unwrap();
    let (service, _) = service(&temp, PathBuf::from("missing-helper"));
    let _stop = Shutdown(service.clone());
    save_value(&service.root, MODE_KEY, &"unrecognized-mode").unwrap();
    let restored = DetectionState::load(&service.root);
    assert!(!restored.settings.enabled && restored.error.is_some());
    service
        .update_meeting_detection(DetectionSettings {
            enabled: true,
            rules: vec![],
        })
        .unwrap();
    let restored = DetectionState::load(&service.root);
    assert!(
        restored.settings.enabled
            && restored.mode == CaptureMode::Remote
            && restored.error.is_none()
    );
    service.reset_detection_context();
    let snapshot = service.snapshot().unwrap();
    assert!(matches!(
        snapshot.meeting_detection.status,
        DetectionStatus::Blocked
    ));
}
#[cfg(unix)]
#[test]
fn every_backend_work_gate_and_in_person_mode_reject_detected_start_without_raw_creation() {
    let temp = TempDir::new().unwrap();
    let (service, _) = self::service(&temp, helper(&temp));
    let _stop = Shutdown(service.clone());
    service
        .update_meeting_detection(DetectionSettings {
            enabled: true,
            rules: vec![],
        })
        .unwrap();
    service
        .set_detection_context(CaptureMode::Remote, false)
        .unwrap();
    let observed_app = app(MeetingAppKind::Meeting);
    service.apply_detection(observed(std::slice::from_ref(&observed_app)));
    let episode = service.snapshot().unwrap().meeting_detection.prompts[0]
        .episode_id
        .clone();
    for gate in 0..6 {
        {
            let mut runtime = service.runtime.lock().unwrap();
            match gate {
                0 => runtime.processing = true,
                1 => runtime.model.busy = true,
                2 => runtime.refinement_model.busy = true,
                3 => runtime.vad_model.busy = true,
                4 => runtime.diarization_model.busy = true,
                _ => runtime.saved_audio_readers = 1,
            }
        }
        assert!(
            service
                .start_detected_recording(&episode, String::new(), true, None)
                .is_err()
        );
        {
            let mut runtime = service.runtime.lock().unwrap();
            runtime.processing = false;
            runtime.model.busy = false;
            runtime.refinement_model.busy = false;
            runtime.vad_model.busy = false;
            runtime.diarization_model.busy = false;
            runtime.saved_audio_readers = 0;
        }
    }
    service
        .set_detection_context(CaptureMode::InPerson, false)
        .unwrap();
    service.apply_detection(observed(std::slice::from_ref(&observed_app)));
    assert!(
        service
            .snapshot()
            .unwrap()
            .meeting_detection
            .prompts
            .is_empty()
    );
    assert!(
        service
            .start_detected_recording(&episode, String::new(), true, None)
            .is_err()
    );
    assert!(store::meetings(&service.root).unwrap().is_empty());
    assert!(!temp.path().join("capture-started").exists());
}

#[test]
fn always_record_rearms_after_short_confirmed_idle_edge_but_never_continuous_activity_after_stop() {
    let temp = TempDir::new().unwrap();
    let mut state = state(&temp);
    let meeting = app(MeetingAppKind::Meeting);
    state.settings.rules = vec![always(&meeting)];
    state.observe(observed(&[]), 0, None, false);
    state.observe(
        observed(std::slice::from_ref(&meeting)),
        SECOND,
        None,
        false,
    );
    let episode = state.auto_candidate(SECOND, false).unwrap();
    state.started(
        "recording",
        CaptureMode::Remote,
        Some(&(meeting.clone(), episode.clone())),
        false,
    );
    state.observe(
        observed(std::slice::from_ref(&meeting)),
        2 * SECOND,
        None,
        false,
    );
    assert!(state.auto_candidate(2 * SECOND, false).is_none());
    state.observe(observed(&[]), 3 * SECOND, None, false);
    state.observe(
        observed(std::slice::from_ref(&meeting)),
        5 * SECOND,
        None,
        false,
    );
    let next = state.auto_candidate(5 * SECOND, false).unwrap();
    assert_ne!(next, episode);
}
#[cfg(unix)]
#[test]
fn global_off_normalizes_unarmed_rules_without_discovery_and_context_toggles_do_not_write_mode() {
    let temp = TempDir::new().unwrap();
    let (service, _) = self::service(&temp, helper(&temp));
    let _stop = Shutdown(service.clone());
    let meeting = app(MeetingAppKind::Meeting);
    let saved = service
        .update_meeting_detection(DetectionSettings {
            enabled: false,
            rules: vec![DetectionRule {
                app_id: meeting.id,
                policy: DetectionPolicy::AlwaysRecord,
                participant_notice_attested: false,
                input: None,
            }],
        })
        .unwrap();
    assert_eq!(
        saved.settings.meeting_detection.rules[0].policy,
        DetectionPolicy::Prompt
    );
    assert!(!temp.path().join("probed").exists());
    let loaded = DetectionState::load(&service.root);
    assert!(!loaded.settings.enabled);
    assert_eq!(loaded.settings.rules[0].policy, DetectionPolicy::Prompt);
    store::open(&service.root).unwrap().execute_batch("CREATE TRIGGER reject_mode BEFORE UPDATE ON app_settings WHEN NEW.key='capture_mode' BEGIN SELECT RAISE(ABORT,'unexpected context mode write'); END;").unwrap();
    service
        .set_detection_context(CaptureMode::Remote, true)
        .unwrap();
    service
        .set_detection_context(CaptureMode::Remote, false)
        .unwrap();
    assert!(
        service
            .set_detection_context(CaptureMode::InPerson, false)
            .is_err()
    );
}
#[cfg(unix)]
#[test]
fn automatic_input_rejection_creates_no_raw_meeting_and_consumes_retry_for_that_episode() {
    for missing in [false, true] {
        let temp = TempDir::new().unwrap();
        let (service, clock) = self::service(&temp, helper(&temp));
        let _stop = Shutdown(service.clone());
        let meeting = app(MeetingAppKind::Meeting);
        let mut rule = always(&meeting);
        if missing {
            rule.input = Some(DetectionInput::Device {
                id: "unavailable-synthetic-device".into(),
            });
        } else {
            fs::write(temp.path().join("bluetooth"), "true").unwrap();
        }
        service
            .update_meeting_detection(DetectionSettings {
                enabled: true,
                rules: vec![rule],
            })
            .unwrap();
        service
            .set_detection_context(CaptureMode::Remote, false)
            .unwrap();
        service.apply_detection(observed(&[]));
        clock.0.store(SECOND, Ordering::Relaxed);
        service.apply_detection(observed(std::slice::from_ref(&meeting)));
        let snapshot = service.snapshot().unwrap();
        assert!(snapshot.meeting_detection.error.is_some());
        assert_eq!(snapshot.meeting_detection.prompts.len(), 1);
        assert!(snapshot.meetings.is_empty());
        assert!(!temp.path().join("capture-started").exists());
        clock.0.store(2 * SECOND, Ordering::Relaxed);
        service.apply_detection(observed(std::slice::from_ref(&meeting)));
        assert!(store::meetings(&service.root).unwrap().is_empty());
    }
}
#[cfg(unix)]
#[test]
fn automatic_async_failure_notifies_once_without_started_notice_but_expected_stop_is_silent() {
    for expected_stop in [false, true] {
        let temp = TempDir::new().unwrap();
        fs::write(
            temp.path().join(if expected_stop {
                "wait-no-audio"
            } else {
                "fatal"
            }),
            "true",
        )
        .unwrap();
        let (service, clock) = self::service(&temp, helper(&temp));
        let _stop = Shutdown(service.clone());
        let meeting = app(MeetingAppKind::Meeting);
        let notifications = Arc::new(Mutex::new(Vec::new()));
        let received = notifications.clone();
        service.set_detection_notifier(Arc::new(move |title, body| {
            received
                .lock()
                .unwrap()
                .push((title.to_owned(), body.to_owned()))
        }));
        service
            .update_meeting_detection(DetectionSettings {
                enabled: true,
                rules: vec![always(&meeting)],
            })
            .unwrap();
        service
            .set_detection_context(CaptureMode::Remote, false)
            .unwrap();
        service.apply_detection(observed(&[]));
        clock.0.store(SECOND, Ordering::Relaxed);
        service.apply_detection(observed(std::slice::from_ref(&meeting)));
        wait(|| temp.path().join("capture-started").exists());
        if expected_stop {
            service.stop().unwrap();
        }
        wait(|| {
            store::meetings(&service.root)
                .unwrap()
                .first()
                .is_some_and(|meeting| meeting.status == "failed")
        });
        for second in 2..=4 {
            clock.0.store(second * SECOND, Ordering::Relaxed);
            service.apply_detection(observed(std::slice::from_ref(&meeting)));
        }
        let notices = notifications.lock().unwrap();
        assert!(
            !notices
                .iter()
                .any(|(_, body)| body.contains("Recording started"))
        );
        if expected_stop {
            assert!(notices.is_empty());
        } else {
            assert_eq!(notices.len(), 1);
            assert!(notices[0].1.contains("Automatic recording did not start"));
            assert!(
                service
                    .snapshot()
                    .unwrap()
                    .meeting_detection
                    .error
                    .is_some()
            );
        }
        assert_eq!(store::meetings(&service.root).unwrap().len(), 1);
    }
}

#[cfg(unix)]
#[test]
fn native_inventory_preflight_is_unlocked_and_rule_dismiss_or_context_changes_cancel_authority() {
    for change in 0..5 {
        let temp = TempDir::new().unwrap();
        fs::write(temp.path().join("hold-inventory"), "hold").unwrap();
        let (service, clock) = self::service(&temp, helper(&temp));
        let _stop = Shutdown(service.clone());
        let notices = Arc::new(Mutex::new(Vec::new()));
        let received = notices.clone();
        service.set_detection_notifier(Arc::new(move |_, body| {
            received.lock().unwrap().push(body.to_owned())
        }));
        let meeting = app(MeetingAppKind::Meeting);
        service
            .update_meeting_detection(DetectionSettings {
                enabled: true,
                rules: vec![always(&meeting)],
            })
            .unwrap();
        service
            .set_detection_context(CaptureMode::Remote, false)
            .unwrap();
        service.apply_detection(observed(&[]));
        clock.0.store(SECOND, Ordering::Relaxed);
        let observed_app = meeting.clone();
        let worker_service = service.clone();
        let worker =
            thread::spawn(move || worker_service.apply_detection(observed(&[observed_app])));
        wait(|| temp.path().join("probed").exists());
        let began = Instant::now();
        service.snapshot().unwrap();
        assert!(began.elapsed() < Duration::from_secs(1));
        match change {
            0 => {
                let mut rule = always(&meeting);
                rule.input = Some(DetectionInput::Device {
                    id: "changed-synthetic-input".into(),
                });
                service
                    .update_meeting_detection(DetectionSettings {
                        enabled: true,
                        rules: vec![rule],
                    })
                    .unwrap();
            }
            1 => {
                let episode = service.meeting_detection.lock().unwrap().episodes[&meeting.id]
                    .id
                    .clone();
                service.dismiss_detection(&episode, false).unwrap();
            }
            2 => {
                service
                    .set_detection_context(CaptureMode::Remote, true)
                    .unwrap();
            }
            3 => {
                service
                    .set_detection_context(CaptureMode::InPerson, false)
                    .unwrap();
            }
            _ => service.reset_detection_context(),
        }
        fs::write(temp.path().join("release-inventory"), "release").unwrap();
        worker.join().unwrap();
        assert!(store::meetings(&service.root).unwrap().is_empty());
        assert!(!temp.path().join("capture-started").exists());
        assert!(
            service
                .snapshot()
                .unwrap()
                .meeting_detection
                .error
                .as_deref()
                .is_none_or(|error| !error.contains("Automatic recording did not start"))
        );
        assert!(
            !notices
                .lock()
                .unwrap()
                .iter()
                .any(|body| body.contains("Recording started"))
        );
    }
}
#[cfg(unix)]
#[test]
fn enabled_worker_polls_only_readonly_snapshots_and_shutdown_bounds_an_inflight_probe() {
    let temp = TempDir::new().unwrap();
    let (service, _) = self::service(&temp, helper(&temp));
    let _stop = Shutdown(service.clone());
    service
        .update_meeting_detection(DetectionSettings {
            enabled: true,
            rules: vec![],
        })
        .unwrap();
    service
        .set_detection_context(CaptureMode::Remote, false)
        .unwrap();
    service.start_meeting_detection().unwrap();
    wait(|| service.snapshot().unwrap().meeting_detection.prompts.len() == 1);
    wait(|| {
        fs::read_to_string(temp.path().join("probed")).is_ok_and(|text| text.lines().count() >= 2)
    });
    assert!(!temp.path().join("capture-started").exists());
    service.shutdown();
    service.join_detection();
    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join("hold-inventory"), "hold").unwrap();
    let (service, _) = self::service(&temp, helper(&temp));
    let _stop = Shutdown(service.clone());
    service
        .update_meeting_detection(DetectionSettings {
            enabled: true,
            rules: vec![],
        })
        .unwrap();
    service
        .set_detection_context(CaptureMode::Remote, false)
        .unwrap();
    service.start_meeting_detection().unwrap();
    wait(|| temp.path().join("probed").exists());
    let began = Instant::now();
    service.shutdown();
    service.join_detection();
    assert!(began.elapsed() < Duration::from_secs(5));
    assert!(!temp.path().join("capture-started").exists());
}

struct SuspendClock {
    tick: AtomicU64,
    observation: Mutex<Option<u64>>,
}
impl health::ResourceProbe for SuspendClock {
    fn free_bytes(&self, _: &Path) -> Result<u64> {
        Ok(4_000_000_000)
    }
    fn uptime_ns(&self) -> Option<u64> {
        None
    }
    fn tick_ns(&self) -> u64 {
        self.tick.load(Ordering::Relaxed)
    }
    fn observation_ns(&self) -> Option<u64> {
        *self.observation.lock().unwrap()
    }
}
#[cfg(unix)]
#[test]
fn suspend_aware_jump_or_unavailable_clock_rejects_old_prompt_without_moving_capture_tick() {
    let temp = TempDir::new().unwrap();
    let clock = Arc::new(SuspendClock {
        tick: AtomicU64::new(0),
        observation: Mutex::new(Some(0)),
    });
    let service =
        AppService::with_resources(temp.path().join("data"), helper(&temp), clock.clone()).unwrap();
    let _stop = Shutdown(service.clone());
    service
        .update_meeting_detection(DetectionSettings {
            enabled: true,
            rules: vec![],
        })
        .unwrap();
    service
        .set_detection_context(CaptureMode::Remote, false)
        .unwrap();
    let meeting = app(MeetingAppKind::Meeting);
    service.apply_detection(observed(std::slice::from_ref(&meeting)));
    let episode = service.snapshot().unwrap().meeting_detection.prompts[0]
        .episode_id
        .clone();
    *clock.observation.lock().unwrap() = Some(3600 * SECOND);
    assert_eq!(clock.tick_ns(), 0);
    assert!(
        service
            .snapshot()
            .unwrap()
            .meeting_detection
            .prompts
            .is_empty()
    );
    assert!(
        service
            .start_detected_recording(&episode, String::new(), true, None)
            .is_err()
    );
    *clock.observation.lock().unwrap() = None;
    assert!(
        service
            .snapshot()
            .unwrap()
            .meeting_detection
            .error
            .unwrap()
            .contains("clock")
    );
    assert!(
        service
            .start_detected_recording(&episode, String::new(), true, None)
            .is_err()
    );
    assert!(
        service
            .meeting_detection
            .lock()
            .unwrap()
            .observation
            .is_none()
    );
    *clock.observation.lock().unwrap() = Some(3601 * SECOND);
    service.apply_detection(observed(std::slice::from_ref(&meeting)));
    assert_eq!(
        service.snapshot().unwrap().meeting_detection.prompts.len(),
        1
    );
    assert!(store::meetings(&service.root).unwrap().is_empty());
    assert!(!temp.path().join("capture-started").exists());
}
#[cfg(unix)]
#[test]
fn native_probe_receipt_after_suspend_cannot_refresh_old_app_evidence_or_arm_auto() {
    let temp = TempDir::new().unwrap();
    let clock = Arc::new(SuspendClock {
        tick: AtomicU64::new(0),
        observation: Mutex::new(Some(3600 * SECOND)),
    });
    let service =
        AppService::with_resources(temp.path().join("data"), helper(&temp), clock.clone()).unwrap();
    let _stop = Shutdown(service.clone());
    let meeting = app(MeetingAppKind::Meeting);
    service
        .update_meeting_detection(DetectionSettings {
            enabled: true,
            rules: vec![always(&meeting)],
        })
        .unwrap();
    service
        .set_detection_context(CaptureMode::Remote, false)
        .unwrap();
    service.apply_detection_at(observed(std::slice::from_ref(&meeting)), Some(0));
    let snapshot = service.snapshot().unwrap();
    assert!(snapshot.meeting_detection.prompts.is_empty());
    assert!(
        snapshot
            .meeting_detection
            .error
            .unwrap()
            .contains("expired")
    );
    assert!(!temp.path().join("capture-started").exists());
    service.apply_detection(observed(std::slice::from_ref(&meeting)));
    assert!(store::meetings(&service.root).unwrap().is_empty());
    assert!(
        service
            .meeting_detection
            .lock()
            .unwrap()
            .auto_candidate(3600 * SECOND, false)
            .is_none()
    );
    // Arming still requires a fresh verified idle edge after the uncertainty.
    let mut state = service.meeting_detection.lock().unwrap();
    state.observe(observed(&[]), 3601 * SECOND, None, false);
    state.observe(
        observed(std::slice::from_ref(&meeting)),
        3602 * SECOND,
        None,
        false,
    );
    assert!(state.auto_candidate(3602 * SECOND, false).is_some());
}
#[cfg(unix)]
#[test]
fn suspension_during_input_inventory_invalidates_its_proof_even_with_unchanged_capture_tick() {
    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join("hold-inventory"), "hold").unwrap();
    let clock = Arc::new(SuspendClock {
        tick: AtomicU64::new(0),
        observation: Mutex::new(Some(0)),
    });
    let service =
        AppService::with_resources(temp.path().join("data"), helper(&temp), clock.clone()).unwrap();
    let _stop = Shutdown(service.clone());
    let meeting = app(MeetingAppKind::Meeting);
    service
        .update_meeting_detection(DetectionSettings {
            enabled: true,
            rules: vec![always(&meeting)],
        })
        .unwrap();
    service
        .set_detection_context(CaptureMode::Remote, false)
        .unwrap();
    service.apply_detection(observed(&[]));
    *clock.observation.lock().unwrap() = Some(SECOND);
    let worker_service = service.clone();
    let worker = thread::spawn(move || worker_service.apply_detection(observed(&[meeting])));
    wait(|| temp.path().join("probed").exists());
    *clock.observation.lock().unwrap() = Some(3600 * SECOND);
    fs::write(temp.path().join("release-inventory"), "release").unwrap();
    worker.join().unwrap();
    assert_eq!(clock.tick_ns(), 0);
    assert!(store::meetings(&service.root).unwrap().is_empty());
    assert!(!temp.path().join("capture-started").exists());
    assert!(
        service
            .snapshot()
            .unwrap()
            .meeting_detection
            .prompts
            .is_empty()
    );
}

#[test]
fn malformed_detection_rules_preserve_valid_in_person_mode_while_disabling_automatic_policy() {
    let temp = TempDir::new().unwrap();
    let state = state(&temp);
    assert!(state.settings.enabled);
    save_value(temp.path(), MODE_KEY, &CaptureMode::InPerson).unwrap();
    save_value(temp.path(), SETTINGS_KEY, &"malformed-rules").unwrap();
    let restored = DetectionState::load(temp.path());
    assert_eq!(restored.mode, CaptureMode::InPerson);
    assert!(!restored.settings.enabled);
    assert!(restored.error.is_some());
    assert!(!restored.mode_needs_repair);
}

#[test]
fn queued_old_page_context_cannot_unblock_reload_or_release_fresh_settings_setup() {
    let temp = TempDir::new().unwrap();
    let (service, _) = self::service(&temp, PathBuf::from("never-start-helper"));
    let _stop = Shutdown(service.clone());
    service
        .update_meeting_detection(DetectionSettings {
            enabled: true,
            rules: vec![],
        })
        .unwrap();
    let old_generation = service
        .snapshot()
        .unwrap()
        .meeting_detection
        .context_generation;
    service
        .set_detection_context_for_generation(CaptureMode::Remote, false, old_generation)
        .unwrap();
    // This request has already captured the old WebView's authority but has
    // not entered the backend, like a queued spawn_blocking IPC task.
    let (release, waiting) = mpsc::channel();
    let old_page = service.clone();
    let queued = thread::spawn(move || {
        waiting.recv().unwrap();
        old_page.set_detection_context_for_generation(CaptureMode::Remote, false, old_generation)
    });
    service.reset_detection_context();
    let reloaded = service.snapshot().unwrap();
    let fresh_generation = reloaded.meeting_detection.context_generation;
    assert_ne!(fresh_generation, old_generation);
    assert!(matches!(
        reloaded.meeting_detection.status,
        DetectionStatus::Blocked
    ));
    let before = {
        let state = service.meeting_detection.lock().unwrap();
        (state.mode, state.setup_open, state.ready, state.revision)
    };
    let error = service
        .set_detection_context_for_generation(CaptureMode::InPerson, false, old_generation)
        .unwrap_err();
    assert!(error.to_string().contains("page context changed"));
    {
        let state = service.meeting_detection.lock().unwrap();
        assert_eq!(
            (state.mode, state.setup_open, state.ready, state.revision),
            before
        );
    }
    assert_eq!(
        load_value::<CaptureMode>(&service.root, MODE_KEY).unwrap(),
        Some(CaptureMode::Remote)
    );
    service
        .set_detection_context_for_generation(CaptureMode::Remote, true, fresh_generation)
        .unwrap();
    let before = {
        let state = service.meeting_detection.lock().unwrap();
        (state.mode, state.setup_open, state.ready, state.revision)
    };
    release.send(()).unwrap();
    assert!(
        queued
            .join()
            .unwrap()
            .unwrap_err()
            .to_string()
            .contains("page context changed")
    );
    {
        let state = service.meeting_detection.lock().unwrap();
        assert_eq!(
            (state.mode, state.setup_open, state.ready, state.revision),
            before
        );
        assert!(state.setup_open && state.ready);
    }
    let confirmed = service
        .set_detection_context_for_generation(CaptureMode::Remote, false, fresh_generation)
        .unwrap();
    assert_eq!(
        confirmed.meeting_detection.context_generation,
        fresh_generation
    );
    assert!(!service.meeting_detection.lock().unwrap().setup_open);
    // Ephemeral generations are not resurrected from app settings on restart.
    assert_eq!(DetectionState::load(&service.root).context_generation, 0);
    assert!(store::meetings(&service.root).unwrap().is_empty());
}

#[test]
fn delayed_old_page_settings_cannot_replace_fresh_opt_out_or_suppression_authority() {
    for enabled in [false, true] {
        let temp = TempDir::new().unwrap();
        let (service, _) = self::service(&temp, PathBuf::from("never-start-helper"));
        let _stop = Shutdown(service.clone());
        let meeting = app(MeetingAppKind::Meeting);
        let old_generation = service
            .snapshot()
            .unwrap()
            .meeting_detection
            .context_generation;
        let old_settings = DetectionSettings {
            enabled: true,
            rules: vec![always(&meeting)],
        };
        service
            .update_meeting_detection_for_generation(old_settings.clone(), old_generation)
            .unwrap();
        service
            .set_detection_context_for_generation(CaptureMode::Remote, false, old_generation)
            .unwrap();
        let (release, waiting) = mpsc::channel();
        let old_page = service.clone();
        let queued = thread::spawn(move || {
            waiting.recv().unwrap();
            old_page.update_meeting_detection_for_generation(old_settings, old_generation)
        });
        service.reset_detection_context();
        let fresh_generation = service
            .snapshot()
            .unwrap()
            .meeting_detection
            .context_generation;
        service
            .set_detection_context_for_generation(CaptureMode::Remote, true, fresh_generation)
            .unwrap();
        let desired = DetectionSettings {
            enabled,
            rules: vec![DetectionRule {
                app_id: meeting.id.clone(),
                policy: DetectionPolicy::Suppress,
                participant_notice_attested: false,
                input: None,
            }],
        };
        let fresh = service
            .update_meeting_detection_for_generation(desired.clone(), fresh_generation)
            .unwrap();
        let expected = serde_json::to_value(&fresh.settings.meeting_detection).unwrap();
        let persisted = || {
            store::open(&service.root)
                .unwrap()
                .query_row(
                    "SELECT value_json FROM app_settings WHERE key=?1",
                    [SETTINGS_KEY],
                    |row| row.get::<_, String>(0),
                )
                .unwrap()
        };
        let saved = persisted();
        let before = {
            let state = service.meeting_detection.lock().unwrap();
            (state.mode, state.setup_open, state.ready, state.revision)
        };
        release.send(()).unwrap();
        assert!(
            queued
                .join()
                .unwrap()
                .unwrap_err()
                .to_string()
                .contains("page context changed")
        );
        assert_eq!(persisted(), saved);
        {
            let state = service.meeting_detection.lock().unwrap();
            assert_eq!(
                (state.mode, state.setup_open, state.ready, state.revision),
                before
            );
            assert_eq!(serde_json::to_value(&state.settings).unwrap(), expected);
        }
        // Fresh-page authority remains usable and old-page authority cannot arm
        // a future idle edge after the fresh page closes Settings.
        service
            .update_meeting_detection_for_generation(desired, fresh_generation)
            .unwrap();
        service
            .set_detection_context_for_generation(CaptureMode::Remote, false, fresh_generation)
            .unwrap();
        {
            let mut state = service.meeting_detection.lock().unwrap();
            state.observe(observed(&[]), 0, None, false);
            state.observe(
                observed(std::slice::from_ref(&meeting)),
                SECOND,
                None,
                false,
            );
            assert!(state.auto_candidate(SECOND, false).is_none());
        }
        assert!(store::meetings(&service.root).unwrap().is_empty());
        assert_eq!(
            serde_json::to_value(DetectionState::load(&service.root).settings).unwrap(),
            expected
        );
    }
}
