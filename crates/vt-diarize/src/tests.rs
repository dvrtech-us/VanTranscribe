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
    let (a, _, visible) = c.assign("a", vec![1., 0.], 1500).unwrap();
    assert!(!visible);
    let (b, _, visible) = c.assign("b", vec![1., 0.], 1500).unwrap();
    assert_eq!(a, b);
    assert!(visible);
}
#[test]
fn track_namespaces_and_capacity_are_bounded() {
    let mut mic = Clusters::new("mic", &[]);
    let mut system = Clusters::new("system", &[]);
    assert_ne!(
        mic.assign("a", vec![1., 0.], 1500).unwrap().0,
        system.assign("b", vec![1., 0.], 1500).unwrap().0
    );
    let mut c = Clusters::new("mic", &[]);
    for n in 0..64 {
        let mut basis = vec![0.; 65];
        basis[n] = 1.;
        c.assign(&format!("t{n}"), basis, 1000).unwrap();
    }
    let mut last = vec![0.; 65];
    last[64] = 1.;
    assert!(matches!(
        c.assign("overflow", last, 1000),
        Err(DiarizeError::Capacity)
    ));
}
#[test]
fn second_best_margin_does_not_silently_assign_ambiguous_voice() {
    let mut c = Clusters::new("mic", &[]);
    let a = c.assign("a", vec![1., 0.], 3000).unwrap().0;
    let b = c.assign("b", vec![0., 1.], 3000).unwrap().0;
    let v = normalize(vec![1., 1.]).unwrap();
    let provisional = c.assign("ambiguous", v, 1500).unwrap();
    assert_ne!(provisional.0, a);
    assert_ne!(provisional.0, b);
    assert!(!provisional.2);
}
#[test]
fn cannot_link_prevents_identical_embeddings_from_merging() {
    let mut c = Clusters::new("mic", &[constraint(ConstraintKind::CannotLink)]);
    let a = c.assign("mic:turn-1", vec![1., 0.], 3000).unwrap().0;
    let b = c.assign("mic:turn-2", vec![1., 0.], 3000).unwrap().0;
    assert_ne!(a, b);
    assert!(c.refine(Some(1)).is_empty());
    c.check_constraints(&BTreeSet::from(["mic:turn-1".into(), "mic:turn-2".into()]))
        .unwrap();
}
#[test]
fn must_link_wins_over_opposite_acoustic_embeddings() {
    let mut c = Clusters::new("mic", &[constraint(ConstraintKind::MustLink)]);
    let a = c.assign("mic:turn-1", vec![1., 0.], 1500).unwrap().0;
    let b = c.assign("mic:turn-2", vec![-1., 0.], 1500).unwrap().0;
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
    c.assign("mic:turn-1", vec![1., 0.], 1500).unwrap();
    assert!(matches!(
        c.assign("mic:turn-2", vec![1., 0.], 1500),
        Err(DiarizeError::Constraint)
    ));
}
#[test]
fn agglomeration_uses_silhouette_without_erasing_all_speaker_separation() {
    let mut c = Clusters::new("mic", &[]);
    c.assign("a", vec![1., 0.], 3000).unwrap();
    c.assign("b", vec![0., 1.], 3000).unwrap();
    c.assign("ambiguous", normalize(vec![1., 1.]).unwrap(), 1500)
        .unwrap();
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
    clusters.assign("a", vec![1., 0.], 3000).unwrap();
    clusters.assign("b", vec![0., 1.], 3000).unwrap();
    clusters
        .assign("c", normalize(vec![1., 1.]).unwrap(), 3000)
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
