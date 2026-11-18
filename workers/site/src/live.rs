//! The live-row check (K11; `decisions/DECISIONS.md`, "`live-check`"): a voter rehashes every row of each tier as of
//! one index and compares the result with the sum `apply` kept. The fold audit compares digests `apply` computed and
//! refolds from snapshots, so a row changed in storage after `apply` wrote it is caught by neither; this check reads
//! the member's own rows.
//!
//! It is a snapshot job with no upload (`SnapshotJob::live_step`): `begin` marks the tier's `to`, `apply` goes on
//! through `snap_undo`, the alarm reads `ROWS_PER_READ` rows at a time for at most `BATCH_MS` and yields between
//! reads, and `end_snapshot` drops the copies. The fold audit's cron starts it (`/fold {"live": true}` to each voter it
//! visits), at most once per `LIVE_EVERY`, and never while the member leads with commands queued, a snapshot job or a
//! refold or install runs, a split parent slims, or its metadata tier is not loaded. A real snapshot that comes due
//! while it runs takes the tier, and the check ends as `yielded`. Past `LIVE_UNDO_MAX` copies it ends as `abandoned`,
//! and is tried at the next due visit. A mismatch logs "fold live-row mismatch" with the tier; the repair is the
//! operator's (`ReplaceMember`), not automatic.

use std::rc::Rc;

use graftyard_fold::{LiveStep, LiveVerdict, SnapshotJob, TierStore, live_abandons, meta};
use graftyard_raft as raft;
use graftyard_types::Tier;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use worker::*;

use crate::family::Repo;
use crate::host::{Host, rows, text};
use crate::now_ms;

/// How old the last check is before a cron visit starts another [guess].
pub const LIVE_EVERY_MS: u64 = 7 * 86_400_000;
/// Rows one read takes, as the snapshot writer's.
const ROWS_PER_READ: usize = 500;
/// An alarm's reading stops after this long, as the snapshot writer's.
const BATCH_MS: u64 = 2_000;

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Run {
    /// 0 core, 1 metadata: the tier being read.
    tier: u8,
    cursor: Option<graftyard_fold::SnapshotCursor>,
    began_ms: u64,
    batches: u32,
    undo_max: u64,
    done: Vec<LiveVerdict>,
}

/// What a voter knows of the member's state, for `may_start`.
#[derive(Clone, Copy, Debug, Default)]
pub struct Busy {
    pub running: bool,
    pub last_ms: Option<u64>,
    /// A snapshot job, or a running snapshot mark, on either tier.
    pub snapshot: bool,
    /// A refold or install job (`crate::load`).
    pub load: bool,
    /// Leader holding commands it has not resolved.
    pub leads_queued: bool,
    pub slimming: bool,
    pub meta_loaded: bool,
}

/// Whether a cron visit starts a check now (`decisions/DECISIONS.md`, "`live-check`", no added latency).
pub fn may_start(b: Busy, now: u64) -> bool {
    !b.running
        && b.last_ms.is_none_or(|l| now >= l + LIVE_EVERY_MS)
        && !b.snapshot
        && !b.load
        && !b.leads_queued
        && !b.slimming
        && b.meta_loaded
}

fn tier_of(n: u8) -> Tier {
    if n == 0 { Tier::Core } else { Tier::Metadata }
}

fn schema(t: Tier) -> graftyard_fold::Schema {
    match t {
        Tier::Core => graftyard_git::tiers::CORE,
        Tier::Metadata => graftyard_git::tiers::METADATA,
    }
}

fn load_run(sql: &SqlStorage) -> Result<Option<Run>> {
    match rows(sql, "SELECT row FROM live_run WHERE id = 1", vec![])?.first() {
        Some(r) => Ok(Some(serde_json::from_str(text(&r[0])?)?)),
        None => Ok(None),
    }
}

fn save_run(sql: &SqlStorage, r: &Run) -> Result<()> {
    sql.exec("INSERT OR REPLACE INTO live_run (id, row) VALUES (1, ?)", vec![serde_json::to_string(r)?.into()])?;
    Ok(())
}

fn last(sql: &SqlStorage) -> Result<Option<Value>> {
    match rows(sql, "SELECT row FROM live_last WHERE id = 1", vec![])?.first() {
        Some(r) => Ok(Some(serde_json::from_str(text(&r[0])?)?)),
        None => Ok(None),
    }
}

fn store(h: &Rc<Host<Repo>>, t: Tier) -> Option<Box<dyn TierStore>> {
    let core = h.core.borrow();
    let f = &core.as_ref()?.m.group_fold().fold;
    let s = if t == Tier::Core { f.core.clone() } else { f.meta.clone() };
    s.loaded().then_some(s)
}

/// Runs the tier's buffered writes (the snapshot mark, or `end_snapshot`'s) at once, outside any Raft input, as the
/// snapshot writer does.
fn seal(sql: &SqlStorage, s: &dyn TierStore) -> Result<()> {
    let mut stmts = Vec::new();
    crate::tier::handle(s).seal(&mut stmts);
    for (q, a) in stmts {
        sql.exec(q, a)?;
    }
    crate::tier::handle(s).flushed();
    Ok(())
}

/// Ends the check: drops the running tier's mark and copies when it holds them, and records `status`.
fn finish(h: &Rc<Host<Repo>>, sql: &SqlStorage, run: &Run, status: &str) -> Result<()> {
    if let (Some(c), Some(s)) = (&run.cursor, store(h, tier_of(run.tier))) {
        let job = SnapshotJob::resume(schema(tier_of(run.tier)), c.clone());
        if job.holds(&*s) {
            let mut st = s.clone();
            graftyard_fold::end_snapshot(&mut st, &schema(tier_of(run.tier)));
            seal(sql, &*st)?;
        }
    }
    let tiers: Vec<Value> = run
        .done
        .iter()
        .map(|v| json!({ "tier": v.tier, "to": v.to.0, "want": v.want.hex(), "got": v.got.hex(), "rows": v.rows, "matched": v.matched() }))
        .collect();
    let row = json!({ "at": now_ms(), "status": status, "tiers": tiers, "beganMs": run.began_ms, "batches": run.batches, "undoMax": run.undo_max });
    sql.exec("INSERT OR REPLACE INTO live_last (id, row) VALUES (1, ?)", vec![row.to_string().into()])?;
    sql.exec("DELETE FROM live_run", None)?;
    Ok(())
}

/// The member's state for `may_start`.
fn busy(h: &Rc<Host<Repo>>, sql: &SqlStorage) -> Result<Busy> {
    let snap = !rows(sql, "SELECT tier FROM snap_run LIMIT 1", vec![])?.is_empty();
    let load = !rows(sql, "SELECT id FROM load_job WHERE id = 1", vec![])?.is_empty();
    let core = h.core.borrow();
    let Some(c) = core.as_ref() else { return Ok(Busy { running: true, ..Busy::default() }) };
    let f = &c.m.group_fold().fold;
    let marked = meta(&*f.core).snap.is_some() || (f.meta.loaded() && meta(&*f.meta).snap.is_some());
    Ok(Busy {
        running: load_run(sql)?.is_some(),
        last_ms: last(sql)?.and_then(|v| v["at"].as_u64()),
        snapshot: snap || marked,
        load,
        leads_queued: c.m.role() == raft::Role::Leader && c.m.admitted_bytes() > 0,
        slimming: f.meta.loaded() && crate::split::slimming(f),
        meta_loaded: f.meta.loaded() && !f.child && !crate::load::meta_dropped(sql),
    })
}

/// `/fold {"live": true}` from the cron: the last check's result, and whether one starts now.
pub fn route(h: &Rc<Host<Repo>>) -> Result<Value> {
    let sql = h.state.storage().sql();
    let b = busy(h, &sql)?;
    let start = may_start(b, now_ms());
    if start {
        save_run(&sql, &Run { tier: 0, cursor: None, began_ms: now_ms(), batches: 0, undo_max: 0, done: Vec::new() })?;
    }
    // A check runs on the member's alarm; a visit wakes one that a reset left without one.
    if start || b.running {
        h.arm_in(0);
    }
    Ok(json!({ "last": last(&sql)?, "running": load_run(&sql)?.is_some(), "started": start }))
}

/// Called by the snapshot writer before it begins a due snapshot of `t`: a check on that tier gives it up.
pub fn yield_to_snapshot(h: &Rc<Host<Repo>>, sql: &SqlStorage, t: Tier) -> Result<()> {
    if let Some(run) = load_run(sql)?
        && tier_of(run.tier) == t
        && run.cursor.is_some()
    {
        finish(h, sql, &run, "yielded")?;
    }
    Ok(())
}

/// One alarm's worth of the running check. Whether more work is waiting.
pub async fn on_alarm(h: &Rc<Host<Repo>>) -> bool {
    match step(h).await {
        Ok(b) => b,
        Err(e) => {
            console_error!("fold live check: {e}");
            false
        }
    }
}

async fn step(h: &Rc<Host<Repo>>) -> Result<bool> {
    let sql = h.state.storage().sql();
    let Some(mut run) = load_run(&sql)? else { return Ok(false) };
    h.settled().await;
    let t = tier_of(run.tier);
    let Some(s) = store(h, t) else {
        finish(h, &sql, &run, "gone")?;
        return Ok(false);
    };
    // A slim pass that began during the check would copy each row it rewrites to `snap_undo`, as it would for a
    // snapshot, which stops for it (`crate::split::slimming`): the check stops too.
    let slimming = h.core.borrow().as_ref().is_some_and(|c| crate::split::slimming(&c.m.group_fold().fold));
    if t == Tier::Metadata && slimming {
        finish(h, &sql, &run, "yielded")?;
        return Ok(false);
    }
    let mut job = match &run.cursor {
        Some(c) => SnapshotJob::resume(schema(t), c.clone()),
        None => {
            // A snapshot job took the tier since the visit started this check: leave it be.
            if meta(&*s).snap.is_some() {
                finish(h, &sql, &run, "yielded")?;
                return Ok(false);
            }
            let chain = h.core.borrow().as_ref().map(|c| c.join.chain);
            let Some(chain) = chain else { return Ok(false) };
            let mut st = s.clone();
            let job = SnapshotJob::begin(
                &mut st,
                schema(t),
                chain,
                crate::host::RELEASE.id,
                graftyard_git::repo::REPO_FORMAT_V1,
            )
            .map_err(|e| Error::RustError(e.to_string()))?;
            // The cursor and the mark go in one turn with no await, so together; the cursor first in any case, so a
            // lost mark ends the check as `gone` and never leaves a mark no run owns to hold off the snapshot writer.
            run.cursor = Some(job.cursor().clone());
            save_run(&sql, &run)?;
            seal(&sql, &*st)?;
            return Ok(true);
        }
    };
    let undo = crate::tier::handle(&*s).undo_rows(&schema(t));
    run.undo_max = run.undo_max.max(undo);
    if live_abandons(undo) {
        finish(h, &sql, &run, "abandoned")?;
        return Ok(false);
    }
    let t0 = now_ms();
    let mut verdict = None;
    while now_ms() - t0 < BATCH_MS {
        match job.live_step(&*s, ROWS_PER_READ) {
            LiveStep::More => {}
            LiveStep::Done(v) => {
                verdict = Some(v);
                break;
            }
            LiveStep::Gone => {
                run.cursor = None;
                finish(h, &sql, &run, "gone")?;
                return Ok(false);
            }
        }
        // Raft inputs run between reads; `snap_undo` keeps every read at `to`.
        Delay::from(std::time::Duration::ZERO).await;
    }
    // The writes below run outside any input's transaction, so only once none is deferred, with no await after.
    h.settled().await;
    // An install that landed during the reads replaced the tier: no verdict, and no `end_snapshot` on the new one.
    if !job.holds(&*s) {
        run.cursor = None;
        finish(h, &sql, &run, "gone")?;
        return Ok(false);
    }
    // A read storage refused answered no rows: no verdict on what was not read.
    if h.fold_lost() {
        return Ok(false);
    }
    run.batches += 1;
    run.cursor = Some(job.cursor().clone());
    let Some(v) = verdict else {
        save_run(&sql, &run)?;
        return Ok(true);
    };
    if !v.matched() {
        console_error!("fold live-row mismatch on {:?}: {v:?}", h.core.borrow().as_ref().map(|c| c.join.chain));
    }
    let mut st = s.clone();
    graftyard_fold::end_snapshot(&mut st, &schema(t));
    seal(&sql, &*st)?;
    run.done.push(v);
    run.cursor = None;
    if run.tier == 0 {
        run.tier = 1;
        save_run(&sql, &run)?;
        return Ok(true);
    }
    finish(h, &sql, &run, "done")?;
    Ok(false)
}

/// The tiers whose last check on a voter did not match, from its `/fold {"live": true}` answer's `live` field.
pub fn mismatches(answer: &Value) -> Vec<Value> {
    answer["last"]["tiers"]
        .as_array()
        .map(|ts| ts.iter().filter(|t| t["matched"] == json!(false)).cloned().collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{Busy, LIVE_EVERY_MS, may_start, mismatches};

    /// `decisions/DECISIONS.md`, "`live-check`", no added latency: a visit starts a check only on a quiet member whose
    /// last one is `LIVE_EVERY` old, and never while it leads with commands queued, a snapshot or refold runs, a split
    /// parent slims, or its metadata tier is not loaded.
    #[test]
    fn a_check_starts_only_on_a_quiet_member_a_week_after_the_last() {
        let quiet = Busy { meta_loaded: true, ..Busy::default() };
        let now = 10 * LIVE_EVERY_MS;
        assert!(may_start(quiet, now), "never checked");
        assert!(may_start(Busy { last_ms: Some(now - LIVE_EVERY_MS), ..quiet }, now), "a week old");
        assert!(!may_start(Busy { last_ms: Some(now - LIVE_EVERY_MS + 1), ..quiet }, now), "a day short of a week");
        for (why, b) in [
            ("one is running", Busy { running: true, ..quiet }),
            ("a snapshot runs", Busy { snapshot: true, ..quiet }),
            ("a refold or install runs", Busy { load: true, ..quiet }),
            ("the leader holds queued commands", Busy { leads_queued: true, ..quiet }),
            ("a slim pass runs", Busy { slimming: true, ..quiet }),
            ("the metadata tier is not loaded", Busy { meta_loaded: false, ..quiet }),
        ] {
            assert!(!may_start(b, now), "{why}");
        }
    }

    #[test]
    fn the_cron_reads_a_voters_mismatching_tiers_from_its_last_check() {
        let a = json!({ "last": { "tiers": [
            { "tier": "core", "matched": true }, { "tier": "metadata", "matched": false, "to": 6 } ] } });
        assert_eq!(mismatches(&a), vec![json!({ "tier": "metadata", "matched": false, "to": 6 })]);
        assert!(mismatches(&json!({ "last": null })).is_empty());
    }
}
