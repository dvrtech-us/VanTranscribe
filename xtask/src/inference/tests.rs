use super::*;
#[test]
fn input_pins_fail_closed_and_references_are_never_overwritten() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("source");
    fs::write(&path, b"public synthetic source").unwrap();
    let hash = fingerprint(&path, 100).unwrap()["sha256"]
        .as_str()
        .unwrap()
        .to_string();
    let pin = PinnedFile {
        path: PathBuf::from("source"),
        sha256: hash,
    };
    assert!(pinned(dir.path(), &pin, 100).is_ok());
    fs::write(&path, b"changed").unwrap();
    assert!(pinned(dir.path(), &pin, 100).is_err());
    assert!(fingerprint(&path, 2).is_err());
}
#[test]
fn hypotheses_use_actual_assignments_and_never_promote_unknown_or_provisional_turns() {
    let turn = vt_diarize::SpeakerTurn {
        id: "mic:t".into(),
        track: "mic".into(),
        start_ms: 1234,
        end_ms: 2345,
        speaker_ids: vec!["mic:speaker-a".into(), "mic:speaker-b".into()],
        label: "Overlap".into(),
        provenance: "audio".into(),
        score: None,
        overlap: true,
        uncertain: false,
        provisional: false,
    };
    let lines = rttm("meeting", std::slice::from_ref(&turn));
    let parsed = crate::diarization::parse_hypothesis_rttm(&lines, "meeting", "1").unwrap();
    assert_eq!(parsed.len(), 2);
    assert_eq!(parsed[0].start, 1.234);
    assert_eq!(parsed[0].end, 2.345);
    for (uncertain, provisional) in [(true, false), (false, true), (true, true)] {
        let mut t = turn.clone();
        t.uncertain = uncertain;
        t.provisional = provisional;
        assert!(rttm("meeting", &[t]).is_empty());
    }
}
#[test]
fn schema_rejects_private_or_unrecognised_configuration_fields() {
    let input = r#"{"schema_version":1,"public_corpus":{"id":"a","recording_id":"b","license":"CC-BY-4.0","attribution":"Synthetic test","source_urls":["https://example.invalid"],"derivation":"Synthetic"},"audio":{"path":"a","sha256":"x"},"reference_text":{"path":"b","sha256":"x"},"reference_rttm":{"path":"c","sha256":"x"},"uem":{"path":"d","sha256":"x"},"models":{"streaming":"e","refinement":"f","diarization":"g"},"private_meeting_id":"never"}"#;
    assert!(serde_json::from_str::<Input>(input).is_err());
}

#[test]
fn diagnostic_gains_are_bounded_unique_and_never_silently_overwrite_each_other() {
    for gains in [vec![2, 4, 8, 16], vec![], vec![8]] {
        let config = diagnostics::Diagnostics {
            ungated: false,
            gains,
            control_wav: None,
        };
        assert!(config.validate().is_ok());
    }
    for gains in [vec![2, 2], vec![1], vec![32], vec![2, 4, 8, 16, 2]] {
        let config = diagnostics::Diagnostics {
            ungated: false,
            gains,
            control_wav: None,
        };
        assert!(config.validate().is_err());
    }
}
