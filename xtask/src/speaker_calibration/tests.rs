use super::*;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tempfile::TempDir;

fn pcm(rate: u32, seconds: u32, value: i16) -> Vec<u8> {
    let bytes = rate * seconds * 2;
    let mut out = Vec::new();
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(bytes + 36).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&bytes.to_le_bytes());
    for _ in 0..rate * seconds {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out
}
fn write_manifest(root: &Path, value: &Value) -> PathBuf {
    let path = root.join("manifest.json");
    fs::write(&path, serde_json::to_vec(value).unwrap()).unwrap();
    path
}
fn fixture() -> (TempDir, Value) {
    let t = TempDir::new().unwrap();
    fs::create_dir(t.path().join("models")).unwrap();
    let mut partitions = Vec::new();
    for partition in ["calibration", "held_out"] {
        let people = (0..50)
            .map(|n| format!("{partition}-p{n}"))
            .collect::<Vec<_>>();
        let mut enrollments = Vec::new();
        let mut probes = Vec::new();
        for role in ["enroll", "probe"] {
            let n = if role == "enroll" { 50 } else { 51 };
            for person in 0..n {
                let subject = format!("{partition}-p{person}");
                let template = t.path().join(format!("template-{partition}-{person}.wav"));
                let data = pcm(8_000, 5, (person + 1) as i16);
                if !template.exists() {
                    fs::write(&template, &data).unwrap();
                }
                let mut windows = Vec::new();
                let count = if role == "enroll" { 4 } else { 3 };
                for index in 0..count {
                    let path = format!("{partition}-{role}-{person}-{index}.wav");
                    fs::hard_link(&template, t.path().join(&path)).unwrap();
                    windows.push(json!({"path":path,"sha256":sha(&data),"source_url":format!("https://example.invalid/{partition}/{role}/{person}.wav"),"source_start_frame":index*40_000,"source_end_frame":(index+1)*40_000,"source_sha256":null,"source_etag":"fixture-etag","sample_rate":8000}));
                }
                let sample = json!({"id":format!("{partition}|{role}|{person}"),"person_id":subject,"meeting_id":format!("{partition}-project-{}",if role=="enroll" {"a"}else {"b"}),"project_id":format!("{partition}-project"),"windows":windows});
                if role == "enroll" {
                    enrollments.push(sample);
                } else {
                    probes.push(sample);
                }
            }
        }
        partitions.push(json!({"id":partition,"gallery_people":people,"enrollments":enrollments,"probes":probes}));
    }
    (
        t,
        json!({"schema_version":1,"protocol_id":"fictional-open-set-test","channel_type":"in_person","license_url":"https://example.invalid/license","annotation_sha256":"a".repeat(64),"partition_sha256":"b".repeat(64),"model_dir":"models","expected_embedding_contract_sha256":vt_diarize::voice_embedding_contract_sha256(),"partitions":partitions}),
    )
}
struct MockEngine {
    calls: Arc<AtomicUsize>,
}
impl EmbeddingEngine for MockEngine {
    fn extract(&mut self, audio: &[f32], _: u32) -> EvalResult<Vec<f32>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let index = (audio[0] * 32768.0).round() as usize - 1;
        let mut value = vec![0.; 512];
        value[index] = 1.;
        Ok(value)
    }
}
#[test]
fn synthetic_full_runner_freezes_policy_then_reports_fixed_fifty_person_research_only_metrics() {
    let (t, value) = fixture();
    let path = write_manifest(t.path(), &value);
    let output = t.path().join("evidence");
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    run_with(&path, &output, EvaluationMode::Full, |_, _| {
        Ok(Box::new(MockEngine { calls: counter }))
    })
    .unwrap();
    assert!(output.join("COMPLETE").exists());
    assert!(!output.join("INCOMPLETE").exists());
    let raw = fs::read(output.join("report.json")).unwrap();
    let report: Value = serde_json::from_slice(&raw).unwrap();
    assert_eq!(report["research_only"], true);
    assert_eq!(report["synthetic_injected_engine"], true);
    assert_eq!(report["actual_native_inference"], false);
    assert_eq!(report["production_eligible"], false);
    assert_eq!(report["runtime_pin_generated"], false);
    assert_eq!(report["status"], "held_out_evaluated_once");
    assert_eq!(report["calibration"]["metrics"]["gallery_people"], 50);
    assert_eq!(report["held_out"]["metrics"]["recognition_rate"], 1.0);
    assert_eq!(
        report["held_out"]["metrics"]["unknown_false_identification_rate"],
        0.0
    );
    assert_eq!(
        report["frozen_research_policy_sha256"],
        sha(&fs::read(output.join("frozen-research-policy.json")).unwrap())
    );
    assert!(
        !String::from_utf8(raw.clone())
            .unwrap()
            .contains("\"embedding\":")
    );
    assert!(!String::from_utf8(raw).unwrap().contains("exemplars"));
    assert!(calls.load(Ordering::SeqCst) > 0);
    let before = fs::read(output.join("report.json")).unwrap();
    assert!(run(&path, &output).is_err());
    assert_eq!(before, fs::read(output.join("report.json")).unwrap());
}
#[test]
fn wrong_version_and_window_hash_fail_before_model_construction_and_leave_no_complete_claim() {
    let (t, mut value) = fixture();
    value["expected_embedding_contract_sha256"] = json!("c".repeat(64));
    let output = t.path().join("wrong-version");
    let called = AtomicUsize::new(0);
    assert!(
        run_with(
            &write_manifest(t.path(), &value),
            &output,
            EvaluationMode::Full,
            |_, _| {
                called.fetch_add(1, Ordering::SeqCst);
                Err("must not construct".into())
            }
        )
        .is_err()
    );
    assert_eq!(called.load(Ordering::SeqCst), 0);
    assert!(output.join("INCOMPLETE").exists());
    assert!(!output.join("COMPLETE").exists());
    value["expected_embedding_contract_sha256"] =
        json!(vt_diarize::voice_embedding_contract_sha256());
    value["partitions"][0]["enrollments"][0]["windows"][0]["sha256"] = json!("d".repeat(64));
    assert!(
        validate(&write_manifest(t.path(), &value))
            .err()
            .expect("must reject invalid evidence")
            .contains("SHA-256 mismatch")
    );
}
#[test]
fn source_leakage_same_meeting_and_cross_partition_person_project_overlap_are_rejected() {
    let (t, value) = fixture();
    let path = write_manifest(t.path(), &value);
    assert!(validate(&path).is_ok());
    let mut modified = value.clone();
    modified["partitions"][0]["probes"][0]["meeting_id"] =
        modified["partitions"][0]["enrollments"][0]["meeting_id"].clone();
    assert!(
        validate(&write_manifest(t.path(), &modified))
            .err()
            .expect("must reject invalid evidence")
            .contains("different meeting")
    );
    let mut modified = value.clone();
    modified["partitions"][1]["probes"][50]["person_id"] =
        modified["partitions"][0]["probes"][50]["person_id"].clone();
    assert!(
        validate(&write_manifest(t.path(), &modified))
            .err()
            .expect("must reject invalid evidence")
            .contains("person- and project-disjoint")
    );
    let mut modified = value.clone();
    for sample in modified["partitions"][1]["enrollments"]
        .as_array_mut()
        .unwrap()
    {
        sample["project_id"] = json!("calibration-project");
    }
    for sample in modified["partitions"][1]["probes"].as_array_mut().unwrap() {
        sample["project_id"] = json!("calibration-project");
    }
    assert!(
        validate(&write_manifest(t.path(), &modified))
            .err()
            .expect("must reject invalid evidence")
            .contains("person- and project-disjoint")
    );
    let mut modified = value.clone();
    modified["partitions"][0]["probes"][0]["windows"][0]["source_url"] =
        modified["partitions"][0]["enrollments"][0]["windows"][0]["source_url"].clone();
    assert!(
        validate(&write_manifest(t.path(), &modified))
            .err()
            .expect("must reject invalid evidence")
            .contains("source recording")
    );
}
#[test]
fn canonical_wave_geometry_and_path_containment_do_not_accept_clipping_or_escape() {
    let (t, mut value) = fixture();
    value["partitions"][0]["enrollments"][0]["windows"][0]["source_end_frame"] = json!(39_999);
    assert!(
        validate(&write_manifest(t.path(), &value))
            .err()
            .expect("must reject invalid evidence")
            .contains("window frames")
    );
    assert!(contained(t.path(), Path::new("../outside.wav")).is_err());
    #[cfg(unix)]
    {
        let outside = TempDir::new().unwrap();
        fs::write(outside.path().join("outside.wav"), [0]).unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("outside.wav"),
            t.path().join("escape.wav"),
        )
        .unwrap();
        assert!(contained(t.path(), Path::new("escape.wav")).is_err());
    }
}
fn ranked(id: &str, expected: Option<&str>, best: &str, score: f64, margin: f64) -> RankedTrial {
    RankedTrial {
        query_id: sha(id.as_bytes()),
        expected_person: expected.map(str::to_string),
        best_person: best.into(),
        best_score: score,
        runner_up_score: Some(score - margin),
        margin: Some(margin),
        clean_speech_ms: 15_000,
        gallery_people: 50,
    }
}
#[test]
fn fitting_only_uses_calibration_and_does_not_hide_known_confusions_or_unknown_false_ids() {
    let trials = vec![
        ranked("a", Some("a"), "a", 0.9, 0.3),
        ranked("b", Some("b"), "b", 0.8, 0.2),
        ranked("u", None, "a", 0.7, 0.1),
    ];
    let (policy, c) = fit(&trials).unwrap();
    assert_eq!(c.correct, 2);
    assert_eq!(c.false_unknown, 0);
    assert!(policy.tau_auto <= 0.8 && policy.tau_auto > 0.7 || policy.margin > 0.1);
    let contrary = [ranked("held", Some("a"), "wrong", 0.99, 0.8)];
    let held = count(&contrary, policy);
    assert_eq!((held.correct, held.wrong_known), (0, 1));
    let (policy_again, _) = fit(&trials).unwrap();
    assert_eq!(policy.tau_auto, policy_again.tau_auto);
    assert_eq!(policy.margin, policy_again.margin);
    let impossible = [
        ranked("known", Some("a"), "a", 1.0, 2.0),
        ranked("unknown", None, "a", 1.0, 2.0),
    ];
    assert!(fit(&impossible).is_none());
    let bad = [
        ranked("known-wrong", Some("b"), "a", 0.99, 0.3),
        ranked("unknown-abstain", None, "a", 0.0, 0.01),
    ];
    let (_, c) = fit(&bad).unwrap();
    assert_eq!(c.correct, 0);
}
#[test]
fn wilson_intervals_and_missing_denominators_do_not_manufacture_certainty() {
    assert_eq!(wilson(0, 0), None);
    assert_eq!(ratio(0, 0), None);
    let interval = wilson(0, 50).unwrap();
    assert!(interval[1] > 0.02);
    assert_eq!(interval[0], 0.0);
    let interval = wilson(50, 50).unwrap();
    assert!(interval[0] < 1.0 && interval[1] <= 1.0);
}
#[test]
fn frozen_partition_binding_rejects_post_freeze_identity_role_and_source_changes() {
    let (_t, value) = fixture();
    let manifest: Manifest = serde_json::from_value(value.clone()).unwrap();
    let partitions=manifest.partitions.iter().map(|p|{
        let samples=[("enrollment",&p.enrollments),("probe",&p.probes)].into_iter().flat_map(|(role,samples)|samples.iter().map(move|s|json!({"id":s.id,"person_id":s.person_id,"meeting_id":s.meeting_id,"role":role,"source_url":s.windows[0].source_url,"source_windows_frames":s.windows.iter().map(|w|vec![w.source_start_frame,w.source_end_frame]).collect::<Vec<_>>(),"selected_frames":s.windows.iter().map(|w|w.source_end_frame-w.source_start_frame).sum::<u64>()}))).collect::<Vec<_>>();
        json!({"id":p.id,"project_ids":vec![format!("{}-project",p.id)],"gallery_people":p.gallery_people,"samples":samples})
    }).collect::<Vec<_>>();
    let frozen=serde_json::to_vec(&json!({"schema_version":1,"protocol_id":manifest.protocol_id,"annotation_archive_sha256":manifest.annotation_sha256,"channel_type":"in_person","partitions":partitions})).unwrap();
    bind_partition(&manifest, &frozen).unwrap();
    for field in ["person_id", "meeting_id", "source_url"] {
        let mut modified = value.clone();
        if field == "source_url" {
            modified["partitions"][0]["probes"][0]["windows"][0][field] =
                json!("https://example.invalid/changed.wav");
        } else {
            modified["partitions"][0]["probes"][0][field] = json!("changed-after-freeze");
        }
        let input: Manifest = serde_json::from_value(modified).unwrap();
        assert!(bind_partition(&input, &frozen).is_err());
    }
}
struct AmbiguousEngine {
    calls: Arc<AtomicUsize>,
}
impl EmbeddingEngine for AmbiguousEngine {
    fn extract(&mut self, _: &[f32], _: u32) -> EvalResult<Vec<f32>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let mut value = vec![0.; 512];
        value[0] = 1.;
        Ok(value)
    }
}
#[test]
fn failed_calibration_recognition_does_not_peek_at_held_out_or_claim_an_available_policy() {
    let (t, value) = fixture();
    let path = write_manifest(t.path(), &value);
    let output = t.path().join("unavailable");
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    run_with(&path, &output, EvaluationMode::Full, |_, _| {
        Ok(Box::new(AmbiguousEngine { calls: counter }))
    })
    .unwrap();
    let report: Value =
        serde_json::from_slice(&fs::read(output.join("report.json")).unwrap()).unwrap();
    assert_eq!(
        report["status"],
        "unavailable_calibration_recognition_below_target"
    );
    assert_eq!(report["calibration"]["metrics"]["recognition_rate"], 0.0);
    assert!(report["held_out"].is_null());
    assert!(report["held_out_trials"].as_array().unwrap().is_empty());
    assert!(!output.join("frozen-research-policy.json").exists());
    assert_eq!(calls.load(Ordering::SeqCst), 50 * 4 + 51 * 3);
    assert_eq!(report["production_eligible"], false);
}

#[test]
fn failed_incomplete_cleanup_cannot_publish_a_complete_result() {
    let (t, value) = fixture();
    let path = write_manifest(t.path(), &value);
    let output = t.path().join("publication-error");
    let calls = Arc::new(AtomicUsize::new(0));
    let result = run_with(&path, &output, EvaluationMode::Full, |_, _| {
        fs::remove_file(output.join("INCOMPLETE")).unwrap();
        fs::create_dir(output.join("INCOMPLETE")).unwrap();
        Ok(Box::new(MockEngine { calls }))
    });
    assert!(result.is_err());
    assert!(!output.join("COMPLETE").exists());
    assert!(output.join("INCOMPLETE").exists());
    assert!(output.join("error.json").exists());
}

#[test]
fn stronger_cross_project_known_probes_are_allowed_within_a_disjoint_partition() {
    let (t, mut value) = fixture();
    value["partitions"][0]["probes"][0]["project_id"] = json!("calibration-independent-project");
    value["partitions"][0]["probes"][0]["meeting_id"] = json!("calibration-independent-meeting");
    let path = write_manifest(t.path(), &value);
    assert!(validate(&path).is_ok());
    // The stronger within-partition evidence must not relax disjoint held-out
    // projects or permit the same source meeting to serve both sample roles.
    value["partitions"][1]["probes"][0]["project_id"] = json!("calibration-independent-project");
    value["partitions"][1]["probes"][0]["meeting_id"] = json!("held-out-independent-meeting");
    let path = write_manifest(t.path(), &value);
    assert!(matches!(validate(&path), Err(error) if error.contains("project-disjoint")));
    value["partitions"][1]["probes"][0]["project_id"] = json!("held_out-project");
    value["partitions"][0]["probes"][0]["project_id"] = json!("calibration-project");
    value["partitions"][0]["probes"][0]["meeting_id"] =
        value["partitions"][0]["enrollments"][0]["meeting_id"].clone();
    let path = write_manifest(t.path(), &value);
    assert!(matches!(validate(&path), Err(error) if error.contains("different meeting")));
}
struct DimensionEngine {
    calls: Arc<AtomicUsize>,
    dimensions: usize,
}
impl EmbeddingEngine for DimensionEngine {
    fn extract(&mut self, audio: &[f32], _: u32) -> EvalResult<Vec<f32>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let index = (audio[0] * 32768.0).round() as usize - 1;
        let mut value = vec![0.; self.dimensions];
        value[index] = 1.;
        Ok(value)
    }
}
#[test]
fn calibration_only_never_extracts_held_out_even_when_selected_profile_passes_calibration() {
    for profile in [
        VoiceEmbeddingProfile::Eres2NetV1,
        VoiceEmbeddingProfile::WeSpeakerResNet34V1,
        VoiceEmbeddingProfile::WeSpeakerResNet34ReferenceFrontendV1,
    ] {
        let (t, mut value) = fixture();
        value["embedding_profile"] = json!(profile.id());
        value["expected_embedding_contract_sha256"] = json!(profile.contract_sha256());
        let path = write_manifest(t.path(), &value);
        let output = t.path().join("calibration-only");
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        run_with(
            &path,
            &output,
            EvaluationMode::CalibrationOnly,
            |_, selected| {
                assert_eq!(selected, profile);
                Ok(Box::new(DimensionEngine {
                    calls: count,
                    dimensions: selected.dimensions(),
                }))
            },
        )
        .unwrap();
        let report: Value =
            serde_json::from_slice(&fs::read(output.join("report.json")).unwrap()).unwrap();
        assert_eq!(report["selected_mode"], "calibration_only");
        assert_eq!(report["status"], "calibration_only_held_out_not_evaluated");
        assert_eq!(report["calibration_policy_status"], "observed_targets_met");
        assert_eq!(report["calibration"]["metrics"]["recognition_rate"], 1.0);
        assert!(report["held_out"].is_null());
        assert!(report["held_out_trials"].as_array().unwrap().is_empty());
        assert!(report["frozen_research_policy_sha256"].is_null());
        assert!(!output.join("frozen-research-policy.json").exists());
        assert_eq!(report["embedding_profile"], profile.id());
        assert_eq!(
            report["limits"]["embedding_dimensions"],
            profile.dimensions()
        );
        assert_eq!(
            report["provenance"]["selected_profile_unavailable_embedding_version"],
            profile.unavailable_version()
        );
        assert_eq!(
            report["provenance"]["legacy_unavailable_embedding_version"],
            vt_diarize::voice_embedding_version()
        );
        assert_eq!(calls.load(Ordering::SeqCst), 50 * 4 + 51 * 3);
        assert!(output.join("COMPLETE").exists());
        assert_eq!(report["production_eligible"], false);
    }
}
#[test]
fn selected_profile_contract_mismatch_fails_before_engine_and_wrong_output_dimensions_cannot_publish()
 {
    let (t, mut value) = fixture();
    value["embedding_profile"] = json!("wespeaker-resnet34-v1");
    let created = AtomicUsize::new(0);
    let result = run_with(
        &write_manifest(t.path(), &value),
        &t.path().join("wrong-profile-version"),
        EvaluationMode::CalibrationOnly,
        |_, _| {
            created.fetch_add(1, Ordering::SeqCst);
            Err("must not construct".into())
        },
    );
    assert!(result.err().unwrap().contains("embedding contract differs"));
    assert_eq!(created.load(Ordering::SeqCst), 0);
    value["expected_embedding_contract_sha256"] =
        json!(VoiceEmbeddingProfile::WeSpeakerResNet34V1.contract_sha256());
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    let output = t.path().join("wrong-profile-dimensions");
    let result = run_with(
        &write_manifest(t.path(), &value),
        &output,
        EvaluationMode::CalibrationOnly,
        |_, profile| {
            assert_eq!(profile.dimensions(), 256);
            Ok(Box::new(MockEngine { calls: counter }))
        },
    );
    assert!(result.err().unwrap().contains("dimension/geometry"));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(output.join("INCOMPLETE").exists());
    assert!(!output.join("COMPLETE").exists());
}
