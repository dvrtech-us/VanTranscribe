use super::*;
use crate::{AppService, Segment, store};
use serde_json::Value;
use tempfile::TempDir;
use vt_capture::{OutputRouteKind, OutputRouteReason, OutputTransport};

fn fixture() -> (TempDir, std::sync::Arc<AppService>, String) {
    let temp = TempDir::new().unwrap();
    // No capture helper or models are opened by construction.
    let app = AppService::new(
        temp.path().into(),
        temp.path().join("unused-capture-helper"),
    )
    .unwrap();
    let recording = vt_record::Recording::create(temp.path(), "echo-fixture").unwrap();
    store::create_meeting(
        temp.path(),
        "echo-fixture",
        "Synthetic echo fixture",
        "remote",
    )
    .unwrap();
    recording.finish().unwrap();
    let run = app.detail("echo-fixture").unwrap().active_run_id.unwrap();
    (temp, app, run)
}
fn eligible() -> OutputRoute {
    OutputRoute {
        kind: OutputRouteKind::BuiltInSpeakers,
        transport: OutputTransport::BuiltIn,
        default_output_known: true,
        data_source_known: true,
        speaker_terminal: true,
        headphone_jack_connected: Some(false),
        active_output_processes: 1,
        process_devices_known: true,
        all_processes_same_output: true,
        global_tap: true,
        reference_available: true,
        reason: OutputRouteReason::Eligible,
    }
}
fn echo(enabled: bool, applied: u64, bypassed: u64) -> EchoInfo {
    EchoInfo {
        enabled,
        applied_mic_ms: applied,
        bypassed_mic_ms: bypassed,
        engine: "synthetic-test-engine".into(),
        warnings: vec!["Synthetic diagnostic".into()],
    }
}
fn segment(id: &str) -> Segment {
    Segment {
        id: id.into(),
        track: "mic".into(),
        start_ms: 0,
        end_ms: 100,
        text: "Synthetic source".into(),
        speaker: "Me".into(),
        words: vec![],
    }
}

#[test]
fn route_evidence_is_idempotent_immutable_and_unknown_when_legacy_or_rebuild_is_unobserved() {
    let (temp, app, _) = fixture();
    let root = temp.path();
    let db = store::open(root).unwrap();
    assert!(!routes_complete(&db, "echo-fixture").unwrap());
    assert!(route_at(&db, "echo-fixture", 0).unwrap().is_none());
    assert!(route_at(&db, "echo-fixture", 99_999).unwrap().is_none());
    let first = RouteObservation {
        epoch: 0,
        start_ns: 10_000_000,
        route: eligible(),
    };
    record_route(root, "echo-fixture", &first).unwrap();
    record_route(root, "echo-fixture", &first).unwrap();
    assert!(route_at(&db, "echo-fixture", 9).unwrap().is_none());
    let observed = route_at(&db, "echo-fixture", 10).unwrap().unwrap();
    assert_eq!((observed.epoch, observed.start_ns), (0, 10_000_000));
    assert!(observed.route.aec_eligible());
    assert_eq!(observed.route, first.route);
    let stored: String = db
        .query_row(
            "SELECT route_json FROM app_output_routes WHERE meeting_id='echo-fixture'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        serde_json::from_str::<OutputRoute>(&stored).unwrap(),
        first.route
    );

    let conflict = RouteObservation {
        route: OutputRoute::default(),
        ..first.clone()
    };
    assert!(record_route(root, "echo-fixture", &conflict).is_err());
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM app_output_routes WHERE meeting_id='echo-fixture'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    assert!(
        route_at(&db, "echo-fixture", 79)
            .unwrap()
            .unwrap()
            .route
            .aec_eligible()
    );
    // A gap resets DSP state at the caller; it does not guess a physical route.
    // The next epoch's unreadable output explicitly replaces prior evidence.
    let unknown = RouteObservation {
        epoch: 1,
        start_ns: 80_000_000,
        route: OutputRoute::default(),
    };
    record_route(root, "echo-fixture", &unknown).unwrap();
    let after_gap = route_at(&db, "echo-fixture", 80).unwrap().unwrap();
    assert_eq!(after_gap.epoch, 1);
    assert_eq!(after_gap.route.reason, OutputRouteReason::Unknown);
    assert!(!after_gap.route.aec_eligible());
    assert!(
        !route_at(&db, "echo-fixture", 999)
            .unwrap()
            .unwrap()
            .route
            .aec_eligible()
    );
    assert!(
        db.execute(
            "UPDATE app_output_routes SET route_json='{}' WHERE meeting_id='echo-fixture'",
            []
        )
        .is_err()
    );
    assert!(!routes_complete(&db, "echo-fixture").unwrap());
    seal_routes(root, "echo-fixture").unwrap();
    seal_routes(root, "echo-fixture").unwrap();
    assert!(routes_complete(&db, "echo-fixture").unwrap());
    store::complete_live_run(root, "echo-fixture").unwrap();
    assert!(record_route(root, "echo-fixture", &first).is_err());
    assert!(
        record_route(
            root,
            "echo-fixture",
            &RouteObservation {
                epoch: 2,
                start_ns: 100_000_000,
                route: eligible()
            }
        )
        .is_err()
    );
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM app_output_routes WHERE meeting_id='echo-fixture'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        2
    );
    assert!(app.detail("echo-fixture").unwrap().echo.is_none());
}

#[test]
fn run_echo_provenance_has_ownership_sealing_history_and_json_export() {
    let (temp, app, live) = fixture();
    let root = temp.path();
    let other_recording = vt_record::Recording::create(root, "other-fixture").unwrap();
    store::create_meeting(root, "other-fixture", "Other synthetic fixture", "remote").unwrap();
    other_recording.finish().unwrap();
    let foreign = app.detail("other-fixture").unwrap().active_run_id.unwrap();
    let original = echo(true, 60, 40);
    assert!(save_info(root, "echo-fixture", &foreign, &original).is_err());
    assert!(save_info(root, "other-fixture", &live, &original).is_err());
    assert!(info(&store::open(root).unwrap(), &live).unwrap().is_none());
    save_info(root, "echo-fixture", &live, &echo(true, 20, 0)).unwrap();
    save_info(root, "echo-fixture", &live, &original).unwrap();
    store::save_segment(root, "echo-fixture", &segment("live-source")).unwrap();
    store::complete_live_run(root, "echo-fixture").unwrap();
    assert!(save_info(root, "echo-fixture", &live, &echo(false, 0, 100)).is_err());
    assert_eq!(
        app.detail("echo-fixture")
            .unwrap()
            .echo
            .as_ref()
            .unwrap()
            .applied_mic_ms,
        60
    );
    let first_export: Value = serde_json::from_slice(
        &std::fs::read(app.export("echo-fixture", "json").unwrap()).unwrap(),
    )
    .unwrap();
    assert_eq!(first_export["echo"]["engine"], "synthetic-test-engine");
    assert_eq!(first_export["echo"]["applied_mic_ms"], 60);
    assert_eq!(first_export["echo"]["bypassed_mic_ms"], 40);

    let refined = store::create_run(root, "echo-fixture", "refine", "{}").unwrap();
    save_info(root, "echo-fixture", &refined.id, &echo(false, 0, 100)).unwrap();
    store::append_run_segments(root, &refined.id, &[segment("refined-source")]).unwrap();
    store::publish_run(root, "echo-fixture", &refined.id).unwrap();
    let now = app.detail("echo-fixture").unwrap();
    assert_eq!(now.run_id.as_deref(), Some(refined.id.as_str()));
    assert!(!now.echo.unwrap().enabled);
    assert!(save_info(root, "echo-fixture", &refined.id, &original).is_err());
    let historical = app.detail_for_run("echo-fixture", Some(&live)).unwrap();
    assert_eq!(historical.echo.unwrap().applied_mic_ms, 60);
    assert_eq!(historical.segments[0].id, "live-source");
    let exported: Value = serde_json::from_slice(
        &std::fs::read(app.export("echo-fixture", "json").unwrap()).unwrap(),
    )
    .unwrap();
    assert_eq!(exported["run_id"], refined.id);
    assert_eq!(exported["echo"]["enabled"], false);
    assert_eq!(exported["echo"]["applied_mic_ms"], 0);
    assert_eq!(exported["echo"]["bypassed_mic_ms"], 100);
    let db = store::open(root).unwrap();
    assert!(
        db.execute(
            "UPDATE app_run_echo_processing SET info_json='{}' WHERE run_id=?1",
            [&live]
        )
        .is_err()
    );
    store::initialize(root).unwrap();
    store::initialize(root).unwrap();
    assert_eq!(info(&db, &live).unwrap().unwrap().applied_mic_ms, 60);
    assert_eq!(
        info(&db, &refined.id).unwrap().unwrap().bypassed_mic_ms,
        100
    );
    assert_eq!(
        db.query_row("SELECT count(*) FROM app_run_echo_processing", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        2
    );
    assert!(app.detail("other-fixture").unwrap().echo.is_none());
}

#[test]
fn adding_echo_tables_to_an_existing_complete_run_preserves_legacy_history() {
    let (temp, app, live) = fixture();
    let root = temp.path();
    store::save_segment(root, "echo-fixture", &segment("legacy-source")).unwrap();
    store::complete_live_run(root, "echo-fixture").unwrap();
    let before = app.detail_for_run("echo-fixture", Some(&live)).unwrap();
    let db = store::open(root).unwrap();
    // Model a database written before this additive migration. Both tables are
    // empty, so removing them discards no fixture data or earlier feature state.
    db.execute_batch("DROP TABLE app_output_routes; DROP TABLE app_route_evidence; DROP TABLE app_run_echo_processing;")
        .unwrap();
    store::initialize(root).unwrap();
    store::initialize(root).unwrap();
    let after = app.detail_for_run("echo-fixture", Some(&live)).unwrap();
    assert_eq!(
        serde_json::to_value(&before).unwrap(),
        serde_json::to_value(&after).unwrap()
    );
    assert!(after.echo.is_none());
    assert!(!routes_complete(&db, "echo-fixture").unwrap());
    assert!(route_at(&db, "echo-fixture", 999).unwrap().is_none());
    assert!(info(&db, &live).unwrap().is_none());
    assert!(save_info(root, "echo-fixture", &live, &echo(true, 100, 0)).is_err());
    // The SQL trigger also protects sealed runs from an accidental lower-level
    // insertion, independently of save_info's ownership/status preflight.
    assert!(
        db.execute(
            "INSERT INTO app_run_echo_processing(run_id,info_json) VALUES (?1,'{}')",
            [&live]
        )
        .is_err()
    );
    let violations: i64 = db
        .query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(violations, 0);
}
