use super::*;
use serde_json::{Value, json};

const RAW: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const OTHER: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
fn artifact() -> Value {
    json!({"format_version":1,"calibration_version":"fictional-test-v1","embedding_contract_sha256":RAW,"embedding_version":identification_version(RAW,"fictional-test-v1").unwrap(),"channel":"in_person","dimensions":2,"protocol_id":"fictional-disjoint-test-v1","evidence_sha256":OTHER,"min_gallery_people":1,"min_auto_speech_ms":15000,"curve":[{"max_gallery_people":50,"tau_auto":0.8,"tau_suggest":0.6,"margin":0.1},{"max_gallery_people":100,"tau_auto":0.95,"tau_suggest":0.7,"margin":0.1}]})
}
fn pin(bytes: &[u8]) -> CalibrationPin {
    // Private test construction deliberately does not create a shipped release pin.
    CalibrationPin {
        artifact_sha256: format!("{:x}", Sha256::digest(bytes)),
        embedding_contract_sha256: RAW.into(),
        calibration_version: "fictional-test-v1".into(),
        channel: ChannelType::InPerson,
        protocol_id: "fictional-disjoint-test-v1".into(),
    }
}
fn bind(value: Value) -> MatchResult<Calibration> {
    let bytes = serde_json::to_vec(&value).unwrap();
    Calibration::bind(&bytes, &pin(&bytes))
}
fn person<'a>(id: &'a str, version: &'a str, exemplars: &'a [Vec<f32>]) -> GalleryPerson<'a> {
    GalleryPerson {
        person_id: id,
        embedding_version: version,
        channel: ChannelType::InPerson,
        stale: false,
        shared_room: false,
        exemplars,
    }
}
fn query<'a>(version: &'a str, embedding: &'a [f32]) -> VoiceQuery<'a> {
    VoiceQuery {
        embedding_version: version,
        channel: ChannelType::InPerson,
        clean_speech_ms: 15_000,
        shared_room: false,
        embedding,
    }
}
fn unit(x: f32) -> Vec<f32> {
    vec![x, (1.0 - x * x).sqrt()]
}

#[test]
fn artifact_claims_and_modified_bytes_do_not_create_calibration_authority() {
    let bytes = serde_json::to_vec(&artifact()).unwrap();
    let trusted = pin(&bytes);
    let mut altered = bytes.clone();
    altered.push(b' ');
    assert!(matches!(
        Calibration::bind(&altered, &trusted),
        Err(MatchError::CalibrationAuthentication)
    ));
    let mut forged = artifact();
    forged["verified"] = json!(true);
    assert!(matches!(bind(forged), Err(MatchError::InvalidCalibration)));
    let mut crossed = pin(&bytes);
    crossed.protocol_id = "other-protocol".into();
    assert!(matches!(
        Calibration::bind(&bytes, &crossed),
        Err(MatchError::InvalidCalibration)
    ));
    let mut crossed = pin(&bytes);
    crossed.channel = ChannelType::RemoteWideband;
    assert!(matches!(
        Calibration::bind(&bytes, &crossed),
        Err(MatchError::InvalidCalibration)
    ));
}
#[test]
fn semantic_calibration_version_changes_comparison_identity_without_circular_hashing() {
    let old = identification_version(RAW, "profile-v1").unwrap();
    let new = identification_version(RAW, "profile-v2").unwrap();
    assert_ne!(old, new);
    assert_ne!(old, identification_version(OTHER, "profile-v1").unwrap());
    let mut wrong = artifact();
    wrong["embedding_version"] = json!(old);
    assert!(matches!(bind(wrong), Err(MatchError::InvalidCalibration)));
}
#[test]
fn malformed_or_uncovered_curves_cannot_lower_large_gallery_thresholds() {
    for (field, value) in [
        ("tau_auto", json!(0.7)),
        ("tau_suggest", json!(0.5)),
        ("margin", json!(0.05)),
        ("max_gallery_people", json!(50)),
    ] {
        let mut a = artifact();
        a["curve"][1][field] = value;
        assert!(
            matches!(bind(a), Err(MatchError::InvalidCalibration)),
            "{field}"
        );
    }
    for value in [0, 14_999, 120_001] {
        let mut a = artifact();
        a["min_auto_speech_ms"] = json!(value);
        assert!(bind(a).is_err());
    }
    let mut absent50 = artifact();
    absent50["curve"][0]["max_gallery_people"] = json!(49);
    assert!(bind(absent50).is_err());
    let mut unchanged = artifact();
    unchanged["curve"][1]["tau_auto"] = json!(0.8);
    assert!(bind(unchanged).is_err());
    let mut overbound = artifact();
    overbound["curve"][1]["max_gallery_people"] = json!(1025);
    assert!(bind(overbound).is_err());
    let mut nan = "".to_owned();
    let bytes = serde_json::to_string(&artifact()).unwrap();
    nan.push_str(&bytes.replace("0.95", "NaN"));
    assert!(Calibration::bind(nan.as_bytes(), &pin(nan.as_bytes())).is_err());
}
#[test]
fn missing_or_cross_version_channel_calibration_returns_no_identity_or_score() {
    let calibration = bind(artifact()).unwrap();
    let a = vec![vec![1.0, 0.0]];
    let b = vec![vec![0.0, 1.0]];
    let gallery = [
        person("a", calibration.embedding_version(), &a),
        person("b", calibration.embedding_version(), &b),
    ];
    assert_eq!(
        identify(
            &query(calibration.embedding_version(), &a[0]),
            &gallery,
            None
        )
        .unwrap(),
        MatchOutcome::Unavailable {
            reason: UnavailableReason::MissingCalibration
        }
    );
    assert_eq!(
        identify(&query(OTHER, &a[0]), &gallery, Some(&calibration)).unwrap(),
        MatchOutcome::Unavailable {
            reason: UnavailableReason::VersionOrChannelNotCovered
        }
    );
    let mut remote = query(calibration.embedding_version(), &a[0]);
    remote.channel = ChannelType::RemoteWideband;
    assert_eq!(
        identify(&remote, &gallery, Some(&calibration)).unwrap(),
        MatchOutcome::Unavailable {
            reason: UnavailableReason::VersionOrChannelNotCovered
        }
    );
}
#[test]
fn margin_compares_distinct_people_and_all_exemplars_form_a_normalized_centroid() {
    let calibration = bind(artifact()).unwrap();
    let a = vec![vec![1.0, 0.0]; 10];
    let b = vec![vec![0.0, 1.0]];
    let gallery = [
        person("a", calibration.embedding_version(), &a),
        person("b", calibration.embedding_version(), &b),
    ];
    let ranked = rank_for_evaluation(&query(calibration.embedding_version(), &a[0]), &gallery)
        .unwrap()
        .unwrap();
    assert_eq!(ranked.gallery_people, 2);
    assert_eq!(ranked.best_person_id, "a");
    assert_eq!(ranked.runner_up_person_id.as_deref(), Some("b"));
    assert!((ranked.margin.unwrap() - 1.0).abs() < 1e-12);
    assert!(
        matches!(identify(&query(calibration.embedding_version(),&a[0]),&gallery,Some(&calibration)).unwrap(),MatchOutcome::Recognized{person_id,..} if person_id=="a")
    );
    let a = vec![unit(0.8), unit(0.8)];
    let b = vec![unit(0.0)];
    let gallery = [
        person("a", calibration.embedding_version(), &a),
        person("b", calibration.embedding_version(), &b),
    ];
    let ranked = rank_for_evaluation(
        &query(calibration.embedding_version(), &[1.0, 0.0]),
        &gallery,
    )
    .unwrap()
    .unwrap();
    assert!((ranked.best_score - 0.8).abs() < 1e-6);
}
#[test]
fn duration_and_ambiguity_downgrade_high_scores_to_suggestions_without_fake_margin() {
    let calibration = bind(artifact()).unwrap();
    let a = vec![vec![1.0, 0.0]];
    let near = vec![unit(0.99)];
    let far = vec![vec![0.0, 1.0]];
    let gallery = [
        person("a", calibration.embedding_version(), &a),
        person("b", calibration.embedding_version(), &near),
    ];
    assert!(
        matches!(identify(&query(calibration.embedding_version(),&a[0]),&gallery,Some(&calibration)).unwrap(),MatchOutcome::Suggest{margin:Some(m),..} if m<0.1)
    );
    let gallery = [
        person("a", calibration.embedding_version(), &a),
        person("b", calibration.embedding_version(), &far),
    ];
    let mut short = query(calibration.embedding_version(), &a[0]);
    short.clean_speech_ms = 14_999;
    assert!(matches!(
        identify(&short, &gallery, Some(&calibration)).unwrap(),
        MatchOutcome::Suggest { .. }
    ));
    let singleton = [person("a", calibration.embedding_version(), &a)];
    assert!(matches!(
        identify(
            &query(calibration.embedding_version(), &a[0]),
            &singleton,
            Some(&calibration)
        )
        .unwrap(),
        MatchOutcome::Suggest { margin: None, .. }
    ));
    let tied = [
        person("a", calibration.embedding_version(), &a),
        person("b", calibration.embedding_version(), &a),
    ];
    assert!(matches!(
        identify(
            &query(calibration.embedding_version(), &a[0]),
            &tied,
            Some(&calibration)
        )
        .unwrap(),
        MatchOutcome::Suggest {
            margin: Some(0.0),
            ..
        }
    ));
}
#[test]
fn gallery_51_uses_raised_threshold_and_never_extrapolates_past_coverage() {
    let calibration = bind(artifact()).unwrap();
    let ids = (0..101).map(|n| format!("p{n:03}")).collect::<Vec<_>>();
    let vectors = (0..101)
        .map(|n| vec![if n == 0 { unit(0.9) } else { vec![-1.0, 0.0] }])
        .collect::<Vec<_>>();
    let gallery = ids
        .iter()
        .zip(&vectors)
        .map(|(id, vecs)| person(id, calibration.embedding_version(), vecs))
        .collect::<Vec<_>>();
    let q = query(calibration.embedding_version(), &[1.0, 0.0]);
    assert!(matches!(
        identify(&q, &gallery[..50], Some(&calibration)).unwrap(),
        MatchOutcome::Recognized { .. }
    ));
    assert!(matches!(
        identify(&q, &gallery[..51], Some(&calibration)).unwrap(),
        MatchOutcome::Suggest { .. }
    ));
    assert_eq!(
        identify(&q, &gallery, Some(&calibration)).unwrap(),
        MatchOutcome::Unavailable {
            reason: UnavailableReason::GalleryNotCovered
        }
    );
}
#[test]
fn malformed_embeddings_or_duplicate_people_fail_and_ineligible_vectors_are_never_compared() {
    let calibration = bind(artifact()).unwrap();
    let good = vec![vec![1.0, 0.0]];
    let bad = vec![vec![f32::NAN, 0.0]];
    let mut excluded = person("wrong-version", OTHER, &bad);
    excluded.shared_room = true;
    let gallery = [
        person("a", calibration.embedding_version(), &good),
        excluded,
    ];
    assert!(
        rank_for_evaluation(&query(calibration.embedding_version(), &good[0]), &gallery)
            .unwrap()
            .is_some()
    );
    for vector in [
        vec![0.0, 0.0],
        vec![2.0, 0.0],
        vec![f32::NAN, 0.0],
        vec![1.0],
    ] {
        assert!(matches!(
            rank_for_evaluation(&query(calibration.embedding_version(), &vector), &gallery),
            Err(MatchError::InvalidEmbedding)
        ));
    }
    let duplicated = [
        person("a", calibration.embedding_version(), &good),
        person("a", calibration.embedding_version(), &good),
    ];
    assert!(
        rank_for_evaluation(
            &query(calibration.embedding_version(), &good[0]),
            &duplicated
        )
        .is_err()
    );
    let opposite = vec![vec![1.0, 0.0], vec![-1.0, 0.0]];
    assert!(
        rank_for_evaluation(
            &query(calibration.embedding_version(), &good[0]),
            &[person(
                "cancelled",
                calibration.embedding_version(),
                &opposite
            )]
        )
        .is_err()
    );
    let mut stale = person("a", calibration.embedding_version(), &bad);
    stale.stale = true;
    let mut room = person("b", calibration.embedding_version(), &bad);
    room.shared_room = true;
    assert_eq!(
        rank_for_evaluation(
            &query(calibration.embedding_version(), &good[0]),
            &[stale, room]
        )
        .unwrap(),
        None
    );
}
#[test]
fn fixed_identity_evaluation_counts_wrong_known_unknown_and_abstentions_in_denominators() {
    let gallery = (0..50).map(|n| format!("person-{n}")).collect::<Vec<_>>();
    let ids = gallery.iter().map(String::as_str).collect::<Vec<_>>();
    let protocol = EvaluationProtocol {
        protocol_id: "fictional-fixed-50",
        embedding_version: RAW,
        channel: ChannelType::InPerson,
        gallery_person_ids: &ids,
    };
    let correct = MatchOutcome::Recognized {
        person_id: gallery[0].clone(),
        score: 0.9,
        margin: 0.2,
    };
    let wrong = MatchOutcome::Recognized {
        person_id: gallery[1].clone(),
        score: 0.9,
        margin: 0.2,
    };
    let unknown = MatchOutcome::Unknown {
        reason: UnknownReason::BelowSuggestionThreshold,
    };
    let suggestion = MatchOutcome::Suggest {
        person_id: gallery[0].clone(),
        score: 0.8,
        margin: None,
    };
    let unavailable = MatchOutcome::Unavailable {
        reason: UnavailableReason::MissingCalibration,
    };
    let trials = [
        IdentificationTrial {
            query_id: "known-correct",
            expected: ExpectedIdentity::Known(&gallery[0]),
            outcome: &correct,
        },
        IdentificationTrial {
            query_id: "known-wrong",
            expected: ExpectedIdentity::Known(&gallery[0]),
            outcome: &wrong,
        },
        IdentificationTrial {
            query_id: "known-suggest",
            expected: ExpectedIdentity::Known(&gallery[0]),
            outcome: &suggestion,
        },
        IdentificationTrial {
            query_id: "known-unavailable",
            expected: ExpectedIdentity::Known(&gallery[0]),
            outcome: &unavailable,
        },
        IdentificationTrial {
            query_id: "unknown-wrong",
            expected: ExpectedIdentity::Unknown,
            outcome: &correct,
        },
        IdentificationTrial {
            query_id: "unknown-abstain",
            expected: ExpectedIdentity::Unknown,
            outcome: &unknown,
        },
    ];
    let r = evaluate_identification(&protocol, &trials).unwrap();
    assert!(r.gallery_is_50);
    assert_eq!((r.known_trials, r.unknown_trials), (4, 2));
    assert_eq!(r.recognition_rate, Some(0.25));
    assert_eq!(r.false_identification_rate, Some(2.0 / 6.0));
    assert_eq!(r.unknown_false_identification_rate, Some(0.5));
    assert_eq!(
        (
            r.correct_suggestions,
            r.wrong_suggestions,
            r.unavailable_trials
        ),
        (1, 0, 1)
    );
    let duplicated = [
        IdentificationTrial {
            query_id: "same",
            expected: ExpectedIdentity::Unknown,
            outcome: &unknown,
        },
        IdentificationTrial {
            query_id: "same",
            expected: ExpectedIdentity::Unknown,
            outcome: &unknown,
        },
    ];
    assert!(evaluate_identification(&protocol, &duplicated).is_err());
    assert!(
        evaluate_identification(
            &protocol,
            &[IdentificationTrial {
                query_id: "bad-label",
                expected: ExpectedIdentity::Known("not-enrolled"),
                outcome: &unknown
            }]
        )
        .is_err()
    );
    let empty = evaluate_identification(&protocol, &[]).unwrap();
    assert_eq!(empty.recognition_rate, None);
    assert_eq!(empty.false_identification_rate, None);
}
