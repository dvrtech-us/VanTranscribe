//! Public synthetic transcript fixtures only; no capture, audio or biometric evidence.
use super::*;
use tempfile::TempDir;

fn fixture() -> TempDir {
    let root = TempDir::new().unwrap();
    initialize(root.path()).unwrap();
    let r = vt_record::Recording::create(root.path(), "bleed").unwrap();
    create_meeting(root.path(), "bleed", "Bleed fixture", "remote").unwrap();
    r.finish().unwrap();
    root
}
fn line(id: &str, track: &str, start: u64, text: &str) -> Segment {
    Segment {
        id: id.into(),
        track: track.into(),
        start_ms: start,
        end_ms: start + 500,
        text: text.into(),
        speaker: track.into(),
        words: vec![],
    }
}
fn begin(root: &Path, segments: &[Segment]) -> String {
    let run = create_run(root, "bleed", "refine", "{}").unwrap();
    append_run_segments(root, &run.id, segments).unwrap();
    run.id
}
fn publish(root: &Path, segments: &[Segment]) -> String {
    let run = begin(root, segments);
    publish_run(root, "bleed", &run).unwrap();
    run
}
fn visible(root: &Path, run: &str) -> BTreeSet<String> {
    hidden_ids(&open(root).unwrap(), run).unwrap()
}
fn revision(root: &Path) -> i64 {
    open(root)
        .unwrap()
        .query_row(
            "SELECT correction_version FROM app_meeting_revision WHERE meeting_id='bleed'",
            [],
            |r| r.get(0),
        )
        .unwrap()
}
fn audit_count(root: &Path) -> i64 {
    open(root)
        .unwrap()
        .query_row("SELECT count(*) FROM app_bleed_decisions", [], |r| r.get(0))
        .unwrap()
}
fn save_fixture_summary(root: &Path, run: &str) {
    save_summary(
        root,
        "bleed",
        run,
        revision(root) as u64,
        "fixture",
        "fixture",
        "general",
        "{}",
        "Preserved summary",
        &["mic".into()],
    )
    .unwrap();
}
fn summary_stale(root: &Path) -> bool {
    open(root)
        .unwrap()
        .query_row(
            "SELECT is_stale FROM app_summaries ORDER BY created_at DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap()
}

#[test]
fn confirm_restore_preserves_raw_and_atomically_updates_search_revision_and_summaries() {
    let t = fixture();
    let root = t.path();
    let source = line("mic", "mic", 100, "UniqueMarker remote team status green");
    let run = publish(
        root,
        &[
            source.clone(),
            line("sys", "system", 100, "remote team status green"),
        ],
    );
    save_fixture_summary(root, &run);
    let initial = revision(root);
    assert_eq!(search_meetings(root, "UniqueMarker").unwrap().len(), 1);
    let d = details(
        &open(root).unwrap(),
        "bleed",
        &run,
        &[
            source.clone(),
            line("sys", "system", 100, "remote team status green"),
        ],
    )
    .unwrap();
    assert_eq!(d.candidates.len(), 1);
    assert!(visible(root, &run).is_empty());
    set_decision(root, "bleed", &run, "mic", "bleed").unwrap();
    assert_eq!(visible(root, &run), BTreeSet::from(["mic".into()]));
    assert!(search_meetings(root, "UniqueMarker").unwrap().is_empty());
    assert_eq!(revision(root), initial + 1);
    assert!(summary_stale(root));
    save_fixture_summary(root, &run);
    assert!(!summary_stale(root));
    let before = revision(root);
    let db = open(root).unwrap();
    db.execute(
        "ALTER TABLE app_search_fts RENAME TO fixture_search_fts",
        [],
    )
    .unwrap();
    assert!(set_decision(root, "bleed", &run, "mic", "genuine").is_err());
    assert_eq!(revision(root), before);
    assert_eq!(audit_count(root), 1);
    assert!(!summary_stale(root));
    assert_eq!(visible(root, &run), BTreeSet::from(["mic".into()]));
    db.execute(
        "ALTER TABLE fixture_search_fts RENAME TO app_search_fts",
        [],
    )
    .unwrap();
    set_decision(root, "bleed", &run, "mic", "genuine").unwrap();
    assert!(visible(root, &run).is_empty());
    assert_eq!(audit_count(root), 2);
    assert_eq!(search_meetings(root, "UniqueMarker").unwrap().len(), 1);
    assert!(summary_stale(root));
    let raw = read_segments(&db, &run, false).unwrap();
    assert_eq!(
        raw.iter().find(|s| s.id == "mic").unwrap().text,
        source.text
    );
    assert!(
        db.execute("UPDATE app_bleed_decisions SET decision='genuine'", [])
            .is_err()
    );
    assert!(db.execute("DELETE FROM app_bleed_decisions", []).is_err());
    assert!(set_decision(root, "bleed", &run, "sys", "bleed").is_err());
    assert!(set_decision(root, "bleed", &run, "mic", "delete").is_err());
}

#[test]
fn exact_reapply_preserves_history_and_ambiguous_missing_decisions_stay_visible() {
    let t = fixture();
    let root = t.path();
    let source = line("mic", "mic", 100, "remote team status green");
    let old = publish(root, std::slice::from_ref(&source));
    set_decision(root, "bleed", &old, "mic", "bleed").unwrap();
    let mut next_source = source.clone();
    next_source.id = "next".into();
    let next = publish(root, std::slice::from_ref(&next_source));
    assert_eq!(visible(root, &next), BTreeSet::from(["next".into()]));
    set_decision(root, "bleed", &next, "next", "genuine").unwrap();
    assert!(visible(root, &next).is_empty());
    assert_eq!(visible(root, &old), BTreeSet::from(["mic".into()]));
    let db = open(root).unwrap();
    assert!(
        db.execute(
            "UPDATE app_bleed_run_matches SET status='dismissed' WHERE run_id=?1",
            [&old]
        )
        .is_err()
    );
    assert!(set_decision(root, "bleed", &old, "mic", "genuine").is_err());
    set_decision(root, "bleed", &next, "next", "bleed").unwrap();
    let mut duplicate = next_source.clone();
    duplicate.id = "duplicate".into();
    let ambiguous = publish(root, &[next_source.clone(), duplicate]);
    assert!(visible(root, &ambiguous).is_empty());
    let d = details(&db, "bleed", &ambiguous, &[]).unwrap();
    assert_eq!(d.decisions.len(), 1);
    assert_eq!(d.decisions[0].status, "needs_review");
    assert!(d.decisions[0].segment_id.is_none());
    let changed = publish(
        root,
        &[line(
            "changed",
            "mic",
            100,
            "Different original text entirely",
        )],
    );
    assert!(visible(root, &changed).is_empty());
    let pending = details(&db, "bleed", &changed, &[])
        .unwrap()
        .decisions
        .remove(0);
    assert_eq!(pending.status, "needs_review");
    dismiss_pending(root, "bleed", &changed, &pending.id).unwrap();
    assert_eq!(
        details(&db, "bleed", &changed, &[]).unwrap().decisions[0].status,
        "dismissed"
    );
    assert!(dismiss_pending(root, "bleed", &changed, &pending.id).is_err());
    let future = publish(root, &[source]);
    assert!(visible(root, &future).is_empty());
    assert!(
        details(&db, "bleed", &future, &[])
            .unwrap()
            .decisions
            .is_empty()
    );
    assert_eq!(
        details(&db, "bleed", &ambiguous, &[]).unwrap().decisions[0].status,
        "needs_review"
    );
    assert_eq!(visible(root, &old), BTreeSet::from(["mic".into()]));
    assert_eq!(audit_count(root), 4);
}

#[test]
fn distinct_annotations_cannot_collapse_onto_one_future_line() {
    let t = fixture();
    let root = t.path();
    let one = line("one", "mic", 0, "remote team status green");
    let mut two = one.clone();
    two.id = "two".into();
    let old = publish(root, &[one.clone(), two]);
    set_decision(root, "bleed", &old, "one", "bleed").unwrap();
    set_decision(root, "bleed", &old, "two", "genuine").unwrap();
    let db = open(root).unwrap();
    let lineages: i64 = db
        .query_row(
            "SELECT count(DISTINCT lineage_id) FROM app_bleed_decisions",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(lineages, 2);
    let next = publish(root, &[one]);
    assert!(visible(root, &next).is_empty());
    let d = details(&db, "bleed", &next, &[]).unwrap();
    assert_eq!(d.decisions.len(), 1);
    assert_eq!(d.decisions[0].decision, "bleed");
    assert!(d.decisions.iter().all(|d| d.status == "needs_review"));
}

#[test]
fn short_replies_and_genuine_repetition_are_visible_until_explicit_decision() {
    let t = fixture();
    let root = t.path();
    let segments = vec![
        line("short", "mic", 0, "Yes I agree"),
        line("shortsys", "system", 0, "Yes I agree"),
        line("repeat", "mic", 5000, "Keep our team fully informed"),
        line("sys", "system", 5000, "keep our team fully informed!"),
        line("reverse", "mic", 10000, "informed fully team our keep"),
        line(
            "reverse-sys",
            "system",
            10000,
            "keep our team fully informed",
        ),
        line("far", "mic", 15000, "Keep our team fully informed"),
        line("far-sys", "system", 17001, "Keep our team fully informed"),
    ];
    let run = publish(root, &segments);
    let db = open(root).unwrap();
    let d = details(&db, "bleed", &run, &segments).unwrap();
    assert_eq!(d.candidates.len(), 1);
    assert_eq!(d.candidates[0].mic_segment_id, "repeat");
    assert!(visible(root, &run).is_empty());
    set_decision(root, "bleed", &run, "repeat", "genuine").unwrap();
    assert!(
        details(&db, "bleed", &run, &segments)
            .unwrap()
            .candidates
            .is_empty()
    );
    set_decision(root, "bleed", &run, "short", "bleed").unwrap();
    assert_eq!(visible(root, &run), BTreeSet::from(["short".into()]));
    set_decision(root, "bleed", &run, "short", "genuine").unwrap();
    assert!(visible(root, &run).is_empty());
    assert_eq!(
        read_segments(&db, &run, false).unwrap().len(),
        segments.len()
    );
}

#[test]
fn textual_exports_exclude_confirmed_bleed_while_json_retains_speech_and_decision() {
    let t = fixture();
    let root = t.path();
    let run = publish(
        root,
        &[
            line(
                "hidden",
                "mic",
                0,
                "HiddenUnique acoustic evidence is unavailable",
            ),
            line(
                "kept",
                "system",
                1000,
                "VisibleUnique genuine source speech",
            ),
        ],
    );
    save_note(root, "bleed", 500, "NoteExportUnique retained note").unwrap();
    set_decision(root, "bleed", &run, "hidden", "bleed").unwrap();
    for format in ["txt", "markdown", "srt", "vtt"] {
        let content = fs::read_to_string(export(root, "bleed", format).unwrap()).unwrap();
        assert!(!content.contains("HiddenUnique"), "{format}");
        assert!(content.contains("VisibleUnique"), "{format}");
        if matches!(format, "txt" | "markdown") {
            assert!(
                content.contains("NoteExportUnique"),
                "notes missing from {format}"
            );
            assert!(
                content.contains("?00:00:00.500"),
                "legacy note timing must remain unknown in {format}"
            );
        }
    }
    let json: serde_json::Value =
        serde_json::from_slice(&fs::read(export(root, "bleed", "json").unwrap()).unwrap()).unwrap();
    assert_eq!(json["segments"].as_array().unwrap().len(), 2);
    assert!(
        json["segments"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["text"].as_str().unwrap().contains("HiddenUnique"))
    );
    assert_eq!(json["bleed_decisions"][0]["decision"], "bleed");
    set_decision(root, "bleed", &run, "hidden", "genuine").unwrap();
    assert!(
        fs::read_to_string(export(root, "bleed", "txt").unwrap())
            .unwrap()
            .contains("HiddenUnique")
    );
}

#[test]
fn genuine_choices_do_not_need_unmatched_review_and_still_suppress_exact_future_candidates() {
    let t = fixture();
    let root = t.path();
    let source = line("mic", "mic", 0, "Remote team status green");
    let system = line("system", "system", 0, "Remote team status green");
    let first = publish(root, &[source.clone(), system.clone()]);
    set_decision(root, "bleed", &first, "mic", "genuine").unwrap();
    let changed = publish(
        root,
        &[line("changed", "mic", 0, "Different speech now present")],
    );
    let db = open(root).unwrap();
    assert!(
        details(&db, "bleed", &changed, &[])
            .unwrap()
            .decisions
            .is_empty()
    );
    let future = publish(root, &[source.clone(), system.clone()]);
    let d = details(&db, "bleed", &future, &[source, system]).unwrap();
    assert_eq!(d.decisions[0].decision, "genuine");
    assert!(d.candidates.is_empty());
    assert!(visible(root, &future).is_empty());
}

#[test]
fn dense_ten_thousand_lines_have_bounded_deterministic_candidate_work() {
    let mut segments = Vec::new();
    for i in 0..5000 {
        segments.push(line(
            &format!("s{i:05}"),
            "system",
            1000,
            "public synthetic repeated phrase",
        ));
        segments.push(line(
            &format!("m{i:05}"),
            "mic",
            1000,
            "public synthetic repeated phrase",
        ));
    }
    let (found, checks) = candidates(&segments, &BTreeSet::new());
    assert_eq!(found.len(), 5000);
    assert_eq!(checks, 5000 * MAX_SYSTEM_CANDIDATES);
    assert!(
        found
            .iter()
            .all(|c| c.system_segment_id == "s00000" && c.similarity == 1.0)
    );
}
