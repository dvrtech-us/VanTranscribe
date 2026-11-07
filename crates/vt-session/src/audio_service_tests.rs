//! App-level playback/capture exclusion with an injected silent sink. No devices
//! or private recordings are used; all FLAC and database data is temporary.
use super::*;
use std::{fs, sync::atomic::AtomicUsize, thread, time::Duration};
use tempfile::TempDir;
use vt_core::{AudioBlock, FramePosition, StreamFormat};
use vt_playback::{AudioRenderer, AudioSink, AudioSinkFactory, OpenedSink, OutputFormat, Playback};

#[derive(Default)]
struct SinkState {
    opens: AtomicUsize,
    plays: AtomicUsize,
    drops: AtomicUsize,
    renderer: Mutex<Option<AudioRenderer>>,
    drop_marker: Option<PathBuf>,
}
#[derive(Clone)]
struct Factory(Arc<SinkState>);
struct Sink(Arc<SinkState>);
impl AudioSink for Sink {
    fn play(&self) -> vt_playback::Result<()> {
        self.0.plays.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}
impl Drop for Sink {
    fn drop(&mut self) {
        *self.0.renderer.lock().unwrap() = None;
        self.0.drops.fetch_add(1, Ordering::SeqCst);
        if let Some(marker) = &self.0.drop_marker {
            fs::write(marker, b"output dropped").unwrap();
        }
    }
}
impl AudioSinkFactory for Factory {
    fn open(&mut self, renderer: AudioRenderer) -> vt_playback::Result<OpenedSink> {
        self.0.opens.fetch_add(1, Ordering::SeqCst);
        *self.0.renderer.lock().unwrap() = Some(renderer);
        Ok(OpenedSink {
            format: OutputFormat {
                sample_rate: 8000,
                channels: 2,
            },
            sink: Box::new(Sink(self.0.clone())),
        })
    }
}
fn fixture() -> (TempDir, Arc<AppService>) {
    let temp = TempDir::new().unwrap();
    let helper = temp.path().join("capture-fixture");
    fs::write(&helper, b"unused").unwrap();
    let service = AppService::new(temp.path().join("data"), helper).unwrap();
    {
        let mut prefs = service.preferences.lock().unwrap();
        prefs.auto_refine = false;
        prefs.auto_summarize = false;
    }
    let mut recording = vt_record::Recording::create(service.root(), "saved-audio").unwrap();
    store::create_meeting(
        service.root(),
        "saved-audio",
        "Synthetic saved audio",
        "remote",
    )
    .unwrap();
    for (track, value) in [(TrackKind::Mic, 0.25), (TrackKind::System, 0.125)] {
        recording
            .write_chunk(
                track,
                &AudioBlock::new(
                    StreamFormat::new(8000, 1).unwrap(),
                    FramePosition {
                        epoch: 0,
                        first_frame: 0,
                        host_time_ns: 0,
                    },
                    vec![value; 2400],
                )
                .unwrap(),
            )
            .unwrap();
    }
    recording.finish().unwrap();
    store::set_status(service.root(), "saved-audio", "ready", None).unwrap();
    (temp, service)
}
fn inject(service: &AppService, marker: Option<PathBuf>) -> Arc<SinkState> {
    let state = Arc::new(SinkState {
        drop_marker: marker,
        ..Default::default()
    });
    let playback =
        Playback::with_sink(service.root(), "saved-audio", Factory(state.clone())).unwrap();
    *service.playback.lock().unwrap() = Some(playback);
    state
}
fn set_recording(service: &AppService, status: &str) {
    let (control, _) = mpsc::channel();
    service.runtime.lock().unwrap().current = Some(Current {
        view: RecordingView {
            id: "different-active-meeting".into(),
            title: "Different meeting".into(),
            status: status.into(),
            elapsed_ms: 0,
            mic_level: 0.0,
            system_level: 0.0,
            gaps: 0,
            transcription_configured: false,
            transcription_active: false,
            error: None,
        },
        control,
    });
}
fn wait_until(mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !predicate() {
        assert!(Instant::now() < deadline, "audio fixture timed out");
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn service_play_pause_seek_volumes_and_waveform_do_not_change_recorded_audio() {
    let (_temp, service) = fixture();
    let db = store::open(service.root()).unwrap();
    let mut query = db
        .prepare("SELECT path,sha256 FROM audio_chunks ORDER BY track,seq")
        .unwrap();
    let before = query
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .unwrap()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap();
    let bytes = before
        .iter()
        .map(|(path, _)| fs::read(service.root().join(path)).unwrap())
        .collect::<Vec<_>>();
    let sink = inject(&service, None);
    assert_eq!(
        sink.opens.load(Ordering::SeqCst),
        0,
        "loading playback must be silent"
    );
    let first = service.audio_waveform("saved-audio", 12).unwrap();
    assert_eq!(first.duration_ms, 300);
    assert_eq!(first.mic.len(), 12);
    assert!(first.mic.iter().all(|value| (*value - 0.25).abs() < 1e-6));
    assert!(
        first
            .system
            .iter()
            .all(|value| (*value - 0.125).abs() < 1e-6)
    );
    assert_eq!(
        sink.opens.load(Ordering::SeqCst),
        0,
        "waveforms must not acquire output"
    );
    let played = service
        .play_audio("saved-audio", Some(100))
        .unwrap()
        .playback
        .unwrap();
    assert_eq!(played.status, "playing");
    assert_eq!(played.position_ms, 100);
    assert_eq!(sink.plays.load(Ordering::SeqCst), 1);
    assert_eq!(
        service.pause_audio().unwrap().playback.unwrap().status,
        "paused"
    );
    let seek = service
        .seek_audio("saved-audio", 200)
        .unwrap()
        .playback
        .unwrap();
    assert_eq!(seek.position_ms, 200);
    assert_eq!(seek.status, "paused");
    let mixed = service
        .set_playback_volumes("saved-audio", 0.4, 0.0)
        .unwrap()
        .playback
        .unwrap();
    assert_eq!(mixed.mic_volume, 0.4);
    assert_eq!(mixed.system_volume, 0.0);
    assert!(
        service
            .set_playback_volumes("saved-audio", f32::NAN, 1.0)
            .is_err()
    );
    service.play_audio("saved-audio", None).unwrap();
    assert!(service.stop_audio().unwrap().playback.is_none());
    assert!(sink.drops.load(Ordering::SeqCst) >= 2);
    let second = service.audio_waveform("saved-audio", 12).unwrap();
    assert_eq!(first.mic, second.mic);
    assert_eq!(first.system, second.system);
    for ((path, hash), original) in before.iter().zip(bytes) {
        assert_eq!(fs::read(service.root().join(path)).unwrap(), original);
        let retained: String = db
            .query_row(
                "SELECT sha256 FROM audio_chunks WHERE path=?1",
                [path],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(&retained, hash);
    }
}

#[test]
fn every_recording_state_blocks_other_meeting_play_pause_seek_and_volume() {
    for status in ["recording", "paused", "processing"] {
        let (_temp, service) = fixture();
        let sink = inject(&service, None);
        service.play_audio("saved-audio", None).unwrap();
        let opens = sink.opens.load(Ordering::SeqCst);
        set_recording(&service, status);
        for result in [
            service.play_audio("saved-audio", Some(100)),
            service.pause_audio(),
            service.seek_audio("saved-audio", 100),
            service.set_playback_volumes("saved-audio", 0.5, 0.5),
        ] {
            assert!(
                result.unwrap_err().to_string().contains("recording"),
                "{status} must exclude output control"
            );
        }
        assert!(service.audio_waveform("saved-audio", 12).is_err());
        assert_eq!(sink.opens.load(Ordering::SeqCst), opens);
        assert!(
            service.stop_audio().unwrap().playback.is_none(),
            "stopping existing output remains safe while capture is active"
        );
        assert_eq!(sink.drops.load(Ordering::SeqCst), 1);
    }
}

#[test]
fn shutdown_drops_output_and_prevents_output_from_reopening() {
    let (_temp, service) = fixture();
    let sink = inject(&service, None);
    service.play_audio("saved-audio", None).unwrap();
    service.shutdown();
    assert!(service.snapshot().unwrap().playback.is_none());
    assert_eq!(sink.drops.load(Ordering::SeqCst), 1);
    assert!(service.play_audio("saved-audio", None).is_err());
    assert!(service.pause_audio().is_err());
    assert!(service.seek_audio("saved-audio", 50).is_err());
    assert!(
        service
            .set_playback_volumes("saved-audio", 1.0, 1.0)
            .is_err()
    );
    assert_eq!(sink.opens.load(Ordering::SeqCst), 1);
    assert!(service.stop_audio().is_ok());
}

#[cfg(unix)]
#[test]
fn starting_capture_drops_existing_output_before_the_helper_executes() {
    use std::os::unix::fs::PermissionsExt;
    let (temp, service) = fixture();
    let dropped = temp.path().join("output-dropped");
    let observed = temp.path().join("capture-observation");
    let script = format!(
        "#!/usr/bin/env python3\nimport sys,json,pathlib\nmarker=pathlib.Path({})\nobserved=pathlib.Path({})\nready=observed.with_suffix('.ready')\nready.write_text('output already dropped' if marker.exists() else 'OUTPUT STILL LIVE')\nready.replace(observed)\nprint(json.dumps({{'type':'format','track':'mic','sample_rate':8000,'channels':1,'epoch':0,'start_ns':0}}),flush=True)\nprint(json.dumps({{'type':'audio','track':'mic','epoch':0,'first_frame':0,'host_time_ns':0,'samples':[0.25]*160}}),flush=True)\nfor line in sys.stdin:\n if line.strip()=='stop': break\n",
        serde_json::to_string(dropped.to_str().unwrap()).unwrap(),
        serde_json::to_string(observed.to_str().unwrap()).unwrap()
    );
    fs::write(&service.helper, script).unwrap();
    fs::set_permissions(&service.helper, fs::Permissions::from_mode(0o700)).unwrap();
    let sink = inject(&service, Some(dropped));
    service.play_audio("saved-audio", None).unwrap();
    assert_eq!(sink.drops.load(Ordering::SeqCst), 0);
    service
        .start(
            "Capture ordering fixture".into(),
            CaptureMode::InPerson,
            true,
            None,
        )
        .unwrap();
    wait_until(|| observed.exists());
    assert_eq!(
        fs::read_to_string(observed).unwrap(),
        "output already dropped"
    );
    assert_eq!(sink.drops.load(Ordering::SeqCst), 1);
    assert!(service.snapshot().unwrap().playback.is_none());
    service.stop().unwrap();
    wait_until(|| service.snapshot().unwrap().recording.is_none());
}

#[test]
fn optional_speech_preferences_are_explicit_persistent_and_do_not_start_model_downloads() {
    let (_temp, service) = fixture();
    let snapshot = service.snapshot().unwrap();
    assert!(snapshot.settings.live_vad_enabled);
    assert!(!snapshot.settings.diarization_enabled);
    assert!(!snapshot.vad_models.installed);
    assert!(!snapshot.diarization_models.installed);
    assert!(
        service.vad_path().is_none(),
        "default enabled preference cannot use an absent model"
    );
    assert!(service.install_diarization_model(false).is_err());
    assert!(!service.snapshot().unwrap().diarization_models.busy);
    let snapshot = service
        .update_audio_preferences(AudioPreferences {
            live_vad_enabled: false,
            diarization_enabled: true,
            echo_cancellation_enabled: true,
        })
        .unwrap();
    assert!(!snapshot.settings.live_vad_enabled);
    assert!(snapshot.settings.diarization_enabled);
    assert!(service.vad_path().is_none());
    assert!(!snapshot.vad_models.busy);
    assert!(!snapshot.diarization_models.busy);
    let saved = audio_preferences::load(service.root()).unwrap();
    assert!(!saved.live_vad_enabled);
    assert!(saved.diarization_enabled);
    service.cancel_vad_model_download();
    service.cancel_diarization_model_download();
    assert!(service.cancel_vad_download.load(Ordering::Relaxed));
    assert!(service.cancel_diarization_download.load(Ordering::Relaxed));
}
