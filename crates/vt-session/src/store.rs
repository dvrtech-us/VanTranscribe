use anyhow::{Result, bail};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    time::Duration,
};
#[path = "bleed_store.rs"]
mod bleed;
#[path = "speaker_store.rs"]
mod speakers;
pub use bleed::{BleedCandidate, BleedDecision};
pub(crate) use bleed::{
    dismiss_pending as resolve_bleed_review, set_decision as set_bleed_decision,
};
pub(crate) use speakers::record_warning as speaker_warning;
pub(crate) use speakers::resolve_review as resolve_speaker_review;
pub(crate) use speakers::retain_previous as retain_speaker_annotations;
pub(crate) use speakers::revoke_constraint as revoke_speaker_constraint;
pub use speakers::{
    DiarizationInfo, SpeakerAssignment, SpeakerConstraint, SpeakerInfo, SpeakerReview,
};
pub(crate) use speakers::{LiveSaveBudget, save_live_updates_with as save_live_diarization_with};
pub(crate) use speakers::{
    add_constraint as add_speaker_constraint, constraints as speaker_constraints,
    mode as meeting_mode, rename as rename_speaker, save_result as save_diarization_result,
};
pub(crate) use speakers::{
    forget_person_links, link_voice_person, set_shared_room as set_speaker_shared_room,
};

#[derive(Clone, Debug, Serialize)]
pub struct Meeting {
    pub id: String,
    pub title: String,
    pub status: String,
    pub started_at: String,
    pub duration_ms: u64,
    pub segment_count: u64,
    pub error: Option<String>,
}
#[derive(Clone, Debug, Serialize)]
pub struct Segment {
    pub id: String,
    pub track: String,
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
    pub speaker: String,
    pub words: Vec<TranscriptWord>,
}
#[derive(Clone, Debug, serde::Deserialize, Serialize)]
pub struct TranscriptWord {
    pub text: String,
    pub start_ms: u64,
    pub end_ms: u64,
}
#[derive(Clone, Debug, Serialize)]
pub struct Note {
    pub timing_provenance: Option<String>,
    pub timing_synchronized: Option<bool>,
    pub id: String,
    pub t_ms: u64,
    pub text: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct MeetingDetail {
    pub mode: crate::CaptureMode,
    pub inference_health: Option<crate::BacklogReport>,
    pub audio_available: bool,
    pub meeting: Meeting,
    pub segments: Vec<Segment>,
    pub notes: Vec<Note>,
    pub summary: Option<String>,
    pub summary_error: Option<String>,
    pub active_run_id: Option<String>,
    pub run_id: Option<String>,
    pub runs: Vec<RunInfo>,
    pub corrections: Vec<Correction>,
    pub summaries: Vec<SummaryInfo>,
    pub correction_version: u64,
    pub speakers: Vec<SpeakerInfo>,
    pub speaker_turns: Vec<vt_diarize::SpeakerTurn>,
    pub speaker_reviews: Vec<SpeakerReview>,
    pub speaker_assignments: Vec<SpeakerAssignment>,
    pub speaker_constraints: Vec<SpeakerConstraint>,
    pub diarization: Option<DiarizationInfo>,
    pub echo: Option<crate::echo_store::EchoInfo>,
    pub bleed_decisions: Vec<BleedDecision>,
    pub bleed_candidates: Vec<BleedCandidate>,
}
impl MeetingDetail {
    pub(crate) fn has_summary_input(&self) -> bool {
        let hidden = self
            .bleed_decisions
            .iter()
            .filter(|d| d.status == "applied" && d.decision == "bleed")
            .filter_map(|d| d.segment_id.as_deref())
            .collect::<std::collections::BTreeSet<_>>();
        self.segments
            .iter()
            .any(|s| !hidden.contains(s.id.as_str()))
    }
}
#[derive(Clone, Debug, Serialize)]
pub struct RunInfo {
    pub id: String,
    pub meeting_id: String,
    pub kind: String,
    pub status: String,
    pub engines_json: String,
    pub constraints_version: u64,
    pub created_at: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct Correction {
    pub id: String,
    pub track: String,
    pub start_ms: u64,
    pub end_ms: u64,
    pub original_text_hash: String,
    pub replacement_text: String,
    pub status: String,
    pub segment_id: Option<String>,
}
#[derive(Clone, Debug, Serialize)]
pub struct SummaryInfo {
    pub id: String,
    pub meeting_id: String,
    pub run_id: String,
    pub correction_version: u64,
    pub provider: String,
    pub model: String,
    pub template: String,
    pub content_json: String,
    pub content_md: String,
    pub is_stale: bool,
    pub created_at: String,
    pub source_segment_ids: Vec<String>,
}

pub(crate) fn open(root: &Path) -> Result<Connection> {
    open_with_timeout(root, Duration::from_secs(5))
}

/// Busy timeout for housekeeping writes made on the capture consumer loop.
/// That loop also feeds the recorder, so it must never wait seconds for a lock.
pub(crate) const LOOP_BUSY_TIMEOUT: Duration = Duration::from_millis(250);

pub(crate) fn open_with_timeout(root: &Path, busy_timeout: Duration) -> Result<Connection> {
    let db = Connection::open(root.join("vantranscribe.db"))?;
    db.busy_timeout(busy_timeout)?;
    db.pragma_update(None, "foreign_keys", "ON")?;
    Ok(db)
}

/// Runs one live save under `budget`, retrying only lock contention
/// (`sqlite_busy`). Each attempt must open its own connection and transaction so
/// a failed attempt releases everything before the backoff. Other errors return
/// at once. Worst case: `attempts * busy_timeout` plus the doubling backoff.
pub(crate) fn retry_busy<T>(
    budget: LiveSaveBudget,
    mut attempt: impl FnMut(Duration) -> Result<T>,
) -> Result<T> {
    let mut count = 1;
    loop {
        match attempt(budget.busy_timeout) {
            Err(error) if count < budget.attempts.max(1) && sqlite_busy(&error) => {
                std::thread::sleep(budget.backoff.saturating_mul(1 << (count - 1).min(8)));
                count += 1;
            }
            other => return other,
        }
    }
}

/// SQLite (primary, extended) result codes carried anywhere in an error chain.
pub(crate) fn sqlite_codes(error: &anyhow::Error) -> Option<(rusqlite::ErrorCode, i32)> {
    error
        .chain()
        .find_map(|cause| match cause.downcast_ref::<rusqlite::Error>()? {
            rusqlite::Error::SqliteFailure(failure, _) => {
                Some((failure.code, failure.extended_code))
            }
            _ => None,
        })
}

/// Lock contention, as opposed to corruption, I/O, constraint or logic errors.
/// Includes SQLITE_BUSY_SNAPSHOT (517), which bypasses the busy handler.
pub(crate) fn sqlite_busy(error: &anyhow::Error) -> bool {
    matches!(
        sqlite_codes(error),
        Some((
            rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked,
            _
        ))
    )
}

fn sqlite_code_name(extended: i32) -> String {
    use rusqlite::ffi;
    match extended {
        ffi::SQLITE_BUSY => "SQLITE_BUSY".into(),
        ffi::SQLITE_BUSY_RECOVERY => "SQLITE_BUSY_RECOVERY".into(),
        ffi::SQLITE_BUSY_SNAPSHOT => "SQLITE_BUSY_SNAPSHOT".into(),
        ffi::SQLITE_BUSY_TIMEOUT => "SQLITE_BUSY_TIMEOUT".into(),
        ffi::SQLITE_LOCKED => "SQLITE_LOCKED".into(),
        ffi::SQLITE_LOCKED_SHAREDCACHE => "SQLITE_LOCKED_SHAREDCACHE".into(),
        ffi::SQLITE_LOCKED_VTAB => "SQLITE_LOCKED_VTAB".into(),
        other => format!("SQLite result {}", other & 0xff),
    }
}

/// Error text plus the SQLite extended result code when one is present.
/// "database is locked" alone cannot distinguish a busy-timeout expiry from a
/// WAL snapshot upgrade failure; the extended code can.
pub(crate) fn describe_error(error: &anyhow::Error) -> String {
    let text = error.to_string();
    match sqlite_codes(error) {
        Some((_, extended)) => format!(
            "{text} (SQLite extended code {extended} {})",
            sqlite_code_name(extended)
        ),
        None => text,
    }
}

pub(crate) fn initialize(root: &Path) -> Result<()> {
    vt_record::initialize(root)?;
    let mut db = open(root)?;
    let tx = db.transaction()?;
    tx.execute_batch(include_str!(
        "../../../documentation/features/app/desktop-session/sql/001-desktop.sql"
    ))?;
    tx.execute_batch(include_str!(
        "../../../documentation/features/app/desktop-session/sql/002-processing-runs.sql"
    ))?;
    tx.execute_batch(include_str!(
        "../../../documentation/features/app/desktop-session/sql/004-speaker-annotations.sql"
    ))?;
    vt_speakers::initialize(&tx)?;
    let identity_columns = {
        let mut query = tx.prepare("PRAGMA table_info(app_speaker_identities)")?;
        query
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    if !identity_columns
        .iter()
        .any(|column| column == "shared_room")
    {
        tx.execute("ALTER TABLE app_speaker_identities ADD COLUMN shared_room INTEGER NOT NULL DEFAULT 0 CHECK(shared_room IN (0,1))", [])?;
    }
    if !identity_columns.iter().any(|column| column == "person_id") {
        tx.execute("ALTER TABLE app_speaker_identities ADD COLUMN person_id TEXT REFERENCES app_people(id) ON DELETE SET NULL", [])?;
    }
    tx.execute_batch(include_str!(
        "../../../documentation/features/app/desktop-session/sql/006-echo-processing.sql"
    ))?;
    tx.execute_batch(include_str!(
        "../../../documentation/features/app/desktop-session/sql/007-bleed-annotations.sql"
    ))?;
    tx.execute_batch(include_str!(
        "../../../documentation/features/app/desktop-session/sql/008-retention.sql"
    ))?;
    tx.execute_batch(include_str!(
        "../../../documentation/features/app/desktop-session/sql/009-retention-delete-guards.sql"
    ))?;
    let speaker_state_has_context = {
        let mut query = tx.prepare("PRAGMA table_info(app_speaker_run_state)")?;
        query
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<rusqlite::Result<Vec<_>>>()?
            .iter()
            .any(|column| column == "segmentation_context_json")
    };
    if !speaker_state_has_context {
        tx.execute(
            "ALTER TABLE app_speaker_run_state ADD COLUMN segmentation_context_json TEXT",
            [],
        )?;
    }
    let reviews_have_constraint = {
        let mut query = tx.prepare("PRAGMA table_info(app_speaker_reviews)")?;
        query
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<rusqlite::Result<Vec<_>>>()?
            .iter()
            .any(|column| column == "constraint_id")
    };
    if !reviews_have_constraint {
        tx.execute("ALTER TABLE app_speaker_reviews ADD COLUMN constraint_id TEXT REFERENCES app_speaker_constraints(id)", [])?;
    }
    let staging_has_words = {
        let mut query = tx.prepare("PRAGMA table_info(app_transcription_staging)")?;
        query
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<rusqlite::Result<Vec<_>>>()?
            .iter()
            .any(|column| column == "words_json")
    };
    if !staging_has_words {
        tx.execute_batch(include_str!(
            "../../../documentation/features/app/desktop-session/sql/005-replay-word-timing.sql"
        ))?;
    }
    let ids = {
        let mut q = tx.prepare("SELECT id,status FROM app_meetings WHERE id NOT IN (SELECT meeting_id FROM app_meeting_revision)")?;
        q.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    for (id, status) in ids {
        let run = insert_run(&tx, &id, "live", r#"{"migration":"legacy-desktop"}"#)?;
        tx.execute("INSERT INTO app_run_segments(run_id,id,track,start_ms,end_ms,text,speaker) SELECT ?1,id,track,start_ms,end_ms,text,speaker FROM app_segments WHERE meeting_id=?2",params![run.id,id])?;
        if !matches!(status.as_str(), "recording" | "paused") {
            tx.execute(
                "UPDATE app_processing_runs SET status='complete' WHERE id=?1",
                [&run.id],
            )?;
        }
        tx.execute(
            "INSERT INTO app_meeting_revision(meeting_id,active_run_id) VALUES (?1,?2)",
            params![id, run.id],
        )?;
        rebuild_fts(&tx, &id)?;
    }
    tx.commit()?;
    Ok(())
}

pub(crate) fn recover_interrupted(root: &Path) -> Result<()> {
    let db = open(root)?;
    db.execute("DELETE FROM app_transcription_staging WHERE meeting_id NOT IN (SELECT meeting_id FROM app_retention_intents WHERE state IN ('confirmed','cleanup'))", [])?;
    db.execute(
        "UPDATE app_processing_runs SET status='abandoned' WHERE status='running' AND meeting_id NOT IN (SELECT meeting_id FROM app_retention_intents WHERE state IN ('confirmed','cleanup'))",
        [],
    )?;
    let mut query = db.prepare(
        "SELECT id FROM app_meetings WHERE status IN ('recording','paused','processing')",
    )?;
    let ids = query
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(query);
    drop(db);
    for id in ids {
        if crate::retention::has_pending(root, &id)? {
            continue;
        }
        if !crate::retention::audio_available(root, &id)? {
            set_status(
                root,
                &id,
                "ready",
                Some("Processing was interrupted. Saved audio was previously removed."),
            )?;
            continue;
        }
        match vt_record::Recording::recover(root, &id) {
            Ok(_) => set_status(root, &id, "recovered", None)?,
            Err(e) => set_status(root, &id, "failed", Some(&e.to_string()))?,
        }
    }
    Ok(())
}

#[cfg(test)]
pub(crate) fn create_meeting(root: &Path, id: &str, title: &str, mode: &str) -> Result<()> {
    create_meeting_with_engines(root, id, title, mode, "{}")
}

pub(crate) fn create_meeting_with_engines(
    root: &Path,
    id: &str,
    title: &str,
    mode: &str,
    engines: &str,
) -> Result<()> {
    let mut db = open(root)?;
    let tx = db.transaction()?;
    tx.execute("INSERT INTO app_meetings(id,title,status,started_at,duration_ms,mode) VALUES (?1,?2,'recording',?3,0,?4)", params![id,title,chrono::Utc::now().to_rfc3339(),mode])?;
    let run = insert_run(&tx, id, "live", engines)?;
    tx.execute(
        "INSERT INTO app_meeting_revision(meeting_id,active_run_id) VALUES (?1,?2)",
        params![id, run.id],
    )?;
    rebuild_fts(&tx, id)?;
    tx.commit()?;
    Ok(())
}

const MEETING_SELECT: &str = "SELECT m.id,m.title,m.status,m.started_at,m.duration_ms,(SELECT COUNT(*) FROM app_run_segments s JOIN app_meeting_revision v ON v.active_run_id=s.run_id WHERE v.meeting_id=m.id),m.error FROM app_meetings m";
pub(crate) fn meetings(root: &Path) -> Result<Vec<Meeting>> {
    let db = open(root)?;
    let mut query = db.prepare(&format!("{MEETING_SELECT} ORDER BY m.started_at DESC"))?;
    Ok(query
        .query_map([], meeting_row)?
        .collect::<rusqlite::Result<_>>()?)
}

fn meeting_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Meeting> {
    Ok(Meeting {
        id: row.get(0)?,
        title: row.get(1)?,
        status: row.get(2)?,
        started_at: row.get(3)?,
        duration_ms: row.get::<_, i64>(4)? as u64,
        segment_count: row.get::<_, i64>(5)? as u64,
        error: row.get(6)?,
    })
}

pub(crate) fn detail(root: &Path, id: &str) -> Result<MeetingDetail> {
    detail_selected(root, id, None)
}
pub(crate) fn detail_run(root: &Path, id: &str, run: &str) -> Result<MeetingDetail> {
    detail_selected(root, id, Some(run))
}
fn detail_selected(root: &Path, id: &str, selected: Option<&str>) -> Result<MeetingDetail> {
    let mut db = open(root)?;
    let tx = db.transaction()?;
    let mut meeting = tx.query_row(
        &format!("{MEETING_SELECT} WHERE m.id=?1"),
        [id],
        meeting_row,
    )?;
    let (active_run_id, version): (Option<String>, u64) = tx.query_row(
        "SELECT active_run_id,correction_version FROM app_meeting_revision WHERE meeting_id=?1",
        [id],
        |r| Ok((r.get(0)?, r.get::<_, i64>(1)? as u64)),
    )?;
    let run_id = selected
        .map(str::to_owned)
        .or_else(|| active_run_id.clone());
    if let Some(run) = &run_id {
        ensure_run(&tx, id, run)?;
    }
    let mut segments = read_segments(&tx, run_id.as_deref().unwrap_or(""), true)?;
    let speaker_assignments =
        speakers::annotate(&tx, run_id.as_deref().unwrap_or(""), &mut segments)?;
    let speaker_constraints = speakers::constraint_history(&tx, id)?;
    let speakers::SpeakerDetails {
        speakers,
        turns: speaker_turns,
        reviews: speaker_reviews,
        info: diarization,
    } = speakers::details(&tx, run_id.as_deref().unwrap_or(""))?;
    meeting.segment_count = segments.len() as u64;
    let notes = {
        let timed:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='app_note_timing')",[],|r|r.get(0))?;
        let sql = if timed {
            "SELECT n.id,n.t_ms,n.text,t.provenance,t.synchronized FROM app_notes n LEFT JOIN app_note_timing t ON t.note_id=n.id WHERE n.meeting_id=?1 ORDER BY n.t_ms,n.id"
        } else {
            "SELECT id,t_ms,text,NULL,NULL FROM app_notes WHERE meeting_id=?1 ORDER BY t_ms,id"
        };
        let mut q = tx.prepare(sql)?;
        q.query_map([id], |r| {
            Ok(Note {
                id: r.get(0)?,
                t_ms: r.get::<_, i64>(1)? as u64,
                text: r.get(2)?,
                timing_provenance: r.get(3)?,
                timing_synchronized: r.get(4)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?
    };
    let runs = read_runs(&tx, id)?;
    let corrections = read_corrections(&tx, id, run_id.as_deref().unwrap_or(""))?;
    let summaries = read_summaries(&tx, id)?;
    let summary_error = tx
        .query_row(
            "SELECT error FROM app_summary_failures WHERE meeting_id=?1",
            [id],
            |r| r.get::<_, String>(0),
        )
        .optional()?;
    let summary = summaries
        .iter()
        .find(|s| !s.is_stale && Some(&s.run_id) == run_id.as_ref())
        .map(|s| s.content_md.clone());
    let inference_health = crate::backlog::info(&tx, run_id.as_deref().unwrap_or(""))?;
    let echo = crate::echo_store::info(&tx, run_id.as_deref().unwrap_or(""))?;
    let bleed::BleedDetails {
        decisions: bleed_decisions,
        candidates: bleed_candidates,
    } = bleed::details(&tx, id, run_id.as_deref().unwrap_or(""), &segments)?;
    tx.commit()?;
    Ok(MeetingDetail {
        mode: if meeting_mode(root, id)? == vt_diarize::MeetingMode::InPerson {
            crate::CaptureMode::InPerson
        } else {
            crate::CaptureMode::Remote
        },
        inference_health,
        audio_available: crate::retention::audio_available(root, id)?,
        meeting,
        segments,
        notes,
        summary,
        summary_error,
        active_run_id,
        run_id,
        runs,
        corrections,
        summaries,
        correction_version: version,
        speakers,
        speaker_turns,
        speaker_reviews,
        speaker_assignments,
        speaker_constraints,
        diarization,
        echo,
        bleed_decisions,
        bleed_candidates,
    })
}

/// Stores a summary-only failure without changing recording status, warnings, or revisions.
pub(crate) fn set_summary_error(root: &Path, id: &str, error: Option<&str>) -> Result<()> {
    let db = open(root)?;
    match error {
        Some(error) => {
            db.execute("INSERT INTO app_summary_failures(meeting_id,error) VALUES (?1,?2) ON CONFLICT(meeting_id) DO UPDATE SET error=excluded.error",params![id,error])?;
        }
        None => {
            db.execute("DELETE FROM app_summary_failures WHERE meeting_id=?1", [id])?;
        }
    }
    Ok(())
}

pub(crate) fn set_status(root: &Path, id: &str, status: &str, error: Option<&str>) -> Result<()> {
    set_status_with(root, id, status, error, Duration::from_secs(5))
}
pub(crate) fn set_status_with(
    root: &Path,
    id: &str,
    status: &str,
    error: Option<&str>,
    busy_timeout: Duration,
) -> Result<()> {
    open_with_timeout(root, busy_timeout)?.execute(
        "UPDATE app_meetings SET status=?2,error=?3 WHERE id=?1",
        params![id, status, error],
    )?;
    Ok(())
}
pub(crate) fn set_duration_with(
    root: &Path,
    id: &str,
    duration: u64,
    busy_timeout: Duration,
) -> Result<()> {
    open_with_timeout(root, busy_timeout)?.execute(
        "UPDATE app_meetings SET duration_ms=?2 WHERE id=?1",
        params![id, duration.min(i64::MAX as u64) as i64],
    )?;
    Ok(())
}
/// The original per-segment live save: a DEFERRED transaction that reads before
/// it writes, then rebuilds the whole meeting's search rows. Recording now uses
/// `save_live_segments`; this remains for fixtures and the regression record.
#[cfg(test)]
pub(crate) fn save_segment(root: &Path, id: &str, segment: &Segment) -> Result<()> {
    let mut db = open(root)?;
    let tx = db.transaction()?;
    let run: String = tx.query_row("SELECT r.id FROM app_processing_runs r JOIN app_meeting_revision v ON v.active_run_id=r.id WHERE v.meeting_id=?1 AND r.kind='live' AND r.status='running'",[id],|r|r.get(0))?;
    insert_segments(&tx, &run, std::slice::from_ref(segment))?;
    invalidate_summaries(&tx, id)?;
    rebuild_fts(&tx, id)?;
    tx.commit()?;
    Ok(())
}

/// Live transcript persistence. One IMMEDIATE transaction (the write lock is
/// taken at BEGIN, so a concurrent commit makes it wait in the busy handler
/// instead of failing with SQLITE_BUSY_SNAPSHOT) holding only point operations:
/// a primary-key check of the run, one idempotent insert and one search row per
/// new segment, and summary invalidation. Segment IDs are minted before the
/// first attempt, so a retried batch never duplicates rows. Returns how many
/// segments were newly inserted.
pub(crate) fn save_live_segments(
    root: &Path,
    meeting: &str,
    run: &str,
    segments: &[Segment],
    budget: LiveSaveBudget,
) -> Result<usize> {
    for segment in segments {
        validate_segment(segment)?;
    }
    retry_busy(budget, |busy_timeout| {
        let mut db = open_with_timeout(root, busy_timeout)?;
        let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let added = insert_live_segments(&tx, meeting, run, segments)?;
        tx.commit()?;
        Ok(added)
    })
}

fn insert_live_segments(
    db: &Connection,
    meeting: &str,
    run: &str,
    segments: &[Segment],
) -> Result<usize> {
    let state: Option<(bool, bool)> = db
        .query_row(
            "SELECT r.status='running',COALESCE(v.active_run_id=r.id,0) FROM app_processing_runs r LEFT JOIN app_meeting_revision v ON v.meeting_id=r.meeting_id WHERE r.id=?1 AND r.meeting_id=?2 AND r.kind='live'",
            params![run, meeting],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    match state {
        Some((true, true)) => (),
        Some((false, _)) => bail!("Only running runs accept segments."),
        _ => bail!("Live segments can only be added to the meeting's active live run."),
    }
    let mut added = 0;
    for s in segments {
        let inserted = db.execute("INSERT INTO app_run_segments(run_id,id,track,start_ms,end_ms,text,speaker,words_json) VALUES (?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT(run_id,id) DO NOTHING",params![run,s.id,s.track,s.start_ms as i64,s.end_ms as i64,s.text,s.speaker,serde_json::to_string(&s.words)?])?;
        if inserted == 1 {
            // A new segment has no correction or bleed match yet, so its
            // effective search text is its raw text (as rebuild_fts derives).
            db.execute("INSERT INTO app_search_fts(meeting_id,source,ref_id,text) VALUES (?1,'segment',?2,?3)",params![meeting,s.id,s.text])?;
            added += 1;
        }
    }
    if added > 0 {
        invalidate_summaries_incremental(db, meeting)?;
    }
    Ok(added)
}

/// Same end state as `invalidate_summaries` followed by `rebuild_fts`'s
/// summary rows: every summary of the meeting becomes stale, so none stays
/// searchable. The full-table search delete runs only if summaries exist.
fn invalidate_summaries_incremental(db: &Connection, meeting: &str) -> Result<()> {
    let any: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM app_summaries WHERE meeting_id=?1)",
        [meeting],
        |r| r.get(0),
    )?;
    if any {
        invalidate_summaries(db, meeting)?;
        db.execute(
            "DELETE FROM app_search_fts WHERE meeting_id=?1 AND source='summary'",
            [meeting],
        )?;
    }
    Ok(())
}

pub(crate) fn stage_segment(
    root: &Path,
    job: &str,
    meeting: &str,
    segment: &Segment,
) -> Result<()> {
    validate_segment(segment)?;
    open(root)?.execute("INSERT INTO app_transcription_staging(job_id,id,meeting_id,track,start_ms,end_ms,text,speaker,words_json) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        params![job,segment.id,meeting,segment.track,segment.start_ms as i64,segment.end_ms as i64,segment.text,segment.speaker,serde_json::to_string(&segment.words)?])?;
    Ok(())
}

pub(crate) fn discard_staged(root: &Path, job: &str) -> Result<()> {
    open(root)?.execute(
        "DELETE FROM app_transcription_staging WHERE job_id=?1",
        [job],
    )?;
    Ok(())
}

#[cfg(test)]
pub(crate) fn publish_staged(root: &Path, job: &str, meeting: &str) -> Result<()> {
    publish_staged_with_echo(root, job, meeting, None)
}

#[cfg(test)]
pub(crate) fn publish_staged_with_echo(
    root: &Path,
    job: &str,
    meeting: &str,
    echo: Option<&crate::EchoInfo>,
) -> Result<()> {
    publish_staged_with_provenance(root, job, meeting, echo, "{}")
}

pub(crate) fn publish_staged_with_provenance(
    root: &Path,
    job: &str,
    meeting: &str,
    echo: Option<&crate::EchoInfo>,
    engines: &str,
) -> Result<()> {
    let mut db = open(root)?;
    let tx = db.transaction()?;
    let run = insert_run(&tx, meeting, "replay", engines)?;
    let foreign: i64 = tx.query_row(
        "SELECT COUNT(*) FROM app_transcription_staging WHERE job_id=?1 AND meeting_id<>?2",
        params![job, meeting],
        |r| r.get(0),
    )?;
    if foreign != 0 {
        bail!("Replay job contains another meeting.");
    }
    tx.execute("INSERT INTO app_run_segments(run_id,id,track,start_ms,end_ms,text,speaker,words_json) SELECT ?1,id,track,start_ms,end_ms,text,speaker,words_json FROM app_transcription_staging WHERE job_id=?2 AND meeting_id=?3",params![run.id,job,meeting])?;
    let _ = speakers::copy_previous(&tx, meeting, &run.id)?;
    if let Some(echo) = echo {
        crate::echo_store::write_info(&tx, meeting, &run.id, echo)?;
    }
    publish_in_transaction(&tx, meeting, &run.id)?;
    tx.execute(
        "DELETE FROM app_transcription_staging WHERE job_id=?1",
        [job],
    )?;
    tx.commit()?;
    Ok(())
}
#[allow(dead_code)] // Preserve the store-only compatibility entry point.
pub(crate) fn save_note(root: &Path, id: &str, time: u64, text: &str) -> Result<()> {
    if save_note_with_id(root, id, time, text)?.is_none() {
        bail!("Notes must contain 1–32,000 bytes of text.");
    }
    Ok(())
}
#[allow(dead_code)] // Used by the compatibility entry point.
pub(crate) fn save_note_with_id(
    root: &Path,
    id: &str,
    time: u64,
    text: &str,
) -> Result<Option<String>> {
    save_note_with_metadata(root, id, time, text, |_, _| Ok(()))
}
pub(crate) fn save_note_with_metadata(
    root: &Path,
    id: &str,
    time: u64,
    text: &str,
    attach: impl FnOnce(&Connection, &str) -> Result<()>,
) -> Result<Option<String>> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    if text.len() > 32_000 {
        bail!("Notes must contain 1–32,000 bytes of text.");
    }
    let time = i64::try_from(time)?;
    let note_id = uuid::Uuid::new_v4().to_string();
    let mut db = open(root)?;
    let tx = db.transaction()?;
    tx.execute(
        "INSERT INTO app_notes(id,meeting_id,t_ms,text) VALUES (?1,?2,?3,?4)",
        params![note_id, id, time, text],
    )?;
    attach(&tx, &note_id)?;
    rebuild_fts(&tx, id)?;
    tx.commit()?;
    Ok(Some(note_id))
}
pub(crate) fn rename(root: &Path, id: &str, title: &str) -> Result<()> {
    let title = title.trim();
    if title.is_empty() || title.chars().count() > 200 {
        bail!("A meeting title must contain 1–200 characters.");
    }
    let mut db = open(root)?;
    let tx = db.transaction()?;
    if tx.execute(
        "UPDATE app_meetings SET title=?2 WHERE id=?1",
        params![id, title],
    )? != 1
    {
        bail!("Meeting not found.");
    }
    rebuild_fts(&tx, id)?;
    tx.commit()?;
    Ok(())
}

/// Called on the capture consumer loop for every chunk, so it takes the loop's
/// short `busy_timeout`; the caller skips it on a lock error.
#[allow(clippy::too_many_arguments)]
pub(crate) fn save_clock(
    root: &Path,
    id: &str,
    track: &str,
    seq: u64,
    source_epoch: u32,
    source_first_frame: u64,
    host_ns: u64,
    busy_timeout: Duration,
) -> Result<()> {
    open_with_timeout(root, busy_timeout)?.execute("INSERT OR REPLACE INTO app_audio_clock(meeting_id,track,seq,source_epoch,source_first_frame,host_time_ns) VALUES (?1,?2,?3,?4,?5,?6)", params![id,track,seq as i64,source_epoch,source_first_frame as i64,host_ns.min(i64::MAX as u64) as i64])?;
    Ok(())
}

fn timestamp(ms: u64, separator: &str) -> String {
    format!(
        "{:02}:{:02}:{:02}{}{:03}",
        ms / 3_600_000,
        (ms / 60_000) % 60,
        (ms / 1000) % 60,
        separator,
        ms % 1000
    )
}

fn note_stamp(note: &Note) -> String {
    if note.timing_provenance.as_deref() == Some("untimed_note") {
        return "Untimed".into();
    }
    let stamp = timestamp(note.t_ms, ".");
    match (note.timing_provenance.as_deref(), note.timing_synchronized) {
        (Some("uptime_raw" | "saved_playback_position"), Some(true)) => stamp,
        (Some("latest_audio_observation"), _) | (_, Some(false)) => format!("~{stamp}"),
        _ => format!("?{stamp}"),
    }
}

pub(crate) fn export(root: &Path, id: &str, format: &str) -> Result<PathBuf> {
    let detail = detail(root, id)?;
    let hidden = detail
        .bleed_decisions
        .iter()
        .filter(|d| d.status == "applied" && d.decision == "bleed")
        .filter_map(|d| d.segment_id.as_deref())
        .collect::<std::collections::BTreeSet<_>>();
    let exported_segments = detail
        .segments
        .iter()
        .filter(|s| !hidden.contains(s.id.as_str()))
        .collect::<Vec<_>>();
    let (extension, content) = match format {
        "json" => ("json", serde_json::to_string_pretty(&detail)?),
        "markdown" => {
            let mut content = format!(
                "---\ntitle: {}\nstarted_at: {}\n---\n\n# {}\n\n",
                serde_json::to_string(&detail.meeting.title)?,
                detail.meeting.started_at,
                detail.meeting.title
            );
            for s in &exported_segments {
                content.push_str(&format!(
                    "**{} · {}** {}\n\n",
                    timestamp(s.start_ms, "."),
                    s.speaker,
                    s.text
                ));
            }
            if !detail.notes.is_empty() {
                content.push_str("## Notes\n\n");
                for n in &detail.notes {
                    content.push_str(&format!("- {} {}\n", note_stamp(n), n.text));
                }
            }
            ("md", content)
        }
        "txt" => {
            let mut content: String = exported_segments
                .iter()
                .map(|s| {
                    format!(
                        "[{}] {}: {}\n",
                        timestamp(s.start_ms, "."),
                        s.speaker,
                        s.text
                    )
                })
                .collect();
            if !detail.notes.is_empty() {
                content.push_str("\nNotes\n");
                for note in &detail.notes {
                    content.push_str(&format!("[{}] {}\n", note_stamp(note), note.text));
                }
            }
            ("txt", content)
        }
        "srt" | "vtt" => {
            let mut content = if format == "vtt" {
                "WEBVTT\n\n".to_string()
            } else {
                String::new()
            };
            for (i, s) in exported_segments.iter().enumerate() {
                let sep = if format == "srt" { "," } else { "." };
                content.push_str(&format!(
                    "{}\n{} --> {}\n{}: {}\n\n",
                    i + 1,
                    timestamp(s.start_ms, sep),
                    timestamp(s.end_ms.max(s.start_ms + 1), sep),
                    s.speaker,
                    s.text.replace('\n', " ")
                ));
            }
            (if format == "srt" { "srt" } else { "vtt" }, content)
        }
        _ => bail!("Choose Markdown, TXT, JSON, SRT or VTT export."),
    };
    let dir = root.join("Exports");
    fs::create_dir_all(&dir)?;
    let path = dir.join(format!(
        "{}-{}.{}",
        detail.meeting.id,
        uuid::Uuid::new_v4(),
        extension
    ));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    file.write_all(content.as_bytes())?;
    file.sync_all()?;
    Ok(path)
}

fn insert_run(db: &Connection, meeting: &str, kind: &str, engines: &str) -> Result<RunInfo> {
    let _: serde_json::Value = serde_json::from_str(engines)?;
    let run = RunInfo {
        id: uuid::Uuid::new_v4().to_string(),
        meeting_id: meeting.into(),
        kind: kind.into(),
        status: "running".into(),
        engines_json: engines.into(),
        constraints_version: speakers::constraint_version(db, meeting)?,
        created_at: chrono::Utc::now().to_rfc3339(),
    };
    db.execute("INSERT INTO app_processing_runs(id,meeting_id,kind,status,engines_json,constraints_version,created_at) VALUES (?1,?2,?3,'running',?4,?5,?6)",params![run.id,meeting,kind,engines,run.constraints_version as i64,run.created_at])?;
    Ok(run)
}
pub(crate) fn create_run(
    root: &Path,
    meeting: &str,
    kind: &str,
    engines_json: &str,
) -> Result<RunInfo> {
    insert_run(&open(root)?, meeting, kind, engines_json)
}
fn ensure_run(db: &Connection, meeting: &str, run: &str) -> Result<()> {
    if !db.query_row(
        "SELECT EXISTS(SELECT 1 FROM app_processing_runs WHERE id=?1 AND meeting_id=?2)",
        params![run, meeting],
        |r| r.get::<_, bool>(0),
    )? {
        bail!("Run does not belong to this meeting.");
    }
    Ok(())
}
fn validate_segment(segment: &Segment) -> Result<()> {
    if segment.start_ms > segment.end_ms || segment.end_ms > i64::MAX as u64 {
        bail!("Invalid segment times.");
    }
    for word in &segment.words {
        if word.start_ms < segment.start_ms
            || word.end_ms > segment.end_ms
            || word.end_ms < word.start_ms
        {
            bail!("Word timestamp is outside its segment.");
        }
    }
    Ok(())
}
fn insert_segments(db: &Connection, run: &str, segments: &[Segment]) -> Result<()> {
    let running: bool = db.query_row(
        "SELECT status='running' FROM app_processing_runs WHERE id=?1",
        [run],
        |r| r.get(0),
    )?;
    if !running {
        bail!("Only running runs accept segments.");
    }
    for s in segments {
        validate_segment(s)?;
        db.execute("INSERT INTO app_run_segments(run_id,id,track,start_ms,end_ms,text,speaker,words_json) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",params![run,s.id,s.track,s.start_ms as i64,s.end_ms as i64,s.text,s.speaker,serde_json::to_string(&s.words)?])?;
    }
    Ok(())
}
pub(crate) fn append_run_segments(root: &Path, run: &str, segments: &[Segment]) -> Result<()> {
    let mut db = open(root)?;
    let tx = db.transaction()?;
    // Visible live finals are written through save_segment, which updates search atomically.
    let active: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM app_meeting_revision WHERE active_run_id=?1)",
        [run],
        |r| r.get(0),
    )?;
    if active {
        bail!("Use live segment persistence for an active run.");
    }
    insert_segments(&tx, run, segments)?;
    tx.commit()?;
    Ok(())
}
fn run_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<RunInfo> {
    Ok(RunInfo {
        id: r.get(0)?,
        meeting_id: r.get(1)?,
        kind: r.get(2)?,
        status: r.get(3)?,
        engines_json: r.get(4)?,
        constraints_version: r.get::<_, i64>(5)? as u64,
        created_at: r.get(6)?,
    })
}
fn read_runs(db: &Connection, meeting: &str) -> Result<Vec<RunInfo>> {
    let mut q = db.prepare("SELECT id,meeting_id,kind,status,engines_json,constraints_version,created_at FROM app_processing_runs WHERE meeting_id=?1 ORDER BY created_at DESC,id")?;
    Ok(q.query_map([meeting], run_row)?
        .collect::<rusqlite::Result<_>>()?)
}
#[cfg(test)]
pub(crate) fn list_runs(root: &Path, meeting: &str) -> Result<Vec<RunInfo>> {
    read_runs(&open(root)?, meeting)
}
pub(crate) fn abandon_run(root: &Path, run: &str) -> Result<()> {
    open(root)?.execute("UPDATE app_processing_runs SET status='abandoned' WHERE id=?1 AND status='running' AND id NOT IN (SELECT active_run_id FROM app_meeting_revision WHERE active_run_id IS NOT NULL)",[run])?;
    Ok(())
}
fn read_segments(db: &Connection, run: &str, effective: bool) -> Result<Vec<Segment>> {
    let mut q = db.prepare("SELECT s.id,s.track,s.start_ms,s.end_ms,s.text,s.speaker,s.words_json,c.replacement_text FROM app_run_segments s LEFT JOIN app_correction_matches x ON x.run_id=s.run_id AND x.segment_id=s.id LEFT JOIN app_segment_corrections c ON c.id=x.correction_id AND c.status<>'dismissed' WHERE s.run_id=?1 ORDER BY s.start_ms,s.id")?;
    let rows = q
        .query_map([run], |r| {
            Ok((
                Segment {
                    id: r.get(0)?,
                    track: r.get(1)?,
                    start_ms: r.get::<_, i64>(2)? as u64,
                    end_ms: r.get::<_, i64>(3)? as u64,
                    text: r.get(4)?,
                    speaker: r.get(5)?,
                    words: vec![],
                },
                r.get::<_, String>(6)?,
                r.get::<_, Option<String>>(7)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut segments = rows
        .into_iter()
        .map(|(mut s, words, replacement)| {
            s.words = serde_json::from_str(&words)?;
            if effective && let Some(text) = replacement {
                if s.text != text {
                    s.words.clear();
                }
                s.text = text;
            }
            Ok(s)
        })
        .collect::<Result<Vec<_>>>()?;
    if effective {
        let _ = speakers::annotate(db, run, &mut segments)?;
    }
    Ok(segments)
}
fn text_hash(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}
fn read_corrections(db: &Connection, meeting: &str, run: &str) -> Result<Vec<Correction>> {
    let mut q = db.prepare("SELECT c.id,c.track,c.start_ms,c.end_ms,c.original_text_hash,c.replacement_text,CASE WHEN c.status='dismissed' THEN 'dismissed' WHEN x.segment_id IS NULL THEN 'needs_review' ELSE 'applied' END,x.segment_id FROM app_segment_corrections c LEFT JOIN app_correction_matches x ON x.correction_id=c.id AND x.run_id=?2 WHERE c.meeting_id=?1 ORDER BY c.created_at,c.id")?;
    Ok(q.query_map(params![meeting, run], |r| {
        Ok(Correction {
            id: r.get(0)?,
            track: r.get(1)?,
            start_ms: r.get::<_, i64>(2)? as u64,
            end_ms: r.get::<_, i64>(3)? as u64,
            original_text_hash: r.get(4)?,
            replacement_text: r.get(5)?,
            status: r.get(6)?,
            segment_id: r.get(7)?,
        })
    })?
    .collect::<rusqlite::Result<_>>()?)
}
fn reapply_corrections(db: &Connection, meeting: &str, run: &str) -> Result<()> {
    let segments = read_segments(db, run, false)?;
    let corrections = read_corrections(db, meeting, run)?;
    // Colliding anchors are all retained for review, rather than picking a winner by order.
    let candidates: Vec<_> = corrections
        .iter()
        .filter(|c| c.status != "dismissed")
        .map(|c| {
            let matches: Vec<_> = segments
                .iter()
                .filter(|s| {
                    s.track == c.track
                        && s.start_ms == c.start_ms
                        && s.end_ms == c.end_ms
                        && text_hash(&s.text) == c.original_text_hash
                })
                .map(|s| s.id.clone())
                .collect();
            (c.id.clone(), matches)
        })
        .collect();
    for (id, matches) in &candidates {
        let unique = matches.len() == 1
            && candidates
                .iter()
                .filter(|(_, m)| m.len() == 1 && m[0] == matches[0])
                .count()
                == 1;
        if unique {
            db.execute("INSERT OR REPLACE INTO app_correction_matches(correction_id,run_id,segment_id) VALUES (?1,?2,?3)",params![id,run,matches[0]])?;
        }
        db.execute(
            "UPDATE app_segment_corrections SET status=?2 WHERE id=?1",
            params![id, if unique { "applied" } else { "needs_review" }],
        )?;
    }
    Ok(())
}
fn invalidate_summaries(db: &Connection, meeting: &str) -> Result<()> {
    db.execute(
        "UPDATE app_summaries SET is_stale=1 WHERE meeting_id=?1",
        [meeting],
    )?;
    Ok(())
}
fn bump_revision(db: &Connection, meeting: &str) -> Result<()> {
    db.execute("UPDATE app_meeting_revision SET correction_version=correction_version+1 WHERE meeting_id=?1",[meeting])?;
    invalidate_summaries(db, meeting)
}
fn publish_in_transaction(db: &Connection, meeting: &str, run: &str) -> Result<()> {
    ensure_run(db, meeting, run)?;
    let status: String = db.query_row(
        "SELECT status FROM app_processing_runs WHERE id=?1",
        [run],
        |r| r.get(0),
    )?;
    if status != "running" {
        bail!("Only a running run can be published.");
    }
    let version: u64 = db.query_row(
        "SELECT constraints_version FROM app_processing_runs WHERE id=?1",
        [run],
        |row| row.get::<_, i64>(0),
    )? as u64;
    if version != speakers::constraint_version(db, meeting)? {
        bail!(
            "Speaker corrections changed during processing. Retry to apply the current constraints."
        );
    }
    reapply_corrections(db, meeting, run)?;
    bleed::reapply(db, meeting, run)?;
    db.execute("UPDATE app_processing_runs SET status='complete' WHERE id=(SELECT active_run_id FROM app_meeting_revision WHERE meeting_id=?1) AND kind='live' AND status='running'",[meeting])?;
    db.execute(
        "UPDATE app_processing_runs SET status='complete' WHERE id=?1",
        [run],
    )?;
    // Prior completed runs keep their complete status and immutable rows for history/citations.
    db.execute(
        "UPDATE app_meeting_revision SET active_run_id=?2 WHERE meeting_id=?1",
        params![meeting, run],
    )?;
    db.execute(
        "UPDATE app_meetings SET status='ready',error=NULL WHERE id=?1",
        [meeting],
    )?;
    invalidate_summaries(db, meeting)?;
    db.execute(
        "DELETE FROM app_summary_failures WHERE meeting_id=?1",
        [meeting],
    )?;
    rebuild_fts(db, meeting)?;
    Ok(())
}
pub(crate) fn publish_run(root: &Path, meeting: &str, run: &str) -> Result<()> {
    let mut db = open(root)?;
    let tx = db.transaction()?;
    publish_in_transaction(&tx, meeting, run)?;
    tx.commit()?;
    Ok(())
}
#[cfg(test)]
pub(crate) fn complete_live_run(root: &Path, meeting: &str) -> Result<()> {
    complete_live_run_with(root, meeting, Duration::from_secs(5))
}
pub(crate) fn complete_live_run_with(
    root: &Path,
    meeting: &str,
    busy_timeout: Duration,
) -> Result<()> {
    let mut db = open_with_timeout(root, busy_timeout)?;
    let tx = db.transaction()?;
    tx.execute("UPDATE app_processing_runs SET status='complete' WHERE id=(SELECT active_run_id FROM app_meeting_revision WHERE meeting_id=?1) AND kind='live' AND status='running'",[meeting])?;
    tx.commit()?;
    Ok(())
}
pub(crate) fn correct_segment(
    root: &Path,
    meeting: &str,
    run: &str,
    segment_id: &str,
    replacement: &str,
) -> Result<()> {
    if replacement.trim().is_empty() || replacement.len() > 32_000 {
        bail!("Correction must contain 1–32,000 bytes.");
    }
    let mut db = open(root)?;
    let tx = db.transaction()?;
    ensure_active(&tx, meeting, run)?;
    let segment = read_segments(&tx, run, false)?
        .into_iter()
        .find(|s| s.id == segment_id)
        .ok_or_else(|| anyhow::anyhow!("Segment not found."))?;
    let existing: Option<String> = tx
        .query_row(
            "SELECT correction_id FROM app_correction_matches WHERE run_id=?1 AND segment_id=?2",
            params![run, segment_id],
            |r| r.get(0),
        )
        .optional()?;
    let id = if let Some(id) = existing {
        tx.execute(
            "UPDATE app_segment_corrections SET replacement_text=?2,status='applied' WHERE id=?1",
            params![id, replacement],
        )?;
        id
    } else {
        let id = uuid::Uuid::new_v4().to_string();
        tx.execute("INSERT INTO app_segment_corrections(id,meeting_id,track,start_ms,end_ms,original_text_hash,replacement_text,status,created_at) VALUES (?1,?2,?3,?4,?5,?6,?7,'applied',?8)",params![id,meeting,segment.track,segment.start_ms as i64,segment.end_ms as i64,text_hash(&segment.text),replacement,chrono::Utc::now().to_rfc3339()])?;
        id
    };
    tx.execute("INSERT OR REPLACE INTO app_correction_matches(correction_id,run_id,segment_id) VALUES (?1,?2,?3)",params![id,run,segment_id])?;
    bump_revision(&tx, meeting)?;
    rebuild_fts(&tx, meeting)?;
    tx.commit()?;
    Ok(())
}
fn ensure_active(db: &Connection, meeting: &str, run: &str) -> Result<()> {
    ensure_run(db, meeting, run)?;
    let active: bool = db.query_row(
        "SELECT active_run_id=?2 FROM app_meeting_revision WHERE meeting_id=?1",
        params![meeting, run],
        |r| r.get(0),
    )?;
    if !active {
        bail!("The active transcript changed. Reload before editing.");
    }
    Ok(())
}
/// Resolve an unmatched correction onto an active segment, or dismiss it explicitly.
pub(crate) fn resolve_correction(
    root: &Path,
    meeting: &str,
    correction: &str,
    target: Option<&str>,
) -> Result<()> {
    let mut db = open(root)?;
    let tx = db.transaction()?;
    let run: String = tx.query_row(
        "SELECT active_run_id FROM app_meeting_revision WHERE meeting_id=?1",
        [meeting],
        |r| r.get(0),
    )?;
    let exists: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM app_segment_corrections WHERE id=?1 AND meeting_id=?2)",
        params![correction, meeting],
        |r| r.get(0),
    )?;
    if !exists {
        bail!("Correction not found.");
    }
    if let Some(target) = target {
        let s = read_segments(&tx, &run, false)?
            .into_iter()
            .find(|s| s.id == target)
            .ok_or_else(|| anyhow::anyhow!("Segment not found."))?;
        tx.execute("UPDATE app_segment_corrections SET track=?2,start_ms=?3,end_ms=?4,original_text_hash=?5,status='applied' WHERE id=?1",params![correction,s.track,s.start_ms as i64,s.end_ms as i64,text_hash(&s.text)])?;
        // A target already claimed by another correction is rejected, never overwritten.
        tx.execute("INSERT INTO app_correction_matches(correction_id,run_id,segment_id) VALUES (?1,?2,?3) ON CONFLICT(correction_id,run_id) DO UPDATE SET segment_id=excluded.segment_id",params![correction,run,target])?;
    } else {
        tx.execute(
            "UPDATE app_segment_corrections SET status='dismissed' WHERE id=?1",
            [correction],
        )?;
    }
    bump_revision(&tx, meeting)?;
    rebuild_fts(&tx, meeting)?;
    tx.commit()?;
    Ok(())
}
fn rebuild_fts(db: &Connection, meeting: &str) -> Result<()> {
    db.execute("DELETE FROM app_search_fts WHERE meeting_id=?1", [meeting])?;
    db.execute("INSERT INTO app_search_fts(meeting_id,source,ref_id,text) SELECT id,'title',id,title FROM app_meetings WHERE id=?1",[meeting])?;
    db.execute("INSERT INTO app_search_fts(meeting_id,source,ref_id,text) SELECT meeting_id,'note',id,text FROM app_notes WHERE meeting_id=?1",[meeting])?;
    db.execute("INSERT INTO app_search_fts(meeting_id,source,ref_id,text) SELECT s.meeting_id,'summary',s.id,s.content_md FROM app_summaries s JOIN app_meeting_revision v ON v.meeting_id=s.meeting_id AND v.active_run_id=s.run_id WHERE s.meeting_id=?1 AND s.is_stale=0",[meeting])?;
    let run: Option<String> = db
        .query_row(
            "SELECT active_run_id FROM app_meeting_revision WHERE meeting_id=?1",
            [meeting],
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    if let Some(run) = run {
        let hidden = bleed::hidden_ids(db, &run)?;
        for s in read_segments(db, &run, true)? {
            if hidden.contains(&s.id) {
                continue;
            }
            db.execute("INSERT INTO app_search_fts(meeting_id,source,ref_id,text) VALUES (?1,'segment',?2,?3)",params![meeting,s.id,s.text])?;
        }
    }
    Ok(())
}
pub(crate) fn search_meetings(root: &Path, query: &str) -> Result<Vec<Meeting>> {
    // Every token is a quoted FTS literal; punctuation/operators cannot alter the grammar.
    let literal = query
        .split_whitespace()
        .map(|s| format!("\"{}\"", s.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" AND ");
    if literal.is_empty() {
        return meetings(root);
    }
    let db = open(root)?;
    let mut q=db.prepare(&format!("{MEETING_SELECT} WHERE m.id IN (SELECT meeting_id FROM app_search_fts WHERE app_search_fts MATCH ?1) ORDER BY m.started_at DESC"))?;
    Ok(q.query_map([literal], meeting_row)?
        .collect::<rusqlite::Result<_>>()?)
}
fn read_summaries(db: &Connection, meeting: &str) -> Result<Vec<SummaryInfo>> {
    let mut q=db.prepare("SELECT id,meeting_id,run_id,correction_version,provider,model,template,content_json,content_md,is_stale,created_at FROM app_summaries WHERE meeting_id=?1 ORDER BY created_at DESC,id")?;
    let mut summaries: Vec<SummaryInfo> = q
        .query_map([meeting], |r| {
            Ok(SummaryInfo {
                id: r.get(0)?,
                meeting_id: r.get(1)?,
                run_id: r.get(2)?,
                correction_version: r.get::<_, i64>(3)? as u64,
                provider: r.get(4)?,
                model: r.get(5)?,
                template: r.get(6)?,
                content_json: r.get(7)?,
                content_md: r.get(8)?,
                is_stale: r.get(9)?,
                created_at: r.get(10)?,
                source_segment_ids: vec![],
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    for summary in &mut summaries {
        let mut sources = db.prepare(
            "SELECT segment_id FROM app_summary_sources WHERE summary_id=?1 ORDER BY segment_id",
        )?;
        summary.source_segment_ids = sources
            .query_map([&summary.id], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
    }
    Ok(summaries)
}
#[allow(clippy::too_many_arguments)]
pub(crate) fn save_summary(
    root: &Path,
    meeting: &str,
    expected_run: &str,
    expected_version: u64,
    provider: &str,
    model: &str,
    template: &str,
    content_json: &str,
    content_md: &str,
    source_segment_ids: &[String],
) -> Result<SummaryInfo> {
    let _: serde_json::Value = serde_json::from_str(content_json)?;
    let mut db = open(root)?;
    let tx = db.transaction()?;
    ensure_active(&tx, meeting, expected_run)?;
    let valid: bool=tx.query_row("SELECT v.correction_version=?2 AND r.status='complete' FROM app_meeting_revision v JOIN app_processing_runs r ON r.id=v.active_run_id WHERE v.meeting_id=?1",params![meeting,i64::try_from(expected_version)?],|r|r.get(0))?;
    if !valid {
        bail!("Transcript changed or is incomplete. Regenerate the summary.");
    }
    let summary = SummaryInfo {
        id: uuid::Uuid::new_v4().to_string(),
        meeting_id: meeting.into(),
        run_id: expected_run.into(),
        correction_version: expected_version,
        provider: provider.into(),
        model: model.into(),
        template: template.into(),
        content_json: content_json.into(),
        content_md: content_md.into(),
        is_stale: false,
        created_at: chrono::Utc::now().to_rfc3339(),
        source_segment_ids: source_segment_ids.to_vec(),
    };
    tx.execute("INSERT INTO app_summaries(id,meeting_id,run_id,correction_version,provider,model,template,content_json,content_md,created_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",params![summary.id,meeting,expected_run,i64::try_from(expected_version)?,provider,model,template,content_json,content_md,summary.created_at])?;
    for source in source_segment_ids {
        tx.execute("INSERT OR IGNORE INTO app_summary_sources(summary_id,run_id,segment_id) VALUES (?1,?2,?3)",params![summary.id,expected_run,source])?;
    }
    rebuild_fts(&tx, meeting)?;
    tx.commit()?;
    Ok(summary)
}

#[cfg(test)]
#[path = "live_segment_store_tests.rs"]
mod live_segment_tests;
#[cfg(test)]
#[path = "speaker_store_tests.rs"]
mod speaker_regressions;
#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;

/// Speaker-separation fallback for a refine run whose separation stopped at a
/// capacity limit: carries the previous published run's speaker keys, timed
/// turns, user overrides and pending reviews onto the running `run` (the same
/// copy as `retain_speaker_annotations`) and appends `warning(retained)` to the
/// run's speaker warnings. Writes nothing from the failed separation attempt.
pub(crate) fn retain_speakers_after_failed_separation(
    root: &Path,
    meeting: &str,
    run: &str,
    warning: impl FnOnce(bool) -> String,
) -> Result<Option<Vec<vt_diarize::SpeakerTurn>>> {
    let mut db = open(root)?;
    let tx = db.transaction()?;
    let turns = speakers::copy_previous(&tx, meeting, run)?;
    let saved: Option<String> = tx
        .query_row(
            "SELECT warnings_json FROM app_speaker_run_state WHERE run_id=?1",
            [run],
            |row| row.get(0),
        )
        .optional()?;
    let mut warnings: Vec<String> = saved
        .as_deref()
        .map(serde_json::from_str)
        .transpose()?
        .unwrap_or_default();
    warnings.push(warning(!turns.is_empty()));
    tx.execute("INSERT INTO app_speaker_run_state(run_id,embedding_version,calibrated,warnings_json) VALUES (?1,?2,0,?3) ON CONFLICT(run_id) DO UPDATE SET warnings_json=excluded.warnings_json",params![run,vt_diarize::EMBEDDING_VERSION,serde_json::to_string(&warnings)?])?;
    tx.commit()?;
    Ok((!turns.is_empty()).then_some(turns))
}
