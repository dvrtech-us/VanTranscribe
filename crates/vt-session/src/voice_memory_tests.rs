use super::*;
use std::sync::atomic::AtomicUsize;
use tempfile::TempDir;
use vt_core::{AudioBlock, FramePosition, StreamFormat};

struct FakeKey {
    loads: AtomicUsize,
    missing: bool,
}
impl FakeKey {
    fn available() -> Self {
        Self {
            loads: AtomicUsize::new(0),
            missing: false,
        }
    }
}
impl KeyProvider for FakeKey {
    fn load(&self) -> vt_speakers::Result<Option<SecretKey>> {
        self.loads.fetch_add(1, Ordering::SeqCst);
        Ok((!self.missing).then(|| Zeroizing::new([37; 32])))
    }
    fn create(&self) -> vt_speakers::Result<SecretKey> {
        Err(vt_speakers::MemoryError::KeyUnavailable)
    }
}
struct FakeEngine;
impl EmbeddingEngine for FakeEngine {
    fn extract(&mut self, samples: &[f32], rate: u32) -> Result<Vec<f32>> {
        assert!(samples.len() >= rate as usize && samples.len() <= rate as usize * 5);
        assert!(samples.iter().all(|v| (*v - 0.125).abs() < 1e-6));
        let mut result = vec![0.; 512];
        result[0] = 1.;
        Ok(result)
    }
}
fn fixture(seconds: u64) -> (TempDir, Arc<AppService>, EnrollVoiceRequest) {
    fixture_source(seconds, 8_000, &[0.125])
}
fn fixture_source(
    seconds: u64,
    rate: u32,
    samples: &[f32],
) -> (TempDir, Arc<AppService>, EnrollVoiceRequest) {
    let t = TempDir::new().unwrap();
    let service = AppService::new(t.path().into(), t.path().join("no-device-helper")).unwrap();
    let mut r = vt_record::Recording::create(t.path(), "voice-source").unwrap();
    store::create_meeting(
        t.path(),
        "voice-source",
        "Fictional voice enrollment",
        "remote",
    )
    .unwrap();
    for second in (0..seconds).step_by(2) {
        let frames = u64::from(rate) * (seconds - second).min(2);
        r.write_chunk(
            TrackKind::System,
            &AudioBlock::new(
                StreamFormat::new(rate, 1).unwrap(),
                FramePosition {
                    epoch: 0,
                    first_frame: second * u64::from(rate),
                    host_time_ns: second * SECOND,
                },
                samples
                    .iter()
                    .copied()
                    .cycle()
                    .skip((second * u64::from(rate)) as usize % samples.len())
                    .take(frames as usize)
                    .collect(),
            )
            .unwrap(),
        )
        .unwrap();
    }
    r.finish().unwrap();
    store::complete_live_run(t.path(), "voice-source").unwrap();
    store::set_status(t.path(), "voice-source", "ready", None).unwrap();
    let run = store::create_run(t.path(), "voice-source", "refine", "{\"fixture\":true}").unwrap();
    store::save_diarization_result(
        t.path(),
        "voice-source",
        &run.id,
        &vt_diarize::DiarizationResult {
            turns: vec![turn("one", 0, seconds * 1000)],
            speakers: vec![],
            merges: vec![],
            embedding_version: vt_diarize::EMBEDDING_VERSION.into(),
            calibrated: false,
            warnings: vec![],
            constraint_reviews: vec![],
            segmentation_context: Some(vt_diarize::Phase::Refined.segmentation_context()),
        },
        false,
    )
    .unwrap();
    store::publish_run(t.path(), "voice-source", &run.id).unwrap();
    let speaker = store::detail(t.path(), "voice-source").unwrap().speakers[0]
        .id
        .clone();
    let request = EnrollVoiceRequest {
        meeting_id: "voice-source".into(),
        run_id: run.id,
        speaker_id: speaker,
        display_name: "Fictional participant".into(),
        person_id: None,
        is_self: false,
        written_consent: true,
        consent_method: "Fictional written consent fixture".into(),
        attested_by: "Fixture operator".into(),
        channel_type: Some(ChannelType::RemoteNarrowband),
    };
    (t, service, request)
}
fn turn(id: &str, start: u64, end: u64) -> vt_diarize::SpeakerTurn {
    vt_diarize::SpeakerTurn {
        id: id.into(),
        track: "system".into(),
        start_ms: start,
        end_ms: end,
        speaker_ids: vec!["system:fixture".into()],
        label: "Speaker 1".into(),
        provenance: "refined-cluster".into(),
        score: None,
        overlap: false,
        uncertain: false,
        provisional: false,
    }
}
fn counts(root: &Path) -> (i64, i64) {
    let db = store::open(root).unwrap();
    (
        db.query_row("SELECT COUNT(*) FROM app_people", [], |r| r.get(0))
            .unwrap(),
        db.query_row("SELECT COUNT(*) FROM app_voiceprints", [], |r| r.get(0))
            .unwrap(),
    )
}
fn enable(service: &AppService) {
    service.update_voice_memory(true, 12, true, 0).unwrap();
}
fn enroll(
    service: &AppService,
    request: EnrollVoiceRequest,
    key: &dyn KeyProvider,
) -> Result<MemoryStatus> {
    service.enroll_voice_with(request, 0, key, || Ok(Box::new(FakeEngine)))
}
#[test]
fn metadata_and_off_or_missing_consent_never_extract_or_access_keys() {
    let (t, service, mut request) = fixture(22);
    let key = FakeKey::available();
    assert!(!service.voice_memory_status().unwrap().enabled);
    assert!(enroll(&service, request.clone(), &key).is_err());
    assert!(service.update_voice_memory(true, 12, false, 0).is_err());
    enable(&service);
    request.written_consent = false;
    assert!(enroll(&service, request, &key).is_err());
    assert_eq!(key.loads.load(Ordering::SeqCst), 0);
    assert_eq!(counts(t.path()), (0, 0));
    assert!(!service.runtime.lock().unwrap().processing);
}
#[test]
fn explicit_enrollment_recomputes_source_and_atomically_links_encrypted_person() {
    let (t, service, request) = fixture(22);
    enable(&service);
    let key = FakeKey::available();
    let speaker = request.speaker_id.clone();
    let state = enroll(&service, request.clone(), &key).unwrap();
    assert!(!state.matching_available);
    assert_eq!(state.people.len(), 1);
    assert_eq!(state.people[0].voiceprints[0].n_exemplars, 1);
    assert_eq!(
        state.people[0].voiceprints[0].channel_provenance,
        "user_declared"
    );
    let detail = service.detail("voice-source").unwrap();
    let s = detail.speakers.iter().find(|s| s.id == speaker).unwrap();
    assert_eq!(s.person_id.as_deref(), Some(state.people[0].id.as_str()));
    assert_eq!(s.user_name.as_deref(), Some("Fictional participant"));
    let db = store::open(t.path()).unwrap();
    let blob: Vec<u8> = db
        .query_row("SELECT exemplars FROM app_voiceprints", [], |r| r.get(0))
        .unwrap();
    assert!(blob.len() > 512 * 4);
    assert!(!blob.windows(4).any(|v| v == 1.0f32.to_le_bytes()));
    drop(db);
    drop(service);
    let reopened = AppService::new(t.path().into(), t.path().join("no-device-helper")).unwrap();
    assert_eq!(reopened.voice_memory_status().unwrap().people, state.people);
    assert!(!reopened.runtime.lock().unwrap().processing);
}
#[test]
fn insufficient_shared_room_unknown_channel_and_stale_refinement_fail_before_keys() {
    let (t, service, request) = fixture(19);
    enable(&service);
    let key = FakeKey::available();
    assert!(enroll(&service, request.clone(), &key).is_err());
    let (t2, s2, mut request2) = fixture(22);
    enable(&s2);
    request2.channel_type = None;
    assert!(enroll(&s2, request2.clone(), &key).is_err());
    request2.channel_type = Some(ChannelType::RemoteWideband);
    assert!(enroll(&s2, request2.clone(), &key).is_err());
    request2.channel_type = Some(ChannelType::RemoteNarrowband);
    store::open(t2.path())
        .unwrap()
        .execute(
            "UPDATE app_speaker_identities SET shared_room=1 WHERE id=?1",
            [&request2.speaker_id],
        )
        .unwrap();
    assert!(enroll(&s2, request2.clone(), &key).is_err());
    store::open(t2.path())
        .unwrap()
        .execute(
            "UPDATE app_speaker_identities SET shared_room=0 WHERE id=?1",
            [&request2.speaker_id],
        )
        .unwrap();
    request2.run_id = "stale-run".into();
    assert!(enroll(&s2, request2, &key).is_err());
    assert_eq!(counts(t.path()), (0, 0));
    assert_eq!(counts(t2.path()), (0, 0));
    assert_eq!(key.loads.load(Ordering::SeqCst), 0);
}
#[test]
fn pure_selection_excludes_overlaps_unknowns_clipping_and_epoch_bridges() {
    let chunks = (0..12u64)
        .map(|index| SourceChunk {
            epoch: u32::from(index >= 2),
            first_frame: if index < 2 {
                index * 16_000
            } else {
                (index - 2) * 16_000
            },
            frames: 16_000,
            rate: 8_000,
            start: index * 2 * SECOND,
            end: (index + 1) * 2 * SECOND,
            ready: true,
            clipped: 0,
            hash: None,
        })
        .collect::<Vec<_>>();
    let clean = turn("clean", 0, 24_000);
    let spans = select_spans(
        std::slice::from_ref(&clean),
        "system:fixture",
        TrackKind::System,
        &chunks,
        ChannelType::RemoteNarrowband,
    )
    .unwrap();
    assert!(
        spans
            .iter()
            .all(|s| s.end <= 4 * SECOND || s.start >= 4 * SECOND)
    );
    let mut overlap = turn("overlap", 0, 4_000);
    overlap.overlap = true;
    assert!(
        select_spans(
            &[clean.clone(), overlap],
            "system:fixture",
            TrackKind::System,
            &chunks,
            ChannelType::RemoteNarrowband
        )
        .is_err()
    );
    let mut uncertain = clean.clone();
    uncertain.uncertain = true;
    assert!(
        select_spans(
            &[uncertain],
            "system:fixture",
            TrackKind::System,
            &chunks,
            ChannelType::RemoteNarrowband
        )
        .is_err()
    );
    let mut clipped = chunks.clone();
    for c in &mut clipped[..6] {
        c.clipped = 1;
    }
    assert!(
        select_spans(
            &[clean],
            "system:fixture",
            TrackKind::System,
            &clipped,
            ChannelType::RemoteNarrowband
        )
        .is_err()
    );
}
#[test]
fn locked_or_missing_existing_key_preserves_voiceprint_and_person_links() {
    let (t, service, request) = fixture(22);
    enable(&service);
    let key = FakeKey::available();
    let before = enroll(&service, request.clone(), &key).unwrap();
    let unavailable = FakeKey {
        loads: AtomicUsize::new(0),
        missing: true,
    };
    assert!(enroll(&service, request, &unavailable).is_err());
    assert_eq!(counts(t.path()), (1, 1));
    assert_eq!(service.voice_memory_status().unwrap().people, before.people);
}
#[test]
fn queued_old_page_enable_cannot_override_new_opt_out() {
    let (t, service, _) = fixture(22);
    enable(&service);
    service.reset_detection_context();
    let generation = service.meeting_detection.lock().unwrap().context_generation;
    service
        .update_voice_memory(false, 12, false, generation)
        .unwrap();
    let db = store::open(t.path()).unwrap();
    let before:String=db.query_row("SELECT enabled||':'||retention_months||':'||explanation_acknowledged_at FROM app_voice_memory_settings",[],|r|r.get(0)).unwrap();
    assert!(service.update_voice_memory(true, 24, true, 0).is_err());
    let after:String=db.query_row("SELECT enabled||':'||retention_months||':'||explanation_acknowledged_at FROM app_voice_memory_settings",[],|r|r.get(0)).unwrap();
    assert_eq!(before, after);
}

struct GateEngine {
    entered: Option<mpsc::Sender<()>>,
    release: Option<mpsc::Receiver<()>>,
}
impl EmbeddingEngine for GateEngine {
    fn extract(&mut self, samples: &[f32], rate: u32) -> Result<Vec<f32>> {
        if let Some(entered) = self.entered.take() {
            entered.send(()).unwrap();
            self.release
                .take()
                .unwrap()
                .recv_timeout(std::time::Duration::from_secs(10))
                .unwrap();
        }
        FakeEngine.extract(samples, rate)
    }
}
#[test]
fn in_flight_enrollment_cannot_survive_reload_or_new_opt_out() {
    for reload in [false, true] {
        let (t, service, request) = fixture(22);
        enable(&service);
        let key = Arc::new(FakeKey::available());
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let worker_service = service.clone();
        let worker_key = key.clone();
        let worker = std::thread::spawn(move || {
            worker_service.enroll_voice_with(request, 0, worker_key.as_ref(), || {
                Ok(Box::new(GateEngine {
                    entered: Some(entered_tx),
                    release: Some(release_rx),
                }))
            })
        });
        entered_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap();
        assert!(service.runtime.lock().unwrap().processing);
        if reload {
            service.reset_detection_context();
        }
        let generation = service.meeting_detection.lock().unwrap().context_generation;
        service
            .update_voice_memory(false, 12, false, generation)
            .unwrap();
        release_tx.send(()).unwrap();
        assert!(worker.join().unwrap().is_err());
        assert_eq!(counts(t.path()), (0, 0));
        assert_eq!(key.loads.load(Ordering::SeqCst), 0);
        let runtime = service.runtime.lock().unwrap();
        assert!(!runtime.processing);
        assert_eq!(runtime.saved_audio_readers, 0);
    }
}
#[test]
fn cancellation_leaves_no_person_or_partial_enrollment() {
    let (t, service, request) = fixture(22);
    enable(&service);
    let key = Arc::new(FakeKey::available());
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let worker_service = service.clone();
    let worker_key = key.clone();
    let worker = std::thread::spawn(move || {
        worker_service.enroll_voice_with(request, 0, worker_key.as_ref(), || {
            Ok(Box::new(GateEngine {
                entered: Some(entered_tx),
                release: Some(release_rx),
            }))
        })
    });
    entered_rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .unwrap();
    service.cancel_processing();
    release_tx.send(()).unwrap();
    assert!(worker.join().unwrap().is_err());
    assert_eq!(counts(t.path()), (0, 0));
    assert_eq!(key.loads.load(Ordering::SeqCst), 0);
}
#[test]
#[ignore = "requires pinned public VT_DIARIZE_CONTEXT_WAV and VT_DIARIZE_MODEL_DIR; no devices or OS secrets"]
fn public_audio_actual_enrollment_recomputes_and_encrypts_with_injected_key_only() {
    let path = std::env::var("VT_DIARIZE_CONTEXT_WAV").unwrap();
    let expected: serde_json::Value = serde_json::from_str(include_str!(
        "../../vt-diarize/src/fixtures/live5-public-control-baseline.json"
    ))
    .unwrap();
    assert_eq!(
        format!("{:x}", Sha256::digest(std::fs::read(&path).unwrap())),
        expected["fixture_sha256"].as_str().unwrap()
    );
    let wave = sherpa_onnx::Wave::read(&path).unwrap();
    // A repeated public control plus a synthetic single-speaker assignment tests
    // trusted source decoding/encryption, not diarization quality or identity.
    let (t, service, mut request) = fixture_source(22, wave.sample_rate() as u32, wave.samples());
    request.channel_type = Some(ChannelType::RemoteWideband);
    enable(&service);
    let key = FakeKey::available();
    let state = service
        .enroll_voice_with(request, 0, &key, || {
            Ok(Box::new(vt_diarize::VoiceEmbedder::new(
                &std::path::PathBuf::from(std::env::var_os("VT_DIARIZE_MODEL_DIR").unwrap()),
            )?))
        })
        .unwrap();
    assert_eq!(state.people.len(), 1);
    assert_eq!(state.people[0].voiceprints[0].n_exemplars, 1);
    assert_eq!(counts(t.path()), (1, 1));
    assert!(!state.matching_available);
}
#[test]
fn renamed_live_annotations_copied_into_refine_are_not_refined_enrollment_evidence() {
    for context in [Some(vt_diarize::Phase::Live.segmentation_context()), None] {
        let (t, service, mut request) = fixture(22);
        enable(&service);
        store::open(t.path())
            .unwrap()
            .execute(
                "UPDATE app_speaker_run_state SET segmentation_context_json=?1 WHERE run_id=?2",
                params![
                    context.map(|value| serde_json::to_string(&value).unwrap()),
                    request.run_id
                ],
            )
            .unwrap();
        service
            .rename_speaker(
                &request.meeting_id,
                &request.speaker_id,
                "Fictional live name",
            )
            .unwrap();
        let run = store::create_run(
            t.path(),
            &request.meeting_id,
            "refine",
            "{\"retained\":true}",
        )
        .unwrap();
        store::retain_speaker_annotations(t.path(), &request.meeting_id, &run.id).unwrap();
        store::publish_run(t.path(), &request.meeting_id, &run.id).unwrap();
        request.run_id = run.id;
        let d = store::detail(t.path(), &request.meeting_id).unwrap();
        assert_eq!(d.diarization.unwrap().segmentation_context, context);
        assert!(d.speaker_turns.iter().all(|turn| turn.provenance == "user"));
        let key = FakeKey::available();
        let created = AtomicBool::new(false);
        assert!(
            service
                .enroll_voice_with(request, 0, &key, || {
                    created.store(true, Ordering::SeqCst);
                    Ok(Box::new(FakeEngine))
                })
                .is_err()
        );
        assert!(!created.load(Ordering::SeqCst));
        assert_eq!(key.loads.load(Ordering::SeqCst), 0);
        assert_eq!(counts(t.path()), (0, 0));
    }
}
#[test]
fn person_link_failure_rolls_back_the_entire_enrollment() {
    let (t, service, request) = fixture(22);
    enable(&service);
    let key = FakeKey::available();
    store::open(t.path()).unwrap().execute_batch("CREATE TRIGGER fixture_reject_person_link BEFORE UPDATE OF person_id ON app_speaker_identities WHEN NEW.person_id IS NOT NULL BEGIN SELECT RAISE(ABORT,'fixture link rejection'); END;").unwrap();
    assert!(enroll(&service, request, &key).is_err());
    assert_eq!(counts(t.path()), (0, 0));
    let db = store::open(t.path()).unwrap();
    let check: bool = db
        .query_row(
            "SELECT key_check IS NULL FROM app_voice_memory_settings",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(check);
    let unchanged: bool = db
        .query_row(
            "SELECT user_name IS NULL AND person_id IS NULL FROM app_speaker_identities LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(unchanged);
}

#[test]
fn retained_actual_refined_speaker_evidence_remains_eligible_on_the_same_verified_source() {
    let (t, service, mut request) = fixture(22);
    enable(&service);
    service
        .rename_speaker(
            &request.meeting_id,
            &request.speaker_id,
            "Fictional retained name",
        )
        .unwrap();
    let run = store::create_run(
        t.path(),
        &request.meeting_id,
        "refine",
        "{\"retained\":true}",
    )
    .unwrap();
    store::retain_speaker_annotations(t.path(), &request.meeting_id, &run.id).unwrap();
    store::publish_run(t.path(), &request.meeting_id, &run.id).unwrap();
    request.run_id = run.id;
    let detail = store::detail(t.path(), &request.meeting_id).unwrap();
    assert_eq!(
        detail.diarization.unwrap().segmentation_context,
        Some(vt_diarize::Phase::Refined.segmentation_context())
    );
    let state = service
        .enroll_voice_with(request, 0, &FakeKey::available(), || {
            Ok(Box::new(FakeEngine))
        })
        .unwrap();
    assert_eq!(state.people.len(), 1);
    assert_eq!(counts(t.path()), (1, 1));
    assert!(!state.matching_available);
}
