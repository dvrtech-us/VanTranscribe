//! Temporary public synthetic FLAC and transcript data only. Never targets app data.
use super::*;
use std::{
    fs,
    process::{Command, Stdio},
    thread,
    time::{Duration as StdDuration, Instant},
};
use tempfile::TempDir;
use vt_core::{AudioBlock, FramePosition, StreamFormat, TrackKind};

fn segment(id: &str) -> store::Segment {
    store::Segment {
        id: id.into(),
        track: "mic".into(),
        start_ms: 0,
        end_ms: 100,
        text: "Public synthetic retention fixture".into(),
        speaker: "Me".into(),
        words: vec![],
    }
}
fn fixture() -> TempDir {
    let t = TempDir::new().unwrap();
    store::initialize(t.path()).unwrap();
    let mut r = vt_record::Recording::create(t.path(), "fixture").unwrap();
    store::create_meeting(t.path(), "fixture", "Retention fixture", "remote").unwrap();
    for track in [TrackKind::Mic, TrackKind::System] {
        r.write_chunk(
            track,
            &AudioBlock::new(
                StreamFormat::new(8000, 1).unwrap(),
                FramePosition {
                    epoch: 0,
                    first_frame: 0,
                    host_time_ns: 0,
                },
                vec![0.125; 800],
            )
            .unwrap(),
        )
        .unwrap();
    }
    store::save_segment(t.path(), "fixture", &segment("line")).unwrap();
    r.finish().unwrap();
    store::complete_live_run(t.path(), "fixture").unwrap();
    store::set_status(t.path(), "fixture", "ready", None).unwrap();
    t
}
fn run(root: &Path, label: &str) -> String {
    let r = store::create_run(root, "fixture", "refine", "{}").unwrap();
    let mut source = segment(label);
    source.text.push_str(&format!(" {label}"));
    store::append_run_segments(root, &r.id, &[source]).unwrap();
    store::publish_run(root, "fixture", &r.id).unwrap();
    r.id
}
fn count(root: &Path, table: &str) -> u64 {
    u64::try_from(
        store::open(root)
            .unwrap()
            .query_row(&format!("SELECT count(*) FROM {}", quote(table)), [], |r| {
                r.get::<_, i64>(0)
            })
            .unwrap(),
    )
    .unwrap()
}
fn intent(root: &Path, token: &str) -> String {
    store::open(root)
        .unwrap()
        .query_row(
            "SELECT state FROM app_retention_intents WHERE token=?1",
            [token],
            |r| r.get(0),
        )
        .unwrap()
}
fn summary(root: &Path) {
    let d = store::detail(root, "fixture").unwrap();
    store::save_summary(
        root,
        "fixture",
        d.active_run_id.as_deref().unwrap(),
        d.correction_version,
        "fixture",
        "fixture",
        "general",
        "{}",
        "Cited fixture summary",
        &[d.segments[0].id.clone()],
    )
    .unwrap();
}

#[test]
fn audio_purge_keeps_transcript_citations_metadata_and_search_without_fabricated_gaps() {
    let t = fixture();
    let root = t.path();
    summary(root);
    let d = store::detail(root, "fixture").unwrap();
    let before = d.correction_version;
    let paths = fs::read_dir(root.join("meetings/fixture/audio/mic"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect::<Vec<_>>();
    let bytes = fs::read(&paths[0]).unwrap();
    assert!(bytes.starts_with(b"fLaC"));
    let p = plan(root, "fixture", DeletionKind::Audio).unwrap();
    assert!(p.bytes > 0);
    assert_eq!(p.kind, "audio");
    assert_eq!(count(root, "audio_chunks"), 2);
    assert!(audio_available(root, "fixture").unwrap());
    assert!(!has_pending(root, "fixture").unwrap());
    let report = commit(root, &p.token).unwrap();
    assert_eq!(report.audio_purged, 1);
    assert!(report.pending_cleanup.is_empty());
    assert!(!root.join("meetings/fixture/audio").exists());
    assert_eq!(count(root, "audio_chunks"), 0);
    assert!(!audio_available(root, "fixture").unwrap());
    let after = store::detail(root, "fixture").unwrap();
    assert_eq!(after.segments[0].text, d.segments[0].text);
    assert_eq!(after.summaries[0].source_segment_ids, vec!["line"]);
    assert!(!after.summaries[0].is_stale);
    assert_eq!(after.correction_version, before);
    assert_eq!(store::search_meetings(root, "retention").unwrap().len(), 1);
    assert!(root.join("meetings/fixture/.recording.lock").exists());
    assert_eq!(intent(root, &p.token), "done");
}

#[test]
fn whole_meeting_deletion_handles_fk_graph_completed_speakers_bleed_corrections_and_exports() {
    let t = fixture();
    let root = t.path();
    let first = store::detail(root, "fixture")
        .unwrap()
        .active_run_id
        .unwrap();
    store::set_bleed_decision(root, "fixture", &first, "line", "bleed").unwrap();
    let current = store::create_run(root, "fixture", "refine", "{}").unwrap();
    store::append_run_segments(root, &current.id, &[segment("second")]).unwrap();
    let result = vt_diarize::DiarizationResult {
        turns: vec![vt_diarize::SpeakerTurn {
            id: "turn".into(),
            track: "mic".into(),
            start_ms: 0,
            end_ms: 100,
            speaker_ids: vec!["mic:me".into()],
            label: "Me".into(),
            provenance: "synthetic".into(),
            score: None,
            overlap: false,
            uncertain: false,
            provisional: false,
        }],
        speakers: vec![],
        merges: vec![],
        constraint_reviews: vec![],
        segmentation_context: None,
        embedding_version: "synthetic".into(),
        calibrated: false,
        warnings: vec![],
    };
    store::save_diarization_result(root, "fixture", &current.id, &result, false).unwrap();
    store::publish_run(root, "fixture", &current.id).unwrap();
    let identity: String = store::open(root)
        .unwrap()
        .query_row(
            "SELECT identity_id FROM app_run_speaker_keys WHERE run_id=?1",
            [&current.id],
            |r| r.get(0),
        )
        .unwrap();
    store::rename_speaker(root, "fixture", &identity, "Fixture Alice").unwrap();
    store::correct_segment(
        root,
        "fixture",
        &current.id,
        "second",
        "Corrected fixture text",
    )
    .unwrap();
    summary(root);
    store::save_note(root, "fixture", 0, "Fixture bookmark").unwrap();
    let export = store::export(root, "fixture", "json").unwrap();
    let outsider = root.join("Exports/fixture-not-a-uuid.txt");
    fs::write(&outsider, b"unowned export").unwrap();
    let db = store::open(root).unwrap();
    db.execute_batch(include_str!(
        "../../../documentation/features/capture/recording-health/sql/001-resource-health.sql"
    ))
    .unwrap();
    db.execute(
        "INSERT INTO app_note_timing SELECT id,'source-clock',1 FROM app_notes",
        [],
    )
    .unwrap();
    db.execute("INSERT INTO app_source_timing VALUES('fixture','{}',0)", [])
        .unwrap();
    db.execute("INSERT INTO app_meeting_health(meeting_id,source_ms,free_bytes,disk_state,inference_suspended,created_at) VALUES('fixture',0,1024,'healthy',0,'fixture')",[]).unwrap();
    db.execute(
        "INSERT INTO app_resource_health VALUES(1,'{}','fixture')",
        [],
    )
    .unwrap();
    db.execute("INSERT INTO app_output_routes(meeting_id,epoch,start_ns,route_json) VALUES('fixture',0,0,'{}')",[]).unwrap();
    db.execute("INSERT INTO app_route_evidence VALUES('fixture',1)", [])
        .unwrap();
    assert!(
        db.execute("DELETE FROM app_run_segments WHERE run_id=?1", [&first])
            .is_err()
    );
    assert!(db.execute("DELETE FROM app_bleed_decisions", []).is_err());
    assert!(
        db.execute("UPDATE app_run_segments SET text='mutation'", [])
            .is_err()
    );
    let p = plan(root, "fixture", DeletionKind::Meeting).unwrap();
    assert!(p.foreign_key_tables.contains(&"app_bleed_decisions".into()));
    assert!(p.runs >= 2);
    let r = commit(root, &p.token).unwrap();
    assert_eq!(r.meetings_deleted, 1);
    assert!(!root.join("meetings/fixture").exists());
    assert!(export.exists());
    assert_eq!(fs::read(&outsider).unwrap(), b"unowned export");
    for table in OWNED {
        if !has_table(&db, table).unwrap() {
            continue;
        }
        assert_eq!(count(root, table), 0, "{table}");
    }
    assert_eq!(count(root, "app_search_fts"), 0);
    assert_eq!(count(root, "app_retention_gate"), 0);
    assert_eq!(count(root, "app_resource_health"), 1);
    assert_eq!(intent(root, &p.token), "done");
    let mut q = db.prepare("PRAGMA foreign_key_check").unwrap();
    assert!(!q.exists([]).unwrap());
}

#[test]
fn stale_previews_busy_writers_and_unrecognized_fk_descendants_fail_closed() {
    let t = fixture();
    let root = t.path();
    let p = plan(root, "fixture", DeletionKind::Audio).unwrap();
    store::save_note(root, "fixture", 0, "Changed after preview").unwrap();
    assert!(
        commit(root, &p.token)
            .unwrap_err()
            .to_string()
            .contains("stale")
    );
    assert_eq!(intent(root, &p.token), "preview");
    for state in ["recording", "paused", "processing"] {
        store::set_status(root, "fixture", state, None).unwrap();
        assert!(plan(root, "fixture", DeletionKind::Meeting).is_err());
    }
    store::set_status(root, "fixture", "ready", None).unwrap();
    let lease = fs::File::open(root.join("meetings/fixture/.recording.lock")).unwrap();
    lease.try_lock().unwrap();
    assert!(plan(root, "fixture", DeletionKind::Audio).is_err());
    drop(lease);
    let running = store::create_run(root, "fixture", "refine", "{}").unwrap();
    assert!(plan(root, "fixture", DeletionKind::Audio).is_err());
    let policy = preview_policy(root, &MaintenancePolicy::default()).unwrap();
    assert_eq!(policy.skipped_busy, vec!["fixture"]);
    assert!(policy.actions.is_empty());
    store::open(root)
        .unwrap()
        .execute(
            "UPDATE app_processing_runs SET status='abandoned' WHERE id=?1",
            [running.id],
        )
        .unwrap();
    for policy in [
        MaintenancePolicy {
            audio_keep_days: Some(0),
            obsolete_run_days: 7,
            cleanup_obsolete_runs: true,
        },
        MaintenancePolicy {
            audio_keep_days: None,
            obsolete_run_days: 0,
            cleanup_obsolete_runs: true,
        },
        MaintenancePolicy {
            audio_keep_days: Some(u32::MAX),
            obsolete_run_days: 7,
            cleanup_obsolete_runs: true,
        },
    ] {
        assert!(preview_policy(root, &policy).is_err());
    }
    let db = store::open(root).unwrap();
    db.execute(
        "CREATE TABLE future_owned(id TEXT,meeting_id TEXT REFERENCES app_meetings(id))",
        [],
    )
    .unwrap();
    assert!(
        plan(root, "fixture", DeletionKind::Meeting)
            .unwrap_err()
            .to_string()
            .contains("Unknown foreign-key")
    );
    assert_eq!(count(root, "audio_chunks"), 2);
}

#[test]
fn database_and_filesystem_failures_restore_original_files_and_do_not_loosen_guards() {
    let t = fixture();
    let root = t.path();
    let path = root.join("meetings/fixture/audio/mic/0-0.flac");
    let bytes = fs::read(&path).unwrap();
    let p = plan(root, "fixture", DeletionKind::Meeting).unwrap();
    let db = store::open(root).unwrap();
    db.execute_batch("CREATE TRIGGER fixture_delete_failure BEFORE DELETE ON app_meetings BEGIN SELECT RAISE(ABORT,'injected DB failure'); END;").unwrap();
    assert!(
        commit(root, &p.token)
            .unwrap_err()
            .to_string()
            .contains("restored")
    );
    assert_eq!(fs::read(&path).unwrap(), bytes);
    assert_eq!(count(root, "audio_chunks"), 2);
    assert_eq!(count(root, "app_retention_gate"), 0);
    assert_eq!(intent(root, &p.token), "failed");
    assert!(db.execute("DELETE FROM app_run_segments", []).is_err());
    db.execute_batch("DROP TRIGGER fixture_delete_failure")
        .unwrap();
    fs::remove_dir(root.join(".retention-trash").join(&p.token)).unwrap();
    fs::remove_dir(root.join(".retention-trash")).unwrap();
    fs::write(root.join(".retention-trash"), b"blocks directory").unwrap();
    let p = plan(root, "fixture", DeletionKind::Audio).unwrap();
    assert!(commit(root, &p.token).is_err());
    assert_eq!(fs::read(path).unwrap(), bytes);
    assert!(audio_available(root, "fixture").unwrap());
}

#[cfg(unix)]
#[test]
fn capability_paths_refuse_symlinks_and_outside_manifest_without_touching_outsiders() {
    use std::os::unix::fs::symlink;
    let t = fixture();
    let outside = TempDir::new().unwrap();
    let sentinel = outside.path().join("private.txt");
    fs::write(&sentinel, b"must survive").unwrap();
    let root = t.path();
    symlink(outside.path(), root.join("meetings/fixture/escape")).unwrap();
    assert!(plan(root, "fixture", DeletionKind::Meeting).is_err());
    fs::remove_file(root.join("meetings/fixture/escape")).unwrap();
    let db = store::open(root).unwrap();
    db.execute(
        "UPDATE audio_chunks SET path=?1 WHERE track='mic'",
        [sentinel.to_str().unwrap()],
    )
    .unwrap();
    assert!(plan(root, "fixture", DeletionKind::Audio).is_err());
    assert_eq!(fs::read(&sentinel).unwrap(), b"must survive");
    assert!(plan(root, "../fixture", DeletionKind::Meeting).is_err());
}

#[test]
fn seven_day_gc_preserves_active_recent_running_citations_corrections_and_bleed_audit_sources() {
    let t = fixture();
    let root = t.path();
    let db = store::open(root).unwrap();
    let bleed = store::detail(root, "fixture")
        .unwrap()
        .active_run_id
        .unwrap();
    store::set_bleed_decision(root, "fixture", &bleed, "line", "bleed").unwrap();
    let cited = run(root, "cited");
    summary(root);
    let corrected = run(root, "corrected");
    store::correct_segment(root, "fixture", &corrected, "corrected", "Corrected source").unwrap();
    let collectible = run(root, "collectible");
    let abandoned = store::create_run(root, "fixture", "replay", "{}").unwrap();
    db.execute(
        "UPDATE app_processing_runs SET status='abandoned' WHERE id=?1",
        [&abandoned.id],
    )
    .unwrap();
    let recent = run(root, "recent");
    let active = run(root, "active");
    db.execute(
        "UPDATE app_processing_runs SET created_at='2000-01-01T00:00:00Z' WHERE id<>?1",
        [&recent],
    )
    .unwrap();
    let p = preview_policy(root, &MaintenancePolicy::default()).unwrap();
    assert_eq!(p.actions.len(), 1);
    assert_eq!(p.actions[0].kind, "runs");
    assert_eq!(p.actions[0].runs, 2);
    assert!(p.actions[0].protected_references >= 3);
    assert!(p.protected_runs >= 5);
    let r = apply_policy(root, &p).unwrap();
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    assert_eq!(r.runs_deleted, 2);
    for id in [&bleed, &cited, &corrected, &recent, &active] {
        assert!(
            db.query_row(
                "SELECT EXISTS(SELECT 1 FROM app_processing_runs WHERE id=?1)",
                [id],
                |r| r.get::<_, bool>(0)
            )
            .unwrap()
        );
    }
    for id in [&collectible, &abandoned.id] {
        assert!(
            !db.query_row(
                "SELECT EXISTS(SELECT 1 FROM app_processing_runs WHERE id=?1)",
                [id],
                |r| r.get::<_, bool>(0)
            )
            .unwrap()
        );
    }
    assert!(audio_available(root, "fixture").unwrap());
    assert_eq!(count(root, "audio_chunks"), 2);
    assert!(
        db.execute("DELETE FROM app_run_segments WHERE run_id=?1", [&bleed])
            .is_err()
    );
}

#[test]
fn keep_all_transcript_versions_preserves_unreferenced_old_runs_while_audio_policy_still_purges() {
    let legacy: MaintenancePolicy =
        serde_json::from_str(r#"{"audio_keep_days":null,"obsolete_run_days":7}"#).unwrap();
    assert!(legacy.cleanup_obsolete_runs);
    assert!(MaintenancePolicy::default().cleanup_obsolete_runs);
    let t = fixture();
    let root = t.path();
    let old = store::detail(root, "fixture")
        .unwrap()
        .active_run_id
        .unwrap();
    let active = run(root, "active");
    let db = store::open(root).unwrap();
    db.execute(
        "UPDATE app_meetings SET started_at='2000-01-01T00:00:00Z'",
        [],
    )
    .unwrap();
    db.execute(
        "UPDATE app_processing_runs SET created_at='2000-01-01T00:00:00Z'",
        [],
    )
    .unwrap();
    let keep = MaintenancePolicy {
        audio_keep_days: None,
        obsolete_run_days: 7,
        cleanup_obsolete_runs: false,
    };
    let keep_preview = preview_policy(root, &keep).unwrap();
    assert!(keep_preview.actions.is_empty());
    assert_eq!(keep_preview.protected_runs, 2);
    assert_eq!(count(root, "audio_chunks"), 2);
    let audio_policy = MaintenancePolicy {
        audio_keep_days: Some(30),
        ..keep
    };
    let preview = preview_policy(root, &audio_policy).unwrap();
    assert_eq!(preview.actions.len(), 1);
    assert_eq!(preview.actions[0].kind, "audio");
    assert_eq!(preview.actions[0].runs, 0);
    assert!(!preview.policy.cleanup_obsolete_runs);
    assert_eq!(preview.actions[0].protected_runs, 2);
    let report = apply_policy(root, &preview).unwrap();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(report.audio_purged, 1);
    assert_eq!(report.runs_deleted, 0);
    assert!(!audio_available(root, "fixture").unwrap());
    for id in [old, active] {
        assert!(
            db.query_row(
                "SELECT EXISTS(SELECT 1 FROM app_processing_runs WHERE id=?1)",
                [id],
                |r| r.get::<_, bool>(0)
            )
            .unwrap()
        );
    }
    assert_eq!(count(root, "app_run_segments"), 2);
    assert_eq!(
        serde_json::to_value(&audio_policy).unwrap()["cleanup_obsolete_runs"],
        false
    );
    assert!(
        MaintenancePolicy {
            audio_keep_days: None,
            obsolete_run_days: 0,
            cleanup_obsolete_runs: false
        }
        .validate()
        .is_err()
    );
}

#[test]
fn explicit_audio_policy_and_history_gc_share_one_atomic_per_meeting_action() {
    let t = fixture();
    let root = t.path();
    let old = store::detail(root, "fixture")
        .unwrap()
        .active_run_id
        .unwrap();
    let active = run(root, "active");
    let db = store::open(root).unwrap();
    db.execute(
        "UPDATE app_meetings SET started_at='2000-01-01T00:00:00Z'",
        [],
    )
    .unwrap();
    db.execute(
        "UPDATE app_processing_runs SET created_at='2000-01-01T00:00:00Z'",
        [],
    )
    .unwrap();
    let default = preview_policy(root, &MaintenancePolicy::default()).unwrap();
    assert_eq!(default.actions[0].kind, "runs");
    assert_eq!(count(root, "audio_chunks"), 2);
    let preview = preview_policy(
        root,
        &MaintenancePolicy {
            audio_keep_days: Some(30),
            obsolete_run_days: 7,
            cleanup_obsolete_runs: true,
        },
    )
    .unwrap();
    assert_eq!(preview.actions.len(), 1);
    assert_eq!(preview.actions[0].kind, "audio");
    assert_eq!(preview.actions[0].runs, 1);
    let report = apply_policy(root, &preview).unwrap();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(report.audio_purged, 1);
    assert_eq!(report.runs_deleted, 1);
    assert!(!audio_available(root, "fixture").unwrap());
    assert!(
        !db.query_row(
            "SELECT EXISTS(SELECT 1 FROM app_processing_runs WHERE id=?1)",
            [old],
            |r| r.get::<_, bool>(0)
        )
        .unwrap()
    );
    assert!(
        db.query_row(
            "SELECT EXISTS(SELECT 1 FROM app_processing_runs WHERE id=?1)",
            [active],
            |r| r.get::<_, bool>(0)
        )
        .unwrap()
    );
}

#[cfg(unix)]
#[test]
fn confirmed_moved_audio_stays_unavailable_when_recovery_cannot_restore_or_finish() {
    let t = fixture();
    let root = t.path();
    let p = plan(root, "fixture", DeletionKind::Audio).unwrap();
    assert!(!has_pending(root, "fixture").unwrap());
    assert!(audio_available(root, "fixture").unwrap());
    kill_at(root, &p.token, "moved");
    assert!(has_pending(root, "fixture").unwrap());
    assert!(!audio_available(root, "fixture").unwrap());
    let db = store::open(root).unwrap();
    let hashes_before: Vec<(String, String)> = db
        .prepare("SELECT track,sha256 FROM audio_chunks ORDER BY track")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    fs::create_dir(root.join("meetings/fixture/audio")).unwrap();
    let obstruction = root.join("meetings/fixture/audio/restore-obstruction.txt");
    fs::write(&obstruction, b"never overwrite a new source").unwrap();
    let report = recover(root).unwrap();
    assert!(!report.errors.is_empty());
    assert_eq!(report.pending_cleanup, vec![p.token.clone()]);
    assert_eq!(intent(root, &p.token), "confirmed");
    let persisted_error: String = db
        .query_row(
            "SELECT error FROM app_retention_intents WHERE token=?1",
            [&p.token],
            |r| r.get(0),
        )
        .unwrap();
    assert!(persisted_error.contains("pending recovery; restore failed"));
    assert!(has_pending(root, "fixture").unwrap());
    assert!(!audio_available(root, "fixture").unwrap());
    assert_eq!(count(root, "audio_chunks"), 2);
    let ready: i64 = db
        .query_row(
            "SELECT count(*) FROM audio_chunks WHERE state='ready'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(ready, 2);
    let hashes_after: Vec<(String, String)> = db
        .prepare("SELECT track,sha256 FROM audio_chunks ORDER BY track")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(hashes_before, hashes_after);
    assert_eq!(
        fs::read(&obstruction).unwrap(),
        b"never overwrite a new source"
    );
    fs::remove_file(obstruction).unwrap();
    fs::remove_dir(root.join("meetings/fixture/audio")).unwrap();
    let recovered = recover(root).unwrap();
    assert!(recovered.errors.is_empty());
    assert_eq!(recovered.audio_purged, 1);
    assert!(!has_pending(root, "fixture").unwrap());
    assert!(!audio_available(root, "fixture").unwrap());
}

#[test]
fn cleanup_intents_remain_pending_even_after_the_meeting_database_rows_are_gone() {
    let t = fixture();
    let root = t.path();
    let p = plan(root, "fixture", DeletionKind::Meeting).unwrap();
    kill_at(root, &p.token, "database");
    assert_eq!(count(root, "app_meetings"), 0);
    assert!(has_pending(root, "fixture").unwrap());
    assert!(!audio_available(root, "fixture").unwrap());
    let report = recover(root).unwrap();
    assert!(report.errors.is_empty());
    assert_eq!(report.meetings_deleted, 1);
    assert!(!has_pending(root, "fixture").unwrap());
    assert!(!audio_available(root, "fixture").unwrap());
}

#[cfg(unix)]
#[test]
fn committed_deletion_reports_pending_cleanup_without_following_a_trash_symlink() {
    use std::os::unix::fs::symlink;
    let t = fixture();
    let root = t.path();
    let p = plan(root, "fixture", DeletionKind::Audio).unwrap();
    kill_at(root, &p.token, "database");
    let outside = TempDir::new().unwrap();
    let sentinel = outside.path().join("private.txt");
    fs::write(&sentinel, b"outside survives").unwrap();
    let link = root.join(trash_base(&p.token)).join("escape");
    symlink(outside.path(), &link).unwrap();
    let report = recover(root).unwrap();
    assert_eq!(report.pending_cleanup, vec![p.token.clone()]);
    assert!(!report.errors.is_empty());
    assert_eq!(intent(root, &p.token), "cleanup");
    assert!(!audio_available(root, "fixture").unwrap());
    assert_eq!(fs::read(sentinel).unwrap(), b"outside survives");
    fs::remove_file(link).unwrap();
    let recovered = recover(root).unwrap();
    assert!(recovered.errors.is_empty());
    assert_eq!(recovered.audio_purged, 1);
    assert_eq!(intent(root, &p.token), "done");
}

fn kill_at(root: &Path, token: &str, phase: &str) {
    let marker = root.join(format!("kill-marker-{phase}"));
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "retention::tests::retention_crash_child",
            "--ignored",
            "--nocapture",
        ])
        .env("VT_RETENTION_CHILD_ROOT", root)
        .env("VT_RETENTION_CHILD_TOKEN", token)
        .env("VT_RETENTION_TEST_PHASE", phase)
        .env("VT_RETENTION_TEST_MARKER", &marker)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + StdDuration::from_secs(15);
    while !marker.exists() {
        if let Some(status) = child.try_wait().unwrap() {
            panic!("child exited before {phase}: {status}");
        }
        assert!(Instant::now() < deadline, "child did not reach {phase}");
        thread::sleep(StdDuration::from_millis(10));
    }
    child.kill().unwrap();
    child.wait().unwrap();
}

#[test]
fn only_confirmed_intents_recover_and_actual_child_kill_at_each_phase_is_safe() {
    if std::env::var_os("VT_RETENTION_CHILD_ROOT").is_some() {
        return;
    }
    for phase in ["confirmed", "moved", "database"] {
        let t = fixture();
        let root = t.path();
        let unconfirmed = plan(root, "fixture", DeletionKind::Meeting).unwrap();
        let p = plan(root, "fixture", DeletionKind::Audio).unwrap();
        let marker = root.join("kill-marker");
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "retention::tests::retention_crash_child",
                "--ignored",
                "--nocapture",
            ])
            .env("VT_RETENTION_CHILD_ROOT", root)
            .env("VT_RETENTION_CHILD_TOKEN", &p.token)
            .env("VT_RETENTION_TEST_PHASE", phase)
            .env("VT_RETENTION_TEST_MARKER", &marker)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + StdDuration::from_secs(15);
        while !marker.exists() {
            if let Some(status) = child.try_wait().unwrap() {
                panic!("child exited before {phase}: {status}");
            }
            assert!(Instant::now() < deadline, "child did not reach {phase}");
            thread::sleep(StdDuration::from_millis(10));
        }
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(plan(root, "fixture", DeletionKind::Meeting).is_err());
        let report = recover(root).unwrap();
        assert!(report.errors.is_empty(), "{phase}: {:?}", report.errors);
        assert_eq!(report.audio_purged, 1);
        assert!(!audio_available(root, "fixture").unwrap());
        assert!(store::detail(root, "fixture").is_ok());
        assert!(
            !store::open(root)
                .unwrap()
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM app_retention_intents WHERE token=?1)",
                    [&unconfirmed.token],
                    |r| r.get::<_, bool>(0)
                )
                .unwrap(),
            "successful deletion invalidates other meeting previews"
        );
        assert_eq!(intent(root, &p.token), "done");
        assert_eq!(count(root, "app_retention_gate"), 0);
    }
}
#[test]
#[ignore = "only launched by isolated parent crash regression"]
fn retention_crash_child() {
    let Some(root) = std::env::var_os("VT_RETENTION_CHILD_ROOT") else {
        return;
    };
    let token = std::env::var("VT_RETENTION_CHILD_TOKEN").unwrap();
    commit(Path::new(&root), &token).unwrap();
}

#[test]
fn repeated_previews_are_bounded_and_expired_tokens_never_touch_audio() {
    let t = fixture();
    let root = t.path();
    let path = root.join("meetings/fixture/audio/mic/0-0.flac");
    let source = fs::read(&path).unwrap();
    let mut tokens = Vec::new();
    for _ in 0..40 {
        tokens.push(plan(root, "fixture", DeletionKind::Audio).unwrap().token);
        let db = store::open(root).unwrap();
        let previews:i64=db.query_row("SELECT count(*) FROM app_retention_intents WHERE meeting_id='fixture' AND state='preview'",[],|r|r.get(0)).unwrap();
        assert!(previews <= MAX_PREVIEWS_PER_MEETING);
    }
    assert_eq!(count(root, "app_retention_intents"), 4);
    assert!(
        commit(root, &tokens[0])
            .unwrap_err()
            .to_string()
            .contains("fresh preview")
    );
    assert!(
        meeting_for_token(root, &tokens[0])
            .unwrap_err()
            .to_string()
            .contains("fresh preview")
    );
    let expired = tokens.last().unwrap();
    store::open(root)
        .unwrap()
        .execute(
            "UPDATE app_retention_intents SET created_at='2000-01-01T00:00:00Z' WHERE token=?1",
            [expired],
        )
        .unwrap();
    assert!(
        commit(root, expired)
            .unwrap_err()
            .to_string()
            .contains("expired")
    );
    assert_eq!(fs::read(&path).unwrap(), source);
    assert_eq!(count(root, "audio_chunks"), 2);
    assert!(!has_pending(root, "fixture").unwrap());
    assert!(!root.join(".retention-trash").exists());
    let report = recover(root).unwrap();
    assert!(report.errors.is_empty());
    assert_eq!(count(root, "app_retention_intents"), 3);
    assert!(
        commit(root, expired)
            .unwrap_err()
            .to_string()
            .contains("expired")
    );
    let next = plan(root, "fixture", DeletionKind::Audio).unwrap();
    assert_eq!(meeting_for_token(root, &next.token).unwrap(), "fixture");
    assert!(meeting_for_token(root, "../invalid").is_err());
    assert_eq!(count(root, "app_retention_intents"), 4);
    assert_eq!(fs::read(path).unwrap(), source);
}

fn insert_receipt(
    db: &Connection,
    template: &Payload,
    state: &str,
    meeting: &str,
    created: &str,
) -> String {
    let mut p = template.clone();
    p.preview.token = uuid::Uuid::new_v4().to_string();
    p.preview.meeting_id = meeting.into();
    db.execute("INSERT INTO app_retention_intents(token,meeting_id,kind,state,payload_json,created_at,error) VALUES(?1,?2,?3,?4,?5,?6,'fixture receipt error')",params![p.preview.token,meeting,p.preview.kind,state,serde_json::to_string(&p).unwrap(),created]).unwrap();
    p.preview.token
}
#[test]
fn terminal_receipts_are_bounded_but_ancient_pending_gated_and_malformed_receipts_are_safe() {
    let t = fixture();
    let root = t.path();
    let preview = plan(root, "fixture", DeletionKind::Audio).unwrap();
    let db = store::open(root).unwrap();
    let (_, mut template) = load(&db, &preview.token).unwrap();
    template.run_ids.push("synthetic-old-run".into());
    let confirmed = insert_receipt(
        &db,
        &template,
        "confirmed",
        "confirmed-fixture",
        "2000-01-01T00:00:00Z",
    );
    let cleanup = insert_receipt(
        &db,
        &template,
        "cleanup",
        "cleanup-fixture",
        "2000-01-01T00:00:00Z",
    );
    let gated = insert_receipt(
        &db,
        &template,
        "confirmed",
        "gated-fixture",
        "2000-01-01T00:00:00Z",
    );
    db.execute("INSERT INTO app_retention_gate VALUES(?1)", [&gated])
        .unwrap();
    db.execute(
        "UPDATE app_retention_intents SET state='failed' WHERE token=?1",
        [&gated],
    )
    .unwrap();
    let payload_for = |token: &str| {
        db.query_row(
            "SELECT payload_json FROM app_retention_intents WHERE token=?1",
            [token],
            |r| r.get::<_, String>(0),
        )
        .unwrap()
    };
    let protected = vec![
        (confirmed.clone(), payload_for(&confirmed)),
        (cleanup.clone(), payload_for(&cleanup)),
        (gated.clone(), payload_for(&gated)),
    ];
    let mut newest = String::new();
    for i in 0..200 {
        newest = insert_receipt(
            &db,
            &template,
            if i % 2 == 0 { "done" } else { "failed" },
            "fixture",
            "2026-11-07T00:00:00Z",
        );
    }
    let malformed = uuid::Uuid::new_v4().to_string();
    db.execute("INSERT INTO app_retention_intents(token,meeting_id,kind,state,payload_json,created_at,error) VALUES(?1,'fixture','audio','failed','not JSON','2099-01-01T00:00:00Z','legacy malformed receipt')",[&malformed]).unwrap();
    prune(root).unwrap();
    let ordinary:i64=db.query_row("SELECT count(*) FROM app_retention_intents i WHERE state IN('done','failed') AND NOT EXISTS(SELECT 1 FROM app_retention_gate g WHERE g.token=i.token)",[],|r|r.get(0)).unwrap();
    assert_eq!(ordinary, MAX_TERMINAL_RECEIPTS);
    for (token, original) in protected {
        assert_eq!(payload_for(&token), original);
    }
    let (state, compact) = load(&db, &newest).unwrap();
    assert_eq!(state, "failed");
    assert!(compact.moves.is_empty() && compact.entries.is_empty() && compact.run_ids.is_empty());
    assert_eq!(compact.preview.files, template.preview.files);
    assert_eq!(payload_for(&malformed), "not JSON");
    assert_eq!(count(root, "audio_chunks"), 2);
    assert!(root.join("meetings/fixture/audio/mic/0-0.flac").exists());
}

#[test]
fn actual_done_and_restored_failed_operations_immediately_keep_only_minimal_receipts() {
    let t = fixture();
    let root = t.path();
    let preview = plan(root, "fixture", DeletionKind::Audio).unwrap();
    let before = store::open(root)
        .unwrap()
        .query_row(
            "SELECT length(payload_json) FROM app_retention_intents WHERE token=?1",
            [&preview.token],
            |r| r.get::<_, i64>(0),
        )
        .unwrap();
    commit(root, &preview.token).unwrap();
    let db = store::open(root).unwrap();
    let (state, p) = load(&db, &preview.token).unwrap();
    assert_eq!(state, "done");
    assert!(p.entries.is_empty() && p.moves.is_empty() && p.run_ids.is_empty());
    assert_eq!(
        serde_json::to_value(&p.preview).unwrap(),
        serde_json::to_value(&preview).unwrap()
    );
    let after: i64 = db
        .query_row(
            "SELECT length(payload_json) FROM app_retention_intents WHERE token=?1",
            [&preview.token],
            |r| r.get(0),
        )
        .unwrap();
    assert!(after < before);
    let failed = plan(root, "fixture", DeletionKind::Meeting).unwrap();
    db.execute_batch("CREATE TRIGGER fixture_receipt_failure BEFORE DELETE ON app_meetings BEGIN SELECT RAISE(ABORT,'receipt rollback fixture'); END;").unwrap();
    assert!(commit(root, &failed.token).is_err());
    let (state, p) = load(&db, &failed.token).unwrap();
    assert_eq!(state, "failed");
    assert!(p.entries.is_empty() && p.moves.is_empty() && p.run_ids.is_empty());
    assert_eq!(p.preview.files, failed.files);
    let error: String = db
        .query_row(
            "SELECT error FROM app_retention_intents WHERE token=?1",
            [&failed.token],
            |r| r.get(0),
        )
        .unwrap();
    assert!(error.contains("receipt rollback fixture"));
    assert!(store::detail(root, "fixture").is_ok());
}

#[test]
fn recovery_reports_failure_even_when_old_failed_receipt_is_pruned_under_pressure() {
    let t = fixture();
    let root = t.path();
    let preview = plan(root, "fixture", DeletionKind::Meeting).unwrap();
    let db = store::open(root).unwrap();
    let (_, template) = load(&db, &preview.token).unwrap();
    db.execute("UPDATE app_retention_intents SET state='confirmed',created_at='2000-01-01T00:00:00Z',confirmed_at='2000-01-01T00:00:00Z' WHERE token=?1",[&preview.token]).unwrap();
    for _ in 0..128 {
        insert_receipt(&db, &template, "done", "fixture", "2099-01-01T00:00:00Z");
    }
    db.execute_batch("CREATE TRIGGER fixture_receipt_failure BEFORE DELETE ON app_meetings BEGIN SELECT RAISE(ABORT,'receipt rollback fixture'); END;").unwrap();
    let report = recover(root).unwrap();
    assert_eq!(report.errors.len(), 1);
    assert!(report.errors[0].contains("original files restored"));
    assert!(report.pending_cleanup.is_empty());
    assert!(audio_available(root, "fixture").unwrap());
    assert_eq!(count(root, "audio_chunks"), 2);
    assert!(
        !db.query_row(
            "SELECT EXISTS(SELECT 1 FROM app_retention_intents WHERE token=?1)",
            [preview.token],
            |r| r.get::<_, bool>(0)
        )
        .unwrap()
    );
}

#[test]
fn backlog_health_is_owned_fingerprinted_and_collected_with_obsolete_runs() {
    let temp = fixture();
    let root = temp.path();
    crate::backlog::initialize(root).unwrap();
    let old = store::create_run(root, "fixture", "replay", "{}").unwrap();
    let db = store::open(root).unwrap();
    db.execute(
        "INSERT INTO app_run_inference_health VALUES(?1,'fixture',?2)",
        params![
            old.id,
            serde_json::to_string(&crate::BacklogReport::default()).unwrap()
        ],
    )
    .unwrap();
    store::publish_run(root, "fixture", &old.id).unwrap();
    run(root, "latest");
    db.execute(
        "UPDATE app_processing_runs SET created_at='2000-01-01T00:00:00Z' WHERE id=?1",
        [&old.id],
    )
    .unwrap();
    assert!(
        graph(&db)
            .unwrap()
            .contains(&"app_run_inference_health".into())
    );
    let preview = preview_policy(root, &MaintenancePolicy::default()).unwrap();
    let report = apply_policy(root, &preview).unwrap();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(report.runs_deleted, 1);
    assert_eq!(count(root, "app_run_inference_health"), 0);
    let deletion = plan(root, "fixture", DeletionKind::Meeting).unwrap();
    assert_eq!(commit(root, &deletion.token).unwrap().meetings_deleted, 1);
}
