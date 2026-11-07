use super::*;
use tempfile::TempDir;

fn fixture() -> TempDir {
    let temp = TempDir::new().unwrap();
    initialize(temp.path()).unwrap();
    let recording = vt_record::Recording::create(temp.path(), "fixture").unwrap();
    create_meeting(temp.path(), "fixture", "Meeting fixture", "remote").unwrap();
    recording.finish().unwrap();
    temp
}
fn segment(id: &str, text: &str) -> Segment {
    Segment {
        id: id.into(),
        track: "mic".into(),
        start_ms: 10,
        end_ms: 100,
        text: text.into(),
        speaker: "Me".into(),
        words: vec![TranscriptWord {
            text: text.into(),
            start_ms: 10,
            end_ms: 100,
        }],
    }
}
fn active(root: &Path) -> String {
    detail(root, "fixture").unwrap().active_run_id.unwrap()
}
fn replay(root: &Path, id: &str, text: &str) -> String {
    let run = create_run(root, "fixture", "refine", "{}").unwrap();
    append_run_segments(root, &run.id, &[segment(id, text)]).unwrap();
    publish_run(root, "fixture", &run.id).unwrap();
    run.id
}

#[test]
fn incomplete_and_abandoned_runs_cannot_replace_viewable_live_transcript() {
    let t = fixture();
    let root = t.path();
    save_segment(root, "fixture", &segment("original", "Visible final")).unwrap();
    let old = active(root);
    let run = create_run(root, "fixture", "refine", "{}").unwrap();
    append_run_segments(root, &run.id, &[segment("new", "Partial refinement")]).unwrap();
    assert_eq!(active(root), old);
    assert_eq!(
        detail(root, "fixture").unwrap().segments[0].text,
        "Visible final"
    );
    abandon_run(root, &run.id).unwrap();
    assert!(publish_run(root, "fixture", &run.id).is_err());
    assert_eq!(active(root), old);
    complete_live_run(root, "fixture").unwrap();
    assert!(save_segment(root, "fixture", &segment("late", "Too late")).is_err());
}
#[test]
fn publication_is_atomic_when_search_rebuild_fails_and_old_rows_are_immutable() {
    let t = fixture();
    let root = t.path();
    save_segment(root, "fixture", &segment("original", "Originalword")).unwrap();
    complete_live_run(root, "fixture").unwrap();
    let old = active(root);
    let run = create_run(root, "fixture", "refine", "{}").unwrap();
    append_run_segments(root, &run.id, &[segment("new", "Replacementword")]).unwrap();
    // Deliberate trigger failure after pointer/status updates proves the whole publication rolls back.
    let db = open(root).unwrap();
    db.execute_batch("CREATE TRIGGER fail_publication BEFORE UPDATE OF active_run_id ON app_meeting_revision BEGIN SELECT RAISE(ABORT,'fixture failure'); END;").unwrap();
    assert!(publish_run(root, "fixture", &run.id).is_err());
    assert_eq!(active(root), old);
    assert_eq!(search_meetings(root, "Originalword").unwrap().len(), 1);
    assert!(search_meetings(root, "Replacementword").unwrap().is_empty());
    assert_eq!(
        list_runs(root, "fixture")
            .unwrap()
            .iter()
            .find(|r| r.id == run.id)
            .unwrap()
            .status,
        "running"
    );
    db.execute_batch("DROP TRIGGER fail_publication").unwrap();
    publish_run(root, "fixture", &run.id).unwrap();
    assert!(search_meetings(root, "Originalword").unwrap().is_empty());
    assert_eq!(search_meetings(root, "Replacementword").unwrap().len(), 1);
    assert_eq!(
        detail_run(root, "fixture", &old).unwrap().segments[0].text,
        "Originalword"
    );
    assert!(
        db.execute(
            "UPDATE app_run_segments SET text='corrupt' WHERE run_id=?1",
            [old]
        )
        .is_err()
    );
    assert!(search_meetings(root, "\" OR * NEAR(foo)").is_ok());
}
#[test]
fn overlays_reapply_only_to_unique_exact_anchors_and_unmatched_can_be_resolved() {
    let t = fixture();
    let root = t.path();
    save_segment(root, "fixture", &segment("original", "Rawword")).unwrap();
    complete_live_run(root, "fixture").unwrap();
    let old = active(root);
    correct_segment(root, "fixture", &old, "original", "Correctword").unwrap();
    let matched = replay(root, "matching", "Rawword");
    assert_eq!(
        detail(root, "fixture").unwrap().segments[0].text,
        "Correctword"
    );
    let unmatched = replay(root, "different", "Changedword");
    let d = detail(root, "fixture").unwrap();
    assert_eq!(d.corrections[0].status, "needs_review");
    assert_eq!(d.segments[0].text, "Changedword");
    resolve_correction(root, "fixture", &d.corrections[0].id, Some("different")).unwrap();
    assert_eq!(
        detail(root, "fixture").unwrap().segments[0].text,
        "Correctword"
    );
    assert_eq!(
        detail_run(root, "fixture", &matched).unwrap().segments[0].text,
        "Correctword"
    );
    assert_eq!(
        open(root)
            .unwrap()
            .query_row(
                "SELECT text FROM app_run_segments WHERE run_id=?1 AND id='different'",
                [unmatched],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
        "Changedword"
    );
}
#[test]
fn summaries_reject_stale_inputs_and_foreign_sources_and_keep_citations() {
    let t = fixture();
    let root = t.path();
    save_segment(root, "fixture", &segment("source", "Evidenceword")).unwrap();
    let run = active(root);
    assert!(
        save_summary(
            root,
            "fixture",
            &run,
            0,
            "fixture",
            "fixture",
            "general",
            "{}",
            "Summary",
            &["source".into()]
        )
        .is_err()
    );
    complete_live_run(root, "fixture").unwrap();
    assert!(
        save_summary(
            root,
            "fixture",
            &run,
            0,
            "fixture",
            "fixture",
            "general",
            "{}",
            "Summary",
            &["wrong".into()]
        )
        .is_err()
    );
    save_summary(
        root,
        "fixture",
        &run,
        0,
        "fixture",
        "fixture",
        "general",
        "{}",
        "Summaryword",
        &["source".into()],
    )
    .unwrap();
    assert_eq!(search_meetings(root, "Summaryword").unwrap().len(), 1);
    correct_segment(root, "fixture", &run, "source", "Editedword").unwrap();
    let d = detail(root, "fixture").unwrap();
    assert!(d.summaries[0].is_stale);
    assert_eq!(d.summaries[0].source_segment_ids, vec!["source"]);
    assert!(search_meetings(root, "Summaryword").unwrap().is_empty());
    assert!(
        save_summary(
            root,
            "fixture",
            &run,
            0,
            "fixture",
            "fixture",
            "general",
            "{}",
            "Old answer",
            &[]
        )
        .is_err()
    );
    save_summary(
        root,
        "fixture",
        &run,
        d.correction_version,
        "fixture",
        "fixture",
        "general",
        "{}",
        "New answer",
        &["source".into()],
    )
    .unwrap();
    replay(root, "replacement", "New transcript");
    assert!(
        detail(root, "fixture")
            .unwrap()
            .summaries
            .iter()
            .all(|s| s.is_stale)
    );
    assert!(
        open(root)
            .unwrap()
            .execute("DELETE FROM app_processing_runs WHERE id=?1", [run])
            .is_err()
    );
}
#[test]
fn legacy_migration_preserves_transcripts_notes_and_is_idempotent() {
    let t = TempDir::new().unwrap();
    let root = t.path();
    vt_record::initialize(root).unwrap();
    vt_record::Recording::create(root, "fixture")
        .unwrap()
        .finish()
        .unwrap();
    let db = open(root).unwrap();
    db.execute_batch(include_str!(
        "../../../documentation/features/app/desktop-session/sql/001-desktop.sql"
    ))
    .unwrap();
    db.execute("INSERT INTO app_meetings(id,title,status,started_at,mode) VALUES ('fixture','Legacy fixture','ready','2026-01-01','remote')",[]).unwrap();
    db.execute("INSERT INTO app_segments(id,meeting_id,track,start_ms,end_ms,text,speaker) VALUES ('legacy','fixture','mic',0,100,'Archivedword','Me')",[]).unwrap();
    db.execute(
        "INSERT INTO app_notes(id,meeting_id,t_ms,text) VALUES ('note','fixture',0,'Legacy note')",
        [],
    )
    .unwrap();
    drop(db);
    initialize(root).unwrap();
    initialize(root).unwrap();
    let d = detail(root, "fixture").unwrap();
    assert_eq!(d.runs.len(), 1);
    assert_eq!(d.segments[0].text, "Archivedword");
    assert_eq!(d.notes[0].text, "Legacy note");
    assert_eq!(search_meetings(root, "Archivedword").unwrap().len(), 1);
    assert_eq!(
        open(root)
            .unwrap()
            .query_row("SELECT COUNT(*) FROM app_segments", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
    replay(root, "new", "Newword");
    assert_eq!(
        detail_run(root, "fixture", &d.run_id.unwrap())
            .unwrap()
            .segments[0]
            .text,
        "Archivedword"
    );
}

#[test]
fn legacy_replay_staging_preserves_prior_run_and_discards_canceled_work() {
    let t = fixture();
    let root = t.path();
    save_segment(root, "fixture", &segment("original", "Originalword")).unwrap();
    let old = active(root);
    stage_segment(
        root,
        "canceled",
        "fixture",
        &segment("canceled", "Canceledword"),
    )
    .unwrap();
    discard_staged(root, "canceled").unwrap();
    assert_eq!(active(root), old);
    stage_segment(
        root,
        "done",
        "fixture",
        &segment("replacement", "Replacementword"),
    )
    .unwrap();
    publish_staged(root, "done", "fixture").unwrap();
    assert_eq!(
        detail(root, "fixture").unwrap().segments[0].text,
        "Replacementword"
    );
    assert_eq!(
        detail_run(root, "fixture", &old).unwrap().segments[0].text,
        "Originalword"
    );
    assert!(append_run_segments(root, &old, &[segment("late", "Lateword")]).is_err());
}

#[test]
fn duplicate_matching_segments_leave_correction_for_review() {
    let t = fixture();
    let root = t.path();
    save_segment(root, "fixture", &segment("original", "Rawword")).unwrap();
    complete_live_run(root, "fixture").unwrap();
    correct_segment(root, "fixture", &active(root), "original", "Correctword").unwrap();
    let run = create_run(root, "fixture", "refine", "{}").unwrap();
    append_run_segments(
        root,
        &run.id,
        &[
            segment("candidate-a", "Rawword"),
            segment("candidate-b", "Rawword"),
        ],
    )
    .unwrap();
    publish_run(root, "fixture", &run.id).unwrap();
    let d = detail(root, "fixture").unwrap();
    assert_eq!(d.corrections[0].status, "needs_review");
    assert!(d.segments.iter().all(|s| s.text == "Rawword"));
}

#[test]
fn title_and_note_edits_refresh_search_without_staling_transcript_only_summaries() {
    let t = fixture();
    let root = t.path();
    save_segment(root, "fixture", &segment("source", "Evidenceword")).unwrap();
    complete_live_run(root, "fixture").unwrap();
    let run = active(root);
    let revision = detail(root, "fixture").unwrap().correction_version;
    save_summary(
        root,
        "fixture",
        &run,
        revision,
        "fixture",
        "fixture",
        "general",
        "{}",
        "Summaryword",
        &["source".into()],
    )
    .unwrap();

    rename(root, "fixture", "Titleword").unwrap();
    save_note(root, "fixture", 0, "Bookmarkword").unwrap();
    let d = detail(root, "fixture").unwrap();
    assert_eq!(d.correction_version, revision);
    assert_eq!(d.summary.as_deref(), Some("Summaryword"));
    assert!(!d.summaries[0].is_stale);
    for query in ["Titleword", "Bookmarkword", "Evidenceword", "Summaryword"] {
        assert_eq!(search_meetings(root, query).unwrap().len(), 1, "{query}");
    }
    assert!(search_meetings(root, "Meeting").unwrap().is_empty());
    // A summary generated before metadata changes still has the identical transcript input.
    save_summary(
        root,
        "fixture",
        &run,
        revision,
        "fixture",
        "fixture",
        "general",
        "{}",
        "Another grounded result",
        &["source".into()],
    )
    .unwrap();
    assert!(
        detail(root, "fixture")
            .unwrap()
            .summaries
            .iter()
            .all(|summary| !summary.is_stale)
    );
}

#[test]
fn summary_failures_persist_independently_from_transcript_health_and_can_be_cleared() {
    let t = fixture();
    let root = t.path();
    save_segment(root, "fixture", &segment("source", "Evidenceword")).unwrap();
    complete_live_run(root, "fixture").unwrap();
    set_status(
        root,
        "fixture",
        "ready",
        Some("A retained recording warning"),
    )
    .unwrap();
    let before = detail(root, "fixture").unwrap();
    assert!(before.summary_error.is_none());
    set_summary_error(root, "fixture", Some("Synthetic provider failure")).unwrap();
    // Reinitialization represents loading an existing database and must preserve the separate error.
    initialize(root).unwrap();
    let failed = detail(root, "fixture").unwrap();
    assert_eq!(
        failed.summary_error.as_deref(),
        Some("Synthetic provider failure")
    );
    assert_eq!(failed.meeting.status, before.meeting.status);
    assert_eq!(failed.meeting.error, before.meeting.error);
    assert_eq!(failed.correction_version, before.correction_version);
    assert_eq!(failed.active_run_id, before.active_run_id);
    assert_eq!(failed.segments[0].text, "Evidenceword");
    assert_eq!(search_meetings(root, "Evidenceword").unwrap().len(), 1);
    set_summary_error(root, "fixture", Some("Replacement synthetic error")).unwrap();
    assert_eq!(
        detail(root, "fixture").unwrap().summary_error.as_deref(),
        Some("Replacement synthetic error")
    );
    set_summary_error(root, "fixture", None).unwrap();
    let cleared = detail(root, "fixture").unwrap();
    assert!(cleared.summary_error.is_none());
    assert_eq!(cleared.meeting.error, before.meeting.error);
    assert!(set_summary_error(root, "missing-meeting", Some("Invalid failure")).is_err());
    set_summary_error(root, "fixture", Some("Failure from the old transcript")).unwrap();
    let next = create_run(root, "fixture", "refine", "{}").unwrap();
    append_run_segments(root, &next.id, &[segment("new-source", "Updatedword")]).unwrap();
    publish_run(root, "fixture", &next.id).unwrap();
    assert!(detail(root, "fixture").unwrap().summary_error.is_none());
}
