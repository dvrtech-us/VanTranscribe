//! Confirmed, crash-recoverable local deletion. Caller owns the desktop lock and runtime leases.
use crate::store;
use anyhow::{Context, Result, bail};
use cap_std::{ambient_authority, fs::Dir};
use chrono::{Duration, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    path::{Component, Path},
};

const MAX_ENTRIES: usize = 100_000;
const MAX_DEPTH: usize = 16;
const PREVIEW_TTL_HOURS: i64 = 24;
const MAX_PREVIEWS_PER_MEETING: i64 = 4;
const MAX_TERMINAL_RECEIPTS: i64 = 128;
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MaintenancePolicy {
    pub audio_keep_days: Option<u32>,
    pub obsolete_run_days: u32,
    #[serde(default = "default_cleanup_obsolete_runs")]
    pub cleanup_obsolete_runs: bool,
}
fn default_cleanup_obsolete_runs() -> bool {
    true
}
impl Default for MaintenancePolicy {
    fn default() -> Self {
        Self {
            audio_keep_days: None,
            obsolete_run_days: 7,
            cleanup_obsolete_runs: true,
        }
    }
}
impl MaintenancePolicy {
    pub fn validate(&self) -> Result<()> {
        if self.obsolete_run_days == 0 || self.audio_keep_days == Some(0) {
            bail!("Retention days must be positive.");
        }
        for days in std::iter::once(self.obsolete_run_days).chain(self.audio_keep_days) {
            Utc::now()
                .checked_sub_signed(Duration::days(i64::from(days)))
                .context("Retention period exceeds supported dates")?;
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DeletionKind {
    Meeting,
    Audio,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeletionPreview {
    pub token: String,
    pub meeting_id: String,
    pub kind: String,
    pub files: u64,
    pub bytes: u64,
    pub runs: u64,
    pub fingerprint: String,
    pub protected_runs: u64,
    pub protected_active: u64,
    pub protected_references: u64,
    pub foreign_key_tables: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PolicyPreview {
    pub policy: MaintenancePolicy,
    pub actions: Vec<DeletionPreview>,
    pub protected_runs: u64,
    pub skipped_busy: Vec<String>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct MaintenanceReport {
    pub completed: u64,
    pub meetings_deleted: u64,
    pub audio_purged: u64,
    pub runs_deleted: u64,
    pub bytes_reclaimed: u64,
    pub pending_cleanup: Vec<String>,
    pub errors: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Payload {
    format_version: u32,
    preview: DeletionPreview,
    moves: Vec<String>,
    entries: Vec<Entry>,
    run_ids: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct Entry {
    path: String,
    directory: bool,
    bytes: u64,
    modified_ns: u128,
}

/// Only disposable metadata is collected. Confirmed, cleanup and gated rows
/// retain their complete recovery capabilities regardless of age or count.
fn prune_receipts(db: &Connection) -> Result<()> {
    let cutoff = (Utc::now() - Duration::hours(PREVIEW_TTL_HOURS)).to_rfc3339();
    db.execute("DELETE FROM app_retention_intents WHERE state='preview' AND (julianday(created_at) IS NULL OR julianday(created_at)<=julianday(?1)) AND NOT EXISTS(SELECT 1 FROM app_retention_gate WHERE token=app_retention_intents.token)",[cutoff])?;
    db.execute("DELETE FROM app_retention_intents WHERE token IN(SELECT i.token FROM app_retention_intents i WHERE i.state IN('done','failed') AND NOT EXISTS(SELECT 1 FROM app_retention_gate g WHERE g.token=i.token) ORDER BY julianday(i.created_at) DESC,i.rowid DESC LIMIT -1 OFFSET ?1)",[MAX_TERMINAL_RECEIPTS])?;
    let tokens = {
        let mut q=db.prepare("SELECT i.token FROM app_retention_intents i WHERE i.state IN('done','failed') AND NOT EXISTS(SELECT 1 FROM app_retention_gate g WHERE g.token=i.token)")?;
        q.query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    for token in tokens {
        let json: String = db.query_row(
            "SELECT payload_json FROM app_retention_intents WHERE token=?1",
            [&token],
            |r| r.get(0),
        )?;
        // Corrupt terminal receipts need no recovery. Do not let one prevent
        // startup recovery of valid confirmed intents; count pruning still applies.
        let Ok(mut p) = serde_json::from_str::<Payload>(&json) else {
            continue;
        };
        if p.format_version != 1
            || (p.moves.is_empty() && p.entries.is_empty() && p.run_ids.is_empty())
        {
            continue;
        }
        p.moves.clear();
        p.entries.clear();
        p.run_ids.clear();
        db.execute(
            "UPDATE app_retention_intents SET payload_json=?2 WHERE token=?1",
            params![token, serde_json::to_string(&p)?],
        )?;
    }
    Ok(())
}
fn prune(root: &Path) -> Result<()> {
    let mut db = store::open(root)?;
    let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    prune_receipts(&tx)?;
    tx.commit()?;
    Ok(())
}
fn terminal(db: &mut Connection, p: &Payload, state: &str, error: Option<&str>) -> Result<()> {
    let compact = Payload {
        format_version: p.format_version,
        preview: p.preview.clone(),
        moves: vec![],
        entries: vec![],
        run_ids: vec![],
    };
    let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let changed=tx.execute("UPDATE app_retention_intents SET state=?2,error=?3,payload_json=?4 WHERE token=?1 AND NOT EXISTS(SELECT 1 FROM app_retention_gate WHERE token=app_retention_intents.token)",params![p.preview.token,state,error,serde_json::to_string(&compact)?])?;
    if changed != 1 {
        bail!("Retention receipt is missing or protected by a deletion gate.");
    }
    prune_receipts(&tx)?;
    tx.commit()?;
    Ok(())
}

fn valid_id(id: &str) -> Result<()> {
    if id.is_empty()
        || id.len() > 64
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        bail!("Invalid owned meeting identifier.");
    }
    Ok(())
}
fn capability(root: &Path) -> Result<Dir> {
    Ok(Dir::open_ambient_dir(root, ambient_authority())?)
}
fn relative(path: &str) -> Result<&Path> {
    let p = Path::new(path);
    if p.as_os_str().is_empty() || p.components().any(|c| !matches!(c, Component::Normal(_))) {
        bail!("Unsafe retention path.");
    }
    Ok(p)
}
fn exists(dir: &Dir, path: &str) -> Result<bool> {
    match dir.symlink_metadata(relative(path)?) {
        Ok(m) => {
            if m.file_type().is_symlink() {
                bail!("Retention refuses symbolic links.");
            }
            Ok(true)
        }
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            ) =>
        {
            Ok(false)
        }
        Err(e) => Err(e.into()),
    }
}
fn inspect(dir: &Dir, path: &str, depth: usize, out: &mut Vec<Entry>) -> Result<()> {
    if depth > MAX_DEPTH || out.len() >= MAX_ENTRIES {
        bail!("Owned deletion tree exceeds safety bounds.");
    }
    let m = dir.symlink_metadata(relative(path)?)?;
    if m.file_type().is_symlink() || (!m.is_file() && !m.is_dir()) {
        bail!("Retention refuses links or special files.");
    }
    out.push(Entry {
        path: path.into(),
        directory: m.is_dir(),
        bytes: if m.is_file() { m.len() } else { 0 },
        modified_ns: m
            .modified()?
            .into_std()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos(),
    });
    if m.is_dir() {
        let mut children = dir
            .read_dir(path)?
            .map(|e| e.map(|e| e.file_name()))
            .collect::<std::io::Result<Vec<_>>>()?;
        children.sort();
        for name in children {
            let Some(name) = name.to_str() else {
                bail!("Owned tree contains a non-UTF8 filename.");
            };
            inspect(dir, &format!("{path}/{name}"), depth + 1, out)?;
        }
    }
    Ok(())
}
fn sync(dir: &Dir) -> Result<()> {
    dir.try_clone()?.into_std_file().sync_all()?;
    Ok(())
}
fn safe_directory(dir: &Dir, path: &str) -> Result<()> {
    if exists(dir, path)? {
        if !dir.symlink_metadata(path)?.is_dir() {
            bail!("Retention directory is not a directory.");
        }
    } else {
        dir.create_dir(path)?;
    }
    Ok(())
}
fn lock_meeting(dir: &Dir, meeting: &str, trash: Option<&str>) -> Result<Option<File>> {
    let source = format!("meetings/{meeting}/.recording.lock");
    let path = if exists(dir, &source)? {
        Some(source)
    } else if let Some(trash) = trash {
        let target = format!("{trash}/.recording.lock");
        exists(dir, &target)?.then_some(target)
    } else {
        None
    };
    if let Some(path) = path {
        let f = dir.open(path)?.into_std();
        f.try_lock()
            .map_err(|e| anyhow::anyhow!("Recording is leased: {e}"))?;
        Ok(Some(f))
    } else {
        Ok(None)
    }
}
fn idle(db: &Connection, meeting: &str) -> Result<()> {
    let status: String = db.query_row(
        "SELECT status FROM app_meetings WHERE id=?1",
        [meeting],
        |r| r.get(0),
    )?;
    if matches!(status.as_str(), "recording" | "paused" | "processing") {
        bail!("Meeting is recording, paused or processing.");
    }
    let running: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM app_processing_runs WHERE meeting_id=?1 AND status='running')",
        [meeting],
        |r| r.get(0),
    )?;
    if running {
        bail!("Meeting has a running processing lease.");
    }
    Ok(())
}
fn pending(db: &Connection, meeting: &str) -> Result<bool> {
    Ok(db.query_row("SELECT EXISTS(SELECT 1 FROM app_retention_intents WHERE meeting_id=?1 AND state IN('confirmed','cleanup'))",[meeting],|r|r.get(0))?)
}
/// Includes intents whose meeting rows have already been removed during cleanup.
/// Block recorder recovery, playback and inference while true: ready chunks can
/// be temporarily moved and must never be converted to missing-audio gaps.
pub(crate) fn has_pending(root: &Path, meeting: &str) -> Result<bool> {
    pending(&store::open(root)?, meeting)
}
pub(crate) fn audio_available(root: &Path, meeting: &str) -> Result<bool> {
    let db = store::open(root)?;
    if pending(&db, meeting)? {
        return Ok(false);
    }
    let exists: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM app_meetings WHERE id=?1)",
        [meeting],
        |r| r.get(0),
    )?;
    if !exists {
        return Ok(false);
    }
    Ok(db
        .query_row(
            "SELECT audio_available FROM app_audio_retention WHERE meeting_id=?1",
            [meeting],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or(true))
}

// Every current FK descendant is explicitly owned. Unknown descendants fail closed until reviewed.
const OWNED: &[&str] = &[
    "meetings",
    "audio_chunks",
    "app_meetings",
    "app_segments",
    "app_notes",
    "app_note_timing",
    "app_source_timing",
    "app_meeting_health",
    "app_transcription_staging",
    "app_audio_clock",
    "app_processing_runs",
    "app_meeting_revision",
    "app_run_segments",
    "app_segment_corrections",
    "app_correction_matches",
    "app_summaries",
    "app_summary_sources",
    "app_summary_failures",
    "app_speaker_identities",
    "app_run_speaker_keys",
    "app_speaker_turns",
    "app_speaker_run_state",
    "app_speaker_constraints",
    "app_speaker_turn_overrides",
    "app_speaker_reviews",
    "app_output_routes",
    "app_route_evidence",
    "app_run_echo_processing",
    "app_run_inference_health",
    "app_bleed_decisions",
    "app_bleed_run_matches",
    "app_audio_retention",
];
fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}
fn has_table(db: &Connection, table: &str) -> Result<bool> {
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
        [table],
        |r| r.get(0),
    )?)
}
fn graph(db: &Connection) -> Result<Vec<String>> {
    let mut q = db.prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")?;
    let tables = q
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut touched = Vec::new();
    for table in tables {
        let mut f = db.prepare(&format!("PRAGMA foreign_key_list({})", quote(&table)))?;
        let refs = f
            .query_map([], |r| r.get::<_, String>(2))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if refs.iter().any(|r| OWNED.contains(&r.as_str())) {
            if !OWNED.contains(&table.as_str()) {
                bail!(
                    "Unknown foreign-key descendant {table}; retention requires a schema update."
                );
            }
            touched.push(table);
        }
    }
    Ok(touched)
}
fn fingerprint(db: &Connection, meeting: &str) -> Result<String> {
    let mut h = Sha256::new();
    for table in OWNED {
        let mut c = db.prepare(&format!("PRAGMA table_info({})", quote(table)))?;
        let cols = c
            .query_map([], |r| r.get::<_, String>(1))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if cols.is_empty() {
            if matches!(
                *table,
                "app_note_timing"
                    | "app_source_timing"
                    | "app_meeting_health"
                    | "app_run_inference_health"
            ) {
                continue;
            }
            bail!("Retention schema is incomplete: {table}");
        }
        let predicate = if cols.iter().any(|c| c == "meeting_id") {
            "meeting_id=?1"
        } else if matches!(*table, "meetings" | "app_meetings") {
            "id=?1"
        } else if cols.iter().any(|c| c == "run_id") {
            "run_id IN(SELECT id FROM app_processing_runs WHERE meeting_id=?1)"
        } else if cols.iter().any(|c| c == "summary_id") {
            "summary_id IN(SELECT id FROM app_summaries WHERE meeting_id=?1)"
        } else if cols.iter().any(|c| c == "note_id") {
            "note_id IN(SELECT id FROM app_notes WHERE meeting_id=?1)"
        } else {
            bail!("Unknown retention ownership for {table}");
        };
        let columns = cols.iter().map(|c| quote(c)).collect::<Vec<_>>().join(",");
        let mut q = db.prepare(&format!(
            "SELECT {columns} FROM {} WHERE {predicate} ORDER BY {columns}",
            quote(table)
        ))?;
        let mut rows = q.query([meeting])?;
        h.update(table.as_bytes());
        while let Some(row) = rows.next()? {
            for i in 0..cols.len() {
                let v = row.get_ref(i)?;
                let bytes = match v {
                    rusqlite::types::ValueRef::Null => vec![0],
                    rusqlite::types::ValueRef::Integer(v) => format!("i{v}").into_bytes(),
                    rusqlite::types::ValueRef::Real(v) => {
                        format!("r{:x}", v.to_bits()).into_bytes()
                    }
                    rusqlite::types::ValueRef::Text(v) => [b"t".as_slice(), v].concat(),
                    rusqlite::types::ValueRef::Blob(v) => [b"b".as_slice(), v].concat(),
                };
                h.update((bytes.len() as u64).to_le_bytes());
                h.update(bytes);
            }
        }
    }
    Ok(format!("{:x}", h.finalize()))
}
fn validate_manifest(db: &Connection, meeting: &str) -> Result<()> {
    let mut q = db.prepare(
        "SELECT path,track,epoch,seq FROM audio_chunks WHERE meeting_id=?1 AND path IS NOT NULL",
    )?;
    let mut rows = q.query([meeting])?;
    while let Some(r) = rows.next()? {
        let (p, t, e, s): (String, String, i64, i64) = (r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?);
        if p != format!("meetings/{meeting}/audio/{t}/{e}-{s}.flac")
            || !matches!(t.as_str(), "mic" | "system")
        {
            bail!("Audio manifest contains an unowned path.");
        }
    }
    Ok(())
}
fn moves_for(dir: &Dir, meeting: &str, kind: &str) -> Result<Vec<String>> {
    let main = if kind == "meeting" {
        format!("meetings/{meeting}")
    } else {
        format!("meetings/{meeting}/audio")
    };
    if kind != "runs" && exists(dir, &main)? {
        Ok(vec![main])
    } else {
        Ok(vec![])
    }
}
fn make_plan(
    root: &Path,
    meeting: &str,
    kind: &str,
    run_ids: Vec<String>,
    protected: (u64, u64, u64),
) -> Result<DeletionPreview> {
    valid_id(meeting)?;
    let dir = capability(root)?;
    let mut db = store::open(root)?;
    idle(&db, meeting)?;
    if pending(&db, meeting)? {
        bail!("This meeting has pending retention recovery or trash cleanup.");
    }
    let _lock = lock_meeting(&dir, meeting, None)?;
    let foreign_key_tables = graph(&db)?;
    validate_manifest(&db, meeting)?;
    let moves = moves_for(&dir, meeting, kind)?;
    let mut entries = Vec::new();
    for p in &moves {
        inspect(&dir, p, 0, &mut entries)?;
    }
    let runs = if kind == "meeting" {
        u64::try_from(db.query_row(
            "SELECT count(*) FROM app_processing_runs WHERE meeting_id=?1",
            [meeting],
            |r| r.get::<_, i64>(0),
        )?)?
    } else {
        run_ids.len() as u64
    };
    let bytes = entries.iter().try_fold(0_u64, |sum, e| {
        sum.checked_add(e.bytes)
            .context("Owned file size total overflow")
    })?;
    i64::try_from(bytes).context("Owned file size total exceeds supported storage range")?;
    let preview = DeletionPreview {
        token: uuid::Uuid::new_v4().to_string(),
        meeting_id: meeting.into(),
        kind: kind.into(),
        files: entries.iter().filter(|e| !e.directory).count() as u64,
        bytes,
        runs,
        fingerprint: fingerprint(&db, meeting)?,
        protected_runs: protected.0,
        protected_active: protected.1,
        protected_references: protected.2,
        foreign_key_tables,
    };
    let payload = Payload {
        format_version: 1,
        preview: preview.clone(),
        moves,
        entries,
        run_ids,
    };
    let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    prune_receipts(&tx)?;
    tx.execute("DELETE FROM app_retention_intents WHERE token IN(SELECT i.token FROM app_retention_intents i WHERE i.meeting_id=?1 AND i.state='preview' AND NOT EXISTS(SELECT 1 FROM app_retention_gate g WHERE g.token=i.token) ORDER BY julianday(i.created_at) DESC,i.rowid DESC LIMIT -1 OFFSET ?2)",params![meeting,MAX_PREVIEWS_PER_MEETING-1])?;
    tx.execute("INSERT INTO app_retention_intents(token,meeting_id,kind,state,payload_json,created_at) VALUES(?1,?2,?3,'preview',?4,?5)",params![preview.token,meeting,kind,serde_json::to_string(&payload)?,Utc::now().to_rfc3339()])?;
    tx.commit()?;
    Ok(preview)
}
pub(crate) fn plan(root: &Path, meeting: &str, kind: DeletionKind) -> Result<DeletionPreview> {
    make_plan(
        root,
        meeting,
        match kind {
            DeletionKind::Meeting => "meeting",
            DeletionKind::Audio => "audio",
        },
        vec![],
        (0, 0, 0),
    )
}
fn load(db: &Connection, token: &str) -> Result<(String, Payload)> {
    uuid::Uuid::parse_str(token).context("Invalid deletion token")?;
    let (state, json): (String, String) = db.query_row(
        "SELECT state,payload_json FROM app_retention_intents WHERE token=?1",
        [token],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let p: Payload = serde_json::from_str(&json)?;
    if p.format_version != 1 {
        bail!("Unsupported durable retention intent version.");
    }
    if p.preview.token != token {
        bail!("Deletion token mismatch.");
    }
    valid_id(&p.preview.meeting_id)?;
    Ok((state, p))
}
pub(crate) fn meeting_for_token(root: &Path, token: &str) -> Result<String> {
    uuid::Uuid::parse_str(token).context("Invalid deletion token")?;
    let meeting: String = store::open(root)?
        .query_row(
            "SELECT meeting_id FROM app_retention_intents WHERE token=?1",
            [token],
            |r| r.get(0),
        )
        .optional()?
        .context("Deletion preview expired or is no longer available. Review a fresh preview.")?;
    valid_id(&meeting)?;
    Ok(meeting)
}
fn inventory(dir: &Dir, p: &Payload) -> Result<Vec<Entry>> {
    let mut entries = Vec::new();
    for path in &p.moves {
        inspect(dir, path, 0, &mut entries)?;
    }
    Ok(entries)
}
pub(crate) fn commit(root: &Path, token: &str) -> Result<MaintenanceReport> {
    let mut db = store::open(root)?;
    let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    uuid::Uuid::parse_str(token).context("Invalid deletion token")?;
    let created: Option<String> = tx
        .query_row(
            "SELECT created_at FROM app_retention_intents WHERE token=?1",
            [token],
            |r| r.get(0),
        )
        .optional()?;
    let Some(created) = created else {
        bail!("Deletion preview expired or is no longer available. Review a fresh preview.");
    };
    let (state, p) = load(&tx, token)?;
    if state != "preview" {
        bail!("Only an unconfirmed preview can be confirmed.");
    }
    let created =
        chrono::DateTime::parse_from_rfc3339(&created).context("Invalid deletion preview date")?;
    if Utc::now().signed_duration_since(created) >= Duration::hours(PREVIEW_TTL_HOURS) {
        bail!("Deletion preview expired. Review a fresh preview.");
    }
    let dir = capability(root)?;
    idle(&tx, &p.preview.meeting_id)?;
    if pending(&tx, &p.preview.meeting_id)? {
        bail!("This meeting has pending retention recovery or trash cleanup.");
    }
    let _lock = lock_meeting(&dir, &p.preview.meeting_id, None)?;
    graph(&tx)?;
    if fingerprint(&tx, &p.preview.meeting_id)? != p.preview.fingerprint
        || inventory(&dir, &p)? != p.entries
    {
        bail!("Deletion preview is stale. Review a fresh preview.");
    }
    tx.execute(
        "UPDATE app_retention_intents SET state='confirmed',confirmed_at=?2 WHERE token=?1",
        params![token, Utc::now().to_rfc3339()],
    )?;
    tx.commit()?;
    checkpoint("confirmed");
    drop(_lock);
    resume(root, token)
}

fn trash_base(token: &str) -> String {
    format!(".retention-trash/{token}")
}
fn destination(token: &str, index: usize) -> String {
    format!("{}/{index}", trash_base(token))
}
fn move_to_trash(dir: &Dir, p: &Payload) -> Result<()> {
    safe_directory(dir, ".retention-trash")?;
    safe_directory(dir, &trash_base(&p.preview.token))?;
    for (i, source) in p.moves.iter().enumerate() {
        let target = destination(&p.preview.token, i);
        match (exists(dir, source)?, exists(dir, &target)?) {
            (true, false) => {
                let mut scratch = Vec::new();
                inspect(dir, source, 0, &mut scratch)?;
                dir.rename(source, dir, &target)?;
                sync(dir)?;
                if let Some(parent) = Path::new(source).parent() {
                    sync(&dir.open_dir(parent)?)?;
                }
                sync(&dir.open_dir(trash_base(&p.preview.token))?)?;
            }
            (false, true) => {}
            (false, false) => bail!("Confirmed deletion source is missing: {source}"),
            (true, true) => bail!("Both original and trash exist; refusing overwrite: {source}"),
        }
        let mut actual = Vec::new();
        inspect(dir, &target, 0, &mut actual)?;
        let expected = p
            .entries
            .iter()
            .filter(|e| e.path == *source || e.path.starts_with(&format!("{source}/")))
            .map(|e| {
                let mut entry = e.clone();
                entry.path = format!(
                    "{target}{}",
                    e.path.strip_prefix(source).expect("selected source prefix")
                );
                entry
            })
            .collect::<Vec<_>>();
        if actual != expected {
            bail!("Owned files changed after confirmation; refusing database deletion.");
        }
    }
    Ok(())
}
fn restore(dir: &Dir, p: &Payload) -> Result<()> {
    for (i, source) in p.moves.iter().enumerate().rev() {
        let target = destination(&p.preview.token, i);
        if exists(dir, &target)? {
            if exists(dir, source)? {
                bail!("Cannot restore over an existing source.");
            }
            dir.rename(&target, dir, source)?;
            sync(dir)?;
            if let Some(parent) = Path::new(source).parent() {
                sync(&dir.open_dir(parent)?)?;
            }
            sync(&dir.open_dir(trash_base(&p.preview.token))?)?;
        } else if !exists(dir, source)? {
            bail!("Cannot restore a missing original and missing trash source.");
        }
    }
    Ok(())
}
fn erase_tree(dir: &Dir, path: &str, depth: usize, count: &mut usize) -> Result<()> {
    if depth > MAX_DEPTH || *count >= MAX_ENTRIES {
        bail!("Trash cleanup exceeds safety bound.");
    }
    *count += 1;
    let m = dir.symlink_metadata(relative(path)?)?;
    if m.file_type().is_symlink() {
        bail!("Trash contains a link; refusing traversal.");
    }
    if m.is_dir() {
        let children = dir
            .read_dir(path)?
            .map(|e| e.map(|e| e.file_name()))
            .collect::<std::io::Result<Vec<_>>>()?;
        for name in children {
            let Some(name) = name.to_str() else {
                bail!("Trash contains non-UTF8 path.");
            };
            erase_tree(dir, &format!("{path}/{name}"), depth + 1, count)?;
        }
        dir.remove_dir(path)?;
    } else if m.is_file() {
        dir.remove_file(path)?;
    } else {
        bail!("Trash contains a special file.");
    }
    Ok(())
}
fn delete_run(db: &Connection, run: &str) -> Result<()> {
    for table in [
        "app_summary_sources",
        "app_correction_matches",
        "app_bleed_run_matches",
        "app_speaker_turn_overrides",
        "app_speaker_reviews",
        "app_run_speaker_keys",
        "app_speaker_turns",
        "app_speaker_run_state",
        "app_run_echo_processing",
        "app_run_inference_health",
        "app_run_segments",
    ] {
        if table == "app_run_inference_health" && !has_table(db, table)? {
            continue;
        }
        db.execute(
            &format!("DELETE FROM {} WHERE run_id=?1", quote(table)),
            [run],
        )?;
    }
    db.execute("DELETE FROM app_processing_runs WHERE id=?1", [run])?;
    Ok(())
}
fn delete_meeting(db: &Connection, meeting: &str) -> Result<()> {
    if has_table(db, "app_note_timing")? {
        db.execute("DELETE FROM app_note_timing WHERE note_id IN(SELECT id FROM app_notes WHERE meeting_id=?1)",[meeting])?;
    }
    for table in ["app_source_timing", "app_meeting_health"] {
        if has_table(db, table)? {
            db.execute(
                &format!("DELETE FROM {} WHERE meeting_id=?1", quote(table)),
                [meeting],
            )?;
        }
    }
    db.execute("DELETE FROM app_summary_sources WHERE summary_id IN(SELECT id FROM app_summaries WHERE meeting_id=?1)",[meeting])?;
    db.execute("DELETE FROM app_summaries WHERE meeting_id=?1", [meeting])?;
    db.execute("DELETE FROM app_bleed_run_matches WHERE decision_id IN(SELECT id FROM app_bleed_decisions WHERE meeting_id=?1)",[meeting])?;
    db.execute(
        "DELETE FROM app_bleed_decisions WHERE meeting_id=?1",
        [meeting],
    )?;
    db.execute("DELETE FROM app_correction_matches WHERE correction_id IN(SELECT id FROM app_segment_corrections WHERE meeting_id=?1)",[meeting])?;
    db.execute(
        "DELETE FROM app_meeting_revision WHERE meeting_id=?1",
        [meeting],
    )?;
    let mut q = db.prepare("SELECT id FROM app_processing_runs WHERE meeting_id=?1")?;
    let runs = q
        .query_map([meeting], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(q);
    for run in runs {
        delete_run(db, &run)?;
    }
    for table in [
        "app_speaker_constraints",
        "app_speaker_identities",
        "app_segment_corrections",
        "app_transcription_staging",
        "app_segments",
        "app_notes",
        "app_audio_clock",
        "app_output_routes",
        "app_route_evidence",
        "app_summary_failures",
        "app_audio_retention",
        "audio_chunks",
    ] {
        db.execute(
            &format!("DELETE FROM {} WHERE meeting_id=?1", quote(table)),
            [meeting],
        )?;
    }
    db.execute("DELETE FROM app_search_fts WHERE meeting_id=?1", [meeting])?;
    db.execute("DELETE FROM app_meetings WHERE id=?1", [meeting])?;
    db.execute("DELETE FROM meetings WHERE id=?1", [meeting])?;
    Ok(())
}
fn cleanup_database(db: &mut Connection, p: &Payload) -> Result<()> {
    let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let meeting = &p.preview.meeting_id;
    idle(&tx, meeting)?;
    graph(&tx)?;
    if fingerprint(&tx, meeting)? != p.preview.fingerprint {
        bail!("Confirmed deletion snapshot changed; original files will be restored.");
    }
    tx.pragma_update(None, "defer_foreign_keys", "ON")?;
    tx.execute(
        "INSERT INTO app_retention_gate(token) VALUES(?1)",
        [&p.preview.token],
    )?;
    match p.preview.kind.as_str() {
        "meeting" => delete_meeting(&tx, meeting)?,
        "audio" => {
            let chunks: i64 = tx.query_row(
                "SELECT count(*) FROM audio_chunks WHERE meeting_id=?1",
                [meeting],
                |r| r.get(0),
            )?;
            tx.execute("DELETE FROM audio_chunks WHERE meeting_id=?1", [meeting])?;
            tx.execute("INSERT INTO app_audio_retention(meeting_id,audio_available,purged_at,original_chunks,original_bytes) VALUES(?1,0,?2,?3,?4) ON CONFLICT(meeting_id) DO UPDATE SET audio_available=0,purged_at=excluded.purged_at",params![meeting,Utc::now().to_rfc3339(),chunks,i64::try_from(p.preview.bytes)?])?;
            for run in &p.run_ids {
                if protected(&tx, run)? {
                    bail!("A history run gained a protected reference.");
                }
                delete_run(&tx, run)?;
            }
        }
        "runs" => {
            for run in &p.run_ids {
                if protected(&tx, run)? {
                    bail!("A history run gained a protected reference.");
                }
                delete_run(&tx, run)?;
            }
        }
        _ => bail!("Invalid deletion kind."),
    }
    tx.execute(
        "DELETE FROM app_retention_gate WHERE token=?1",
        [&p.preview.token],
    )?;
    tx.execute(
        "UPDATE app_retention_intents SET state='cleanup',error=NULL WHERE token=?1",
        [&p.preview.token],
    )?;
    tx.execute("DELETE FROM app_retention_intents WHERE meeting_id=?1 AND state='preview' AND NOT EXISTS(SELECT 1 FROM app_retention_gate WHERE token=app_retention_intents.token)",[meeting])?;
    tx.commit()?;
    Ok(())
}
fn resume(root: &Path, token: &str) -> Result<MaintenanceReport> {
    let mut db = store::open(root)?;
    let (state, p) = load(&db, token)?;
    let dir = capability(root)?;
    let mut report = MaintenanceReport::default();
    if state == "confirmed" {
        let trash_lock = p
            .moves
            .iter()
            .position(|path| path == &format!("meetings/{}", p.preview.meeting_id))
            .map(|i| destination(token, i));
        let _lock = lock_meeting(&dir, &p.preview.meeting_id, trash_lock.as_deref())?;
        let result = move_to_trash(&dir, &p).and_then(|()| {
            checkpoint("moved");
            cleanup_database(&mut db, &p)
        });
        if let Err(error) = result {
            let restored = restore(&dir, &p);
            let message = match restored {
                Ok(()) => {
                    terminal(&mut db, &p, "failed", Some(&error.to_string()))?;
                    format!("Deletion failed; original files restored: {error}")
                }
                Err(restore) => {
                    let message = format!(
                        "Deletion pending recovery; restore failed: {restore}; original error: {error}"
                    );
                    // Recovery capability and the primary error take precedence
                    // if the same storage fault also prevents saving its status.
                    let _=db.execute("UPDATE app_retention_intents SET error=?2 WHERE token=?1 AND state='confirmed'",params![token,message]);
                    message
                }
            };
            bail!("{message}");
        }
    } else if state != "cleanup" {
        bail!("Intent is not confirmed or awaiting cleanup.");
    }
    checkpoint("database");
    let cleanup = (|| -> Result<()> {
        let base = trash_base(token);
        if exists(&dir, &base)? {
            erase_tree(&dir, &base, 0, &mut 0)?;
        }
        sync(&dir)?;
        Ok(())
    })();
    if let Err(error) = cleanup {
        report.pending_cleanup.push(token.into());
        report.errors.push(format!(
            "Data deletion committed; trash cleanup pending: {error}"
        ));
        db.execute(
            "UPDATE app_retention_intents SET error=?2 WHERE token=?1",
            params![token, error.to_string()],
        )?;
        return Ok(report);
    }
    terminal(&mut db, &p, "done", None)?;
    report.completed = 1;
    report.bytes_reclaimed = p.preview.bytes;
    match p.preview.kind.as_str() {
        "meeting" => report.meetings_deleted = 1,
        "audio" => {
            report.audio_purged = 1;
            report.runs_deleted = p.run_ids.len() as u64;
        }
        "runs" => report.runs_deleted = p.run_ids.len() as u64,
        _ => {}
    }
    Ok(report)
}
pub(crate) fn recover(root: &Path) -> Result<MaintenanceReport> {
    prune(root)?;
    let db = store::open(root)?;
    let mut q=db.prepare("SELECT token FROM app_retention_intents WHERE state IN('confirmed','cleanup') ORDER BY created_at,token")?;
    let tokens = q
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(q);
    drop(db);
    let mut report = MaintenanceReport::default();
    for token in tokens {
        match resume(root, &token) {
            Ok(r) => merge(&mut report, r),
            Err(e) => {
                let db = store::open(root)?;
                let state: Option<String> = db
                    .query_row(
                        "SELECT state FROM app_retention_intents WHERE token=?1",
                        [&token],
                        |r| r.get(0),
                    )
                    .optional()?;
                if matches!(state.as_deref(), Some("confirmed" | "cleanup")) {
                    report.pending_cleanup.push(token);
                }
                report.errors.push(e.to_string());
            }
        }
    }
    Ok(report)
}
fn merge(a: &mut MaintenanceReport, b: MaintenanceReport) {
    a.completed += b.completed;
    a.meetings_deleted += b.meetings_deleted;
    a.audio_purged += b.audio_purged;
    a.runs_deleted += b.runs_deleted;
    a.bytes_reclaimed += b.bytes_reclaimed;
    a.pending_cleanup.extend(b.pending_cleanup);
    a.errors.extend(b.errors);
}
fn protected(db: &Connection, run: &str) -> Result<bool> {
    Ok(db.query_row("SELECT EXISTS(SELECT 1 FROM app_meeting_revision WHERE active_run_id=?1) OR EXISTS(SELECT 1 FROM app_processing_runs WHERE id=?1 AND status='running') OR EXISTS(SELECT 1 FROM app_summaries WHERE run_id=?1) OR EXISTS(SELECT 1 FROM app_summary_sources WHERE run_id=?1) OR EXISTS(SELECT 1 FROM app_bleed_decisions WHERE source_run_id=?1) OR EXISTS(SELECT 1 FROM app_correction_matches WHERE run_id=?1)",[run],|r|r.get(0))?)
}
pub(crate) fn preview_policy(root: &Path, policy: &MaintenancePolicy) -> Result<PolicyPreview> {
    policy.validate()?;
    let db = store::open(root)?;
    let mut q = db.prepare("SELECT id,started_at FROM app_meetings ORDER BY id")?;
    let meetings = q
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let now = Utc::now();
    let mut out = PolicyPreview {
        policy: policy.clone(),
        actions: vec![],
        protected_runs: 0,
        skipped_busy: vec![],
    };
    for (meeting, started) in meetings {
        if idle(&db, &meeting).is_err() || pending(&db, &meeting)? {
            out.skipped_busy.push(meeting);
            continue;
        }
        let mut audio_due = false;
        if let Some(days) = policy.audio_keep_days {
            let started = chrono::DateTime::parse_from_rfc3339(&started)
                .context("Invalid saved meeting date")?;
            let cutoff = now
                .checked_sub_signed(Duration::days(i64::from(days)))
                .context("Audio retention period exceeds supported dates")?;
            audio_due = started < cutoff && audio_available(root, &meeting)?;
        }
        let mut q = db.prepare(
            "SELECT id,status,created_at FROM app_processing_runs WHERE meeting_id=?1 ORDER BY id",
        )?;
        let runs = q
            .query_map([&meeting], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let (mut candidates, mut protected_runs, mut active, mut references) =
            (Vec::new(), 0, 0, 0);
        for (id, status, created) in runs {
            let current: bool = db.query_row(
                "SELECT EXISTS(SELECT 1 FROM app_meeting_revision WHERE active_run_id=?1)",
                [&id],
                |r| r.get(0),
            )?;
            if current || status == "running" {
                protected_runs += 1;
                active += 1;
            } else if protected(&db, &id)? {
                protected_runs += 1;
                references += 1;
            } else if !policy.cleanup_obsolete_runs {
                protected_runs += 1;
            } else if chrono::DateTime::parse_from_rfc3339(&created)?
                < now
                    .checked_sub_signed(Duration::days(i64::from(policy.obsolete_run_days)))
                    .context("History retention period exceeds supported dates")?
            {
                candidates.push(id);
            } else {
                protected_runs += 1;
            }
        }
        out.protected_runs += protected_runs;
        if audio_due || !candidates.is_empty() {
            out.actions.push(make_plan(
                root,
                &meeting,
                if audio_due { "audio" } else { "runs" },
                candidates,
                (protected_runs, active, references),
            )?);
        }
    }
    Ok(out)
}
pub(crate) fn apply_policy(root: &Path, preview: &PolicyPreview) -> Result<MaintenanceReport> {
    let mut report = MaintenanceReport::default();
    for action in &preview.actions {
        match commit(root, &action.token) {
            Ok(r) => merge(&mut report, r),
            Err(e) => report.errors.push(format!("{}: {e}", action.meeting_id)),
        }
    }
    Ok(report)
}

#[cfg(test)]
#[path = "retention_tests.rs"]
mod tests;

#[cfg(not(test))]
fn checkpoint(_: &str) {}
#[cfg(test)]
fn checkpoint(phase: &str) {
    if std::env::var("VT_RETENTION_TEST_PHASE").as_deref() == Ok(phase) {
        let marker = std::env::var_os("VT_RETENTION_TEST_MARKER").expect("test marker");
        std::fs::write(marker, phase).expect("test checkpoint");
        loop {
            std::thread::park_timeout(std::time::Duration::from_millis(50));
        }
    }
}
