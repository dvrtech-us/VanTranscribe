use super::*;
use std::collections::{BTreeMap, BTreeSet};
use vt_diarize::{DiarizationResult, SpeakerTurn};

// Matches vt_diarize::Diarizer::new/set_temporal_constraints. Historical,
// renamed and revoked records do not consume the engine's temporal-pair limit.
const MAX_ACTIVE_TEMPORAL_CONSTRAINTS: i64 = 4096;

#[derive(Clone, Debug, serde::Serialize)]
pub struct SpeakerInfo {
    pub id: String,
    pub track: String,
    pub label: String,
    pub user_name: Option<String>,
    pub shared_room: bool,
    pub person_id: Option<String>,
    pub speech_ms: u64,
    pub provisional: bool,
}
#[derive(Clone, Debug, serde::Serialize)]
pub struct SpeakerReview {
    pub id: String,
    pub kind: String,
    pub previous_ids: Vec<String>,
    pub previous_labels: Vec<String>,
    pub current_ids: Vec<String>,
    pub reason: String,
    pub status: String,
    pub constraint_id: Option<String>,
}
#[derive(Clone, Debug, serde::Serialize)]
pub struct DiarizationInfo {
    pub segmentation_context: Option<vt_diarize::SegmentationContext>,
    pub embedding_version: String,
    pub calibrated: bool,
    pub warnings: Vec<String>,
}
#[derive(Clone, Debug, serde::Serialize)]
pub struct WordSpeaker {
    pub index: usize,
    pub speaker_ids: Vec<String>,
    pub label: String,
    pub overlap: bool,
    pub uncertain: bool,
}
#[derive(Clone, Debug, serde::Serialize)]
pub struct SpeakerAssignment {
    pub segment_id: String,
    pub speaker_ids: Vec<String>,
    pub provenance: String,
    pub overlap: bool,
    pub uncertain: bool,
    pub words: Vec<WordSpeaker>,
}
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct TurnAnchor {
    pub track: String,
    pub start_ms: u64,
    pub end_ms: u64,
}
#[derive(Clone, Debug, serde::Serialize)]
pub struct SpeakerConstraint {
    pub id: String,
    pub kind: String,
    pub left: TurnAnchor,
    pub right: TurnAnchor,
    pub active: bool,
}

type ResolvedIdentity = (String, String, Option<String>);
type IdentityCache = BTreeMap<String, (ResolvedIdentity, usize)>;

fn identity(db: &Connection, id: &str) -> Result<ResolvedIdentity> {
    identity_cached(db, id, &mut BTreeMap::new())
}

/// A cache belongs to one read/transaction only, so subsequent snapshots see
/// fresh user names. Alias nodes are cached too: shared chains are not re-read
/// once per historical turn or per aliased local key.
fn identity_cached(
    db: &Connection,
    id: &str,
    cache: &mut IdentityCache,
) -> Result<ResolvedIdentity> {
    let mut current = id.to_owned();
    let mut visited = BTreeSet::new();
    let mut path = Vec::new();
    let (resolved, mut depth) = loop {
        if let Some((resolved, depth)) = cache.get(&current) {
            if path.len() + depth > 128 {
                bail!("Speaker identity links contain a cycle.");
            }
            break (resolved.clone(), *depth);
        }
        if !visited.insert(current.clone()) || visited.len() > 128 {
            bail!("Speaker identity links contain a cycle.");
        }
        path.push(current.clone());
        let (alias, label, name): (Option<String>, String, Option<String>) = db.query_row(
            "SELECT alias_of,label,user_name FROM app_speaker_identities WHERE id=?1",
            [&current],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        if let Some(parent) = alias {
            current = parent;
        } else {
            break ((current, label, name), 0);
        }
    };
    // Retain full alias depth in cached tails, so caching cannot bypass the
    // existing 128-node safety bound on a later path sharing that tail.
    for node in path.into_iter().rev() {
        depth += 1;
        cache.insert(node, (resolved.clone(), depth));
    }
    Ok(resolved)
}

fn create_identity(db: &Connection, meeting: &str, track: &str, is_me: bool) -> Result<String> {
    create_identity_with_reuse(db, meeting, track, is_me, true)
}

fn create_identity_with_reuse(
    db: &Connection,
    meeting: &str,
    track: &str,
    is_me: bool,
    reuse_me: bool,
) -> Result<String> {
    if is_me && reuse_me && let Some(id) = db.query_row("SELECT id FROM app_speaker_identities WHERE meeting_id=?1 AND track=?2 AND is_me=1",params![meeting,track],|row|row.get::<_,String>(0)).optional()? { return Ok(id); }
    let count: i64 = db.query_row(
        "SELECT COUNT(*) FROM app_speaker_identities WHERE meeting_id=?1 AND track=?2 AND is_me=0",
        params![meeting, track],
        |row| row.get(0),
    )?;
    let id = uuid::Uuid::new_v4().to_string();
    let label = if is_me {
        "Me".into()
    } else {
        format!("Speaker {}", count + 1)
    };
    db.execute("INSERT INTO app_speaker_identities(id,meeting_id,track,label,is_me,created_at) VALUES (?1,?2,?3,?4,?5,?6)",params![id,meeting,track,label,is_me,chrono::Utc::now().to_rfc3339()])?;
    Ok(id)
}

pub(crate) fn mode(root: &Path, meeting: &str) -> Result<vt_diarize::MeetingMode> {
    let value: String = open(root)?.query_row(
        "SELECT mode FROM app_meetings WHERE id=?1",
        [meeting],
        |row| row.get(0),
    )?;
    Ok(if value == "in_person" {
        vt_diarize::MeetingMode::InPerson
    } else {
        vt_diarize::MeetingMode::Remote
    })
}

pub(crate) fn constraint_version(db: &Connection, meeting: &str) -> Result<u64> {
    Ok(db.query_row(
        "SELECT COALESCE(MAX(version),0) FROM app_speaker_constraints WHERE meeting_id=?1",
        [meeting],
        |row| row.get::<_, i64>(0),
    )? as u64)
}

pub(crate) fn record_warning(root: &Path, meeting: &str, warning: &str) -> Result<()> {
    let db = open(root)?;
    let run: String = db.query_row(
        "SELECT active_run_id FROM app_meeting_revision WHERE meeting_id=?1",
        [meeting],
        |row| row.get(0),
    )?;
    let running: bool = db.query_row(
        "SELECT status='running' FROM app_processing_runs WHERE id=?1",
        [&run],
        |row| row.get(0),
    )?;
    if !running {
        return Ok(());
    }
    let saved: Option<String> = db
        .query_row(
            "SELECT warnings_json FROM app_speaker_run_state WHERE run_id=?1",
            [&run],
            |row| row.get(0),
        )
        .optional()?;
    let mut warnings: Vec<String> = saved
        .as_deref()
        .map(serde_json::from_str)
        .transpose()?
        .unwrap_or_default();
    if !warnings.iter().any(|value| value == warning) && warnings.len() < 20 {
        warnings.push(warning.into());
    }
    db.execute("INSERT INTO app_speaker_run_state(run_id,embedding_version,calibrated,warnings_json) VALUES (?1,?2,0,?3) ON CONFLICT(run_id) DO UPDATE SET warnings_json=excluded.warnings_json",params![run,vt_diarize::EMBEDDING_VERSION,serde_json::to_string(&warnings)?])?;
    Ok(())
}

pub(crate) fn copy_previous(db: &Connection, meeting: &str, run: &str) -> Result<Vec<SpeakerTurn>> {
    ensure_run(db, meeting, run)?;
    if !db.query_row(
        "SELECT status='running' FROM app_processing_runs WHERE id=?1",
        [run],
        |row| row.get::<_, bool>(0),
    )? {
        bail!("Speaker annotations can only be copied to a running transcript.");
    }
    let previous: Option<String> = db.query_row(
        "SELECT active_run_id FROM app_meeting_revision WHERE meeting_id=?1",
        [meeting],
        |row| row.get(0),
    )?;
    let Some(previous) = previous.filter(|previous| previous != run) else {
        return Ok(Vec::new());
    };
    db.execute("INSERT INTO app_run_speaker_keys(run_id,local_id,identity_id) SELECT ?1,local_id,identity_id FROM app_run_speaker_keys WHERE run_id=?2",params![run,previous])?;
    db.execute("INSERT INTO app_speaker_turns(run_id,id,track,start_ms,end_ms,speaker_ids_json,label,provenance,score,overlap,uncertain,provisional) SELECT ?1,id,track,start_ms,end_ms,speaker_ids_json,label,provenance,score,overlap,uncertain,provisional FROM app_speaker_turns WHERE run_id=?2",params![run,previous])?;
    db.execute("INSERT INTO app_speaker_run_state(run_id,embedding_version,calibrated,warnings_json,segmentation_context_json) SELECT ?1,embedding_version,calibrated,warnings_json,segmentation_context_json FROM app_speaker_run_state WHERE run_id=?2",params![run,previous])?;
    db.execute("INSERT INTO app_speaker_turn_overrides(run_id,turn_id,identity_id,constraint_id) SELECT ?1,turn_id,identity_id,constraint_id FROM app_speaker_turn_overrides WHERE run_id=?2",params![run,previous])?;
    db.execute("INSERT INTO app_speaker_reviews(id,run_id,kind,previous_ids_json,current_ids_json,reason,status,constraint_id) SELECT lower(hex(randomblob(16))),?1,kind,previous_ids_json,current_ids_json,reason,status,constraint_id FROM app_speaker_reviews WHERE run_id=?2 AND status='needs_review'",params![run,previous])?;
    read_turns(db, run)
}

pub(crate) fn retain_previous(
    root: &Path,
    meeting: &str,
    run: &str,
) -> Result<Option<Vec<SpeakerTurn>>> {
    let mut db = open(root)?;
    let tx = db.transaction()?;
    let turns = copy_previous(&tx, meeting, run)?;
    if turns.is_empty() {
        tx.commit()?;
        return Ok(None);
    }
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
    warnings.push("Speaker analysis was not rerun because its model is disabled or unavailable. Previous timed labels and user corrections were retained.".into());
    tx.execute(
        "UPDATE app_speaker_run_state SET warnings_json=?2 WHERE run_id=?1",
        params![run, serde_json::to_string(&warnings)?],
    )?;
    tx.commit()?;
    Ok(Some(turns))
}

pub(crate) fn constraints(root: &Path, meeting: &str) -> Result<Vec<SpeakerConstraint>> {
    Ok(constraint_history(&open(root)?, meeting)?
        .into_iter()
        .filter(|constraint| constraint.active)
        .collect())
}

pub(crate) fn constraint_history(db: &Connection, meeting: &str) -> Result<Vec<SpeakerConstraint>> {
    let mut query=db.prepare("SELECT id,kind,left_anchor_json,right_anchor_json,is_active FROM app_speaker_constraints WHERE meeting_id=?1 AND kind IN ('must_link','cannot_link') ORDER BY version")?;
    query
        .query_map([meeting], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, bool>(4)?,
            ))
        })?
        .map(|row| {
            let (id, kind, left, right, active) = row?;
            Ok(SpeakerConstraint {
                id,
                kind,
                left: serde_json::from_str(&left)?,
                right: serde_json::from_str(&right)?,
                active,
            })
        })
        .collect()
}

fn remove_constraint_effect(db: &Connection, id: &str) -> Result<()> {
    db.execute("UPDATE app_speaker_identities SET alias_of=NULL,alias_constraint_id=NULL WHERE alias_constraint_id=?1",[id])?;
    db.execute(
        "DELETE FROM app_speaker_turn_overrides WHERE constraint_id=?1",
        [id],
    )?;
    db.execute(
        "UPDATE app_speaker_constraints SET is_active=0 WHERE id=?1",
        [id],
    )?;
    db.execute("UPDATE app_speaker_reviews SET status='resolved' WHERE constraint_id=?1 AND kind='constraint' AND status='needs_review'",[id])?;
    Ok(())
}

pub(crate) fn revoke_constraint(root: &Path, meeting: &str, id: &str) -> Result<()> {
    let mut db = open(root)?;
    let tx = db.transaction()?;
    let valid:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM app_speaker_constraints WHERE id=?1 AND meeting_id=?2 AND kind IN ('must_link','cannot_link') AND is_active=1)",params![id,meeting],|row|row.get(0))?;
    if !valid {
        bail!("This speaker correction is unavailable or already removed.");
    }
    let version = constraint_version(&tx, meeting)? + 1;
    remove_constraint_effect(&tx, id)?;
    tx.execute("INSERT INTO app_speaker_constraints(id,meeting_id,kind,left_anchor_json,value,version,created_at,is_active) VALUES (?1,?2,'revoke','{}',?3,?4,?5,0)",params![uuid::Uuid::new_v4().to_string(),meeting,id,version as i64,chrono::Utc::now().to_rfc3339()])?;
    bump_revision(&tx, meeting)?;
    rebuild_fts(&tx, meeting)?;
    tx.commit()?;
    Ok(())
}

pub(crate) fn read_turns(db: &Connection, run: &str) -> Result<Vec<SpeakerTurn>> {
    let mut query = db.prepare("SELECT t.id,t.track,t.start_ms,t.end_ms,t.speaker_ids_json,t.label,t.provenance,t.score,t.overlap,t.uncertain,t.provisional,o.identity_id FROM app_speaker_turns t LEFT JOIN app_speaker_turn_overrides o ON o.run_id=t.run_id AND o.turn_id=t.id WHERE t.run_id=?1 ORDER BY t.start_ms,t.track,t.id")?;
    let rows = query
        .query_map([run], |row| {
            Ok((
                SpeakerTurn {
                    id: row.get(0)?,
                    track: row.get(1)?,
                    start_ms: row.get::<_, i64>(2)? as u64,
                    end_ms: row.get::<_, i64>(3)? as u64,
                    speaker_ids: vec![],
                    label: row.get(5)?,
                    provenance: row.get(6)?,
                    score: row.get(7)?,
                    overlap: row.get(8)?,
                    uncertain: row.get(9)?,
                    provisional: row.get(10)?,
                },
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(11)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut turns = Vec::with_capacity(rows.len());
    let mut identities = BTreeMap::new();
    for (mut turn, ids, overridden) in rows {
        let stored_ids: Vec<String> = serde_json::from_str(&ids)?;
        // Failed/ambiguous constraint anchors are deliberately masked by the
        // inference layer. A prior local override must not manufacture an ID or
        // clear the supplied uncertainty on a turn with no current identity.
        if stored_ids.is_empty() {
            turns.push(turn);
            continue;
        }
        let ids: Vec<String> = if let Some(id) = overridden {
            turn.provenance = "user".into();
            turn.uncertain = false;
            turn.provisional = false;
            vec![id]
        } else {
            stored_ids
        };
        let mut labels = Vec::new();
        for id in ids {
            let (id, label, name) = identity_cached(db, &id, &mut identities)?;
            turn.speaker_ids.push(id);
            if name.is_some() {
                turn.provenance = "user".into();
                turn.uncertain = false;
                turn.provisional = false;
            }
            labels.push(name.unwrap_or(label));
        }
        turn.speaker_ids.sort();
        turn.speaker_ids.dedup();
        labels.sort();
        labels.dedup();
        if turn.overlap {
            turn.label = "Overlap".into();
        } else if !turn.provisional && !labels.is_empty() {
            turn.label = labels.join(" / ");
        }
        turns.push(turn);
    }
    Ok(turns)
}

pub(crate) fn save_result(
    root: &Path,
    meeting: &str,
    run: &str,
    result: &DiarizationResult,
    reconcile: bool,
) -> Result<Vec<SpeakerTurn>> {
    save_result_inner(root, meeting, run, result, reconcile, true)
}

/// Incremental live publication writes only the supplied bounded update. It
/// avoids resolving/scanning the complete meeting history inside the write lock.
pub(crate) fn save_live_updates(
    root: &Path,
    meeting: &str,
    run: &str,
    result: &DiarizationResult,
) -> Result<()> {
    save_result_inner(root, meeting, run, result, false, false).map(|_| ())
}

/// Constraint ambiguity is review metadata, never an invented assignment. Keep
/// one review per run/source constraint even if finalization is retried.
fn save_constraint_review(
    db: &Connection,
    meeting: &str,
    run: &str,
    review: &vt_diarize::ConstraintReview,
) -> Result<()> {
    let source = review
        .provenance
        .strip_prefix("user:")
        .map(|id| {
            db.query_row(
                "SELECT id,is_active FROM app_speaker_constraints WHERE id=?1 AND meeting_id=?2",
                params![id, meeting],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, bool>(1)?)),
            )
            .optional()
        })
        .transpose()?
        .flatten();
    let constraint_id = source.as_ref().map(|(id, _)| id.clone());
    let active = source.as_ref().is_none_or(|(_, active)| *active);
    let ranges = review
        .anchors
        .iter()
        .map(|anchor| (anchor.track.as_str(), anchor.start_ms, anchor.end_ms))
        .collect::<BTreeSet<_>>();
    let bounds = ranges
        .into_iter()
        .map(|(track, start, end)| format!("{track} {start}–{end} ms"))
        .collect::<Vec<_>>()
        .join("; ");
    let reason = if review.reason.trim().is_empty() {
        "A speaker constraint could not be reapplied."
    } else {
        review.reason.trim()
    };
    let reason = if bounds.is_empty() {
        reason.to_owned()
    } else {
        format!("{reason} Affected audio: {bounds}.")
    };
    let existing: Option<(String, String)> = if let Some(id) = &constraint_id {
        db.query_row("SELECT id,reason FROM app_speaker_reviews WHERE run_id=?1 AND kind='constraint' AND constraint_id=?2 ORDER BY id LIMIT 1",params![run,id],|row|Ok((row.get(0)?,row.get(1)?))).optional()?
    } else {
        db.query_row("SELECT id,reason FROM app_speaker_reviews WHERE run_id=?1 AND kind='constraint' AND constraint_id IS NULL AND reason=?2 ORDER BY id LIMIT 1",params![run,reason],|row|Ok((row.get(0)?,row.get(1)?))).optional()?
    };
    if let Some((id, previous_reason)) = existing {
        let reason = if previous_reason.contains(&reason) {
            previous_reason
        } else {
            format!("{previous_reason}\n{reason}")
        };
        db.execute("UPDATE app_speaker_reviews SET reason=?2,status=CASE WHEN ?3=0 AND status='needs_review' THEN 'resolved' ELSE status END WHERE id=?1",params![id,reason,active])?;
    } else {
        db.execute("INSERT INTO app_speaker_reviews(id,run_id,kind,previous_ids_json,current_ids_json,reason,status,constraint_id) VALUES (?1,?2,'constraint','[]','[]',?3,?4,?5)",params![uuid::Uuid::new_v4().to_string(),run,reason,if active {"needs_review"} else {"resolved"},constraint_id])?;
    }
    Ok(())
}

fn insert_identity_review(
    db: &Connection,
    run: &str,
    kind: &str,
    previous: &[String],
    current: &[String],
    reason: &str,
) -> Result<()> {
    db.execute("INSERT INTO app_speaker_reviews(id,run_id,kind,previous_ids_json,current_ids_json,reason) VALUES (?1,?2,?3,?4,?5,?6)",params![uuid::Uuid::new_v4().to_string(),run,kind,serde_json::to_string(previous)?,serde_json::to_string(current)?,reason])?;
    Ok(())
}

fn candidates_for_previous(
    db: &Connection,
    meeting: &str,
    previous: &[String],
    result: &DiarizationResult,
) -> Result<Vec<String>> {
    let mut tracks = BTreeSet::new();
    for id in previous {
        if let Some(track) = db
            .query_row(
                "SELECT track FROM app_speaker_identities WHERE id=?1 AND meeting_id=?2",
                params![id, meeting],
                |row| row.get::<_, String>(0),
            )
            .optional()?
        {
            tracks.insert(track);
        }
    }
    Ok(result
        .turns
        .iter()
        .filter(|turn| tracks.contains(&turn.track))
        .flat_map(|turn| turn.speaker_ids.iter().cloned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect())
}

fn save_result_inner(
    root: &Path,
    meeting: &str,
    run: &str,
    result: &DiarizationResult,
    reconcile: bool,
    need_resolved_turns: bool,
) -> Result<Vec<SpeakerTurn>> {
    let mut db = open(root)?;
    let tx = db.transaction()?;
    ensure_run(&tx, meeting, run)?;
    let running: bool = tx.query_row(
        "SELECT status='running' FROM app_processing_runs WHERE id=?1",
        [run],
        |row| row.get(0),
    )?;
    if !running {
        bail!("Speaker annotations can only be added to a running transcript.");
    }
    let mut matches = BTreeMap::new();
    let mut ambiguous = BTreeSet::new();
    if reconcile {
        let previous: Option<String> = tx.query_row(
            "SELECT active_run_id FROM app_meeting_revision WHERE meeting_id=?1",
            [meeting],
            |row| row.get(0),
        )?;
        let old = previous
            .as_deref()
            .map(|id| read_turns(&tx, id))
            .transpose()?
            .unwrap_or_default();
        let unique_old_ids = old
            .iter()
            .flat_map(|turn| turn.speaker_ids.iter().cloned())
            .collect::<BTreeSet<_>>();
        let mut identities = BTreeMap::new();
        let named = unique_old_ids
            .iter()
            .map(|id| identity_cached(&tx, id, &mut identities))
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .filter_map(|(id, _, name)| name.map(|name| (id, name)))
            .collect();
        let plan = crate::speaker_reconcile::reconcile(&old, &result.turns, &named);
        matches = plan.matches;
        let inherited = matches.values().cloned().collect::<BTreeSet<_>>();
        let mut reviewed = BTreeSet::new();
        // Repeated finalization must not resurrect an explicitly dismissed or
        // resolved identity review, nor duplicate one already pending here.
        {
            let mut query=tx.prepare("SELECT id,previous_ids_json,status FROM app_speaker_reviews WHERE run_id=?1 AND kind!='constraint'")?;
            let rows = query
                .query_map([run], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            for (id, previous, status) in rows {
                let ids: Vec<String> = serde_json::from_str(&previous)?;
                if status == "needs_review"
                    && !ids.is_empty()
                    && ids.iter().all(|id| inherited.contains(id))
                {
                    tx.execute(
                        "UPDATE app_speaker_reviews SET status='resolved' WHERE id=?1",
                        [id],
                    )?;
                }
                reviewed.extend(ids);
            }
        }
        let referenced = plan
            .reviews
            .iter()
            .flat_map(|review| review.previous_ids.iter().cloned())
            .collect::<BTreeSet<_>>();
        for review in plan.reviews {
            ambiguous.extend(review.current_ids.iter().cloned());
            let previous = review
                .previous_ids
                .iter()
                .filter(|id| !reviewed.contains(*id))
                .cloned()
                .collect::<Vec<_>>();
            if previous.is_empty() && !review.previous_ids.is_empty() {
                continue;
            }
            insert_identity_review(
                &tx,
                run,
                &review.kind,
                &previous,
                &review.current_ids,
                &review.reason,
            )?;
            reviewed.extend(previous);
        }
        for id in named.keys() {
            if inherited.contains(id) || referenced.contains(id) || reviewed.contains(id) {
                continue;
            }
            let previous = vec![id.clone()];
            let current = candidates_for_previous(&tx, meeting, &previous, result)?;
            insert_identity_review(
                &tx,
                run,
                "unmatched_name",
                &previous,
                &current,
                "A previously user-named speaker has no unambiguous current match. Review a current excerpt before preserving the name; no identity was assigned automatically.",
            )?;
            reviewed.insert(id.clone());
        }
        // A masked pass can contain no IDs at all. Carry unresolved identity
        // names from its review metadata, rather than relying on current turns
        // to keep that history reachable on the next full refinement.
        if let Some(previous_run) = previous.as_deref().filter(|previous| *previous != run) {
            let mut query=tx.prepare("SELECT kind,previous_ids_json,reason FROM app_speaker_reviews WHERE run_id=?1 AND kind!='constraint' AND status='needs_review'")?;
            let rows = query
                .query_map([previous_run], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            for (kind, ids, reason) in rows {
                let ids: Vec<String> = serde_json::from_str(&ids)?;
                let previous = ids
                    .into_iter()
                    .filter(|id| !inherited.contains(id) && !reviewed.contains(id))
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect::<Vec<_>>();
                if previous.is_empty() {
                    continue;
                }
                let current = candidates_for_previous(&tx, meeting, &previous, result)?;
                insert_identity_review(&tx, run, &kind, &previous, &current, &reason)?;
                reviewed.extend(previous);
            }
        }
    }
    for review in &result.constraint_reviews {
        save_constraint_review(&tx, meeting, run, review)?;
    }
    let mut prepared_keys = BTreeSet::new();
    for turn in &result.turns {
        if turn.start_ms >= turn.end_ms
            || turn.end_ms > i64::MAX as u64
            || !matches!(turn.track.as_str(), "mic" | "system")
            || turn.score.is_some_and(|score| !score.is_finite())
        {
            bail!("Invalid speaker turn.");
        }
        for local in &turn.speaker_ids {
            if !prepared_keys.insert(local.clone()) {
                continue;
            }
            let exists: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM app_run_speaker_keys WHERE run_id=?1 AND local_id=?2)",
                params![run, local],
                |row| row.get(0),
            )?;
            if !exists || (reconcile && (matches.contains_key(local) || ambiguous.contains(local)))
            {
                let id = if let Some(id) = matches.get(local) {
                    id.clone()
                } else {
                    create_identity_with_reuse(
                        &tx,
                        meeting,
                        &turn.track,
                        local.ends_with(":me"),
                        !reconcile,
                    )?
                };
                tx.execute("INSERT INTO app_run_speaker_keys(run_id,local_id,identity_id) VALUES (?1,?2,?3) ON CONFLICT(run_id,local_id) DO UPDATE SET identity_id=excluded.identity_id",params![run,local,id])?;
            }
        }
    }
    for merge in &result.merges {
        tx.execute("UPDATE app_run_speaker_keys SET identity_id=(SELECT identity_id FROM app_run_speaker_keys WHERE run_id=?1 AND local_id=?3) WHERE run_id=?1 AND local_id=?2 AND EXISTS(SELECT 1 FROM app_run_speaker_keys WHERE run_id=?1 AND local_id=?3)",params![run,merge.from,merge.into])?;
    }
    for turn in &result.turns {
        let ids = turn
            .speaker_ids
            .iter()
            .map(|local| {
                tx.query_row(
                    "SELECT identity_id FROM app_run_speaker_keys WHERE run_id=?1 AND local_id=?2",
                    params![run, local],
                    |row| row.get::<_, String>(0),
                )
                .map_err(Into::into)
            })
            .collect::<Result<Vec<_>>>()?;
        tx.execute("INSERT INTO app_speaker_turns(run_id,id,track,start_ms,end_ms,speaker_ids_json,label,provenance,score,overlap,uncertain,provisional) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12) ON CONFLICT(run_id,id) DO UPDATE SET speaker_ids_json=excluded.speaker_ids_json,label=excluded.label,provenance=excluded.provenance,score=excluded.score,overlap=excluded.overlap,uncertain=excluded.uncertain,provisional=excluded.provisional",params![run,turn.id,turn.track,turn.start_ms as i64,turn.end_ms as i64,serde_json::to_string(&ids)?,turn.label,turn.provenance,turn.score,turn.overlap,turn.uncertain,turn.provisional])?;
    }
    tx.execute("INSERT INTO app_speaker_run_state(run_id,embedding_version,calibrated,warnings_json,segmentation_context_json) VALUES (?1,?2,?3,?4,?5) ON CONFLICT(run_id) DO UPDATE SET embedding_version=excluded.embedding_version,calibrated=excluded.calibrated,warnings_json=excluded.warnings_json,segmentation_context_json=excluded.segmentation_context_json",params![run,result.embedding_version,result.calibrated,serde_json::to_string(&result.warnings)?,result.segmentation_context.as_ref().map(serde_json::to_string).transpose()?])?;
    let turns = if need_resolved_turns {
        read_turns(&tx, run)?
    } else {
        Vec::new()
    };
    tx.commit()?;
    Ok(turns)
}

pub(crate) fn resolve_review(
    root: &Path,
    meeting: &str,
    run: &str,
    review: &str,
    previous: Option<&str>,
    target: Option<&str>,
) -> Result<()> {
    let mut db = open(root)?;
    let tx = db.transaction()?;
    ensure_active(&tx, meeting, run)?;
    let (old,new):(String,String)=tx.query_row("SELECT previous_ids_json,current_ids_json FROM app_speaker_reviews WHERE id=?1 AND run_id=?2 AND status='needs_review'",params![review,run],|row|Ok((row.get(0)?,row.get(1)?)))?;
    if let Some(target) = target {
        let previous = previous
            .ok_or_else(|| anyhow::anyhow!("Choose the earlier named speaker to preserve."))?;
        let old: Vec<String> = serde_json::from_str(&old)?;
        let new: Vec<String> = serde_json::from_str(&new)?;
        let current = new
            .into_iter()
            .map(|local| {
                tx.query_row(
                    "SELECT identity_id FROM app_run_speaker_keys WHERE run_id=?1 AND local_id=?2",
                    params![run, local],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map(|id| id.unwrap_or(local))
            })
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if !old.iter().any(|id| id == previous) || !current.iter().any(|id| id == target) {
            bail!("Choose a speaker from this reconciliation review.");
        }
        let (_, label, name) = identity(&tx, previous)?;
        let name = name.unwrap_or(label);
        let (target, _, _) = identity(&tx, target)?;
        tx.execute(
            "UPDATE app_speaker_identities SET user_name=?2 WHERE id=?1",
            params![target, name],
        )?;
        let version = constraint_version(&tx, meeting)? + 1;
        tx.execute("INSERT INTO app_speaker_constraints(id,meeting_id,kind,left_anchor_json,identity_id,value,version,created_at) VALUES (?1,?2,'rename','{}',?3,?4,?5,?6)",params![uuid::Uuid::new_v4().to_string(),meeting,target,name,version as i64,chrono::Utc::now().to_rfc3339()])?;
        bump_revision(&tx, meeting)?;
        rebuild_fts(&tx, meeting)?;
    }
    tx.execute(
        "UPDATE app_speaker_reviews SET status=?2 WHERE id=?1",
        params![
            review,
            if target.is_some() {
                "resolved"
            } else {
                "dismissed"
            }
        ],
    )?;
    tx.commit()?;
    Ok(())
}

pub(crate) fn rename(root: &Path, meeting: &str, speaker: &str, name: &str) -> Result<()> {
    let name = name.trim();
    if name.chars().count() > 120 {
        bail!("Speaker names must contain at most 120 characters.");
    }
    let mut db = open(root)?;
    let tx = db.transaction()?;
    let belongs: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM app_speaker_identities WHERE id=?1 AND meeting_id=?2)",
        params![speaker, meeting],
        |row| row.get(0),
    )?;
    if !belongs {
        bail!("Speaker does not belong to this meeting.");
    }
    let (speaker, _, _) = identity(&tx, speaker)?;
    let version = constraint_version(&tx, meeting)? + 1;
    tx.execute(
        "UPDATE app_speaker_identities SET user_name=?2 WHERE id=?1",
        params![speaker, (!name.is_empty()).then_some(name)],
    )?;
    tx.execute("INSERT INTO app_speaker_constraints(id,meeting_id,kind,left_anchor_json,identity_id,value,version,created_at) VALUES (?1,?2,'rename','{}',?3,?4,?5,?6)",params![uuid::Uuid::new_v4().to_string(),meeting,speaker,name,version as i64,chrono::Utc::now().to_rfc3339()])?;
    bump_revision(&tx, meeting)?;
    rebuild_fts(&tx, meeting)?;
    tx.commit()?;
    Ok(())
}

pub(crate) fn set_shared_room(
    root: &Path,
    meeting: &str,
    speaker: &str,
    shared: bool,
) -> Result<()> {
    let mut db = open(root)?;
    let tx = db.transaction()?;
    let belongs: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM app_speaker_identities WHERE id=?1 AND meeting_id=?2)",
        params![speaker, meeting],
        |row| row.get(0),
    )?;
    if !belongs {
        bail!("Speaker does not belong to this meeting.");
    }
    let (speaker, _, _) = identity(&tx, speaker)?;
    tx.execute(
        "UPDATE app_speaker_identities SET shared_room=?2 WHERE id=?1",
        params![speaker, shared],
    )?;
    bump_revision(&tx, meeting)?;
    tx.commit()?;
    Ok(())
}

pub(crate) fn link_voice_person(
    db: &Connection,
    meeting: &str,
    speaker: &str,
    person: &str,
    name: &str,
) -> Result<()> {
    let (canonical, _, _) = identity(db, speaker)?;
    let changed = db.execute("UPDATE app_speaker_identities SET user_name=?3,person_id=?4 WHERE id=?1 AND meeting_id=?2 AND shared_room=0", params![canonical,meeting,name,person])?;
    if changed != 1 {
        bail!("The selected speaker changed or is a shared room. Refresh and retry.");
    }
    let version = constraint_version(db, meeting)? + 1;
    db.execute("INSERT INTO app_speaker_constraints(id,meeting_id,kind,left_anchor_json,identity_id,value,version,created_at) VALUES (?1,?2,'rename','{}',?3,?4,?5,?6)",params![uuid::Uuid::new_v4().to_string(),meeting,canonical,name,version as i64,chrono::Utc::now().to_rfc3339()])?;
    bump_revision(db, meeting)?;
    rebuild_fts(db, meeting)?;
    Ok(())
}

/// Remove only explicit person links and their optional speaker-name overlays.
/// Spoken text, immutable source annotations and already exported copies are retained.
pub(crate) fn forget_person_links(
    db: &Connection,
    people: &[String],
    remove_names: bool,
) -> Result<()> {
    let mut meetings = BTreeSet::new();
    for person in people {
        let rows = {
            let mut query =
                db.prepare("SELECT id,meeting_id FROM app_speaker_identities WHERE person_id=?1")?;
            query
                .query_map([person], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        for (speaker, meeting) in rows {
            db.execute(
                "UPDATE app_speaker_identities SET person_id=NULL WHERE id=?1",
                [&speaker],
            )?;
            if remove_names {
                db.execute(
                    "UPDATE app_speaker_identities SET user_name=NULL WHERE id=?1",
                    [&speaker],
                )?;
                db.execute("UPDATE app_speaker_constraints SET value='',is_active=0 WHERE identity_id=?1 AND kind='rename'", [&speaker])?;
                // An explicit empty overlay prevents older rename evidence from
                // being reapplied during a later refinement.
                let version = constraint_version(db, &meeting)? + 1;
                db.execute("INSERT INTO app_speaker_constraints(id,meeting_id,kind,left_anchor_json,identity_id,value,version,created_at) VALUES (?1,?2,'rename','{}',?3,'',?4,?5)",params![uuid::Uuid::new_v4().to_string(),meeting,speaker,version as i64,chrono::Utc::now().to_rfc3339()])?;
                meetings.insert(meeting);
            }
        }
    }
    for meeting in meetings {
        bump_revision(db, &meeting)?;
        rebuild_fts(db, &meeting)?;
    }
    Ok(())
}

pub(crate) fn add_constraint(
    root: &Path,
    meeting: &str,
    run: &str,
    left: &str,
    right: &str,
    kind: &str,
) -> Result<()> {
    if !matches!(kind, "must_link" | "cannot_link") || left == right {
        bail!("Choose two different speaker excerpts and a valid relationship.");
    }
    let mut db = open(root)?;
    let tx = db.transaction()?;
    ensure_active(&tx, meeting, run)?;
    let turns = read_turns(&tx, run)?;
    let left = turns
        .iter()
        .find(|turn| turn.id == left)
        .ok_or_else(|| anyhow::anyhow!("First speaker excerpt is unavailable."))?;
    let right = turns
        .iter()
        .find(|turn| turn.id == right)
        .ok_or_else(|| anyhow::anyhow!("Second speaker excerpt is unavailable."))?;
    if left.track != right.track
        || left.overlap
        || right.overlap
        || left.uncertain
        || right.uncertain
        || left.provisional
        || right.provisional
        || left.speaker_ids.is_empty()
        || right.speaker_ids.is_empty()
        || left.end_ms - left.start_ms < 1000
        || right.end_ms - right.start_ms < 1000
    {
        bail!(
            "Speaker constraints require two clean excerpts of at least one second on the same track."
        );
    }
    let anchor = |turn: &SpeakerTurn| TurnAnchor {
        track: turn.track.clone(),
        start_ms: turn.start_ms,
        end_ms: turn.end_ms,
    };
    // Check user-name conflicts before undoing any earlier relationship for
    // this pair; otherwise removing a split could hide the second chosen name.
    if kind == "must_link" && left.speaker_ids.len() == 1 && right.speaker_ids.len() == 1 {
        let (left_id, _, left_name) = identity(&tx, &left.speaker_ids[0])?;
        let (right_id, _, right_name) = identity(&tx, &right.speaker_ids[0])?;
        if left_id != right_id
            && left_name.is_some()
            && right_name.is_some()
            && left_name != right_name
        {
            bail!("These excerpts have different user names. Rename one before merging them.");
        }
    }
    let version = constraint_version(&tx, meeting)? + 1;
    let left_anchor = serde_json::to_string(&anchor(left))?;
    let right_anchor = serde_json::to_string(&anchor(right))?;
    let left_id = left.id.clone();
    let right_id = right.id.clone();
    let old = {
        let mut query=tx.prepare("SELECT id FROM app_speaker_constraints WHERE meeting_id=?1 AND is_active=1 AND kind IN ('must_link','cannot_link') AND ((left_anchor_json=?2 AND right_anchor_json=?3) OR (left_anchor_json=?3 AND right_anchor_json=?2))")?;
        query
            .query_map(params![meeting, left_anchor, right_anchor], |row| {
                row.get::<_, String>(0)
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    for id in old {
        remove_constraint_effect(&tx, &id)?;
    }
    // Check after superseding this exact (unordered) pair so replacing a
    // relationship still works at capacity. A rejection rolls the removals back
    // with the rest of this transaction, including aliases/reviews/revisions.
    let active: i64 = tx.query_row(
        "SELECT COUNT(*) FROM app_speaker_constraints WHERE meeting_id=?1 AND is_active=1 AND kind IN ('must_link','cannot_link')",
        [meeting],
        |row| row.get(0),
    )?;
    if active >= MAX_ACTIVE_TEMPORAL_CONSTRAINTS {
        bail!(
            "This meeting already has {MAX_ACTIVE_TEMPORAL_CONSTRAINTS} active speaker corrections. Remove an existing correction before adding another."
        );
    }
    let constraint_id = uuid::Uuid::new_v4().to_string();
    tx.execute("INSERT INTO app_speaker_constraints(id,meeting_id,kind,left_anchor_json,right_anchor_json,version,created_at) VALUES (?1,?2,?3,?4,?5,?6,?7)",params![constraint_id,meeting,kind,left_anchor,right_anchor,version as i64,chrono::Utc::now().to_rfc3339()])?;
    let refreshed = read_turns(&tx, run)?;
    let left = refreshed
        .iter()
        .find(|turn| turn.id == left_id)
        .expect("validated left turn");
    let right = refreshed
        .iter()
        .find(|turn| turn.id == right_id)
        .expect("validated right turn");
    if kind == "must_link" && left.speaker_ids.len() == 1 && right.speaker_ids.len() == 1 {
        let (keep, _, keep_name) = identity(&tx, &left.speaker_ids[0])?;
        let (other, _, other_name) = identity(&tx, &right.speaker_ids[0])?;
        if keep != other {
            if keep_name.is_some() && other_name.is_some() && keep_name != other_name {
                bail!("These excerpts have different user names. Rename one before merging them.");
            }
            let (keep, other) = if other_name.is_some() && keep_name.is_none() {
                (other, keep)
            } else {
                (keep, other)
            };
            // A merge cannot turn a known shared room into an eligible voice.
            tx.execute("UPDATE app_speaker_identities SET shared_room=MAX(shared_room,(SELECT shared_room FROM app_speaker_identities WHERE id=?2)) WHERE id=?1",params![keep,other])?;
            tx.execute(
                "UPDATE app_speaker_identities SET alias_of=?2,alias_constraint_id=?3 WHERE id=?1",
                params![other, keep, constraint_id],
            )?;
        }
    } else if kind == "cannot_link"
        && left
            .speaker_ids
            .iter()
            .any(|id| right.speaker_ids.contains(id))
    {
        let distinct = create_identity(&tx, meeting, &right.track, false)?;
        tx.execute("INSERT INTO app_speaker_turn_overrides(run_id,turn_id,identity_id,constraint_id) VALUES (?1,?2,?3,?4) ON CONFLICT(run_id,turn_id) DO UPDATE SET identity_id=excluded.identity_id,constraint_id=excluded.constraint_id",params![run,right.id,distinct,constraint_id])?;
    }
    bump_revision(&tx, meeting)?;
    rebuild_fts(&tx, meeting)?;
    tx.commit()?;
    Ok(())
}

struct Timeline {
    turns: Vec<SpeakerTurn>,
    max_end: Vec<u64>,
}
impl Timeline {
    fn new(mut turns: Vec<SpeakerTurn>) -> Self {
        turns.sort_by_key(|turn| turn.start_ms);
        let mut max = 0;
        let max_end = turns
            .iter()
            .map(|turn| {
                max = max.max(turn.end_ms);
                max
            })
            .collect();
        Self { turns, max_end }
    }
    fn at(&self, time: u64) -> Vec<&SpeakerTurn> {
        let mut i = self.turns.partition_point(|turn| turn.start_ms <= time);
        let mut found = Vec::new();
        while i > 0 {
            i -= 1;
            if self.max_end[i] <= time {
                break;
            }
            if self.turns[i].end_ms > time {
                found.push(&self.turns[i]);
            }
        }
        found
    }
}

pub(crate) fn annotate(
    db: &Connection,
    run: &str,
    segments: &mut [Segment],
) -> Result<Vec<SpeakerAssignment>> {
    let turns = read_turns(db, run)?;
    if turns.is_empty() {
        return Ok(Vec::new());
    }
    let timelines: [Timeline; 2] = [
        Timeline::new(
            turns
                .iter()
                .filter(|turn| turn.track == "mic")
                .cloned()
                .collect(),
        ),
        Timeline::new(
            turns
                .into_iter()
                .filter(|turn| turn.track == "system")
                .collect(),
        ),
    ];
    let mut assignments = Vec::new();
    for segment in segments {
        let timeline = &timelines[usize::from(segment.track == "system")];
        let mut ids = BTreeSet::new();
        let mut labels = BTreeSet::new();
        let mut overlap = false;
        let mut uncertain = false;
        let mut user = false;
        let mut words = Vec::new();
        let spans = if segment.words.is_empty() {
            vec![(segment.start_ms, segment.end_ms)]
        } else {
            segment
                .words
                .iter()
                .map(|word| (word.start_ms, word.end_ms))
                .collect()
        };
        for (index, (start, end)) in spans.into_iter().enumerate() {
            let active = timeline.at(start.saturating_add(end.saturating_sub(start) / 2));
            let word_overlap = active.iter().any(|turn| turn.overlap) || active.len() > 1;
            let word_uncertain =
                active.is_empty() || active.iter().any(|turn| turn.uncertain || turn.provisional);
            let word_ids = active
                .iter()
                .flat_map(|turn| turn.speaker_ids.iter().cloned())
                .collect::<BTreeSet<_>>();
            let word_label = if word_overlap {
                "Overlap".into()
            } else if word_uncertain {
                "Unknown".into()
            } else {
                active
                    .iter()
                    .map(|turn| turn.label.clone())
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect::<Vec<_>>()
                    .join(" / ")
            };
            overlap |= word_overlap;
            uncertain |= word_uncertain;
            user |= active.iter().any(|turn| turn.provenance == "user");
            ids.extend(word_ids.iter().cloned());
            labels.insert(word_label.clone());
            if !segment.words.is_empty() {
                words.push(WordSpeaker {
                    index,
                    speaker_ids: word_ids.into_iter().collect(),
                    label: word_label,
                    overlap: word_overlap,
                    uncertain: word_uncertain,
                });
            }
        }
        if !labels.is_empty() {
            segment.speaker = labels.into_iter().collect::<Vec<_>>().join(" / ");
        }
        assignments.push(SpeakerAssignment {
            segment_id: segment.id.clone(),
            speaker_ids: ids.into_iter().collect(),
            provenance: if user { "user" } else { "audio-derived" }.into(),
            overlap,
            uncertain,
            words,
        });
    }
    Ok(assignments)
}

pub(crate) struct SpeakerDetails {
    pub speakers: Vec<SpeakerInfo>,
    pub turns: Vec<SpeakerTurn>,
    pub reviews: Vec<SpeakerReview>,
    pub info: Option<DiarizationInfo>,
}

pub(crate) fn details(db: &Connection, run: &str) -> Result<SpeakerDetails> {
    let turns = read_turns(db, run)?;
    let mut seen: BTreeMap<String, (u64, bool)> = BTreeMap::new();
    for turn in &turns {
        for id in &turn.speaker_ids {
            let entry = seen.entry(id.clone()).or_insert((0, true));
            entry.0 = entry.0.saturating_add(turn.end_ms - turn.start_ms);
            entry.1 &= turn.provisional;
        }
    }
    let speakers = seen
        .into_iter()
        .map(|(id, (speech_ms, provisional))| {
            let (id, label, user_name) = identity(db, &id)?;
            let (track, shared_room, person_id) = db.query_row(
                "SELECT track,shared_room,person_id FROM app_speaker_identities WHERE id=?1",
                [&id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?;
            Ok(SpeakerInfo {
                id,
                track,
                label: user_name.clone().unwrap_or(label),
                user_name,
                shared_room,
                person_id,
                speech_ms,
                provisional,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let mut query=db.prepare("SELECT id,kind,previous_ids_json,current_ids_json,reason,status,constraint_id FROM app_speaker_reviews WHERE run_id=?1")?;
    let reviews=query.query_map([run],|row|Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,String>(2)?,row.get::<_,String>(3)?,row.get::<_,String>(4)?,row.get::<_,String>(5)?,row.get::<_,Option<String>>(6)?)))?.map(|row|{let(id,kind,previous,current,reason,status,constraint_id)=row?;
        let previous_ids:Vec<String>=serde_json::from_str(&previous)?;
        let previous_labels=previous_ids.iter().map(|id|identity(db,id).map(|(_,label,name)|name.unwrap_or(label))).collect::<Result<Vec<_>>>()?;
        let locals:Vec<String>=serde_json::from_str(&current)?;
        let current_ids=locals.into_iter().map(|local|db.query_row("SELECT identity_id FROM app_run_speaker_keys WHERE run_id=?1 AND local_id=?2",params![run,local],|row|row.get::<_,String>(0)).optional().map(|id|id.unwrap_or(local))).collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(SpeakerReview{id,kind,previous_ids,previous_labels,current_ids,reason,status,constraint_id})}).collect::<Result<Vec<_>>>()?;
    let info=db.query_row("SELECT embedding_version,calibrated,warnings_json,segmentation_context_json FROM app_speaker_run_state WHERE run_id=?1",[run],|row|Ok((row.get::<_,String>(0)?,row.get::<_,bool>(1)?,row.get::<_,String>(2)?,row.get::<_,Option<String>>(3)?))).optional()?.map(|(embedding_version,calibrated,warnings,context)|Ok::<_,anyhow::Error>(DiarizationInfo{embedding_version,calibrated,warnings:serde_json::from_str(&warnings)?,segmentation_context:context.as_deref().map(serde_json::from_str).transpose()?})).transpose()?;
    Ok(SpeakerDetails {
        speakers,
        turns,
        reviews,
        info,
    })
}
