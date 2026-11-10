use super::*;
use crate::cluster::{Clusters, normalize};
use std::collections::BTreeSet;
fn constraint(kind: ConstraintKind) -> PairConstraint {
    PairConstraint {
        left_turn_id: "mic:turn-1".into(),
        right_turn_id: "mic:turn-2".into(),
        kind,
        provenance: "synthetic-user-correction".into(),
    }
}
#[test]
fn no_model_is_enabled_or_downloaded_by_construction() {
    let temp = tempfile::TempDir::new().unwrap();
    let manager = ModelManager::new(temp.path().into());
    assert!(!manager.installed());
    assert!(Diarizer::new(&manager.model_dir(), DiarizerConfig::default()).is_err());
    assert!(std::fs::read_dir(temp.path()).unwrap().next().is_none());
}
#[test]
fn normalized_embeddings_reject_bad_native_values() {
    assert!(normalize(vec![f32::NAN, 1.]).is_err());
    assert!(normalize(vec![0., 0.]).is_err());
    assert_eq!(normalize(vec![3., 4.]).unwrap(), vec![0.6, 0.8]);
}
#[test]
fn same_voice_accumulates_three_seconds_before_visibility() {
    let mut c = Clusters::new("mic", &[]);
    let (a, _, visible) = c.assign("a", vec![1., 0.], 1500).unwrap().unwrap();
    assert!(!visible);
    let (b, _, visible) = c.assign("b", vec![1., 0.], 1500).unwrap().unwrap();
    assert_eq!(a, b);
    assert!(visible);
}
#[test]
fn track_namespaces_and_capacity_are_bounded() {
    let mut mic = Clusters::new("mic", &[]);
    let mut system = Clusters::new("system", &[]);
    assert_ne!(
        mic.assign("a", vec![1., 0.], 1500).unwrap().unwrap().0,
        system.assign("b", vec![1., 0.], 1500).unwrap().unwrap().0
    );
    let mut c = Clusters::new("mic", &[]);
    for n in 0..64 {
        let mut basis = vec![0.; 65];
        basis[n] = 1.;
        c.assign(&format!("t{n}"), basis, 1000).unwrap().unwrap();
    }
    let mut last = vec![0.; 65];
    last[64] = 1.;
    assert!(matches!(
        c.assign("overflow", last, 1000),
        Err(DiarizeError::Capacity(CapacityLimit::Clusters))
    ));
}
#[test]
fn second_best_margin_does_not_silently_assign_ambiguous_voice() {
    // An ambiguous window is never given an existing speaker. Equidistant
    // with best ~0.707 (>= MERGE): left unattributed. Equidistant with best
    // ~0.65 (in [ASSIGN, MERGE)): a new provisional cluster, still not a or b.
    let mut c = Clusters::new("mic", &[]);
    let a = c.assign("a", vec![1., 0., 0.], 3000).unwrap().unwrap().0;
    let b = c.assign("b", vec![0., 1., 0.], 3000).unwrap().unwrap().0;
    let high = normalize(vec![1., 1., 0.]).unwrap();
    assert!(c.assign("high", high, 1500).unwrap().is_none());
    assert!(c.speaker_for("high").is_none());
    let low = normalize(vec![0.65, 0.65, 0.394]).unwrap();
    let provisional = c.assign("low", low, 1500).unwrap().unwrap();
    assert_ne!(provisional.0, a);
    assert_ne!(provisional.0, b);
    assert!(!provisional.2);
}
#[test]
fn cannot_link_prevents_identical_embeddings_from_merging() {
    let mut c = Clusters::new("mic", &[constraint(ConstraintKind::CannotLink)]);
    let a = c
        .assign("mic:turn-1", vec![1., 0.], 3000)
        .unwrap()
        .unwrap()
        .0;
    let b = c
        .assign("mic:turn-2", vec![1., 0.], 3000)
        .unwrap()
        .unwrap()
        .0;
    assert_ne!(a, b);
    assert!(c.refine(Some(1)).is_empty());
    c.check_constraints(&BTreeSet::from(["mic:turn-1".into(), "mic:turn-2".into()]))
        .unwrap();
}
#[test]
fn must_link_wins_over_opposite_acoustic_embeddings() {
    let mut c = Clusters::new("mic", &[constraint(ConstraintKind::MustLink)]);
    let a = c
        .assign("mic:turn-1", vec![1., 0.], 1500)
        .unwrap()
        .unwrap()
        .0;
    let b = c
        .assign("mic:turn-2", vec![-1., 0.], 1500)
        .unwrap()
        .unwrap()
        .0;
    assert_eq!(a, b);
}
#[test]
fn contradictory_constraints_fail_instead_of_guessing() {
    let mut c = Clusters::new(
        "mic",
        &[
            constraint(ConstraintKind::MustLink),
            constraint(ConstraintKind::CannotLink),
        ],
    );
    c.assign("mic:turn-1", vec![1., 0.], 1500).unwrap().unwrap();
    assert!(matches!(
        c.assign("mic:turn-2", vec![1., 0.], 1500),
        Err(DiarizeError::Constraint)
    ));
}
#[test]
fn agglomeration_uses_silhouette_without_erasing_all_speaker_separation() {
    // c opens far from a (0.5 < ASSIGN); a then drifts toward c through two
    // clear assignments, so refinement finds a >= MERGE pair to merge.
    let mut c = Clusters::new("mic", &[]);
    let deg = |d: f32| vec![d.to_radians().cos(), 0., d.to_radians().sin()];
    c.assign("a1", deg(0.), 3000).unwrap().unwrap();
    c.assign("c", deg(60.), 1500).unwrap().unwrap();
    c.assign("b", vec![0., 1., 0.], 3000).unwrap().unwrap();
    c.assign("a2", deg(20.), 3000).unwrap().unwrap();
    c.assign("a3", deg(25.), 3000).unwrap().unwrap();
    assert_eq!(c.speakers("mic").len(), 3);
    let merges = c.refine(Some(2));
    assert!(!merges.is_empty());
    assert_eq!(c.speakers("mic").len(), 2);
}
fn turn(start: u64, end: u64, label: &str) -> SpeakerTurn {
    SpeakerTurn {
        id: format!("t{start}"),
        track: "mic".into(),
        start_ms: start,
        end_ms: end,
        speaker_ids: vec![label.into()],
        label: label.into(),
        provenance: "synthetic".into(),
        score: None,
        overlap: false,
        uncertain: false,
        provisional: false,
    }
}
#[test]
fn independent_turns_map_words_by_midpoint_with_half_open_boundaries() {
    let turns = [turn(0, 500, "one"), turn(500, 1500, "two")];
    assert_eq!(turns_at_midpoint(&turns, "mic", 100, 300)[0].label, "one");
    assert_eq!(turns_at_midpoint(&turns, "mic", 400, 600)[0].label, "two");
    assert!(turns_at_midpoint(&turns, "system", 400, 600).is_empty());
}
#[test]
fn setup_requires_explicit_source_notice_acknowledgement() {
    let temp = tempfile::TempDir::new().unwrap();
    let manager = ModelManager::new(temp.path().into());
    assert!(matches!(
        manager.install(
            |_, _| {},
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            false
        ),
        Err(DiarizeError::Terms)
    ));
    assert!(std::fs::read_dir(temp.path()).unwrap().next().is_none());
}
#[test]
#[ignore = "requires VT_DIARIZE_MODEL_DIR verified models and VT_DIARIZE_FIXTURE_WAV public test fixture; never downloads"]
fn real_public_fixture_has_audio_derived_turns_and_separate_track_modes() {
    use vt_core::TrackKind;
    let model = std::env::var_os("VT_DIARIZE_MODEL_DIR").unwrap();
    let file = std::env::var("VT_DIARIZE_FIXTURE_WAV").unwrap();
    let wave = sherpa_onnx::Wave::read(&file).unwrap();
    let rate = wave.sample_rate() as u32;
    let mut diarizer = Diarizer::new(
        std::path::Path::new(&model),
        DiarizerConfig {
            mode: MeetingMode::InPerson,
            ..Default::default()
        },
    )
    .unwrap();
    let source_duration = wave.samples().len() as u64 * 1000 / u64::from(rate);
    let mut updates = Vec::new();
    for (i, chunk) in wave.samples().chunks(rate as usize).enumerate() {
        updates.extend(
            diarizer
                .accept(TrackKind::Mic, chunk, rate, 1000 + i as u64 * 1000)
                .unwrap(),
        );
    }
    let result = diarizer.finish().unwrap();
    assert!(!updates.is_empty());
    assert!(!result.turns.is_empty());
    assert!(
        result.speakers.len() >= 2,
        "public four-speaker fixture did not separate any voices"
    );
    assert!(!result.calibrated);
    println!(
        "Public model smoke: {} turns, {} speaker clusters, {} overlap turns; all timestamps audio-derived, calibration pending.",
        result.turns.len(),
        result.speakers.len(),
        result.turns.iter().filter(|t| t.overlap).count()
    );
    assert!(result.turns.iter().all(|t| t.start_ms >= 1000
        && t.end_ms <= 1000 + source_duration
        && t.start_ms < t.end_ms));
    assert!(
        result
            .turns
            .iter()
            .filter(|t| t.end_ms - t.start_ms < 1000)
            .all(|t| t.speaker_ids.is_empty())
    );
    assert!(result.turns.iter().any(|t| !t.speaker_ids.is_empty()));
    assert!(
        diarizer
            .accept(TrackKind::Mic, &[0.; 160], 16000, 0)
            .is_err()
    );
    let mut remote =
        Diarizer::new(std::path::Path::new(&model), DiarizerConfig::default()).unwrap();
    for (i, chunk) in wave.samples().chunks(rate as usize).take(8).enumerate() {
        remote
            .accept(TrackKind::Mic, chunk, rate, i as u64 * 1000)
            .unwrap();
    }
    let remote = remote.finish().unwrap();
    assert!(remote.turns.iter().any(|t| t.label == "Me"));
    assert!(
        remote
            .turns
            .iter()
            .filter(|t| !t.speaker_ids.is_empty())
            .all(|t| t.speaker_ids == ["mic:me"] && t.provenance == "channel_prior")
    );
}

#[test]
fn temporal_constraints_follow_time_and_track_instead_of_serial_turn_ids() {
    let turns = [turn(0, 1500, "voice-a"), turn(1500, 3000, "voice-b")];
    let constraint = TemporalConstraint {
        left: Anchor {
            track: "mic".into(),
            start_ms: 0,
            end_ms: 1400,
        },
        right: Anchor {
            track: "mic".into(),
            start_ms: 1600,
            end_ms: 2900,
        },
        kind: ConstraintKind::CannotLink,
        provenance: "user-split".into(),
    };
    let first = resolve_temporal_constraints(&turns, std::slice::from_ref(&constraint)).unwrap();
    assert_eq!(first[0].left_turn_id, "t0");
    assert_eq!(first[0].right_turn_id, "t1500");
    let mut rerun = turns.clone();
    rerun[0].id = "different-run-a".into();
    rerun[1].id = "different-run-b".into();
    let second = resolve_temporal_constraints(&rerun, &[constraint]).unwrap();
    assert_eq!(second[0].left_turn_id, "different-run-a");
    assert_eq!(second[0].right_turn_id, "different-run-b");
}
#[test]
fn tied_missing_and_cross_track_temporal_anchors_require_review() {
    let turns = [turn(0, 1000, "a"), turn(1000, 2000, "b")];
    let mut c = TemporalConstraint {
        left: Anchor {
            track: "mic".into(),
            start_ms: 0,
            end_ms: 2000,
        },
        right: Anchor {
            track: "mic".into(),
            start_ms: 1100,
            end_ms: 1900,
        },
        kind: ConstraintKind::CannotLink,
        provenance: "user".into(),
    };
    assert!(matches!(
        resolve_temporal_constraints(&turns, &[c.clone()]),
        Err(DiarizeError::Constraint)
    ));
    c.left.start_ms = 3000;
    c.left.end_ms = 4000;
    assert!(resolve_temporal_constraints(&turns, &[c.clone()]).is_err());
    c.left.track = "system".into();
    assert!(resolve_temporal_constraints(&turns, &[c]).is_err());
}
#[test]
fn must_link_can_join_multiple_existing_acoustic_clusters() {
    let constraints = [
        PairConstraint {
            left_turn_id: "c".into(),
            right_turn_id: "a".into(),
            kind: ConstraintKind::MustLink,
            provenance: "user".into(),
        },
        PairConstraint {
            left_turn_id: "c".into(),
            right_turn_id: "b".into(),
            kind: ConstraintKind::MustLink,
            provenance: "user".into(),
        },
    ];
    let mut clusters = Clusters::new("mic", &constraints);
    clusters.assign("a", vec![1., 0.], 3000).unwrap().unwrap();
    clusters.assign("b", vec![0., 1.], 3000).unwrap().unwrap();
    clusters
        .assign("c", normalize(vec![1., 1.]).unwrap(), 3000)
        .unwrap()
        .unwrap();
    assert_eq!(clusters.speaker_for("a"), clusters.speaker_for("b"));
    assert_eq!(clusters.speaker_for("b"), clusters.speaker_for("c"));
}

#[test]
fn embedding_version_hash_covers_actual_preprocessing_and_calibration_constants() {
    use sha2::{Digest, Sha256};
    let descriptor = format!(
        "weights_sha256=1a331345f04805badbb495c775a6ddffcdd1a732567d5ec8b3d5749e3c7a5e4b;sherpa-onnx=2.0.1;mono=16000;normalize_samples=1;feature_normalize_type=global-mean;post=l2;calibration=experimental-v1;assign={:.2};merge={:.2};margin={:.2}",
        crate::models::ASSIGN,
        crate::models::MERGE,
        crate::models::MARGIN
    );
    assert_eq!(
        format!("{:x}", Sha256::digest(descriptor.as_bytes())),
        EMBEDDING_VERSION
    );
}

#[test]
#[ignore = "requires VT_DIARIZE_MODEL_DIR and public VT_DIARIZE_FIXTURE_WAV; never downloads"]
fn real_public_gaps_and_48k_resampling_preserve_track_clocks() {
    use vt_core::TrackKind;
    let model = std::env::var_os("VT_DIARIZE_MODEL_DIR").unwrap();
    let file = std::env::var("VT_DIARIZE_FIXTURE_WAV").unwrap();
    let wave = sherpa_onnx::Wave::read(&file).unwrap();
    let rate = wave.sample_rate() as u32;
    let source = &wave.samples()[..rate as usize * 8];
    let high = sherpa_onnx::LinearResampler::create(rate as i32, 48000)
        .unwrap()
        .resample(source, true);
    let mut d = Diarizer::new(
        std::path::Path::new(&model),
        DiarizerConfig {
            mode: MeetingMode::InPerson,
            ..Default::default()
        },
    )
    .unwrap();
    for (track, origin) in [
        (TrackKind::Mic, 1000),
        (TrackKind::Mic, 60000),
        (TrackKind::System, 90000),
    ] {
        for (i, packet) in high.chunks(48000).enumerate() {
            d.accept(track, packet, 48000, origin + i as u64 * 1000)
                .unwrap();
        }
        d.reset_track(track).unwrap();
    }
    let result = d.finish().unwrap();
    for (track, origin) in [("mic", 1000), ("mic", 60000), ("system", 90000)] {
        let turns = result
            .turns
            .iter()
            .filter(|t| t.track == track && t.start_ms >= origin && t.start_ms < origin + 8000)
            .collect::<Vec<_>>();
        assert!(!turns.is_empty());
        assert!(
            turns
                .iter()
                .all(|t| t.end_ms <= origin + 8000 && t.start_ms < t.end_ms)
        );
    }
    assert!(result.turns.iter().all(|t| {
        t.speaker_ids
            .iter()
            .all(|id| id.starts_with(&format!("{}:", t.track)))
    }));
    assert_eq!(
        result
            .turns
            .iter()
            .map(|t| &t.id)
            .collect::<BTreeSet<_>>()
            .len(),
        result.turns.len()
    );
    let mut invalid =
        Diarizer::new(std::path::Path::new(&model), DiarizerConfig::default()).unwrap();
    invalid
        .accept(TrackKind::Mic, &[0.; 160], 16000, 0)
        .unwrap();
    assert!(
        invalid
            .accept(TrackKind::Mic, &[f32::NAN], 16000, 10)
            .is_err()
    );
    for _ in 0..100 {
        assert!(
            invalid
                .accept(TrackKind::Mic, &[0.; 32000], 16000, 10)
                .is_err()
        );
    }
    assert!(invalid.finish().is_err());
}

fn final_turn(id: &str, start_ms: u64) -> SpeakerTurn {
    SpeakerTurn {
        id: id.into(),
        track: "mic".into(),
        start_ms,
        end_ms: start_ms + 3000,
        speaker_ids: vec!["old-local-id".into()],
        label: "Old live label".into(),
        provenance: "live-cluster".into(),
        score: Some(1.),
        overlap: false,
        uncertain: false,
        provisional: false,
    }
}
fn final_records(turns: &[SpeakerTurn]) -> Vec<crate::finalize::Record> {
    turns
        .iter()
        .map(|t| crate::finalize::Record {
            turn: t.id.clone(),
            embedding: vec![1., 0.],
            duration: t.end_ms - t.start_ms,
        })
        .collect()
}
fn anchored(
    left: (u64, u64),
    right: (u64, u64),
    kind: ConstraintKind,
    provenance: &str,
) -> TemporalConstraint {
    TemporalConstraint {
        left: Anchor {
            track: "mic".into(),
            start_ms: left.0,
            end_ms: left.1,
        },
        right: Anchor {
            track: "mic".into(),
            start_ms: right.0,
            end_ms: right.1,
        },
        kind,
        provenance: provenance.into(),
    }
}
#[test]
fn unresolved_constraint_preserves_turns_and_only_masks_affected_speech() {
    let turns = vec![
        final_turn("mic:a", 0),
        final_turn("mic:b", 4000),
        final_turn("mic:c", 8000),
    ];
    let records = final_records(&turns);
    let config = DiarizerConfig {
        mode: MeetingMode::InPerson,
        temporal_constraints: vec![anchored(
            (30000, 33000),
            (0, 3000),
            ConstraintKind::CannotLink,
            "user:missing",
        )],
        ..Default::default()
    };
    let result = crate::finalize::finish(turns, &records, &config, true, &[]).unwrap();
    assert_eq!(result.turns.len(), 3);
    assert_eq!(result.constraint_reviews.len(), 1);
    assert_eq!(result.constraint_reviews[0].provenance, "user:missing");
    assert!(result.turns[0].speaker_ids.is_empty());
    assert!(result.turns[0].uncertain);
    assert_eq!(result.turns[0].label, "Unknown");
    assert!(result.turns[1..].iter().all(|t| !t.speaker_ids.is_empty()));
    assert!(result.warnings.iter().any(|w| w.contains("need review")));
}
#[test]
fn tied_undercovered_and_overlap_anchors_become_reviews_instead_of_job_errors() {
    for (left, overlap) in [((0, 7000), false), ((0, 20000), false), ((0, 3000), true)] {
        let mut turns = vec![
            final_turn("mic:a", 0),
            final_turn("mic:b", 4000),
            final_turn("mic:c", 8000),
            final_turn("mic:d", 40000),
        ];
        turns[0].overlap = overlap;
        let records = final_records(&turns);
        let config = DiarizerConfig {
            mode: MeetingMode::InPerson,
            temporal_constraints: vec![anchored(
                left,
                (8000, 11000),
                ConstraintKind::CannotLink,
                "user:ambiguous",
            )],
            ..Default::default()
        };
        let result = crate::finalize::finish(turns, &records, &config, true, &[]).unwrap();
        assert_eq!(result.turns.len(), 4);
        assert_eq!(result.constraint_reviews.len(), 1);
        assert!(result.turns[0].speaker_ids.is_empty());
        assert!(result.turns[2].speaker_ids.is_empty());
        assert!(!result.turns[3].speaker_ids.is_empty());
    }
}
#[test]
fn contradictory_component_is_masked_but_unrelated_valid_constraint_still_applies() {
    let turns = vec![
        final_turn("mic:a", 0),
        final_turn("mic:b", 4000),
        final_turn("mic:c", 8000),
        final_turn("mic:d", 12000),
    ];
    let records = final_records(&turns);
    let pair = |a: &str, b: &str, kind, provenance: &str| PairConstraint {
        left_turn_id: a.into(),
        right_turn_id: b.into(),
        kind,
        provenance: provenance.into(),
    };
    let config = DiarizerConfig {
        mode: MeetingMode::InPerson,
        constraints: vec![
            pair("mic:a", "mic:b", ConstraintKind::MustLink, "user:same"),
            pair(
                "mic:a",
                "mic:b",
                ConstraintKind::CannotLink,
                "user:different",
            ),
            pair("mic:c", "mic:d", ConstraintKind::CannotLink, "user:valid"),
        ],
        ..Default::default()
    };
    let result = crate::finalize::finish(turns, &records, &config, true, &[]).unwrap();
    assert_eq!(result.constraint_reviews.len(), 2);
    assert!(
        result.turns[..2]
            .iter()
            .all(|t| t.speaker_ids.is_empty() && t.uncertain)
    );
    assert!(!result.turns[2].speaker_ids.is_empty());
    assert!(!result.turns[3].speaker_ids.is_empty());
    assert_ne!(result.turns[2].speaker_ids, result.turns[3].speaker_ids);
}
#[test]
fn transitive_constraint_closure_prevents_greedy_seed_from_conflicting_later() {
    let turns = vec![
        final_turn("mic:a", 0),
        final_turn("mic:b", 4000),
        final_turn("mic:c", 8000),
    ];
    let records = final_records(&turns);
    let config = DiarizerConfig {
        mode: MeetingMode::InPerson,
        constraints: vec![
            PairConstraint {
                left_turn_id: "mic:a".into(),
                right_turn_id: "mic:c".into(),
                kind: ConstraintKind::MustLink,
                provenance: "user:same".into(),
            },
            PairConstraint {
                left_turn_id: "mic:b".into(),
                right_turn_id: "mic:c".into(),
                kind: ConstraintKind::CannotLink,
                provenance: "user:different".into(),
            },
        ],
        ..Default::default()
    };
    let result = crate::finalize::finish(turns, &records, &config, true, &[]).unwrap();
    assert!(result.constraint_reviews.is_empty());
    assert_eq!(result.turns[0].speaker_ids, result.turns[2].speaker_ids);
    assert_ne!(result.turns[0].speaker_ids, result.turns[1].speaker_ids);
}
#[test]
fn late_remote_mic_cannot_link_reviews_unknown_while_default_me_prior_is_unchanged() {
    let mut turns = vec![
        final_turn("mic:a", 0),
        final_turn("mic:b", 4000),
        final_turn("mic:c", 8000),
    ];
    for t in &mut turns {
        t.speaker_ids = vec!["mic:me".into()];
        t.label = "Me".into();
        t.provenance = "channel_prior".into();
    }
    let default =
        crate::finalize::finish(turns.clone(), &[], &DiarizerConfig::default(), false, &[])
            .unwrap();
    assert!(default.constraint_reviews.is_empty());
    assert!(default.turns.iter().all(|t| t.speaker_ids == ["mic:me"]));
    let config = DiarizerConfig {
        temporal_constraints: vec![anchored(
            (0, 3000),
            (4000, 7000),
            ConstraintKind::CannotLink,
            "user:late",
        )],
        ..Default::default()
    };
    assert!(crate::finalize::mic_cannot_link(&config));
    let result = crate::finalize::finish(turns, &[], &config, false, &[]).unwrap();
    assert_eq!(result.constraint_reviews.len(), 1);
    assert!(result.constraint_reviews[0].reason.contains("Refine again"));
    assert!(
        result.turns[..2]
            .iter()
            .all(|t| t.speaker_ids.is_empty() && t.uncertain)
    );
    assert_eq!(result.turns[2].speaker_ids, ["mic:me"]);
}
#[test]
fn missing_pair_ids_are_reviewed_and_conservatively_mask_their_track() {
    let turns = vec![final_turn("mic:a", 0), final_turn("mic:b", 4000)];
    let records = final_records(&turns);
    let config = DiarizerConfig {
        mode: MeetingMode::InPerson,
        constraints: vec![PairConstraint {
            left_turn_id: "mic:gone".into(),
            right_turn_id: "mic:missing".into(),
            kind: ConstraintKind::CannotLink,
            provenance: "user:old-ids".into(),
        }],
        ..Default::default()
    };
    let result = crate::finalize::finish(turns, &records, &config, true, &[]).unwrap();
    assert_eq!(result.constraint_reviews.len(), 1);
    assert!(result.turns.iter().all(|t| t.speaker_ids.is_empty()));
}

#[test]
#[ignore = "requires verified VT_DIARIZE_MODEL_DIR and public VT_DIARIZE_FIXTURE_WAV; never downloads"]
fn real_public_remote_mic_user_cannot_link_overrides_prior_and_unresolved_does_not_abort() {
    use vt_core::TrackKind;
    let model = std::env::var_os("VT_DIARIZE_MODEL_DIR").unwrap();
    let file = std::env::var("VT_DIARIZE_FIXTURE_WAV").unwrap();
    let wave = sherpa_onnx::Wave::read(&file).unwrap();
    let rate = wave.sample_rate() as u32;
    // Refined owns 2.5s prefixes only after 10s of source context; supply
    // enough real source for two clean owned turns instead of a single short flush.
    let audio = &wave.samples()[..rate as usize * 16];
    let feed = |d: &mut Diarizer| {
        for (i, packet) in audio.chunks(rate as usize).enumerate() {
            d.accept(TrackKind::Mic, packet, rate, i as u64 * 1000)
                .unwrap();
        }
    };
    let mut baseline =
        Diarizer::new(std::path::Path::new(&model), DiarizerConfig::default()).unwrap();
    feed(&mut baseline);
    let baseline = baseline.finish().unwrap();
    let clean = baseline
        .turns
        .iter()
        .filter(|t| !t.overlap && t.end_ms - t.start_ms >= 1000 && t.speaker_ids == ["mic:me"])
        .take(2)
        .collect::<Vec<_>>();
    assert_eq!(clean.len(), 2);
    let constraint = anchored(
        (clean[0].start_ms, clean[0].end_ms),
        (clean[1].start_ms, clean[1].end_ms),
        ConstraintKind::CannotLink,
        "user:public-correction",
    );
    let config = DiarizerConfig {
        temporal_constraints: vec![constraint.clone()],
        ..Default::default()
    };
    let mut corrected = Diarizer::new(std::path::Path::new(&model), config).unwrap();
    feed(&mut corrected);
    let corrected = corrected.finish().unwrap();
    assert!(corrected.constraint_reviews.is_empty());
    let pair =
        resolve_temporal_constraints(&corrected.turns, std::slice::from_ref(&constraint)).unwrap();
    let a = corrected
        .turns
        .iter()
        .find(|t| t.id == pair[0].left_turn_id)
        .unwrap();
    let b = corrected
        .turns
        .iter()
        .find(|t| t.id == pair[0].right_turn_id)
        .unwrap();
    assert!(!a.speaker_ids.is_empty());
    assert!(!b.speaker_ids.is_empty());
    assert_ne!(a.speaker_ids, b.speaker_ids);
    assert!(corrected.turns.iter().all(|t| t.speaker_ids != ["mic:me"]));
    let mut late = Diarizer::new(std::path::Path::new(&model), DiarizerConfig::default()).unwrap();
    feed(&mut late);
    late.set_temporal_constraints(vec![constraint.clone()])
        .unwrap();
    let late = late.finish().unwrap();
    assert_eq!(late.constraint_reviews.len(), 1);
    assert!(late.constraint_reviews[0].reason.contains("Refine again"));
    assert!(
        late.turns
            .iter()
            .filter(|t| [&constraint.left, &constraint.right]
                .iter()
                .any(|a| t.start_ms < a.end_ms && a.start_ms < t.end_ms))
            .all(|t| t.speaker_ids.is_empty() && t.uncertain)
    );
    let mut unresolved = constraint;
    unresolved.left.start_ms += 900000;
    unresolved.left.end_ms += 900000;
    let mut missing = Diarizer::new(
        std::path::Path::new(&model),
        DiarizerConfig {
            mode: MeetingMode::InPerson,
            temporal_constraints: vec![unresolved],
            ..Default::default()
        },
    )
    .unwrap();
    feed(&mut missing);
    let missing = missing.finish().unwrap();
    assert_eq!(missing.constraint_reviews.len(), 1);
    assert_eq!(missing.turns.len(), baseline.turns.len());
    assert!(
        missing
            .turns
            .iter()
            .any(|t| t.speaker_ids.is_empty() && t.uncertain)
    );
}

#[test]
fn defensive_replay_failure_preserves_unrelated_same_track_and_other_track_must_links() {
    let mut turns = vec![
        final_turn("mic:failed-a", 0),
        final_turn("mic:failed-b", 4000),
        final_turn("mic:independent-a", 8000),
        final_turn("mic:independent-b", 12000),
        final_turn("system:independent-a", 20000),
        final_turn("system:independent-b", 24000),
    ];
    for t in &mut turns {
        if t.id.starts_with("system:") {
            t.track = "system".into();
        }
    }
    let mut records = final_records(&turns);
    for r in &mut records {
        if r.turn.ends_with("independent-b") {
            r.embedding = vec![-1., 0.];
        }
    }
    let pair = |left: &str, right: &str, provenance: &str| PairConstraint {
        left_turn_id: left.into(),
        right_turn_id: right.into(),
        kind: ConstraintKind::MustLink,
        provenance: provenance.into(),
    };
    let config = DiarizerConfig {
        mode: MeetingMode::InPerson,
        constraints: vec![
            pair("mic:failed-a", "mic:failed-b", "user:failed-component"),
            pair("mic:independent-a", "mic:independent-b", "user:valid-mic"),
            pair(
                "system:independent-a",
                "system:independent-b",
                "user:valid-system",
            ),
        ],
        ..Default::default()
    };
    // Valid closure normally prevents this defensive error. Fault injection is
    // compiled only for tests and exercises the actual bounded recovery wrapper.
    let (result, attempts) = crate::finalize::finish_with_component_failure_for_test(
        turns,
        &records,
        &config,
        "mic:failed-a",
    )
    .unwrap();
    assert_eq!(attempts, 2);
    assert_eq!(result.constraint_reviews.len(), 1);
    assert_eq!(
        result.constraint_reviews[0].provenance,
        "user:failed-component"
    );
    let get = |id: &str| result.turns.iter().find(|t| t.id == id).unwrap();
    for id in ["mic:failed-a", "mic:failed-b"] {
        assert!(get(id).speaker_ids.is_empty());
        assert!(get(id).uncertain);
        assert_eq!(get(id).label, "Unknown");
    }
    for prefix in ["mic", "system"] {
        let a = get(&format!("{prefix}:independent-a"));
        let b = get(&format!("{prefix}:independent-b"));
        assert!(!a.speaker_ids.is_empty());
        assert_eq!(a.speaker_ids, b.speaker_ids);
        assert_eq!(a.provenance, "user");
        assert_eq!(b.provenance, "user");
    }
}

#[test]
fn dependent_correction_waits_on_original_failed_source_without_misleading_undo_target() {
    let mut turns = vec![
        final_turn("mic:affected-a", 0),
        final_turn("mic:dependent-b", 4000),
        final_turn("system:valid-a", 8000),
        final_turn("system:valid-b", 12000),
    ];
    for t in &mut turns {
        if t.id.starts_with("system:") {
            t.track = "system".into();
        }
    }
    let mut records = final_records(&turns);
    records[3].embedding = vec![-1., 0.];
    let config = DiarizerConfig {
        mode: MeetingMode::InPerson,
        temporal_constraints: vec![anchored(
            (90000, 93000),
            (0, 3000),
            ConstraintKind::CannotLink,
            "user:original-invalid",
        )],
        constraints: vec![
            PairConstraint {
                left_turn_id: "mic:affected-a".into(),
                right_turn_id: "mic:dependent-b".into(),
                kind: ConstraintKind::MustLink,
                provenance: "user:valid-dependent".into(),
            },
            PairConstraint {
                left_turn_id: "system:valid-a".into(),
                right_turn_id: "system:valid-b".into(),
                kind: ConstraintKind::MustLink,
                provenance: "user:valid-system".into(),
            },
        ],
        ..Default::default()
    };
    let result = crate::finalize::finish(turns, &records, &config, true, &[]).unwrap();
    assert_eq!(result.constraint_reviews.len(), 1);
    let review = &result.constraint_reviews[0];
    assert_eq!(review.provenance, "user:original-invalid");
    assert!(
        review
            .reason
            .contains("temporarily deferred, not marked invalid")
    );
    assert!(review.reason.contains("original correction"));
    assert!(
        review
            .anchors
            .iter()
            .any(|a| a.track == "mic" && a.start_ms == 4000 && a.end_ms == 7000)
    );
    assert!(
        result.turns[..2]
            .iter()
            .all(|t| t.speaker_ids.is_empty() && t.uncertain)
    );
    assert!(!result.turns[2].speaker_ids.is_empty());
    assert_eq!(result.turns[2].speaker_ids, result.turns[3].speaker_ids);
}

#[test]
fn segmentation_context_is_explicit_serializable_and_legacy_results_remain_unknown() {
    assert_eq!(
        Phase::Live.segmentation_context(),
        SegmentationContext {
            source_window_ms: 5000,
            ownership_stride_ms: 2500,
            native_window_ms: 10000
        }
    );
    assert_eq!(
        Phase::Refined.segmentation_context(),
        SegmentationContext {
            source_window_ms: 10000,
            ownership_stride_ms: 2500,
            native_window_ms: 10000
        }
    );
    assert_eq!(Phase::default(), Phase::Refined);
    let legacy = serde_json::json!({"turns":[],"speakers":[],"merges":[],"embedding_version":"legacy","calibrated":false,"warnings":[],"constraint_reviews":[]});
    let old: DiarizationResult = serde_json::from_value(legacy).unwrap();
    assert!(old.segmentation_context.is_none());
    let mut current = old;
    current.segmentation_context = Some(Phase::Refined.segmentation_context());
    let json = serde_json::to_value(&current).unwrap();
    assert_eq!(json["segmentation_context"]["source_window_ms"], 10000);
    assert_eq!(
        serde_json::from_value::<DiarizationResult>(json)
            .unwrap()
            .segmentation_context,
        current.segmentation_context
    );
}

#[test]
fn diagnostics_count_open_reasons_and_survive_capacity() {
    let mut c = Clusters::new("mic", &[]);
    c.assign("a", vec![1., 0., 0.], 3000).unwrap().unwrap(); // no candidate
    c.assign("b", vec![0.95, 0.31, 0.], 3000).unwrap().unwrap(); // cos ~0.95, joins
    c.assign("c", vec![0., 1., 0.], 3000).unwrap().unwrap(); // best below ASSIGN
    let d = c.diagnostics();
    assert_eq!(d.opened_no_candidate, 1);
    assert_eq!(d.opened_below_assign, 1);
    assert_eq!(d.assigned_existing, 1);
    assert_eq!((d.opened_total, d.clusters, d.peak_clusters), (2, 2, 2));
    assert_eq!(d.embeddings_assigned, 3);
    assert_eq!(d.singleton_clusters, 1);
    assert_eq!(d.best_below_assign_hist.iter().sum::<u32>(), 1);
    let mut c = Clusters::new("mic", &[]);
    for n in 0..64 {
        let mut basis = vec![0.; 65];
        basis[n] = 1.;
        c.assign(&format!("t{n}"), basis, 1000).unwrap().unwrap();
    }
    let mut last = vec![0.; 65];
    last[64] = 1.;
    let err = c.assign("overflow", last, 1000).unwrap_err();
    assert_eq!(err.capacity_limit(), Some(CapacityLimit::Clusters));
    assert!(err.to_string().starts_with(
        "Diarization capacity reached; previous results must not be published as complete."
    ));
    let d = c.diagnostics();
    assert!(d.capacity_refused);
    assert_eq!((d.peak_clusters, d.singleton_clusters), (64, 64));
}
#[test]
fn diagnostics_record_margin_failures() {
    let mut c = Clusters::new("mic", &[]);
    c.assign("a", vec![1., 0., 0.], 3000).unwrap();
    c.assign("b", vec![0., 1., 0.], 3000).unwrap();
    // equidistant, best ~0.707 >= MERGE, margin 0: unattributed
    c.assign("m", normalize(vec![1., 1., 0.]).unwrap(), 3000)
        .unwrap();
    // equidistant, best ~0.65 in [ASSIGN, MERGE): opens
    c.assign("n", normalize(vec![0.65, 0.65, 0.394]).unwrap(), 3000)
        .unwrap();
    let d = c.diagnostics();
    assert_eq!(d.margin_failures, 2);
    assert_eq!(d.margin_failure_best_at_least_merge, 1);
    assert_eq!(d.ambiguous_unattributed, 1);
    assert_eq!(d.opened_margin_failure, 1);
    assert_eq!(d.margin_failure_hist[0], 1);
    assert_eq!(d.best_margin_failure_hist.iter().sum::<u32>(), 1);
    // centroid cosine of a and b is 0 -> bucket [0, 0.1)
    assert_eq!(d.margin_failure_centroid_hist[10], 2);
    assert_eq!(d.embeddings_assigned, 3);
    assert_eq!(d.clusters, 3);
}

// ---- ambiguous margin-failure rule (best >= MERGE is left unattributed) ----
const DIM: usize = 128;
struct Rng(u64);
impl Rng {
    fn uniform(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 40) as f32 / (1u64 << 24) as f32 * 2. - 1.
    }
    fn noisy(&mut self, center: &[f32], r: f32) -> Vec<f32> {
        let e = (0..center.len())
            .map(|_| self.uniform() + self.uniform() + self.uniform())
            .collect::<Vec<_>>();
        let n = e.iter().map(|x| x * x).sum::<f32>().sqrt();
        normalize(center.iter().zip(&e).map(|(c, x)| c + x * r / n).collect()).unwrap()
    }
}
/// Two unit voice centres with the given cosine, and their exact midpoint.
fn voices(cos: f32) -> (Vec<f32>, Vec<f32>, Vec<f32>) {
    let mut a = vec![0.; DIM];
    a[0] = 1.;
    let mut b = vec![0.; DIM];
    b[0] = cos;
    b[1] = (1. - cos * cos).sqrt();
    let mid = normalize(a.iter().zip(&b).map(|(x, y)| x + y).collect()).unwrap();
    (a, b, mid)
}
/// Alternating A, B, midpoint windows as finalize records/turns on "mic:".
/// Midpoints start after five rounds: each voice speaks a little before the
/// first ambiguous window, so its centroid is not a single noisy window.
fn interleaved(
    cos: f32,
    rounds: usize,
    noise: f32,
    mid_noise: f32,
) -> Vec<crate::finalize::Record> {
    let (a, b, mid) = voices(cos);
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let mut records = Vec::new();
    for round in 0..rounds {
        for (kind, center, r) in [("a", &a, noise), ("b", &b, noise), ("m", &mid, mid_noise)] {
            if kind == "m" && round < 5 {
                continue;
            }
            records.push(crate::finalize::Record {
                turn: format!("mic:{kind}-{round}"),
                embedding: if r == 0. {
                    center.clone()
                } else {
                    rng.noisy(center, r)
                },
                duration: 1500,
            });
        }
    }
    records
}
fn turns_for(records: &[crate::finalize::Record]) -> Vec<SpeakerTurn> {
    records
        .iter()
        .enumerate()
        .map(|(i, r)| final_turn(&r.turn, i as u64 * 4000))
        .collect()
}
fn in_person(phase: Phase) -> DiarizerConfig {
    DiarizerConfig {
        mode: MeetingMode::InPerson,
        phase,
        ..Default::default()
    }
}
#[test]
fn ambiguous_window_at_or_above_merge_opens_nothing_and_leaves_centroids_unchanged() {
    let mut c = Clusters::new("mic", &[]);
    c.assign("a", vec![1., 0., 0.], 3000).unwrap().unwrap();
    c.assign("b", vec![0., 1., 0.], 3000).unwrap().unwrap();
    let before = c.speakers("mic");
    assert!(
        c.assign("m", normalize(vec![1., 1., 0.]).unwrap(), 1500)
            .unwrap()
            .is_none()
    );
    let after = c.speakers("mic");
    assert_eq!(before.len(), after.len());
    for (x, y) in before.iter().zip(&after) {
        assert_eq!((&x.id, x.speech_ms), (&y.id, y.speech_ms));
    }
    assert!(c.speaker_for("m").is_none());
    // Centroids untouched: the exact original voices still score 1.0.
    let (_, score, _) = c.assign("a2", vec![1., 0., 0.], 1000).unwrap().unwrap();
    assert!((score - 1.).abs() < 1e-6);
    let (_, score, _) = c.assign("b2", vec![0., 1., 0.], 1000).unwrap().unwrap();
    assert!((score - 1.).abs() < 1e-6);
    let d = c.diagnostics();
    assert_eq!((d.ambiguous_unattributed, d.opened_margin_failure), (1, 0));
}
#[test]
fn ambiguous_window_between_assign_and_merge_still_opens() {
    let mut c = Clusters::new("mic", &[]);
    c.assign("a", vec![1., 0., 0.], 3000).unwrap().unwrap();
    c.assign("b", vec![0., 1., 0.], 3000).unwrap().unwrap();
    let v = normalize(vec![0.65, 0.65, 0.394]).unwrap();
    let opened = c.assign("m", v, 1500).unwrap().unwrap();
    assert_eq!(c.speakers("mic").len(), 3);
    assert_eq!(c.speaker_for("m"), Some(opened.0.as_str()));
    assert_eq!(c.diagnostics().opened_margin_failure, 1);
}
#[test]
fn genuinely_new_voice_still_opens_after_many_ambiguous_windows() {
    let mut c = Clusters::new("mic", &[]);
    c.assign("a", vec![1., 0., 0.], 3000).unwrap().unwrap();
    c.assign("b", vec![0., 1., 0.], 3000).unwrap().unwrap();
    for n in 0..200 {
        let m = normalize(vec![1., 1., 0.]).unwrap();
        assert!(c.assign(&format!("m{n}"), m, 1500).unwrap().is_none());
    }
    assert_eq!(c.speakers("mic").len(), 2);
    let new = c.assign("new", vec![0., 0., 1.], 3000).unwrap().unwrap();
    assert_eq!(c.speakers("mic").len(), 3);
    assert_eq!(c.speaker_for("new"), Some(new.0.as_str()));
    // a, b and the new voice all open with best < ASSIGN (orthogonal).
    assert_eq!(c.diagnostics().opened_below_assign, 2);
    assert_eq!(c.diagnostics().opened_total, 3);
}
#[test]
fn long_noisy_two_voice_call_with_midpoints_stays_far_below_cap() {
    let records = interleaved(0.3, 150, 0.6, 0.15);
    assert!(records.len() >= 400);
    let mut c = Clusters::new("mic", &[]);
    for r in &records {
        c.assign(&r.turn, r.embedding.clone(), r.duration).unwrap();
    }
    let d = c.diagnostics();
    assert!(!d.capacity_refused);
    assert!(d.peak_clusters <= 8, "peak {}", d.peak_clusters);
    assert!(
        d.ambiguous_unattributed > 100,
        "{}",
        d.ambiguous_unattributed
    );
    c.refine(None);
    assert_eq!(c.speakers("mic").len(), 2);
    let result = crate::finalize::finish(
        turns_for(&records),
        &records,
        &in_person(Phase::Refined),
        true,
        &[],
    )
    .unwrap();
    assert_eq!(result.speakers.len(), 2);
}
#[test]
fn close_voices_keep_two_speakers_and_midpoints_surface_unknown_in_both_phases() {
    let records = interleaved(0.65, 60, 0.6, 0.);
    for phase in [Phase::Refined, Phase::Live] {
        let result =
            crate::finalize::finish(turns_for(&records), &records, &in_person(phase), true, &[])
                .unwrap();
        assert_eq!(result.speakers.len(), 2, "{phase:?}");
        let mids = result
            .turns
            .iter()
            .filter(|t| t.id.starts_with("mic:m-"))
            .collect::<Vec<_>>();
        assert_eq!(mids.len(), 55);
        for t in mids {
            assert!(t.speaker_ids.is_empty(), "{phase:?} {}", t.id);
            assert_eq!(t.label, "Unknown");
            assert!(t.uncertain && !t.provisional);
            assert!(t.score.is_none());
        }
        // Voice windows themselves are attributed and confident.
        let voiced = result
            .turns
            .iter()
            .filter(|t| !t.id.starts_with("mic:m-"))
            .filter(|t| !t.speaker_ids.is_empty() && !t.uncertain)
            .count();
        assert!(voiced >= 110, "{phase:?} {voiced}");
    }
}
#[test]
fn record_order_permutation_gives_same_refined_speaker_count() {
    let records = interleaved(0.3, 150, 0.6, 0.15);
    let turns = turns_for(&records);
    let config = in_person(Phase::Refined);
    let base = crate::finalize::finish(turns.clone(), &records, &config, true, &[]).unwrap();
    for seed in [7u64, 99, 12345] {
        let mut rng = Rng(seed);
        let mut shuffled = records
            .iter()
            .map(|r| crate::finalize::Record {
                turn: r.turn.clone(),
                embedding: r.embedding.clone(),
                duration: r.duration,
            })
            .collect::<Vec<_>>();
        for i in (1..shuffled.len()).rev() {
            let j = ((rng.uniform() + 1.) / 2. * (i + 1) as f32) as usize % (i + 1);
            shuffled.swap(i, j);
        }
        let other = crate::finalize::finish(turns.clone(), &shuffled, &config, true, &[]).unwrap();
        assert_eq!(other.speakers.len(), base.speakers.len(), "seed {seed}");
    }
    assert_eq!(base.speakers.len(), 2);
}
#[test]
fn constraint_anchors_keep_old_behaviour_when_ambiguous() {
    // A must-link anchor is never left unattributed: it opens as before so
    // its partner can join and the constraint check sees both in one cluster.
    let must = PairConstraint {
        left_turn_id: "mic:m1".into(),
        right_turn_id: "mic:m2".into(),
        kind: ConstraintKind::MustLink,
        provenance: "user".into(),
    };
    let mut c = Clusters::new("mic", &[must]);
    c.assign("mic:a", vec![1., 0., 0.], 3000).unwrap().unwrap();
    c.assign("mic:b", vec![0., 1., 0.], 3000).unwrap().unwrap();
    let mid = normalize(vec![1., 1., 0.]).unwrap();
    let m1 = c.assign("mic:m1", mid.clone(), 1500).unwrap().unwrap();
    let m2 = c.assign("mic:m2", vec![0., 0., 1.], 1500).unwrap().unwrap();
    assert_eq!(m1.0, m2.0);
    assert!(c.assign("mic:free", mid.clone(), 1500).unwrap().is_none());
    let seen = ["mic:a", "mic:b", "mic:m1", "mic:m2", "mic:free"]
        .map(String::from)
        .into_iter()
        .collect();
    c.check_constraints(&seen).unwrap();
    // Cannot-link anchor: opens as before, and stays apart from its partner.
    let cannot = PairConstraint {
        left_turn_id: "mic:a".into(),
        right_turn_id: "mic:m1".into(),
        kind: ConstraintKind::CannotLink,
        provenance: "user".into(),
    };
    let mut c = Clusters::new("mic", &[cannot]);
    let a = c.assign("mic:a", vec![1., 0., 0.], 3000).unwrap().unwrap();
    c.assign("mic:b", vec![0.2, 0.98, 0.], 3000)
        .unwrap()
        .unwrap();
    // m1 sees only b (a is forbidden): one candidate, assigned normally.
    let m1 = c.assign("mic:m1", mid, 1500).unwrap().unwrap();
    assert_ne!(m1.0, a.0);
    c.check_constraints(&["mic:a", "mic:m1"].map(String::from).into_iter().collect())
        .unwrap();
}
