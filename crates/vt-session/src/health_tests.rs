use super::*;
#[test]
fn exact_decimal_thresholds_preserve_raw_until_critical_and_gate_processing() {
    for (bytes, state) in [
        (2_000_000_000, DiskState::Healthy),
        (1_999_999_999, DiskState::Warning),
        (500_000_000, DiskState::Warning),
        (499_999_999, DiskState::InferenceSuspended),
        (100_000_000, DiskState::InferenceSuspended),
        (99_999_999, DiskState::Critical),
    ] {
        let snapshot = HealthSnapshot::from_free(Some(bytes));
        assert_eq!(snapshot.disk_state, state);
        assert_eq!(snapshot.inference_suspended, bytes < SHED_BYTES);
        assert_eq!(snapshot.automatic_processing_allowed, bytes >= SHED_BYTES);
    }
    assert!(!HealthSnapshot::from_free(None).automatic_processing_allowed);
}
#[test]
fn thirty_second_cadence_and_transitions_are_injectable_without_sleep_or_disk_fill() {
    let mut monitor = HealthMonitor::default();
    assert!(monitor.due(0));
    assert!(monitor.observe(0, DiskState::Healthy));
    assert!(!monitor.due(29_999_999_999));
    assert!(monitor.due(30_000_000_000));
    assert!(monitor.observe(30_000_000_000, DiskState::Healthy));
    assert!(!monitor.observe(31_000_000_000, DiskState::Healthy));
    assert!(monitor.observe(32_000_000_000, DiskState::InferenceSuspended));
    assert!(!monitor.observe(33_000_000_000, DiskState::InferenceSuspended));
    assert!(monitor.observe(34_000_000_000, DiskState::Critical));
}

use crate::{AppService, CaptureMode, Segment, TimingProvenance};
use std::{
    fs,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use tempfile::TempDir;

struct FakeResources {
    free: AtomicU64,
    tick: AtomicU64,
    uptime: AtomicU64,
}
impl FakeResources {
    fn new(free: u64) -> Arc<Self> {
        Arc::new(Self {
            free: AtomicU64::new(free),
            tick: AtomicU64::new(0),
            uptime: AtomicU64::new(0),
        })
    }
}
impl ResourceProbe for FakeResources {
    fn free_bytes(&self, _: &Path) -> Result<u64> {
        Ok(self.free.load(Ordering::Relaxed))
    }
    fn uptime_ns(&self) -> Option<u64> {
        let n = self.uptime.load(Ordering::Relaxed);
        (n != 0).then_some(n)
    }
    fn tick_ns(&self) -> u64 {
        self.tick.load(Ordering::Relaxed)
    }
}
#[cfg(unix)]
fn helper(temp: &TempDir, anchor: bool) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = temp.path().join("fake-capture");
    fs::write(&path,format!(r#"#!/usr/bin/env python3
import json,pathlib,select,sys,time
folder=pathlib.Path(__file__).parent
assert sys.argv[1:3]==['--mode','remote']
def emit(value): print(json.dumps(value),flush=True)
def stopped():
    return bool(select.select([sys.stdin],[],[],0)[0]) and sys.stdin.readline().strip()=='stop'
if {anchor}:
    emit({{'type':'source_clock','clock':'uptime_raw','origin_ns':1000000000,'observed_ns':1000000000}})
for track in ['mic','system']:
    emit({{'type':'format','track':track,'sample_rate':16000,'channels':1,'epoch':0,'start_ns':0}})
(folder/'ready').write_text('ready')
while not (folder/'feed').exists():
    if stopped():
        (folder/'stopped').write_text('0')
        sys.exit(0)
    time.sleep(0.005)
frames=0
while not stopped():
    for track in ['mic','system']:
        emit({{'type':'audio','track':track,'epoch':0,'first_frame':frames,'host_time_ns':frames*1000000000//16000,'samples':[0.25]*320}})
    frames+=320
    time.sleep(0.005)
(folder/'stopped').write_text(str(frames))
"#,anchor=if anchor {"True"} else {"False"})).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    path
}
fn wait(mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(8);
    while !ready() {
        assert!(Instant::now() < deadline, "fixture condition timed out");
        std::thread::sleep(Duration::from_millis(10));
    }
}
struct StopOnDrop(Arc<AppService>);
impl Drop for StopOnDrop {
    fn drop(&mut self) {
        self.0.wait_for_shutdown();
    }
}

#[cfg(unix)]
#[test]
fn critical_preflight_never_starts_the_helper_or_creates_a_meeting() {
    let temp = TempDir::new().unwrap();
    let resources = FakeResources::new(99_999_999);
    let app = AppService::with_resources(temp.path().join("data"), helper(&temp, false), resources)
        .unwrap();
    let _stop = StopOnDrop(app.clone());
    assert!(
        app.start(
            "Synthetic critical preflight".into(),
            CaptureMode::Remote,
            true,
            None
        )
        .is_err()
    );
    assert!(!temp.path().join("ready").exists());
    let snapshot = app.snapshot().unwrap();
    assert!(snapshot.meetings.is_empty() && snapshot.recording.is_none());
    assert_eq!(snapshot.health.disk_state, DiskState::Critical);
    assert!(app.ensure_processing_space().is_err());
}

#[cfg(unix)]
#[test]
fn active_pressure_sheds_partials_keeps_raw_and_stops_cleanly_with_buffered_frames_saved() {
    let temp = TempDir::new().unwrap();
    let resources = FakeResources::new(3_000_000_000);
    let app = AppService::with_resources(
        temp.path().join("data"),
        helper(&temp, false),
        resources.clone(),
    )
    .unwrap();
    let _stop = StopOnDrop(app.clone());
    let snapshot = app
        .start(
            "Synthetic resource pressure".into(),
            CaptureMode::Remote,
            true,
            None,
        )
        .unwrap();
    let meeting = snapshot.recording.unwrap().id;
    wait(|| temp.path().join("ready").exists());
    fs::write(temp.path().join("feed"), b"feed").unwrap();
    wait(|| {
        app.snapshot()
            .unwrap()
            .recording
            .as_ref()
            .is_some_and(|r| r.elapsed_ms >= 100)
    });
    app.runtime.lock().unwrap().partials.insert(
        vt_core::TrackKind::Mic,
        Segment {
            id: "partial-resource-fixture".into(),
            track: "mic".into(),
            start_ms: 0,
            end_ms: 100,
            text: "Synthetic partial".into(),
            speaker: "Me".into(),
            words: vec![],
        },
    );
    resources.free.store(499_999_999, Ordering::Relaxed);
    resources.tick.store(30_000_000_000, Ordering::Relaxed);
    wait(|| app.snapshot().unwrap().health.inference_suspended);
    assert!(app.inference_suspended.load(Ordering::Relaxed));
    assert!(app.runtime.lock().unwrap().partials.is_empty());
    assert_eq!(
        app.snapshot().unwrap().recording.unwrap().status,
        "recording"
    );
    assert!(!temp.path().join("stopped").exists());
    assert!(app.ensure_processing_space().is_err());
    // Recovered space permits later saved processing, while live inference
    // remains suspended for the current capture rather than reopening models.
    resources.free.store(3_000_000_000, Ordering::Relaxed);
    let recovered = app.refresh_health(true).unwrap();
    assert!(recovered.inference_suspended && recovered.automatic_processing_allowed);
    resources.free.store(90_000_000, Ordering::Relaxed);
    // Even the largest two-second192kHz/eight-channel PCM24 chunk fits
    // the remaining90MB while requesting immediate clean shutdown.
    app.recording_budget(2 * 192_000 * 8 * 3 + 4096, &mut false)
        .unwrap();
    resources.tick.store(60_000_000_000, Ordering::Relaxed);
    wait(|| temp.path().join("stopped").exists());
    wait(|| app.snapshot().unwrap().recording.is_none());
    let frames: usize = fs::read_to_string(temp.path().join("stopped"))
        .unwrap()
        .parse()
        .unwrap();
    assert!(frames > 0);
    let detail = app.detail(&meeting).unwrap();
    assert_eq!(detail.meeting.status, "ready");
    assert!(
        detail
            .runs
            .iter()
            .any(|r| r.kind == "live" && r.status == "complete")
    );
    assert!(!app.snapshot().unwrap().health.automatic_processing_allowed);
    let report = vt_record::Recording::recover(app.root(), &meeting).unwrap();
    assert!(report.verified_chunks >= 2);
    let db = store::open(app.root()).unwrap();
    let mut query=db.prepare("SELECT track,path FROM audio_chunks WHERE meeting_id=?1 AND state='ready' ORDER BY track,seq").unwrap();
    let mut counts = [0, 0];
    for row in query
        .query_map([&meeting], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })
        .unwrap()
    {
        let (track, path) = row.unwrap();
        let mut decoder = claxon::FlacReader::open(app.root().join(path)).unwrap();
        let decoded = decoder
            .samples()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap();
        assert!(decoded.iter().all(|sample| *sample == 2_097_152));
        counts[usize::from(track == "system")] += decoded.len();
    }
    assert_eq!(
        counts,
        [frames, frames],
        "critical shutdown must drain every accepted raw frame"
    );
    let states: Vec<String> = db
        .prepare("SELECT disk_state FROM app_meeting_health WHERE meeting_id=?1 ORDER BY id")
        .unwrap()
        .query_map([&meeting], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert!(states.iter().any(|s| s == "inference_suspended"));
    assert!(states.iter().any(|s| s == "critical"));
}

#[cfg(unix)]
#[test]
fn live_notes_use_shared_source_clock_and_legacy_fallback_is_atomic_and_explicit() {
    for anchor in [true, false] {
        let temp = TempDir::new().unwrap();
        let resources = FakeResources::new(3_000_000_000);
        resources.uptime.store(1_125_000_000, Ordering::Relaxed);
        let app =
            AppService::with_resources(temp.path().join("data"), helper(&temp, anchor), resources)
                .unwrap();
        let _stop = StopOnDrop(app.clone());
        let meeting = app
            .start(
                "Synthetic note clock".into(),
                CaptureMode::Remote,
                true,
                None,
            )
            .unwrap()
            .recording
            .unwrap()
            .id;
        wait(|| temp.path().join("ready").exists());
        if anchor {
            wait(|| {
                app.snapshot()
                    .unwrap()
                    .source_timing
                    .is_some_and(|t| t.synchronized)
            });
            assert_eq!(app.snapshot().unwrap().recording.unwrap().elapsed_ms, 125);
            app.save_note(&meeting, "Synthetic synchronized bookmark")
                .unwrap();
            assert_eq!(app.detail(&meeting).unwrap().notes[0].t_ms, 125);
        } else {
            assert!(app.save_note(&meeting, "Too early").is_err());
            fs::write(temp.path().join("feed"), b"feed").unwrap();
            wait(|| {
                app.snapshot()
                    .unwrap()
                    .source_timing
                    .is_some_and(|t| t.latest_audio_ms.is_some())
            });
            app.save_note(&meeting, "Synthetic approximate bookmark")
                .unwrap();
            let timing = app.snapshot().unwrap().source_timing.unwrap();
            assert!(!timing.synchronized && timing.warning.is_some());
            assert_eq!(timing.provenance, TimingProvenance::LatestAudioObservation);
        }
        let db = store::open(app.root()).unwrap();
        let row:(String,bool)=db.query_row("SELECT provenance,synchronized FROM app_note_timing JOIN app_notes ON note_id=app_notes.id WHERE meeting_id=?1",[&meeting],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
        assert_eq!(
            row.0,
            if anchor {
                "uptime_raw"
            } else {
                "latest_audio_observation"
            }
        );
        assert_eq!(row.1, anchor);
        let before = app.detail(&meeting).unwrap().notes.len();
        db.execute_batch("CREATE TRIGGER reject_note_timing BEFORE INSERT ON app_note_timing BEGIN SELECT RAISE(ABORT,'synthetic metadata failure'); END;").unwrap();
        assert!(
            app.save_note(&meeting, "Must roll back with timing")
                .is_err()
        );
        assert_eq!(app.detail(&meeting).unwrap().notes.len(), before);
        assert!(
            store::search_meetings(app.root(), "Must roll back with timing")
                .unwrap()
                .is_empty()
        );
    }
}

#[cfg(unix)]
#[test]
fn low_space_preflight_records_raw_without_opening_the_configured_speech_model() {
    let temp = TempDir::new().unwrap();
    let resources = FakeResources::new(499_999_999);
    let app = AppService::with_resources(temp.path().join("data"), helper(&temp, false), resources)
        .unwrap();
    let _stop = StopOnDrop(app.clone());
    // Missing model files would fail immediately if a worker were opened.
    app.runtime.lock().unwrap().model.installed = true;
    let meeting = app
        .start(
            "Synthetic raw-only preflight".into(),
            CaptureMode::Remote,
            true,
            None,
        )
        .unwrap()
        .recording
        .unwrap()
        .id;
    wait(|| temp.path().join("ready").exists());
    fs::write(temp.path().join("feed"), b"feed").unwrap();
    wait(|| {
        app.snapshot()
            .unwrap()
            .recording
            .as_ref()
            .is_some_and(|r| r.elapsed_ms >= 100)
    });
    let live = app.snapshot().unwrap();
    assert!(live.health.inference_suspended);
    assert!(live.recording.as_ref().unwrap().transcription_configured);
    assert!(!live.recording.unwrap().transcription_active);
    app.stop().unwrap();
    wait(|| app.snapshot().unwrap().recording.is_none());
    assert_eq!(app.detail(&meeting).unwrap().meeting.status, "ready");
    assert!(
        vt_record::Recording::recover(app.root(), &meeting)
            .unwrap()
            .verified_chunks
            >= 2
    );
}

#[test]
fn saved_bookmarks_use_paused_playback_position_and_other_notes_are_explicitly_untimed() {
    use std::sync::atomic::AtomicUsize;
    use vt_core::{AudioBlock, FramePosition, StreamFormat, TrackKind};
    use vt_playback::{AudioRenderer, AudioSinkFactory, OpenedSink, Playback, PlaybackError};
    struct NoOutput(Arc<AtomicUsize>);
    impl AudioSinkFactory for NoOutput {
        fn open(&mut self, _: AudioRenderer) -> vt_playback::Result<OpenedSink> {
            self.0.fetch_add(1, Ordering::Relaxed);
            Err(PlaybackError::Output(
                "Output is prohibited by this synthetic test".into(),
            ))
        }
    }
    let temp = TempDir::new().unwrap();
    let app = AppService::with_resources(
        temp.path().join("data"),
        temp.path().join("unused-helper"),
        FakeResources::new(3_000_000_000),
    )
    .unwrap();
    let _stop = StopOnDrop(app.clone());
    let mut recording = vt_record::Recording::create(app.root(), "saved-bookmark").unwrap();
    store::create_meeting(
        app.root(),
        "saved-bookmark",
        "Synthetic saved bookmark",
        "remote",
    )
    .unwrap();
    recording
        .write_chunk(
            TrackKind::Mic,
            &AudioBlock::new(
                StreamFormat::new(16000, 1).unwrap(),
                FramePosition {
                    epoch: 0,
                    first_frame: 0,
                    host_time_ns: 0,
                },
                vec![0.25; 3200],
            )
            .unwrap(),
        )
        .unwrap();
    recording.finish().unwrap();
    store::complete_live_run(app.root(), "saved-bookmark").unwrap();
    app.save_note("saved-bookmark", "Synthetic untimed note")
        .unwrap();
    let opened = Arc::new(AtomicUsize::new(0));
    let playback =
        Playback::with_sink(app.root(), "saved-bookmark", NoOutput(opened.clone())).unwrap();
    playback.seek(125).unwrap();
    *app.playback.lock().unwrap() = Some(playback);
    app.save_note("saved-bookmark", "Synthetic paused bookmark")
        .unwrap();
    assert_eq!(
        opened.load(Ordering::Relaxed),
        0,
        "bookmarking must not open an output device"
    );
    let db = store::open(app.root()).unwrap();
    let values:Vec<(u64,String,bool)>=db.prepare("SELECT t_ms,provenance,synchronized FROM app_notes JOIN app_note_timing ON note_id=app_notes.id WHERE meeting_id='saved-bookmark' ORDER BY app_notes.rowid").unwrap().query_map([],|r|Ok((r.get::<_,i64>(0)? as u64,r.get(1)?,r.get(2)?))).unwrap().collect::<rusqlite::Result<_>>().unwrap();
    assert_eq!(
        values,
        vec![
            (0, "untimed_note".into(), false),
            (125, "saved_playback_position".into(), true)
        ]
    );
}

#[cfg(target_os = "macos")]
#[test]
fn detection_observation_clock_is_bracketed_by_continuous_raw_time_without_audio() {
    fn continuous() -> u64 {
        let time =
            nix::time::clock_gettime(nix::time::ClockId::from_raw(nix::libc::CLOCK_MONOTONIC_RAW))
                .unwrap();
        u64::try_from(time.tv_sec()).unwrap() * 1_000_000_000
            + u64::try_from(time.tv_nsec()).unwrap()
    }
    let resources = SystemResources::default();
    let before = continuous();
    let observed = resources.observation_ns().unwrap();
    let after = continuous();
    assert!(before <= observed && observed <= after);
    assert!(resources.observation_ns().unwrap() >= observed);
    // The audio source epoch remains the independently selected raw-uptime
    // clock; this check neither sleeps nor opens any audio device.
    assert!(resources.uptime_ns().is_some());
}
