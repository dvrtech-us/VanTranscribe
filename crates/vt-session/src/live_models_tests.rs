//! Temporary settings only; these checks never acquire capture/output devices.
use super::*;
use tempfile::TempDir;
fn fixture() -> (TempDir, Arc<AppService>) {
    let temp = TempDir::new().unwrap();
    let service =
        AppService::new(temp.path().join("data"), temp.path().join("unused-helper")).unwrap();
    (temp, service)
}
#[test]
fn explicit_model_choice_survives_restart_without_downloading_or_removing_a_pack() {
    let (temp, service) = fixture();
    assert_eq!(
        service.live_model_catalog().unwrap().selected,
        LiveModel::EnglishStandard
    );
    let snapshot = service
        .select_live_model(LiveModel::EnglishStandard)
        .unwrap();
    assert!(!snapshot.models.installed);
    assert!(!service.root().join("models").exists());
    let catalog = service.live_model_catalog().unwrap();
    assert_eq!(catalog.selected, LiveModel::EnglishStandard);
    assert_eq!(catalog.choices.len(), 2);
    assert!(catalog.choices.iter().all(|c| !c.installed));
    assert_eq!(
        service
            .live_model_manager()
            .unwrap()
            .model_dir()
            .file_name()
            .unwrap(),
        LiveModel::EnglishStandard.id()
    );
    drop(service);
    let restored =
        AppService::new(temp.path().join("data"), temp.path().join("unused-helper")).unwrap();
    assert_eq!(
        restored.live_model_catalog().unwrap().selected,
        LiveModel::EnglishStandard
    );
    assert!(!restored.root().join("models").exists());
    restored.select_live_model(LiveModel::EnglishSmall).unwrap();
    assert_eq!(
        restored.live_model_manager().unwrap().model(),
        LiveModel::EnglishSmall
    );
}
#[test]
fn model_choice_cannot_change_during_any_active_work() {
    for status in ["recording", "paused", "processing"] {
        let (_temp, service) = fixture();
        let (control, _) = mpsc::channel();
        service.runtime.lock().unwrap().current = Some(Current {
            control,
            view: RecordingView {
                id: "active".into(),
                title: "Active fixture".into(),
                status: status.into(),
                elapsed_ms: 0,
                mic_level: 0.0,
                system_level: 0.0,
                gaps: 0,
                transcription_configured: false,
                transcription_active: false,
                live_transcription: None,
                capture: Default::default(),
                error: None,
            },
        });
        assert!(
            service
                .select_live_model(LiveModel::EnglishStandard)
                .is_err()
        );
        assert_eq!(
            selected(service.root()).unwrap(),
            LiveModel::EnglishStandard
        );
    }
    for setup in [true, false] {
        let (_temp, service) = fixture();
        {
            let mut runtime = service.runtime.lock().unwrap();
            if setup {
                runtime.model.busy = true;
            } else {
                runtime.processing = true;
            }
        }
        assert!(
            service
                .select_live_model(LiveModel::EnglishStandard)
                .is_err()
        );
        assert_eq!(
            selected(service.root()).unwrap(),
            LiveModel::EnglishStandard
        );
    }
}
#[test]
fn selected_model_provenance_is_atomic_with_live_run_creation() {
    let (_temp, service) = fixture();
    service
        .select_live_model(LiveModel::EnglishStandard)
        .unwrap();
    let engines = provenance(service.root(), false).unwrap();
    let _recording = vt_record::Recording::create(service.root(), "provenance").unwrap();
    store::create_meeting_with_engines(
        service.root(),
        "provenance",
        "Provenance fixture",
        "remote",
        &engines,
    )
    .unwrap();
    let _invalid_source =
        vt_record::Recording::create(service.root(), "invalid-provenance").unwrap();
    assert!(
        store::create_meeting_with_engines(
            service.root(),
            "invalid-provenance",
            "Invalid fixture",
            "remote",
            "not-json"
        )
        .is_err()
    );
    assert!(
        !store::meetings(service.root())
            .unwrap()
            .iter()
            .any(|m| m.id == "invalid-provenance")
    );
    let detail = store::detail(service.root(), "provenance").unwrap();
    let stored: serde_json::Value = serde_json::from_str(&detail.runs[0].engines_json).unwrap();
    assert_eq!(
        stored["live_stt"]["requested_model"],
        LiveModel::EnglishStandard.id()
    );
    assert_eq!(stored["live_stt"]["enabled"], false);
    assert_eq!(stored["live_stt"]["files"].as_array().unwrap().len(), 4);
    assert_eq!(
        stored["live_stt"]["archive_sha256"],
        LiveModel::EnglishStandard.archive_sha256()
    );
}

#[test]
fn legacy_model_directories_keep_their_selection_and_fresh_setups_use_standard() {
    let (_temp, service) = fixture();
    assert_eq!(
        selected(service.root()).unwrap(),
        LiveModel::EnglishStandard
    );
    let legacy =
        ModelManager::for_model(service.root().join("models"), LiveModel::EnglishSmall).model_dir();
    std::fs::create_dir_all(&legacy).unwrap();
    assert_eq!(selected(service.root()).unwrap(), LiveModel::EnglishSmall);
    service
        .select_live_model(LiveModel::EnglishStandard)
        .unwrap();
    assert_eq!(
        selected(service.root()).unwrap(),
        LiveModel::EnglishStandard
    );
    assert!(legacy.exists());
    assert_eq!(vad_options(LiveModel::EnglishSmall).threshold, 0.5);
    assert_eq!(vad_options(LiveModel::EnglishStandard).threshold, 0.2);
}

#[test]
fn replay_model_provenance_does_not_rewrite_the_previous_live_version() {
    let (_temp, service) = fixture();
    let _source = vt_record::Recording::create(service.root(), "replay-provenance").unwrap();
    service.select_live_model(LiveModel::EnglishSmall).unwrap();
    let original = provenance(service.root(), true).unwrap();
    store::create_meeting_with_engines(
        service.root(),
        "replay-provenance",
        "Replay fixture",
        "remote",
        &original,
    )
    .unwrap();
    store::complete_live_run(service.root(), "replay-provenance").unwrap();
    store::set_status(service.root(), "replay-provenance", "ready", None).unwrap();
    let old = store::detail(service.root(), "replay-provenance")
        .unwrap()
        .active_run_id
        .unwrap();
    service
        .select_live_model(LiveModel::EnglishStandard)
        .unwrap();
    let engines = provenance(service.root(), true).unwrap();
    store::stage_segment(
        service.root(),
        "job-profile",
        "replay-provenance",
        &Segment {
            id: "synthetic-line".into(),
            track: "mic".into(),
            start_ms: 0,
            end_ms: 1000,
            text: "Synthetic replay line".into(),
            speaker: "Me".into(),
            words: vec![],
        },
    )
    .unwrap();
    assert!(
        store::publish_staged_with_provenance(
            service.root(),
            "job-profile",
            "replay-provenance",
            None,
            "invalid-json"
        )
        .is_err()
    );
    assert_eq!(
        store::detail(service.root(), "replay-provenance")
            .unwrap()
            .active_run_id
            .as_deref(),
        Some(old.as_str())
    );
    store::publish_staged_with_provenance(
        service.root(),
        "job-profile",
        "replay-provenance",
        None,
        &engines,
    )
    .unwrap();
    let detail = store::detail(service.root(), "replay-provenance").unwrap();
    let previous = detail.runs.iter().find(|r| r.id == old).unwrap();
    assert_eq!(previous.engines_json, original);
    let active = detail
        .runs
        .iter()
        .find(|r| Some(&r.id) == detail.active_run_id.as_ref())
        .unwrap();
    let config: serde_json::Value = serde_json::from_str(&active.engines_json).unwrap();
    assert_eq!(active.kind, "replay");
    assert_eq!(
        config["live_stt"]["requested_model"],
        LiveModel::EnglishStandard.id()
    );
    assert_eq!(detail.segments[0].text, "Synthetic replay line");
}

#[test]
fn shutdown_refuses_new_capture_and_setup_without_creating_recording_or_models() {
    let (_temp, service) = fixture();
    std::fs::write(&service.helper, b"not an executable capture helper").unwrap();
    // An invalid choice makes this regression safe even if the shutdown setup
    // guard breaks: it must still fail before any possible network worker.
    store::open(service.root()).unwrap().execute("INSERT INTO app_settings(key,value_json) VALUES('live_model','\"invalid-fixture-model\"')",[]).unwrap();
    service.shutdown();
    assert!(
        service
            .start("Blocked".into(), CaptureMode::Remote, true, None)
            .unwrap_err()
            .to_string()
            .contains("closing")
    );
    assert!(
        service
            .install_model()
            .unwrap_err()
            .to_string()
            .contains("closing")
    );
    assert!(
        service
            .select_live_model(LiveModel::EnglishStandard)
            .unwrap_err()
            .to_string()
            .contains("closing")
    );
    assert!(store::meetings(service.root()).unwrap().is_empty());
    assert!(!service.root().join("models").exists());
}

#[test]
fn invalid_saved_choices_open_for_repair_without_activating_a_fallback_model() {
    for (bad, legacy) in [("not-json", false), ("\"future-unsupported-model\"", true)] {
        let (temp, service) = fixture();
        if legacy {
            std::fs::create_dir_all(
                ModelManager::for_model(service.root().join("models"), LiveModel::EnglishSmall)
                    .model_dir(),
            )
            .unwrap();
        }
        store::open(service.root())
            .unwrap()
            .execute(
                "INSERT INTO app_settings(key,value_json) VALUES('live_model',?1)",
                [bad],
            )
            .unwrap();
        drop(service);
        let repaired =
            AppService::new(temp.path().join("data"), temp.path().join("unused-helper")).unwrap();
        let catalog = repaired.live_model_catalog().unwrap();
        assert_eq!(
            catalog.selected,
            if legacy {
                LiveModel::EnglishSmall
            } else {
                LiveModel::EnglishStandard
            }
        );
        assert!(catalog.selection_warning.is_some());
        let snapshot = repaired.snapshot().unwrap();
        assert!(!snapshot.models.installed);
        assert!(snapshot.models.error.is_some());
        assert!(repaired.live_model_manager().is_err());
        let stored: String = store::open(repaired.root())
            .unwrap()
            .query_row(
                "SELECT value_json FROM app_settings WHERE key='live_model'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            stored, bad,
            "opening must not silently rewrite the saved choice"
        );
        let metadata: serde_json::Value =
            serde_json::from_str(&capture_provenance(repaired.root(), false, false).unwrap())
                .unwrap();
        assert!(metadata["live_stt"]["requested_model"].is_null());
        repaired.select_live_model(catalog.selected).unwrap();
        assert!(
            repaired
                .live_model_catalog()
                .unwrap()
                .selection_warning
                .is_none()
        );
        assert!(repaired.live_model_manager().is_ok());
    }
}
#[test]
fn disk_suspended_capture_does_not_claim_a_failed_or_missing_audit() {
    let (_temp, service) = fixture();
    for (id, suspended) in [("suspended-audit", true), ("expected-audit", false)] {
        let _source = vt_record::Recording::create(service.root(), id).unwrap();
        let metadata = capture_provenance(service.root(), true, suspended).unwrap();
        store::create_meeting_with_engines(
            service.root(),
            id,
            "Audit expectation fixture",
            "remote",
            &metadata,
        )
        .unwrap();
        store::complete_live_run(service.root(), id).unwrap();
        store::set_status(service.root(), id, "ready", None).unwrap();
        let detail = store::detail(service.root(), id).unwrap();
        if suspended {
            assert!(detail.inference_health.is_none());
        } else {
            let report = detail.inference_health.unwrap();
            assert!(!report.complete);
            assert!(report.truncated);
            assert!(!report.warnings.is_empty());
        }
    }
}
