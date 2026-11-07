use super::*;
fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/synthetic/manifest.json")
}
fn near(a: f64, b: f64) {
    assert!((a - b).abs() < 1e-9, "{a} != {b}")
}
#[test]
fn fictional_manifest_has_hand_computed_aggregate_metrics_and_no_acceptance_claim() {
    let report = evaluate_manifest(&fixture()).unwrap();
    assert_eq!(report.recordings.len(), 2);
    assert_eq!(report.acceptance, "not_claimed");
    near(report.diarization.as_ref().unwrap().der, 1.0 / 3.0);
    near(report.diarization.as_ref().unwrap().jer, 1.0 / 3.0);
    near(report.wer["live"].wer.unwrap(), 1.0 / 3.0);
    near(report.wer["batch"].wer.unwrap(), 0.0);
    assert_eq!(report.paired_live_batch_wer.units, 1);
    assert_eq!(report.paired_live_batch_wer.live_to_batch_wer_ratio, None);
    assert_eq!(report.bleed.as_ref().unwrap().lost_genuine_short_replies, 0);
    assert_eq!(report.measurements.partial_ms.as_ref().unwrap().p50, 450.0);
    assert_eq!(report.measurements.cpu_sample_mean_percent, Some(30.0));
    assert!(
        report
            .targets
            .iter()
            .any(|target| target.id == "m0_live_cpu_sample_mean_percent"
                && target.within_target == Some(false))
    );
    assert!(report.sources.iter().all(|file| file.sha256.len() == 64));
}
#[test]
fn speaker_galleries_are_independent_across_meetings_not_global_across_a_corpus() {
    let first = diarization::score(
        &[diarization::Turn {
            speaker: "A".into(),
            start: 0.0,
            end: 4.0,
        }],
        &[diarization::Turn {
            speaker: "X".into(),
            start: 0.0,
            end: 4.0,
        }],
        &[diarization::Span {
            start: 0.0,
            end: 4.0,
        }],
    )
    .unwrap();
    let second = diarization::score(
        &[diarization::Turn {
            speaker: "A".into(),
            start: 0.0,
            end: 4.0,
        }],
        &[diarization::Turn {
            speaker: "Y".into(),
            start: 0.0,
            end: 4.0,
        }],
        &[diarization::Span {
            start: 0.0,
            end: 4.0,
        }],
    )
    .unwrap();
    let mut aggregate = DiarizationScore::default();
    aggregate.accumulate(&first);
    aggregate.accumulate(&second);
    near(aggregate.der, 0.0);
    near(aggregate.jer, 0.0);
    assert_eq!(first.mapping["A"], "X");
    assert_eq!(second.mapping["A"], "Y");
}
#[test]
fn missing_reference_and_unpaired_data_fail_instead_of_inventing_scores() {
    let temp = tempfile::tempdir().unwrap();
    let source = fs::read_to_string(fixture()).unwrap();
    let mut manifest: serde_json::Value = serde_json::from_str(&source).unwrap();
    manifest["recordings"][0]["diarization"]["reference_rttm"] = "missing.rttm".into();
    let path = temp.path().join("manifest.json");
    fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    assert!(
        evaluate_manifest(&path)
            .err()
            .unwrap()
            .contains("required input")
    );
    let ref_bleed=serde_json::from_str(r#"{"segments":[{"id":"one","track":"mic","start_ms":0,"end_ms":1,"kind":"genuine_short_reply"}]}"#).unwrap();
    let hyp = serde_json::from_str(r#"{"annotations":[]}"#).unwrap();
    assert!(observations::bleed(ref_bleed, hyp).is_err());
}
#[test]
fn output_is_compact_atomic_and_cannot_overwrite_an_input() {
    let report = evaluate_manifest(&fixture()).unwrap();
    assert!(write_report(&report, &fixture()).is_err());
    let temp = tempfile::tempdir().unwrap();
    let output = temp.path().join("nested/report.json");
    write_report(&report, &output).unwrap();
    let text = fs::read_to_string(output).unwrap();
    assert_eq!(text.lines().count(), 1);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&text).unwrap()["acceptance"],
        "not_claimed"
    );
}
#[test]
fn absent_families_are_unavailable_and_latency_requires_reference_clocks() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("r.txt"), "A B").unwrap();
    fs::write(temp.path().join("h.txt"), "A").unwrap();
    let path = temp.path().join("manifest.json");
    fs::write(&path,r#"{"schema_version":1,"corpus":{"id":"missing-evidence","kind":"synthetic","description":"fictional","provenance":["test"]},"normalization":"exact_whitespace","recordings":[{"id":"m","channel":"1","transcripts":[{"id":"u","reference":"r.txt","hypotheses":{"live":"h.txt"}}]}]}"#).unwrap();
    let report = evaluate_manifest(&path).unwrap();
    assert!(report.diarization.is_none());
    assert!(report.measurements.partial_ms.is_none());
    assert!(
        report
            .targets
            .iter()
            .all(|target| target.within_target.is_none())
    );
    let bad:observations::Observations=serde_json::from_str(r#"{"clock":"monotonic_session_ms","metadata":{"machine":"fictional","source":"test"},"latency":[{"kind":"partial","reference_time_ms":100,"emitted_time_ms":50}]}"#).unwrap();
    assert!(Measurements::default().add(bad).is_err());
}
#[test]
fn gallery_of_fifty_uses_one_to_one_assignment_and_keeps_every_identity() {
    let weights = (0..50)
        .map(|row| {
            (0..50)
                .map(|column| if column == row * 7 % 50 { 5.0 } else { 0.0 })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let mapping = diarization::maximum_assignment(&weights);
    assert_eq!(mapping.len(), 50);
    for (row, column) in mapping.into_iter().enumerate() {
        assert_eq!(column, Some(row * 7 % 50));
    }
}
