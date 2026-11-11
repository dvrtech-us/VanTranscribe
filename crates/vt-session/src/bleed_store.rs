//! Reversible user-confirmed bleed annotations; text similarity only suggests visible candidates.
use super::*;
use std::collections::{BTreeMap, BTreeSet};

const WINDOW_MS: u64 = 2_000;
const MAX_SYSTEM_CANDIDATES: usize = 64;
const MAX_TOKENS: usize = 128;

#[derive(Clone, Debug, Serialize)]
pub struct BleedDecision {
    pub id: String,
    pub segment_id: Option<String>,
    pub decision: String,
    pub reason: String,
    pub status: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct BleedCandidate {
    pub mic_segment_id: String,
    pub system_segment_id: String,
    pub similarity: f64,
}
#[derive(Clone, Debug, Serialize)]
pub struct BleedDetails {
    pub decisions: Vec<BleedDecision>,
    pub candidates: Vec<BleedCandidate>,
}
#[derive(Clone)]
struct Event {
    id: String,
    lineage: String,
    source_run: String,
    source_segment: String,
    start: i64,
    end: i64,
    hash: String,
    decision: String,
    reason: String,
    retired: bool,
}
fn latest(db: &Connection, meeting: &str) -> Result<Vec<Event>> {
    let mut q = db.prepare("SELECT d.id,d.lineage_id,d.source_run_id,d.source_segment_id,d.start_ms,d.end_ms,d.original_text_hash,d.decision,d.reason,d.retired FROM app_bleed_decisions d WHERE d.meeting_id=?1 AND NOT EXISTS(SELECT 1 FROM app_bleed_decisions n WHERE n.meeting_id=d.meeting_id AND n.lineage_id=d.lineage_id AND n.seq>d.seq) ORDER BY d.seq")?;
    Ok(q.query_map([meeting], |r| {
        Ok(Event {
            id: r.get(0)?,
            lineage: r.get(1)?,
            source_run: r.get(2)?,
            source_segment: r.get(3)?,
            start: r.get(4)?,
            end: r.get(5)?,
            hash: r.get(6)?,
            decision: r.get(7)?,
            reason: r.get(8)?,
            retired: r.get(9)?,
        })
    })?
    .collect::<rusqlite::Result<_>>()?)
}
fn editable(db: &Connection, meeting: &str, run: &str) -> Result<()> {
    ensure_active(db, meeting, run)?;
    let allowed: bool = db.query_row("SELECT status='complete' OR (status='running' AND kind='live') FROM app_processing_runs WHERE id=?1", [run], |r|r.get(0))?;
    if !allowed {
        bail!("Bleed decisions require a completed transcript or a finalized live line.");
    }
    Ok(())
}
fn append(db: &Connection, meeting: &str, e: &Event) -> Result<()> {
    db.execute("INSERT INTO app_bleed_decisions(id,lineage_id,meeting_id,source_run_id,source_segment_id,track,start_ms,end_ms,original_text_hash,decision,reason,retired,created_at) VALUES(?1,?2,?3,?4,?5,'mic',?6,?7,?8,?9,?10,?11,?12)",params![e.id,e.lineage,meeting,e.source_run,e.source_segment,e.start,e.end,e.hash,e.decision,e.reason,e.retired,chrono::Utc::now().to_rfc3339()])?;
    Ok(())
}
fn snapshot(
    db: &Connection,
    run: &str,
    e: &Event,
    segment: Option<&str>,
    status: &str,
    reason: &str,
) -> Result<()> {
    db.execute("INSERT INTO app_bleed_run_matches(run_id,decision_id,segment_id,status,reason) VALUES(?1,?2,?3,?4,?5)",params![run,e.id,segment,status,reason])?;
    Ok(())
}
pub(crate) fn set_decision(
    root: &Path,
    meeting: &str,
    run: &str,
    segment: &str,
    decision: &str,
) -> Result<()> {
    if !matches!(decision, "bleed" | "genuine") {
        bail!("Decision must be bleed or genuine.");
    }
    super::write(root, |tx| {
        editable(tx, meeting, run)?;
        let (track, start, end, text): (String, i64, i64, String) = tx.query_row(
            "SELECT track,start_ms,end_ms,text FROM app_run_segments WHERE run_id=?1 AND id=?2",
            params![run, segment],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )?;
        if track != "mic" {
            bail!("Only microphone transcript lines can be marked as bleed.");
        }
        let hash = text_hash(&text);
        let current:Option<String>=tx.query_row("SELECT d.lineage_id FROM app_bleed_run_matches m JOIN app_bleed_decisions d ON d.id=m.decision_id WHERE m.run_id=?1 AND m.segment_id=?2 AND m.status='applied' ORDER BY d.seq DESC LIMIT 1",params![run,segment],|r|r.get(0)).optional()?;
        let lineage = if let Some(current) = current {
            current
        } else {
            let mut possible = Vec::new();
            for e in latest(tx, meeting)? {
                if !e.retired && e.start == start && e.end == end && e.hash == hash {
                    let used:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM app_bleed_run_matches m JOIN app_bleed_decisions d ON d.id=m.decision_id WHERE m.run_id=?1 AND d.lineage_id=?2 AND m.status='applied' AND m.segment_id<>?3)",params![run,e.lineage,segment],|r|r.get(0))?;
                    if !used {
                        possible.push(e.lineage);
                    }
                }
            }
            if possible.len() == 1 {
                possible.remove(0)
            } else {
                uuid::Uuid::new_v4().to_string()
            }
        };
        let e = Event {
            id: uuid::Uuid::new_v4().to_string(),
            lineage,
            source_run: run.into(),
            source_segment: segment.into(),
            start,
            end,
            hash,
            decision: decision.into(),
            reason: if decision == "bleed" {
                "User confirmed microphone speech as bleed."
            } else {
                "User kept microphone speech as genuine."
            }
            .into(),
            retired: false,
        };
        append(tx, meeting, &e)?;
        tx.execute("UPDATE app_bleed_run_matches SET status='superseded' WHERE run_id=?1 AND (segment_id=?2 OR decision_id IN(SELECT id FROM app_bleed_decisions WHERE lineage_id=?3))",params![run,segment,e.lineage])?;
        snapshot(tx, run, &e, Some(segment), "applied", &e.reason)?;
        bump_revision(tx, meeting)?;
        refresh_search_after_edit(tx, meeting)?;
        Ok(())
    })
}
/// Retire an unmatched decision without inventing a correspondence to current speech.
pub(crate) fn dismiss_pending(
    root: &Path,
    meeting: &str,
    run: &str,
    decision_id: &str,
) -> Result<()> {
    super::write(root, |tx| {
        editable(tx, meeting, run)?;
        let pending:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM app_bleed_run_matches WHERE run_id=?1 AND decision_id=?2 AND status='needs_review')",params![run,decision_id],|r|r.get(0))?;
        if !pending {
            bail!("This bleed review is no longer pending.");
        }
        let Some(mut e) = latest(tx, meeting)?
            .into_iter()
            .find(|e| e.id == decision_id)
        else {
            bail!("This bleed decision has been superseded.");
        };
        e.id = uuid::Uuid::new_v4().to_string();
        e.decision = "genuine".into();
        e.reason = "User dismissed the unmatched bleed decision and kept speech visible.".into();
        e.retired = true;
        append(tx, meeting, &e)?;
        tx.execute(
        "UPDATE app_bleed_run_matches SET status='superseded' WHERE run_id=?1 AND decision_id=?2",
        params![run, decision_id],
    )?;
        snapshot(tx, run, &e, None, "dismissed", &e.reason)?;
        bump_revision(tx, meeting)?;
        refresh_search_after_edit(tx, meeting)?;
        Ok(())
    })
}
/// Invoke inside the publication transaction, while this run is still running.
pub(crate) fn reapply(db: &Connection, meeting: &str, run: &str) -> Result<()> {
    ensure_run(db, meeting, run)?;
    let running: bool = db.query_row(
        "SELECT status='running' FROM app_processing_runs WHERE id=?1",
        [run],
        |r| r.get(0),
    )?;
    if !running {
        bail!("Only a running transcript can receive a new bleed match snapshot.");
    }
    let mut index: BTreeMap<(i64, i64, String), Vec<String>> = BTreeMap::new();
    for s in read_segments(db, run, false)? {
        if s.track == "mic" {
            index
                .entry((s.start_ms as i64, s.end_ms as i64, text_hash(&s.text)))
                .or_default()
                .push(s.id);
        }
    }
    let events: Vec<_> = latest(db, meeting)?
        .into_iter()
        .filter(|e| !e.retired)
        .collect();
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for e in &events {
        if let Some(ids) = index.get(&(e.start, e.end, e.hash.clone()))
            && ids.len() == 1
        {
            *counts.entry(ids[0].clone()).or_default() += 1;
        }
    }
    db.execute("DELETE FROM app_bleed_run_matches WHERE run_id=?1", [run])?;
    for e in events {
        let ids = index.get(&(e.start, e.end, e.hash.clone()));
        if let Some(ids) = ids.filter(|ids| ids.len() == 1 && counts.get(&ids[0]) == Some(&1)) {
            snapshot(db, run, &e, Some(&ids[0]), "applied", &e.reason)?;
        } else if e.decision == "bleed" {
            snapshot(
                db,
                run,
                &e,
                None,
                "needs_review",
                "Original microphone speech has no unique exact match; speech remains visible.",
            )?;
        }
    }
    Ok(())
}
pub(crate) fn hidden_ids(db: &Connection, run: &str) -> Result<BTreeSet<String>> {
    let mut q=db.prepare("SELECT m.segment_id FROM app_bleed_run_matches m JOIN app_bleed_decisions d ON d.id=m.decision_id WHERE m.run_id=?1 AND m.status='applied' AND d.decision='bleed' AND m.segment_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM app_bleed_run_matches n JOIN app_bleed_decisions e ON e.id=n.decision_id WHERE n.run_id=m.run_id AND n.segment_id=m.segment_id AND n.status='applied' AND e.seq>d.seq)")?;
    Ok(q.query_map([run], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?)
}
pub(crate) fn details(
    db: &Connection,
    meeting: &str,
    run: &str,
    segments: &[Segment],
) -> Result<BleedDetails> {
    ensure_run(db, meeting, run)?;
    let mut q=db.prepare("SELECT d.id,m.segment_id,d.decision,m.reason,m.status FROM app_bleed_run_matches m JOIN app_bleed_decisions d ON d.id=m.decision_id WHERE m.run_id=?1 AND m.status<>'superseded' ORDER BY d.seq")?;
    let decisions: Vec<BleedDecision> = q
        .query_map([run], |r| {
            Ok(BleedDecision {
                id: r.get(0)?,
                segment_id: r.get(1)?,
                decision: r.get(2)?,
                reason: r.get(3)?,
                status: r.get(4)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    let excluded = decisions
        .iter()
        .filter(|d| d.status == "applied")
        .filter_map(|d| d.segment_id.clone())
        .collect();
    Ok(BleedDetails {
        decisions,
        candidates: candidates(segments, &excluded).0,
    })
}
fn tokens(text: &str) -> Vec<String> {
    let normalized: String = text
        .chars()
        .flat_map(char::to_lowercase)
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect();
    normalized.split_whitespace().map(str::to_owned).collect()
}
fn similarity(a: &[String], b: &[String]) -> f64 {
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, x) in a.iter().enumerate() {
        let mut prior = row[0];
        row[0] = i + 1;
        for (j, y) in b.iter().enumerate() {
            let old = row[j + 1];
            row[j + 1] = (prior + usize::from(x != y)).min(row[j] + 1).min(old + 1);
            prior = old;
        }
    }
    1.0 - row[b.len()] as f64 / a.len().max(b.len()) as f64
}
fn candidates(segments: &[Segment], excluded: &BTreeSet<String>) -> (Vec<BleedCandidate>, usize) {
    let mut system: Vec<_> = segments
        .iter()
        .filter(|s| s.track == "system")
        .filter_map(|s| {
            let t = tokens(&s.text);
            (!t.is_empty() && t.len() <= MAX_TOKENS).then_some((s, t))
        })
        .collect();
    system.sort_by(|a, b| (a.0.start_ms, &a.0.id).cmp(&(b.0.start_ms, &b.0.id)));
    let mut out = Vec::new();
    let mut checks = 0;
    for mic in segments
        .iter()
        .filter(|s| s.track == "mic" && !excluded.contains(&s.id))
    {
        let mt = tokens(&mic.text);
        if mt.len() < 4 || mt.len() > MAX_TOKENS {
            continue;
        }
        let pivot = system.partition_point(|s| s.0.start_ms < mic.start_ms);
        let (mut left, mut right) = (pivot, pivot);
        let mut best: Option<(f64, u64, u64, &str)> = None;
        for _ in 0..MAX_SYSTEM_CANDIDATES {
            let l = left
                .checked_sub(1)
                .map(|i| (i, system[i].0.start_ms.abs_diff(mic.start_ms)));
            let r = (right < system.len())
                .then(|| (right, system[right].0.start_ms.abs_diff(mic.start_ms)));
            let next = match (l, r) {
                (Some(l), Some(r)) => {
                    if (l.1, system[l.0].0.start_ms, &system[l.0].0.id)
                        <= (r.1, system[r.0].0.start_ms, &system[r.0].0.id)
                    {
                        Some(l)
                    } else {
                        Some(r)
                    }
                }
                (l, None) => l,
                (None, r) => r,
            };
            let Some((i, distance)) = next else {
                break;
            };
            if distance > WINDOW_MS {
                break;
            }
            if i < left {
                left = i;
            } else {
                right = i + 1;
            }
            checks += 1;
            let (s, st) = &system[i];
            if mt.len().abs_diff(st.len()) * 5 > mt.len().max(st.len()) {
                continue;
            }
            let score = similarity(&mt, st);
            if score < 0.8 {
                continue;
            }
            let better = best.as_ref().is_none_or(|b| {
                score > b.0
                    || (score == b.0 && (distance, s.start_ms, s.id.as_str()) < (b.1, b.2, b.3))
            });
            if better {
                best = Some((score, distance, s.start_ms, &s.id));
            }
        }
        if let Some((similarity, _, _, id)) = best {
            out.push(BleedCandidate {
                mic_segment_id: mic.id.clone(),
                system_segment_id: id.into(),
                similarity,
            });
        }
    }
    (out, checks)
}
#[cfg(test)]
#[path = "bleed_store_tests.rs"]
mod tests;
