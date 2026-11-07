//! Synthetic meeting-local speaker persistence; no models, embeddings or devices.
use super::*;
use tempfile::TempDir;
use vt_diarize::{DiarizationResult, SpeakerTurn};

fn fixture() -> TempDir {
    let root = TempDir::new().unwrap();
    initialize(root.path()).unwrap();
    let recording = vt_record::Recording::create(root.path(), "speakers").unwrap();
    create_meeting(root.path(), "speakers", "Speaker fixture", "remote").unwrap();
    recording.finish().unwrap();
    root
}
fn turn(id: &str, track: &str, start: u64, end: u64, locals: &[&str]) -> SpeakerTurn {
    SpeakerTurn {
        id: id.into(),
        track: track.into(),
        start_ms: start,
        end_ms: end,
        speaker_ids: locals.iter().map(|id| (*id).into()).collect(),
        label: if track == "mic" { "Me" } else { "Speaker" }.into(),
        provenance: "audio-derived".into(),
        score: Some(0.9),
        overlap: false,
        uncertain: false,
        provisional: false,
    }
}
fn result(turns: Vec<SpeakerTurn>) -> DiarizationResult {
    DiarizationResult {
        turns,
        speakers: vec![],
        merges: vec![],
        constraint_reviews: vec![],
        segmentation_context: None,
        embedding_version: "synthetic-version-only".into(),
        calibrated: false,
        warnings: vec!["Synthetic uncalibrated speaker fixture".into()],
    }
}
fn segment(id: &str, track: &str, words: &[(&str, u64, u64)]) -> Segment {
    Segment {
        id: id.into(),
        track: track.into(),
        start_ms: words.first().unwrap().1,
        end_ms: words.last().unwrap().2,
        text: words
            .iter()
            .map(|word| word.0)
            .collect::<Vec<_>>()
            .join(" "),
        speaker: "Original channel label".into(),
        words: words
            .iter()
            .map(|(text, start, end)| TranscriptWord {
                text: (*text).into(),
                start_ms: *start,
                end_ms: *end,
            })
            .collect(),
    }
}
fn begin(root: &Path, segments: &[Segment], turns: Vec<SpeakerTurn>, reconcile: bool) -> String {
    let run = create_run(root, "speakers", "refine", r#"{"engine":"synthetic"}"#).unwrap();
    append_run_segments(root, &run.id, segments).unwrap();
    speakers::save_result(root, "speakers", &run.id, &result(turns), reconcile).unwrap();
    run.id
}
fn seed(root: &Path, segments: &[Segment], turns: Vec<SpeakerTurn>) -> String {
    let run = begin(root, segments, turns, false);
    publish_run(root, "speakers", &run).unwrap();
    run
}
fn two_segments() -> Vec<Segment> {
    vec![
        segment("one", "system", &[("Alpha", 100, 500)]),
        segment("two", "system", &[("Beta", 2100, 2500)]),
    ]
}
fn two_turns() -> Vec<SpeakerTurn> {
    vec![
        turn("one", "system", 0, 1500, &["system:a"]),
        turn("two", "system", 2000, 3500, &["system:b"]),
    ]
}
fn id_for(root: &Path, run: &str, turn_id: &str) -> String {
    speakers::read_turns(&open(root).unwrap(), run)
        .unwrap()
        .into_iter()
        .find(|turn| turn.id == turn_id)
        .unwrap()
        .speaker_ids[0]
        .clone()
}
fn summary(root: &Path, run: &str) {
    let d = detail(root, "speakers").unwrap();
    save_summary(
        root,
        "speakers",
        run,
        d.correction_version,
        "synthetic",
        "fixture",
        "general",
        "{}",
        "Summaryword",
        &[d.segments[0].id.clone()],
    )
    .unwrap();
}

#[test]
fn native_word_midpoints_derive_labels_overlap_and_user_provenance_without_mutating_raw_history() {
    let t = fixture();
    let root = t.path();
    let mut overlap = turn("overlap", "system", 3000, 4000, &["system:a", "system:b"]);
    overlap.overlap = true;
    let source = segment(
        "combined",
        "system",
        &[
            ("Alpha", 100, 500),
            ("Beta", 2100, 2500),
            ("Together", 3500, 3900),
        ],
    );
    let run = seed(
        root,
        &[
            source.clone(),
            segment("microphone", "mic", &[("Me", 200, 400)]),
        ],
        vec![
            turn("first", "system", 0, 2000, &["system:a"]),
            turn("second", "system", 2000, 3000, &["system:b"]),
            overlap,
            turn("mic", "mic", 0, 1000, &["mic:me"]),
        ],
    );
    let d = detail(root, "speakers").unwrap();
    let assignment = d
        .speaker_assignments
        .iter()
        .find(|a| a.segment_id == "combined")
        .unwrap();
    assert_eq!(assignment.words.len(), 3);
    assert_eq!(assignment.words[0].speaker_ids.len(), 1);
    assert_ne!(
        assignment.words[0].speaker_ids,
        assignment.words[1].speaker_ids
    );
    assert_eq!(assignment.words[2].speaker_ids.len(), 2);
    assert!(assignment.words[2].overlap);
    assert_eq!(assignment.words[2].label, "Overlap");
    assert!(assignment.overlap);
    assert_eq!(assignment.provenance, "audio-derived");
    assert_eq!(
        d.segments
            .iter()
            .find(|s| s.id == "microphone")
            .unwrap()
            .speaker,
        "Me"
    );
    let info = d.diarization.unwrap();
    assert_eq!(info.embedding_version, "synthetic-version-only");
    assert!(!info.calibrated);
    assert_eq!(info.warnings.len(), 1);
    let first = id_for(root, &run, "first");
    speakers::rename(root, "speakers", &first, "Alice").unwrap();
    let d = detail(root, "speakers").unwrap();
    let assignment = d
        .speaker_assignments
        .iter()
        .find(|a| a.segment_id == "combined")
        .unwrap();
    assert_eq!(assignment.words[0].label, "Alice");
    assert_eq!(assignment.provenance, "user");
    let db = open(root).unwrap();
    let raw = read_segments(&db, &run, false)
        .unwrap()
        .into_iter()
        .find(|s| s.id == "combined")
        .unwrap();
    assert_eq!(
        serde_json::to_value(raw).unwrap(),
        serde_json::to_value(source).unwrap()
    );
    assert!(
        db.execute(
            "UPDATE app_run_segments SET speaker='overwrite' WHERE run_id=?1",
            [&run]
        )
        .is_err()
    );
    assert!(
        db.execute(
            "UPDATE app_speaker_turns SET label='overwrite' WHERE run_id=?1",
            [&run]
        )
        .is_err()
    );
    assert!(speakers::save_result(root, "speakers", &run, &result(two_turns()), false).is_err());
}

#[test]
fn names_invalidate_summary_and_revision_but_title_and_notes_do_not() {
    let t = fixture();
    let root = t.path();
    let run = seed(root, &two_segments(), two_turns());
    summary(root, &run);
    let before = detail(root, "speakers").unwrap();
    rename(root, "speakers", "Updated title").unwrap();
    save_note(root, "speakers", 500, "A new bookmark").unwrap();
    let metadata = detail(root, "speakers").unwrap();
    assert_eq!(metadata.correction_version, before.correction_version);
    assert!(!metadata.summaries[0].is_stale);
    assert_eq!(metadata.summary.as_deref(), Some("Summaryword"));
    speakers::rename(root, "speakers", &id_for(root, &run, "one"), "Alice").unwrap();
    let named = detail(root, "speakers").unwrap();
    assert_eq!(named.correction_version, before.correction_version + 1);
    assert!(named.summaries[0].is_stale);
    assert!(named.summary.is_none());
    assert_eq!(named.segments[0].speaker, "Alice");
    assert_eq!(
        speakers::constraint_version(&open(root).unwrap(), "speakers").unwrap(),
        1
    );
    assert!(
        save_summary(
            root,
            "speakers",
            &run,
            before.correction_version,
            "fixture",
            "fixture",
            "general",
            "{}",
            "Stale input",
            &["one".into()]
        )
        .is_err()
    );
}

#[test]
fn temporal_constraints_persist_exact_anchors_and_reject_cross_track_short_or_overlap_excerpts() {
    let t = fixture();
    let root = t.path();
    let mut turns = two_turns();
    turns.push(turn("mic", "mic", 0, 1500, &["mic:me"]));
    turns.push(turn("short", "system", 4000, 4500, &["system:short"]));
    let mut overlap = turn("overlap", "system", 5000, 6500, &["system:a", "system:b"]);
    overlap.overlap = true;
    turns.push(overlap);
    let run = seed(root, &two_segments(), turns);
    for right in ["mic", "short", "overlap", "one"] {
        assert!(
            speakers::add_constraint(root, "speakers", &run, "one", right, "must_link").is_err()
        );
        assert!(
            speakers::add_constraint(root, "speakers", &run, "one", right, "cannot_link").is_err()
        );
    }
    assert_eq!(
        speakers::constraint_version(&open(root).unwrap(), "speakers").unwrap(),
        0
    );
    assert_eq!(detail(root, "speakers").unwrap().correction_version, 0);
    speakers::add_constraint(root, "speakers", &run, "one", "two", "cannot_link").unwrap();
    initialize(root).unwrap();
    let anchors = speakers::constraints(root, "speakers").unwrap();
    assert_eq!(anchors.len(), 1);
    assert_eq!(anchors[0].kind, "cannot_link");
    assert_eq!(
        (
            anchors[0].left.track.as_str(),
            anchors[0].left.start_ms,
            anchors[0].left.end_ms
        ),
        ("system", 0, 1500)
    );
    assert_eq!(
        (
            anchors[0].right.track.as_str(),
            anchors[0].right.start_ms,
            anchors[0].right.end_ms
        ),
        ("system", 2000, 3500)
    );
}

#[test]
fn manual_split_uses_an_override_and_named_merge_conflicts_roll_back_every_side_effect() {
    let t = fixture();
    let root = t.path();
    let same = vec![
        turn("one", "system", 0, 1500, &["system:a"]),
        turn("two", "system", 2000, 3500, &["system:a"]),
    ];
    let run = seed(root, &two_segments(), same);
    let original = id_for(root, &run, "one");
    assert_eq!(id_for(root, &run, "two"), original);
    speakers::add_constraint(root, "speakers", &run, "one", "two", "cannot_link").unwrap();
    let right = id_for(root, &run, "two");
    assert_ne!(right, original);
    let db = open(root).unwrap();
    let raw: Vec<String> = serde_json::from_str(
        &db.query_row(
            "SELECT speaker_ids_json FROM app_speaker_turns WHERE run_id=?1 AND id='two'",
            [&run],
            |row| row.get::<_, String>(0),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(raw, vec![original.clone()]);
    let turns = speakers::read_turns(&db, &run).unwrap();
    assert_eq!(
        turns
            .iter()
            .find(|turn| turn.id == "two")
            .unwrap()
            .provenance,
        "user"
    );
    speakers::rename(root, "speakers", &original, "Alice").unwrap();
    speakers::rename(root, "speakers", &right, "Bob").unwrap();
    summary(root, &run);
    let before = detail(root, "speakers").unwrap();
    let version = speakers::constraint_version(&db, "speakers").unwrap();
    assert!(
        speakers::add_constraint(root, "speakers", &run, "one", "two", "must_link")
            .unwrap_err()
            .to_string()
            .contains("different user names")
    );
    let after = detail(root, "speakers").unwrap();
    assert_eq!(after.correction_version, before.correction_version);
    assert_eq!(
        speakers::constraint_version(&db, "speakers").unwrap(),
        version
    );
    assert!(!after.summaries[0].is_stale);
    assert_eq!(id_for(root, &run, "one"), original);
    assert_eq!(id_for(root, &run, "two"), right);
    // A separate clean merge preserves the existing single user name via alias.
    let merge = fixture();
    let root = merge.path();
    let run = seed(root, &two_segments(), two_turns());
    let first = id_for(root, &run, "one");
    let second = id_for(root, &run, "two");
    speakers::rename(root, "speakers", &second, "Named participant").unwrap();
    speakers::add_constraint(root, "speakers", &run, "one", "two", "must_link").unwrap();
    assert_eq!(id_for(root, &run, "one"), second);
    assert_eq!(id_for(root, &run, "two"), second);
    assert_ne!(first, second);
    assert_eq!(
        detail(root, "speakers").unwrap().segments[0].speaker,
        "Named participant"
    );
    let anchored = speakers::constraints(root, "speakers").unwrap();
    assert_eq!(anchored.len(), 1);
    assert_eq!(anchored[0].kind, "must_link");
    assert_eq!(
        (anchored[0].left.start_ms, anchored[0].right.start_ms),
        (0, 2000)
    );
}

#[test]
fn refinement_reconciliation_inherits_unique_named_identity_and_persists_split_merge_reviews() {
    let t = fixture();
    let root = t.path();
    let old = seed(root, &two_segments(), two_turns());
    let stable = id_for(root, &old, "one");
    speakers::rename(root, "speakers", &stable, "Alice").unwrap();
    let new = begin(
        root,
        &two_segments(),
        vec![
            turn("new-one", "system", 0, 1500, &["system:new-a"]),
            turn("new-two", "system", 2000, 3500, &["system:new-b"]),
        ],
        true,
    );
    assert_eq!(id_for(root, &new, "new-one"), stable);
    publish_run(root, "speakers", &new).unwrap();
    let current = detail(root, "speakers").unwrap();
    assert!(current.speaker_reviews.is_empty());
    assert_eq!(current.segments[0].speaker, "Alice");
    assert_eq!(current.speaker_assignments[0].provenance, "user");
    for kind in ["split", "merge"] {
        let t = fixture();
        let root = t.path();
        let old_turns = if kind == "split" {
            vec![turn("old", "system", 0, 4000, &["system:a"])]
        } else {
            vec![
                turn("old-a", "system", 0, 2000, &["system:a"]),
                turn("old-b", "system", 2000, 4000, &["system:b"]),
            ]
        };
        let old = seed(root, &two_segments(), old_turns);
        let ids = detail(root, "speakers")
            .unwrap()
            .speakers
            .into_iter()
            .map(|speaker| speaker.id)
            .collect::<Vec<_>>();
        for (i, id) in ids.iter().enumerate() {
            speakers::rename(root, "speakers", id, &format!("Person {i}")).unwrap();
        }
        let new_turns = if kind == "split" {
            vec![
                turn("new-a", "system", 0, 2000, &["system:new-a"]),
                turn("new-b", "system", 2000, 4000, &["system:new-b"]),
            ]
        } else {
            vec![turn("new", "system", 0, 4000, &["system:new"])]
        };
        let new = begin(root, &two_segments(), new_turns, true);
        publish_run(root, "speakers", &new).unwrap();
        let current = detail(root, "speakers").unwrap();
        assert!(
            current
                .speaker_reviews
                .iter()
                .any(|review| review.kind == kind
                    && review.status == "needs_review"
                    && !review.previous_ids.is_empty()
                    && !review.current_ids.is_empty())
        );
        assert!(
            current
                .speakers
                .iter()
                .all(|speaker| speaker.user_name.is_none())
        );
        assert!(
            detail_run(root, "speakers", &old)
                .unwrap()
                .speakers
                .iter()
                .all(|speaker| speaker.user_name.is_some())
        );
    }
}

#[test]
fn stale_constraint_version_cannot_publish_an_inflight_refinement() {
    let t = fixture();
    let root = t.path();
    let old = seed(root, &two_segments(), two_turns());
    let pending = begin(root, &two_segments(), two_turns(), false);
    assert_eq!(
        list_runs(root, "speakers")
            .unwrap()
            .iter()
            .find(|run| run.id == pending)
            .unwrap()
            .constraints_version,
        0
    );
    speakers::rename(root, "speakers", &id_for(root, &old, "one"), "Alice").unwrap();
    assert!(
        publish_run(root, "speakers", &pending)
            .unwrap_err()
            .to_string()
            .contains("Speaker corrections changed")
    );
    let current = detail(root, "speakers").unwrap();
    assert_eq!(current.active_run_id.as_deref(), Some(old.as_str()));
    assert_eq!(
        current
            .runs
            .iter()
            .find(|run| run.id == pending)
            .unwrap()
            .status,
        "running"
    );
    assert_eq!(current.segments[0].speaker, "Alice");
}

#[test]
fn replay_copies_native_word_times_and_speaker_turns_overrides_and_provenance() {
    let t = fixture();
    let root = t.path();
    let old = seed(
        root,
        &two_segments(),
        vec![
            turn("one", "system", 0, 1500, &["system:a"]),
            turn("two", "system", 2000, 3500, &["system:a"]),
        ],
    );
    speakers::rename(root, "speakers", &id_for(root, &old, "one"), "Alice").unwrap();
    speakers::add_constraint(root, "speakers", &old, "one", "two", "cannot_link").unwrap();
    let before = detail(root, "speakers").unwrap();
    let db = open(root).unwrap();
    let raw = read_segments(&db, &old, false).unwrap();
    for source in &raw {
        stage_segment(root, "replay-speakers", "speakers", source).unwrap();
    }
    publish_staged(root, "replay-speakers", "speakers").unwrap();
    let after = detail(root, "speakers").unwrap();
    let new = after.active_run_id.as_deref().unwrap();
    assert_ne!(new, old);
    assert_eq!(
        after.runs.iter().find(|run| run.id == new).unwrap().kind,
        "replay"
    );
    assert_eq!(
        serde_json::to_value(read_segments(&db, new, false).unwrap()).unwrap(),
        serde_json::to_value(raw).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&after.speaker_turns).unwrap(),
        serde_json::to_value(&before.speaker_turns).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&after.speaker_assignments).unwrap(),
        serde_json::to_value(&before.speaker_assignments).unwrap()
    );
    assert_eq!(
        after.diarization.unwrap().embedding_version,
        "synthetic-version-only"
    );
    assert!(
        after
            .segments
            .iter()
            .all(|segment| !segment.words.is_empty())
    );
    assert_eq!(
        detail_run(root, "speakers", &old).unwrap().segments[0].speaker,
        "Alice"
    );
}

#[test]
fn legacy_schema_upgrade_is_idempotent_and_persists_no_embedding_vectors() {
    let t = TempDir::new().unwrap();
    let root = t.path();
    vt_record::initialize(root).unwrap();
    let recording = vt_record::Recording::create(root, "legacy-speakers").unwrap();
    let db = open(root).unwrap();
    db.execute_batch(include_str!(
        "../../../documentation/features/app/desktop-session/sql/001-desktop.sql"
    ))
    .unwrap();
    db.execute("INSERT INTO app_meetings(id,title,status,started_at,mode) VALUES ('legacy-speakers','Legacy','ready','2026-11-06T12:00:00Z','remote')",[]).unwrap();
    db.execute("INSERT INTO app_segments(id,meeting_id,track,start_ms,end_ms,text,speaker) VALUES ('legacy-segment','legacy-speakers','system',0,1000,'Legacy transcript','Them')",[]).unwrap();
    recording.finish().unwrap();
    initialize(root).unwrap();
    initialize(root).unwrap();
    let detail = detail(root, "legacy-speakers").unwrap();
    assert_eq!(detail.runs.len(), 1);
    assert_eq!(detail.segments[0].text, "Legacy transcript");
    assert!(detail.segments[0].words.is_empty());
    assert_eq!(detail.segments[0].speaker, "Them");
    assert!(detail.speaker_turns.is_empty());
    let tables = db
        .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name LIKE '%speaker%'")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap();
    assert!(!tables.is_empty());
    for table in tables {
        let mut query = db.prepare(&format!("PRAGMA table_info({table})")).unwrap();
        let columns = query
            .query_map([], |row| {
                Ok((row.get::<_, String>(1)?, row.get::<_, String>(2)?))
            })
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap();
        for (name, kind) in columns {
            assert!(
                !name.contains("embedding") || name == "embedding_version",
                "vector column persisted: {table}.{name}"
            );
            assert!(
                !kind.eq_ignore_ascii_case("BLOB"),
                "speaker schema must not store vector blobs"
            );
        }
    }
    let columns = db
        .prepare("PRAGMA table_info(app_transcription_staging)")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(
        columns
            .iter()
            .filter(|name| name.as_str() == "words_json")
            .count(),
        1
    );
    let legacy_rows: i64 = db
        .query_row(
            "SELECT COUNT(*) FROM app_segments WHERE meeting_id='legacy-speakers'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(legacy_rows, 1);
}

#[test]
fn speaker_relationships_can_be_replaced_and_revoked_without_losing_history() {
    let t = fixture();
    let root = t.path();
    let mut turns = two_turns();
    turns[1].speaker_ids = turns[0].speaker_ids.clone();
    let run = seed(root, &two_segments(), turns);
    let original = id_for(root, &run, "one");
    speakers::add_constraint(root, "speakers", &run, "one", "two", "cannot_link").unwrap();
    let different = speakers::constraints(root, "speakers").unwrap().remove(0);
    assert_ne!(id_for(root, &run, "two"), original);
    speakers::revoke_constraint(root, "speakers", &different.id).unwrap();
    assert_eq!(id_for(root, &run, "two"), original);
    assert!(speakers::constraints(root, "speakers").unwrap().is_empty());
    let history = detail(root, "speakers").unwrap().speaker_constraints;
    assert_eq!(history.len(), 1);
    assert!(!history[0].active);
    let version = speakers::constraint_version(&open(root).unwrap(), "speakers").unwrap();
    assert!(version >= 2);
    speakers::add_constraint(root, "speakers", &run, "one", "two", "cannot_link").unwrap();
    speakers::add_constraint(root, "speakers", &run, "two", "one", "must_link").unwrap();
    let active = speakers::constraints(root, "speakers").unwrap();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].kind, "must_link");
    assert_eq!(id_for(root, &run, "one"), id_for(root, &run, "two"));
    assert_eq!(
        detail(root, "speakers").unwrap().speaker_constraints.len(),
        3
    );
    assert!(speakers::revoke_constraint(root, "other-meeting", &active[0].id).is_err());
    speakers::revoke_constraint(root, "speakers", &active[0].id).unwrap();
    assert!(speakers::constraints(root, "speakers").unwrap().is_empty());
}

#[test]
fn reconciliation_review_requires_a_valid_choice_and_explicitly_preserves_or_dismisses_a_name() {
    let t = fixture();
    let root = t.path();
    let previous = seed(
        root,
        &two_segments(),
        vec![turn("old", "system", 0, 4000, &["system:old"])],
    );
    let old_id = id_for(root, &previous, "old");
    speakers::rename(root, "speakers", &old_id, "Alice").unwrap();
    let next = begin(
        root,
        &two_segments(),
        vec![
            turn("first", "system", 0, 2000, &["system:new-a"]),
            turn("second", "system", 2000, 4000, &["system:new-b"]),
        ],
        true,
    );
    publish_run(root, "speakers", &next).unwrap();
    summary(root, &next);
    let before = detail(root, "speakers").unwrap();
    let review = &before.speaker_reviews[0];
    assert_eq!(review.previous_labels, vec!["Alice"]);
    assert!(
        before
            .speakers
            .iter()
            .all(|speaker| !review.current_ids.is_empty()
                && review.current_ids.iter().any(|id| id == &speaker.id))
    );
    assert!(
        speakers::resolve_review(
            root,
            "speakers",
            &next,
            &review.id,
            Some(&old_id),
            Some("unrelated-speaker")
        )
        .is_err()
    );
    assert_eq!(
        detail(root, "speakers").unwrap().correction_version,
        before.correction_version
    );
    let target = &review.current_ids[0];
    speakers::resolve_review(
        root,
        "speakers",
        &next,
        &review.id,
        Some(&old_id),
        Some(target),
    )
    .unwrap();
    let resolved = detail(root, "speakers").unwrap();
    assert_eq!(
        resolved
            .speakers
            .iter()
            .find(|speaker| &speaker.id == target)
            .unwrap()
            .label,
        "Alice"
    );
    assert_eq!(resolved.speaker_reviews[0].status, "resolved");
    assert!(resolved.summaries[0].is_stale);
    assert_eq!(resolved.correction_version, before.correction_version + 1);
    assert_eq!(
        detail_run(root, "speakers", &previous).unwrap().segments[0].speaker,
        "Alice"
    );
    let db = open(root).unwrap();
    db.execute("INSERT INTO app_speaker_reviews(id,run_id,kind,previous_ids_json,current_ids_json,reason) SELECT 'dismiss-fixture',run_id,kind,previous_ids_json,current_ids_json,reason FROM app_speaker_reviews WHERE id=?1",[&review.id]).unwrap();
    drop(db);
    speakers::resolve_review(root, "speakers", &next, "dismiss-fixture", None, None).unwrap();
    let dismissed = detail(root, "speakers").unwrap();
    assert!(
        dismissed
            .speaker_reviews
            .iter()
            .any(|review| review.id == "dismiss-fixture" && review.status == "dismissed")
    );
    assert_eq!(dismissed.correction_version, resolved.correction_version);
}

#[test]
fn pending_constraint_review_keeps_complete_text_links_user_correction_and_closes_on_revocation() {
    let t = fixture();
    let root = t.path();
    let old = seed(root, &two_segments(), two_turns());
    speakers::add_constraint(root, "speakers", &old, "one", "two", "cannot_link").unwrap();
    let constraint = speakers::constraints(root, "speakers").unwrap().remove(0);
    let db = open(root).unwrap();
    let original = serde_json::to_value(read_segments(&db, &old, false).unwrap()).unwrap();
    let next = create_run(
        root,
        "speakers",
        "refine",
        r#"{"engine":"synthetic-pending-constraint"}"#,
    )
    .unwrap();
    let mut unknown_left = turn("unknown-left", "system", 0, 1500, &[]);
    unknown_left.label = "Unknown".into();
    unknown_left.uncertain = true;
    unknown_left.provenance = "constraint-review".into();
    let mut unknown_right = turn("unknown-right", "system", 2000, 3500, &[]);
    unknown_right.label = "Unknown".into();
    unknown_right.uncertain = true;
    unknown_right.provenance = "constraint-review".into();
    // Public result serialization avoids coupling this regression to the name of
    // the new review record type, while requiring its provenance/anchor contract.
    let mut payload = serde_json::to_value(result(vec![unknown_left, unknown_right])).unwrap();
    payload["constraint_reviews"] = serde_json::json!([{"provenance":format!("user:{}",constraint.id),"reason":"Saved clean excerpts no longer resolve unambiguously; affected speech remains Unknown.","anchors":[{"track":"system","start_ms":0,"end_ms":1500},{"track":"system","start_ms":2000,"end_ms":3500}]}]);
    let pending: DiarizationResult = serde_json::from_value(payload).unwrap();
    speakers::save_result(root, "speakers", &next.id, &pending, true).unwrap();
    // Bounded live updates return no accumulated resolved history and the same
    // pending correction must not create another review on every input update.
    for _ in 0..2 {
        let update: Result<()> = speakers::save_live_updates(root, "speakers", &next.id, &pending);
        update.unwrap();
    }
    let review_count: i64 = db
        .query_row(
            "SELECT COUNT(*) FROM app_speaker_reviews WHERE run_id=?1 AND kind='constraint'",
            [&next.id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(review_count, 1);
    append_run_segments(root, &next.id, &two_segments()).unwrap();
    publish_run(root, "speakers", &next.id).unwrap();
    let published = detail(root, "speakers").unwrap();
    assert_eq!(published.meeting.status, "ready");
    assert_eq!(published.active_run_id.as_deref(), Some(next.id.as_str()));
    assert!(
        published
            .runs
            .iter()
            .any(|run| run.id == next.id && run.status == "complete")
    );
    assert_eq!(
        published
            .segments
            .iter()
            .map(|segment| segment.text.as_str())
            .collect::<Vec<_>>(),
        ["Alpha", "Beta"]
    );
    assert!(
        published
            .segments
            .iter()
            .all(|segment| segment.speaker == "Unknown" && !segment.words.is_empty())
    );
    assert!(
        published
            .speaker_assignments
            .iter()
            .all(|assignment| assignment.uncertain
                && assignment.words.iter().all(|word| word.label == "Unknown"))
    );
    let review = published
        .speaker_reviews
        .iter()
        .find(|review| review.kind == "constraint")
        .expect("an unresolved user correction must remain reviewable");
    let review_json = serde_json::to_value(review).unwrap();
    assert_eq!(review_json["constraint_id"], constraint.id);
    assert_eq!(review.status, "needs_review");
    assert!(review.reason.contains("no longer resolve"));
    assert!(review.previous_ids.is_empty());
    assert!(review.current_ids.is_empty());
    assert!(
        published
            .speaker_constraints
            .iter()
            .any(|entry| entry.id == constraint.id && entry.active)
    );
    assert_eq!(
        serde_json::to_value(read_segments(&db, &old, false).unwrap()).unwrap(),
        original,
        "pending diarization must never rewrite completed text/history"
    );
    let stored: Option<String> = db
        .query_row(
            "SELECT constraint_id FROM app_speaker_reviews WHERE id=?1",
            [&review.id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(stored.as_deref(), Some(constraint.id.as_str()));
    summary(root, &next.id);
    let revision = detail(root, "speakers").unwrap().correction_version;
    let version = speakers::constraint_version(&db, "speakers").unwrap();
    assert_eq!(next.constraints_version, version);
    speakers::revoke_constraint(root, "speakers", &constraint.id).unwrap();
    let removed = detail(root, "speakers").unwrap();
    assert_eq!(removed.correction_version, revision + 1);
    assert_eq!(
        speakers::constraint_version(&db, "speakers").unwrap(),
        version + 1
    );
    assert_eq!(removed.active_run_id.as_deref(), Some(next.id.as_str()));
    assert!(removed.summaries[0].is_stale);
    assert!(
        removed
            .speaker_reviews
            .iter()
            .find(|entry| entry.id == review.id)
            .unwrap()
            .status
            != "needs_review"
    );
    assert!(speakers::constraints(root, "speakers").unwrap().is_empty());
    assert!(
        removed
            .speaker_constraints
            .iter()
            .any(|entry| entry.id == constraint.id && !entry.active)
    );
    let retained: i64 = db
        .query_row(
            "SELECT COUNT(*) FROM app_speaker_constraints WHERE id=?1 AND is_active=0",
            [&constraint.id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        retained, 1,
        "revoking a correction retains its audit anchor"
    );
    fn has_vector(value: &serde_json::Value) -> bool {
        match value {
            serde_json::Value::Object(fields) => fields.iter().any(|(key, value)| {
                matches!(
                    key.as_str(),
                    "embedding" | "embeddings" | "vector" | "vectors"
                ) || has_vector(value)
            }),
            serde_json::Value::Array(values) => values.iter().any(has_vector),
            _ => false,
        }
    }
    assert!(!has_vector(&serde_json::to_value(published).unwrap()));
    assert!(!has_vector(&serde_json::to_value(removed).unwrap()));
}

#[test]
fn a_named_identity_survives_unknown_masks_and_remains_explicitly_resolvable_on_a_later_pass() {
    let t = fixture();
    let root = t.path();
    let original = seed(
        root,
        &two_segments(),
        vec![
            turn("left", "system", 0, 1500, &["system:original"]),
            turn("right", "system", 2000, 3500, &["system:original"]),
        ],
    );
    let alice = id_for(root, &original, "left");
    speakers::rename(root, "speakers", &alice, "Alice").unwrap();
    speakers::add_constraint(root, "speakers", &original, "left", "right", "cannot_link").unwrap();
    let constraint = speakers::constraints(root, "speakers").unwrap().remove(0);
    let unknown = |id: &str, start, end| {
        let mut masked = turn(id, "system", start, end, &[]);
        masked.label = "Unknown".into();
        masked.uncertain = true;
        masked.provenance = "constraint-review".into();
        masked
    };
    let mut pending = result(vec![
        unknown("masked-left", 0, 1500),
        unknown("masked-right", 2000, 3500),
    ]);
    pending.constraint_reviews = vec![vt_diarize::ConstraintReview {
        provenance: format!("user:{}", constraint.id),
        reason: "Synthetic stale excerpts cannot distinguish the original voice safely.".into(),
        anchors: vec![
            vt_diarize::Anchor {
                track: "system".into(),
                start_ms: 0,
                end_ms: 1500,
            },
            vt_diarize::Anchor {
                track: "system".into(),
                start_ms: 2000,
                end_ms: 3500,
            },
        ],
    }];
    let masked = create_run(root, "speakers", "refine", "{}").unwrap();
    append_run_segments(root, &masked.id, &two_segments()).unwrap();
    speakers::save_result(root, "speakers", &masked.id, &pending, true).unwrap();
    publish_run(root, "speakers", &masked.id).unwrap();
    let second = detail(root, "speakers").unwrap();
    assert!(
        second
            .segments
            .iter()
            .all(|segment| segment.speaker == "Unknown")
    );
    let name_review = second
        .speaker_reviews
        .iter()
        .find(|review| review.kind == "unmatched_name" && review.previous_ids.contains(&alice))
        .expect("masking a named voice must retain its name for explicit review");
    assert_eq!(name_review.status, "needs_review");
    assert!(
        name_review
            .previous_labels
            .iter()
            .any(|name| name == "Alice")
    );
    speakers::revoke_constraint(root, "speakers", &constraint.id).unwrap();
    assert!(
        detail(root, "speakers")
            .unwrap()
            .speaker_reviews
            .iter()
            .any(|review| review.kind == "unmatched_name"
                && review.previous_ids.contains(&alice)
                && review.status == "needs_review")
    );
    let third = begin(
        root,
        &two_segments(),
        vec![turn("fresh-turn", "system", 0, 4000, &["system:fresh"])],
        true,
    );
    publish_run(root, "speakers", &third).unwrap();
    let current = detail(root, "speakers").unwrap();
    let fresh = id_for(root, &third, "fresh-turn");
    assert_ne!(fresh, alice);
    assert!(
        current
            .speakers
            .iter()
            .all(|speaker| speaker.user_name.is_none()),
        "the missing acoustic match must not silently inherit Alice"
    );
    let review = current
        .speaker_reviews
        .iter()
        .find(|review| {
            review.status == "needs_review"
                && review.previous_ids.contains(&alice)
                && review.current_ids.contains(&fresh)
        })
        .expect("earlier unresolved identity must be offered against the later native cluster");
    assert!(review.previous_labels.iter().any(|name| name == "Alice"));
    speakers::resolve_review(
        root,
        "speakers",
        &third,
        &review.id,
        Some(&alice),
        Some(&fresh),
    )
    .unwrap();
    let resolved = detail(root, "speakers").unwrap();
    assert!(
        resolved
            .segments
            .iter()
            .all(|segment| segment.speaker == "Alice")
    );
    assert!(
        resolved
            .speaker_assignments
            .iter()
            .all(|assignment| assignment.provenance == "user")
    );
    assert_eq!(
        resolved
            .speaker_reviews
            .iter()
            .find(|entry| entry.id == review.id)
            .unwrap()
            .status,
        "resolved"
    );
    assert!(
        detail_run(root, "speakers", &masked.id)
            .unwrap()
            .segments
            .iter()
            .all(|segment| segment.speaker == "Unknown")
    );
}

#[test]
fn ambiguous_microphone_me_reconciliation_never_reuses_one_of_two_old_user_names() {
    let t = fixture();
    let root = t.path();
    let mut sources = two_segments();
    for segment in &mut sources {
        segment.track = "mic".into();
    }
    let old = seed(
        root,
        &sources,
        vec![
            turn("mic-me", "mic", 0, 2000, &["mic:me"]),
            turn("mic-other", "mic", 2000, 4000, &["mic:other"]),
        ],
    );
    let alice = id_for(root, &old, "mic-me");
    let bob = id_for(root, &old, "mic-other");
    speakers::rename(root, "speakers", &alice, "Alice").unwrap();
    speakers::rename(root, "speakers", &bob, "Bob").unwrap();
    let merged = begin(
        root,
        &sources,
        vec![turn("new-mic", "mic", 0, 4000, &["mic:me"])],
        true,
    );
    publish_run(root, "speakers", &merged).unwrap();
    let current = detail(root, "speakers").unwrap();
    let fresh = id_for(root, &merged, "new-mic");
    assert_ne!(
        fresh, alice,
        "the channel Me identity is not evidence for selecting Alice after an ambiguous merge"
    );
    assert_ne!(fresh, bob);
    assert!(
        current
            .speakers
            .iter()
            .all(|speaker| speaker.user_name.is_none())
    );
    assert!(
        current
            .segments
            .iter()
            .all(|segment| !["Alice", "Bob"].contains(&segment.speaker.as_str()))
    );
    let review = current
        .speaker_reviews
        .iter()
        .find(|review| review.kind == "merge")
        .expect("both earlier named microphone identities require review");
    assert!(review.previous_ids.contains(&alice));
    assert!(review.previous_ids.contains(&bob));
    assert!(review.previous_labels.iter().any(|name| name == "Alice"));
    assert!(review.previous_labels.iter().any(|name| name == "Bob"));
    assert!(review.current_ids.contains(&fresh));
}

#[test]
fn active_constraint_capacity_rejects_new_pairs_atomically_but_allows_replacement_and_reuse() {
    let t = fixture();
    let root = t.path();
    let mut turns = two_turns();
    turns.push(turn("three", "system", 4000, 5500, &["system:c"]));
    let run = seed(root, &two_segments(), turns);
    let alice = id_for(root, &run, "one");
    speakers::rename(root, "speakers", &alice, "Alice").unwrap();
    speakers::add_constraint(root, "speakers", &run, "one", "two", "cannot_link").unwrap();
    let original = speakers::constraints(root, "speakers")
        .unwrap()
        .remove(0)
        .id;
    let mut db = open(root).unwrap();
    let version = speakers::constraint_version(&db, "speakers").unwrap();
    {
        let tx = db.transaction().unwrap();
        let mut insert=tx.prepare("INSERT INTO app_speaker_constraints(id,meeting_id,kind,left_anchor_json,right_anchor_json,version,created_at,is_active) VALUES (?1,'speakers','cannot_link',?2,?3,?4,'synthetic-capacity-fixture',1)").unwrap();
        for index in 1..4096_u64 {
            let start = 10_000 + index * 4000;
            let left = serde_json::json!({"track":"system","start_ms":start,"end_ms":start+1500})
                .to_string();
            let right =
                serde_json::json!({"track":"system","start_ms":start+2000,"end_ms":start+3500})
                    .to_string();
            insert
                .execute(params![
                    format!("bulk-pair-{index}"),
                    left,
                    right,
                    (version + index) as i64
                ])
                .unwrap();
        }
        drop(insert);
        tx.commit().unwrap();
    }
    // Rename history and inactive audit rows must not count toward the active
    // temporal limit; the engine receives exactly these 4096 temporal pairs.
    assert_eq!(speakers::constraints(root, "speakers").unwrap().len(), 4096);
    summary(root, &run);
    let snapshot = detail(root, "speakers").unwrap();
    let before_revision = snapshot.correction_version;
    let before_version = speakers::constraint_version(&db, "speakers").unwrap();
    let constraint_rows = |db: &Connection| {
        let mut query = db
            .prepare("SELECT id,is_active,version FROM app_speaker_constraints ORDER BY id")
            .unwrap();
        query
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, bool>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    };
    let identities = |db: &Connection| {
        let mut query = db
            .prepare("SELECT id,alias_of,user_name FROM app_speaker_identities ORDER BY id")
            .unwrap();
        query
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    };
    let before_rows = constraint_rows(&db);
    let before_identities = identities(&db);
    let rejected = speakers::add_constraint(root, "speakers", &run, "one", "three", "must_link")
        .unwrap_err()
        .to_string();
    assert!(
        rejected.contains("4096") && rejected.contains("Remove"),
        "{rejected}"
    );
    assert_eq!(
        constraint_rows(&db),
        before_rows,
        "4097th pair must not change constraint history or active state"
    );
    assert_eq!(
        identities(&db),
        before_identities,
        "rejection must preserve user names and identity links"
    );
    let rejected = detail(root, "speakers").unwrap();
    assert_eq!(rejected.correction_version, before_revision);
    assert_eq!(
        speakers::constraint_version(&db, "speakers").unwrap(),
        before_version
    );
    assert!(!rejected.summaries[0].is_stale);
    assert_eq!(rejected.active_run_id.as_deref(), Some(run.as_str()));
    // Reverse-order selection is the same pair, so its old active relationship
    // is superseded before enforcing the cap; the chosen name is preserved.
    speakers::add_constraint(root, "speakers", &run, "two", "one", "must_link").unwrap();
    let active = speakers::constraints(root, "speakers").unwrap();
    assert_eq!(active.len(), 4096);
    let replacement = active
        .iter()
        .find(|entry| {
            entry.kind == "must_link" && entry.left.start_ms == 2000 && entry.right.start_ms == 0
        })
        .unwrap()
        .id
        .clone();
    assert_ne!(replacement, original);
    assert!(
        !db.query_row(
            "SELECT is_active FROM app_speaker_constraints WHERE id=?1",
            [&original],
            |row| row.get::<_, bool>(0)
        )
        .unwrap()
    );
    assert_eq!(id_for(root, &run, "two"), alice);
    assert_eq!(
        detail(root, "speakers").unwrap().correction_version,
        before_revision + 1
    );
    assert_eq!(
        speakers::constraint_version(&db, "speakers").unwrap(),
        before_version + 1
    );
    speakers::revoke_constraint(root, "speakers", &replacement).unwrap();
    assert_eq!(speakers::constraints(root, "speakers").unwrap().len(), 4095);
    speakers::add_constraint(root, "speakers", &run, "one", "three", "cannot_link").unwrap();
    let active = speakers::constraints(root, "speakers").unwrap();
    assert_eq!(active.len(), 4096);
    assert!(active.iter().any(|entry| entry.kind == "cannot_link"
        && entry.left.start_ms == 0
        && entry.right.start_ms == 4000));
    assert_eq!(
        speakers::constraint_version(&db, "speakers").unwrap(),
        before_version + 3
    );
    assert_eq!(
        detail(root, "speakers").unwrap().correction_version,
        before_revision + 3
    );
}

fn context_json(root: &Path, run: &str) -> Option<String> {
    open(root)
        .unwrap()
        .query_row(
            "SELECT segmentation_context_json FROM app_speaker_run_state WHERE run_id=?1",
            [run],
            |row| row.get(0),
        )
        .unwrap()
}
#[test]
fn segmentation_context_persists_live_updates_copies_exactly_and_survives_restart() {
    let temp = fixture();
    let root = temp.path();
    let live = detail(root, "speakers").unwrap().active_run_id.unwrap();
    let mut measured = result(two_turns());
    measured.segmentation_context = Some(vt_diarize::Phase::Live.segmentation_context());
    speakers::save_live_updates(root, "speakers", &live, &measured).unwrap();
    assert_eq!(
        detail(root, "speakers")
            .unwrap()
            .diarization
            .unwrap()
            .segmentation_context,
        measured.segmentation_context
    );
    let original = context_json(root, &live).unwrap();
    speakers::record_warning(root, "speakers", "Separate capture warning").unwrap();
    assert_eq!(
        context_json(root, &live).as_deref(),
        Some(original.as_str())
    );
    complete_live_run(root, "speakers").unwrap();
    let copied = create_run(root, "speakers", "refine", "{}").unwrap();
    assert!(
        speakers::retain_previous(root, "speakers", &copied.id)
            .unwrap()
            .is_some()
    );
    assert_eq!(
        context_json(root, &copied.id).as_deref(),
        Some(original.as_str())
    );
    // A copied annotation retains its actual Live context despite Refine run kind.
    assert_eq!(
        detail_run(root, "speakers", &copied.id)
            .unwrap()
            .diarization
            .unwrap()
            .segmentation_context,
        measured.segmentation_context
    );
    publish_run(root, "speakers", &copied.id).unwrap();
    initialize(root).unwrap();
    initialize(root).unwrap();
    assert_eq!(
        context_json(root, &copied.id).as_deref(),
        Some(original.as_str())
    );
    assert_eq!(
        detail_run(root, "speakers", &live)
            .unwrap()
            .diarization
            .unwrap()
            .segmentation_context,
        measured.segmentation_context
    );
    let before = context_json(root, &copied.id);
    measured.segmentation_context = Some(vt_diarize::Phase::Refined.segmentation_context());
    assert!(speakers::save_result(root, "speakers", &copied.id, &measured, false).is_err());
    assert!(speakers::retain_previous(root, "speakers", &copied.id).is_err());
    speakers::record_warning(root, "speakers", "Cannot rewrite sealed state").unwrap();
    assert_eq!(context_json(root, &copied.id), before);
}
#[test]
fn segmentation_context_result_and_copy_failures_roll_back_together_with_turns_and_keys() {
    let temp = fixture();
    let root = temp.path();
    let previous = seed(root, &two_segments(), two_turns());
    let next = create_run(root, "speakers", "refine", "{}").unwrap();
    append_run_segments(root, &next.id, &two_segments()).unwrap();
    let mut measured = result(two_turns());
    measured.segmentation_context = Some(vt_diarize::Phase::Refined.segmentation_context());
    let db = open(root).unwrap();
    let identities: i64 = db
        .query_row("SELECT count(*) FROM app_speaker_identities", [], |row| {
            row.get(0)
        })
        .unwrap();
    db.execute_batch("CREATE TRIGGER fail_context BEFORE INSERT ON app_speaker_run_state BEGIN SELECT RAISE(ABORT,'synthetic context failure'); END;").unwrap();
    assert!(speakers::save_result(root, "speakers", &next.id, &measured, false).is_err());
    for table in [
        "app_speaker_turns",
        "app_run_speaker_keys",
        "app_speaker_run_state",
    ] {
        assert_eq!(
            db.query_row(
                &format!("SELECT count(*) FROM {table} WHERE run_id=?1"),
                [&next.id],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
    }
    assert_eq!(
        db.query_row("SELECT count(*) FROM app_speaker_identities", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        identities
    );
    assert_eq!(
        detail(root, "speakers").unwrap().active_run_id.as_deref(),
        Some(previous.as_str())
    );
    // The same INSERT failure must also undo earlier SQL-copy writes.
    assert!(speakers::retain_previous(root, "speakers", &next.id).is_err());
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM app_speaker_turns WHERE run_id=?1",
            [&next.id],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    db.execute_batch("DROP TRIGGER fail_context").unwrap();
    speakers::save_result(root, "speakers", &next.id, &measured, false).unwrap();
    publish_run(root, "speakers", &next.id).unwrap();
    assert_eq!(
        detail(root, "speakers")
            .unwrap()
            .diarization
            .unwrap()
            .segmentation_context,
        measured.segmentation_context
    );
    assert!(
        detail_run(root, "speakers", &previous)
            .unwrap()
            .diarization
            .unwrap()
            .segmentation_context
            .is_none()
    );
}
#[test]
fn segmentation_context_upgrade_is_guarded_preserves_legacy_null_and_warning_only_is_unknown() {
    let temp = fixture();
    let root = temp.path();
    let run = seed(root, &two_segments(), two_turns());
    let before = serde_json::to_value(detail(root, "speakers").unwrap()).unwrap();
    let db = open(root).unwrap();
    // This is the exact pre-column table shape, with existing complete rows.
    db.execute_batch("ALTER TABLE app_speaker_run_state DROP COLUMN segmentation_context_json")
        .unwrap();
    initialize(root).unwrap();
    initialize(root).unwrap();
    assert_eq!(context_json(root, &run), None);
    assert_eq!(
        serde_json::to_value(detail(root, "speakers").unwrap()).unwrap(),
        before
    );
    assert_eq!(db.query_row("SELECT count(*) FROM pragma_table_info('app_speaker_run_state') WHERE name='segmentation_context_json'",[],|row|row.get::<_,i64>(0)).unwrap(),1);
    let _recording = vt_record::Recording::create(root, "warning-only").unwrap();
    create_meeting(root, "warning-only", "Warning only", "remote").unwrap();
    speakers::record_warning(root, "warning-only", "Model setup was unavailable").unwrap();
    let warning = detail(root, "warning-only").unwrap().diarization.unwrap();
    assert!(warning.segmentation_context.is_none());
    assert_eq!(warning.warnings, vec!["Model setup was unavailable"]);
}

#[test]
fn voice_person_forgetting_removes_only_linked_labels_and_preserves_source_text() {
    for remove_names in [false, true] {
        let root = fixture();
        let run = seed(root.path(), &two_segments(), two_turns());
        let first = id_for(root.path(), &run, "one");
        let second = id_for(root.path(), &run, "two");
        rename_speaker(root.path(), "speakers", &second, "Other fixture person").unwrap();
        let original = detail(root.path(), "speakers")
            .unwrap()
            .segments
            .iter()
            .map(|s| s.text.clone())
            .collect::<Vec<_>>();
        let mut db = open(root.path()).unwrap();
        let tx = db.transaction().unwrap();
        tx.execute("INSERT INTO app_people(id,display_name,is_self,created_at) VALUES ('fixture-person','Fixture person',0,'2026-11-07T00:00:00Z')", []).unwrap();
        link_voice_person(&tx, "speakers", &first, "fixture-person", "Fixture person").unwrap();
        tx.commit().unwrap();
        let before = detail(root.path(), "speakers").unwrap();
        assert_eq!(
            before
                .speakers
                .iter()
                .find(|s| s.id == first)
                .unwrap()
                .person_id
                .as_deref(),
            Some("fixture-person")
        );
        summary(root.path(), &run);
        let tx = db.transaction().unwrap();
        forget_person_links(&tx, &["fixture-person".into()], remove_names).unwrap();
        vt_speakers::forget_person(&tx, "fixture-person").unwrap();
        tx.commit().unwrap();
        let after = detail(root.path(), "speakers").unwrap();
        let speaker = after.speakers.iter().find(|s| s.id == first).unwrap();
        assert!(speaker.person_id.is_none());
        assert_eq!(
            speaker.user_name.as_deref(),
            (!remove_names).then_some("Fixture person")
        );
        assert_eq!(
            after
                .speakers
                .iter()
                .find(|s| s.id == second)
                .unwrap()
                .user_name
                .as_deref(),
            Some("Other fixture person")
        );
        assert_eq!(
            original,
            after
                .segments
                .iter()
                .map(|s| s.text.clone())
                .collect::<Vec<_>>()
        );
        if remove_names {
            assert!(after.correction_version > before.correction_version);
            assert!(after.summaries.iter().all(|s| s.is_stale));
            let active_old: i64 = db.query_row("SELECT COUNT(*) FROM app_speaker_constraints WHERE identity_id=?1 AND kind='rename' AND value!='' AND is_active=1", [&first], |r|r.get(0)).unwrap();
            assert_eq!(active_old, 0);
        }
        initialize(root.path()).unwrap();
        assert_eq!(
            detail(root.path(), "speakers")
                .unwrap()
                .speakers
                .iter()
                .find(|s| s.id == first)
                .unwrap()
                .user_name,
            speaker.user_name
        );
    }
}

#[test]
fn shared_room_survives_identity_merge_and_wrong_meeting_cannot_change_it() {
    let root = fixture();
    let run = seed(root.path(), &two_segments(), two_turns());
    let first = id_for(root.path(), &run, "one");
    let second = id_for(root.path(), &run, "two");
    set_speaker_shared_room(root.path(), "speakers", &second, true).unwrap();
    assert!(set_speaker_shared_room(root.path(), "unrelated", &second, false).is_err());
    add_speaker_constraint(root.path(), "speakers", &run, "one", "two", "must_link").unwrap();
    let current = detail(root.path(), "speakers").unwrap();
    assert!(
        current
            .speakers
            .iter()
            .find(|s| s.id == first)
            .unwrap()
            .shared_room
    );
    let db = open(root.path()).unwrap();
    assert!(
        db.query_row(
            "SELECT shared_room FROM app_speaker_identities WHERE id=?1",
            [&second],
            |r| r.get::<_, bool>(0)
        )
        .unwrap()
    );
}

#[test]
fn legacy_speaker_identities_receive_idempotent_memory_columns_without_key_access() {
    let root = fixture();
    let run = seed(root.path(), &two_segments(), two_turns());
    let db = open(root.path()).unwrap();
    db.execute_batch("ALTER TABLE app_speaker_identities DROP COLUMN person_id; ALTER TABLE app_speaker_identities DROP COLUMN shared_room;").unwrap();
    initialize(root.path()).unwrap();
    initialize(root.path()).unwrap();
    let d = detail(root.path(), "speakers").unwrap();
    assert_eq!(d.run_id.as_deref(), Some(run.as_str()));
    assert!(!d.speakers.is_empty());
    assert!(
        d.speakers
            .iter()
            .all(|s| !s.shared_room && s.person_id.is_none())
    );
    let check: Option<Vec<u8>> = db
        .query_row(
            "SELECT key_check FROM app_voice_memory_settings WHERE singleton=1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(check.is_none());
}
