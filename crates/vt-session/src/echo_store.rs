use super::*;
use anyhow::Context;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use vt_capture::OutputRoute;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct EchoInfo {
    pub enabled: bool,
    pub applied_mic_ms: u64,
    pub bypassed_mic_ms: u64,
    pub engine: String,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteObservation {
    pub epoch: u32,
    pub start_ns: u64,
    pub route: OutputRoute,
}

#[cfg(test)]
#[path = "echo_store_tests.rs"]
mod tests;

pub(super) fn record_route(root: &Path, id: &str, value: &RouteObservation) -> Result<()> {
    let start = i64::try_from(value.start_ns)?;
    let mut db = store::open(root)?;
    let tx = db.transaction()?;
    let live: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM app_meeting_revision v JOIN app_processing_runs r ON r.id=v.active_run_id WHERE v.meeting_id=?1 AND r.kind='live' AND r.status='running')",
        [id], |r|r.get(0),
    )?;
    if !live {
        bail!("Route observations require the current running recording.");
    }
    let json = serde_json::to_string(&value.route)?;
    let previous: Option<String> = tx.query_row(
        "SELECT route_json FROM app_output_routes WHERE meeting_id=?1 AND start_ns=?2 AND epoch=?3",
        params![id,start,value.epoch],|r|r.get(0),
    ).optional()?;
    match previous {
        Some(previous) if previous != json => {
            bail!("A capture route observation cannot be replaced.")
        }
        Some(_) => (),
        None => {
            tx.execute(
                "INSERT INTO app_output_routes(meeting_id,epoch,start_ns,route_json) VALUES (?1,?2,?3,?4)",
                params![id,value.epoch,start,json],
            )?;
        }
    }
    tx.commit()?;
    Ok(())
}

pub(super) fn route_at(
    db: &Connection,
    id: &str,
    start_ms: u64,
) -> Result<Option<RouteObservation>> {
    let start = i64::try_from(
        start_ms
            .checked_mul(1_000_000)
            .context("Route time overflow")?,
    )?;
    let row = db.query_row(
        "SELECT epoch,start_ns,route_json FROM app_output_routes WHERE meeting_id=?1 AND start_ns<=?2 ORDER BY start_ns DESC,epoch DESC LIMIT 1",
        params![id,start], |r|Ok((r.get::<_,u32>(0)?,r.get::<_,i64>(1)?,r.get::<_,String>(2)?)),
    ).optional()?;
    row.map(|(epoch, start_ns, json)| {
        Ok(RouteObservation {
            epoch,
            start_ns: u64::try_from(start_ns)?,
            route: serde_json::from_str(&json)?,
        })
    })
    .transpose()
}

pub(super) fn seal_routes(root: &Path, id: &str) -> Result<()> {
    let db = store::open(root)?;
    let running:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM app_meeting_revision v JOIN app_processing_runs r ON r.id=v.active_run_id WHERE v.meeting_id=?1 AND r.kind='live' AND r.status='running')",[id],|r|r.get(0))?;
    if !running {
        bail!("Only a running recording can seal its captured route history.");
    }
    db.execute("INSERT INTO app_route_evidence(meeting_id,complete) VALUES (?1,1) ON CONFLICT(meeting_id) DO UPDATE SET complete=1",[id])?;
    Ok(())
}
pub(super) fn routes_complete(db: &Connection, id: &str) -> Result<bool> {
    Ok(db
        .query_row(
            "SELECT complete FROM app_route_evidence WHERE meeting_id=?1",
            [id],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or(false))
}

pub(super) fn info(db: &Connection, run: &str) -> Result<Option<EchoInfo>> {
    let value: Option<String> = db
        .query_row(
            "SELECT info_json FROM app_run_echo_processing WHERE run_id=?1",
            [run],
            |r| r.get(0),
        )
        .optional()?;
    value
        .map(|json| serde_json::from_str(&json).map_err(Into::into))
        .transpose()
}

pub(super) fn save_info(root: &Path, id: &str, run: &str, info: &EchoInfo) -> Result<()> {
    let db = store::open(root)?;
    write_info(&db, id, run, info)
}
pub(super) fn save_info_with(
    root: &Path,
    id: &str,
    run: &str,
    info: &EchoInfo,
    busy_timeout: std::time::Duration,
) -> Result<()> {
    let db = store::open_with_timeout(root, busy_timeout)?;
    write_info(&db, id, run, info)
}

pub(super) fn write_info(db: &Connection, id: &str, run: &str, info: &EchoInfo) -> Result<()> {
    let belongs: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM app_processing_runs WHERE id=?1 AND meeting_id=?2 AND status='running')",
        params![run,id],|r|r.get(0),
    )?;
    if !belongs {
        bail!("Echo processing requires a running run in this meeting.");
    }
    db.execute(
        "INSERT INTO app_run_echo_processing(run_id,info_json) VALUES (?1,?2) ON CONFLICT(run_id) DO UPDATE SET info_json=excluded.info_json",
        params![run,serde_json::to_string(info)?],
    )?;
    Ok(())
}
