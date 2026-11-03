//! A repository member's fold work outside `apply` (`design/DESIGN.md`, Shape, "Packs", the snapshot block;
//! `design/SHARED.md`, "Folds"). It all runs on the member's alarm, so none of it waits on or holds a client request:
//!
//! - **Metadata load.** A member whose metadata tier is behind its core tier, such as a new member or a restored
//!   core tier, applies the stored log to that tier in batches of `META_BATCH` entries per alarm. It then marks the
//!   tier loaded, and from there `apply` writes it in the same transaction as the core tier. Core requests never wait
//!   for the load. A command that needs the metadata tier while it loads is refused by `RepoScratch`.
//! - **Snapshots.** The member `snapshot_writer` names for the next `to` begins a `SnapshotJob` once `snapshot_due`
//!   holds for a tier. It writes the job as frames in alarm-driven batches of `ROWS_PER_ALARM` rows while `apply`
//!   goes on through `snap_undo`. Frames go into a part buffer. Each full `PART_BYTES` part becomes one part of a
//!   multipart upload under `R2Key::Snapshot` in two buckets: its own site's, and the next site's. The key names the
//!   digest, so the job takes it at `begin` from the sum `apply` kept (`SnapshotJob::want`). After the trailer both
//!   uploads complete, and the writer asks the leader to commit `Archived` with the key (`/snapshot_commit`). The
//!   leader HEADs both copies first. When the snapshot's row applies, the writer runs `end_snapshot`.
//! - **Retention.** Applying a snapshot's `Archived` drops the `snapshot` rows that `snapshots_to_keep` no longer
//!   keeps (`graftyard_git::tiers`). The writer then deletes those snapshots' objects from both buckets.
//!
//! The job's cursor, its uploads' ids and parts, and the unsent part buffer are kept in `snap_job`, written in the
//! same alarm as the part they follow, so a reset object resumes at its last stored part.

use std::rc::Rc;

use graftyard_fold::{
    SNAPSHOT_BYTES, Schema, SnapshotCursor, SnapshotFrame, SnapshotJob, TierStore, digest_at, encode_frame,
    fold_digest, frames_header, meta, snapshot_due, snapshot_writer,
};
use graftyard_git as git;
use graftyard_raft::{self as raft, GroupConfig, Index};
use graftyard_types::{Digest, Placement, SiteId, Tier, Timestamp};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use worker::*;

use crate::family::Repo;
use crate::host::{Host, blob, i, rows};
use crate::{SiteEnv, now_ms};

/// Rows a snapshot batch reads per alarm [guess]: well inside one request's CPU and memory with 2 KB rows.
const ROWS_PER_ALARM: usize = 4_000;
/// A multipart part. R2 wants every part but the last the same size, and at least 5 MiB.
const PART_BYTES: usize = 8 << 20;
/// Entries a metadata load applies per alarm [guess].
const META_BATCH: u64 = 500;
/// Alarm delay while a job or load is running.
const BUSY_MS: i64 = 20;
/// Alarm delay, after a member applies entries, to check whether a snapshot is due.
pub const IDLE_MS: i64 = 30_000;

pub const SCHEMA: &[&str] = &[
    // The running snapshot job of each tier: its state, and the bytes not yet in a part.
    "CREATE TABLE IF NOT EXISTS snap_job (tier INTEGER PRIMARY KEY, row BLOB NOT NULL, buf BLOB NOT NULL)",
    // A snapshot job's record, per (tier, to): its key, times, and batch sizes, read by BF-20.
    "CREATE TABLE IF NOT EXISTS snap_log (tier INTEGER NOT NULL, to_idx INTEGER NOT NULL, row TEXT NOT NULL, PRIMARY KEY (tier, to_idx))",
    // Snapshot objects this writer put, so retention deletes the ones whose rows the fold dropped.
    "CREATE TABLE IF NOT EXISTS snap_put (key TEXT PRIMARY KEY, tier INTEGER NOT NULL, to_idx INTEGER NOT NULL, sites TEXT NOT NULL)",
];

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Upload {
    site: u8,
    id: String,
    parts: Vec<(u16, String)>,
}

/// A running snapshot, as `snap_job` stores it.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Job {
    cursor: SnapshotCursor,
    to: u64,
    to_at: u64,
    key: String,
    digest: Digest,
    uploads: Vec<Upload>,
    /// The trailer is in the buffer: complete once the buffer is uploaded.
    ended: bool,
    /// Both uploads completed; waiting for `Archived` to commit.
    completed: bool,
    began_ms: u64,
    batches: u32,
    /// The longest alarm batch, in ms, and the sum, for BF-20.
    batch_max_ms: u64,
    batch_sum_ms: u64,
    rows: u64,
    bytes: u64,
}

fn tier_int(t: Tier) -> u64 {
    match t {
        Tier::Core => 0,
        Tier::Metadata => 1,
    }
}

fn schema(t: Tier) -> Schema {
    match t {
        Tier::Core => git::tiers::CORE,
        Tier::Metadata => git::tiers::METADATA,
    }
}

fn load_job(sql: &SqlStorage, t: Tier) -> Result<Option<(Job, Vec<u8>)>> {
    let r = rows(sql, "SELECT row, buf FROM snap_job WHERE tier = ?", vec![i(tier_int(t))])?;
    let Some(r) = r.first() else { return Ok(None) };
    let job: Job = serde_json::from_slice(blob(&r[0])?)?;
    Ok(Some((job, blob(&r[1])?.to_vec())))
}

fn save_job(sql: &SqlStorage, t: Tier, job: &Job, buf: &[u8]) -> Result<()> {
    sql.exec(
        "INSERT OR REPLACE INTO snap_job (tier, row, buf) VALUES (?, ?, ?)",
        vec![i(tier_int(t)), serde_json::to_vec(job)?.into(), buf.to_vec().into()],
    )?;
    Ok(())
}

fn log_job(sql: &SqlStorage, t: Tier, job: &Job, status: &str) -> Result<()> {
    let v = json!({
        "tier": tier_int(t), "to": job.to, "toAt": job.to_at, "key": job.key, "digest": job.digest.hex(),
        "status": status, "beganMs": job.began_ms, "atMs": now_ms(), "batches": job.batches,
        "batchMaxMs": job.batch_max_ms, "batchSumMs": job.batch_sum_ms, "rows": job.rows, "bytes": job.bytes,
    });
    sql.exec(
        "INSERT OR REPLACE INTO snap_log (tier, to_idx, row) VALUES (?, ?, ?)",
        vec![i(tier_int(t)), i(job.to), v.to_string().into()],
    )?;
    Ok(())
}

/// The two buckets a snapshot goes to: this site's, and the next site's in the install's table.
fn two_sites(env: &Env) -> Result<[SiteId; 2]> {
    let site = SiteEnv::read(env)?;
    let ids: Vec<SiteId> = site.sites.0.iter().map(|s| s.id).collect();
    let me = ids.iter().position(|s| *s == site.id).ok_or_else(|| Error::RustError("own site".into()))?;
    Ok([site.id, ids[(me + 1) % ids.len()]])
}

fn bucket(env: &Env, s: u8) -> Result<Bucket> {
    env.bucket(&format!("BUCKET_{s}"))
}

/// The member's alarm work, before its Raft tick, and the next alarm it needs.
pub async fn on_alarm(h: &Rc<Host<Repo>>) {
    let sql = h.state.storage().sql();
    // The alarm writes tier rows outside any input's transaction, so it starts only once no deferred one is pending,
    // and does each write with no await between its read and its exec.
    h.settled().await;
    let mut busy = match load_meta(h, &sql) {
        Ok(b) => b,
        Err(e) => {
            alarm(h, format!("metadata load: {e}"));
            false
        }
    };
    for t in [Tier::Core, Tier::Metadata] {
        h.settled().await;
        match step(h, &sql, t).await {
            Ok(b) => busy |= b,
            Err(e) => alarm(h, format!("snapshot {t:?}: {e}")),
        }
    }
    // An idle member is woken again only by applying an entry (`Family::fold_alarm_ms`), so an idle group bills no
    // alarms.
    if busy && h.joined() {
        h.arm_in(BUSY_MS);
    }
}

fn alarm(h: &Rc<Host<Repo>>, m: String) {
    console_error!("{m}");
    let mut s = h.stats.borrow_mut();
    if s.alarms.len() < 50 {
        s.alarms.push(format!("{} {m}", now_ms()));
    }
}

/// Applies up to `META_BATCH` stored entries to a metadata tier that is behind the core tier, and marks it loaded
/// once it has caught up. The entries come from the `log` table, which holds every entry since the last archive.
/// Whether it is still loading.
fn load_meta(h: &Rc<Host<Repo>>, sql: &SqlStorage) -> Result<bool> {
    let core = h.core.borrow();
    let Some(c) = core.as_ref() else { return Ok(false) };
    let fold = &c.m.group_fold().fold;
    if fold.meta.loaded() {
        return Ok(false);
    }
    let target = meta(&*fold.core).applied;
    let mut m = crate::tier::handle(&*fold.meta).clone();
    let from = meta(&m).applied;
    if from >= target {
        m.set_loaded(true);
        return Ok(false);
    }
    let tiers = fold.tiers();
    drop(core);
    let q = "SELECT row FROM log WHERE idx > ? AND idx <= ? ORDER BY idx";
    for r in rows(sql, q, vec![i(from.0), i(target.0.min(from.0 + META_BATCH))])? {
        let e: raft::LogEntry<git::repo::RepoEvent> =
            raft::codec::from_bytes(blob(&r[0])?).map_err(|e| Error::RustError(e.to_string()))?;
        graftyard_fold::apply(&tiers, &mut m, Tier::Metadata, &e)
            .map_err(|e| Error::RustError(format!("metadata load at {:?}: {e}", e)))?;
    }
    // The load's own writes, outside any Raft input: they change nothing a peer or a client has seen.
    let mut stmts = Vec::new();
    m.seal(&mut stmts);
    for (q, a) in stmts {
        sql.exec(q, a)?;
    }
    m.flushed();
    Ok(true)
}

/// The tier's store, once loaded, and the core tier's, which holds every tier's `snapshot` rows.
fn tier_store(h: &Rc<Host<Repo>>, t: Tier) -> Option<(Box<dyn TierStore>, Box<dyn TierStore>)> {
    let core = h.core.borrow();
    let f = &core.as_ref()?.m.group_fold().fold;
    let s = match t {
        Tier::Core => f.core.clone(),
        Tier::Metadata => f.meta.clone(),
    };
    s.loaded().then(|| (s, f.core.clone()))
}

/// The placement of the group's latest config, for `snapshot_writer`.
fn placement(c: &GroupConfig) -> &Placement {
    match c {
        GroupConfig::Normal(p) | GroupConfig::Degraded { placement: p, .. } => p,
        GroupConfig::Joint { new, .. } => new,
    }
}

/// One alarm's worth of a tier's snapshot: begin one if due and this member writes it; read and upload the next
/// batch; complete it and ask the leader to commit; or end it once committed. Whether more work is waiting.
async fn step(h: &Rc<Host<Repo>>, sql: &SqlStorage, t: Tier) -> Result<bool> {
    let Some((store, core)) = tier_store(h, t) else { return Ok(false) };
    let Some((mut job, mut buf)) = load_job(sql, t)? else {
        return begin(h, sql, t, store, &*core);
    };
    if git::tiers::snapshots(&*core, t).iter().any(|(to, _, _)| to.0 == job.to) {
        let mut s = store.clone();
        graftyard_fold::end_snapshot(&mut s, &schema(t));
        let mut stmts = Vec::new();
        crate::tier::handle(&*s).seal(&mut stmts);
        for (q, a) in stmts {
            sql.exec(q, a)?;
        }
        log_job(sql, t, &job, "committed")?;
        sql.exec("DELETE FROM snap_job WHERE tier = ?", vec![i(tier_int(t))])?;
        retain(h, sql, t, &*core).await?;
        return Ok(true);
    }
    if job.completed {
        request_commit(h, t, &job).await?;
        return Ok(false);
    }
    if job.uploads.is_empty() {
        // The upload ids are stored before any part goes up, so a reset object resumes the same uploads.
        for s in two_sites(&h.env)? {
            let up = bucket(&h.env, s.0)?.create_multipart_upload(&job.key).execute().await?;
            job.uploads.push(Upload { site: s.0, id: up.upload_id().await, parts: Vec::new() });
        }
        let sites: Vec<u8> = job.uploads.iter().map(|u| u.site).collect();
        sql.exec(
            "INSERT OR REPLACE INTO snap_put (key, tier, to_idx, sites) VALUES (?, ?, ?, ?)",
            vec![job.key.clone().into(), i(tier_int(t)), i(job.to), serde_json::to_string(&sites)?.into()],
        )?;
        save_job(sql, t, &job, &buf)?;
        return Ok(true);
    }
    let t0 = now_ms();
    if !job.ended {
        let mut sj = SnapshotJob::resume(schema(t), job.cursor.clone());
        // One batch of rows per alarm, so each alarm's reads stay within one request's limits.
        let f = sj.next(&*store, ROWS_PER_ALARM).ok_or_else(|| Error::RustError("job past its trailer".into()))?;
        match &f {
            SnapshotFrame::Rows { rows, .. } => job.rows += rows.len() as u64,
            SnapshotFrame::End(tr) => {
                if tr.fold_digest != job.digest {
                    return Err(Error::RustError(format!(
                        "trailer digest {:?} is not the key's {:?}",
                        tr.fold_digest, job.digest
                    )));
                }
                job.ended = true;
            }
        }
        encode_frame(&f, &mut buf);
        job.cursor = sj.cursor().clone();
    }
    while buf.len() >= PART_BYTES || (job.ended && !buf.is_empty()) {
        let n = buf.len().min(PART_BYTES);
        let part: Vec<u8> = buf.drain(..n).collect();
        job.bytes += part.len() as u64;
        for u in &mut job.uploads {
            let up = bucket(&h.env, u.site)?.resume_multipart_upload(&job.key, &u.id)?;
            let p = up.upload_part(u.parts.len() as u16 + 1, part.clone()).await?;
            u.parts.push((p.part_number(), p.etag()));
        }
    }
    if job.ended && buf.is_empty() {
        for u in &job.uploads {
            let up = bucket(&h.env, u.site)?.resume_multipart_upload(&job.key, &u.id)?;
            up.complete(u.parts.iter().map(|(n, e)| UploadedPart::new(*n, e.clone()))).await?;
        }
        job.completed = true;
    }
    let ms = now_ms() - t0;
    job.batches += 1;
    job.batch_max_ms = job.batch_max_ms.max(ms);
    job.batch_sum_ms += ms;
    save_job(sql, t, &job, &buf)?;
    log_job(sql, t, &job, if job.completed { "uploaded" } else { "running" })?;
    if job.completed {
        request_commit(h, t, &job).await?;
    }
    Ok(true)
}

/// The tier's last committed snapshot, or `None`.
fn last_snapshot(store: &dyn TierStore, t: Tier) -> Option<(Index, Timestamp)> {
    git::tiers::snapshots(store, t).last().map(|(to, at, _)| (*to, *at))
}

/// Log bytes since the tier's last snapshot, from the `log` table's stored rows (an estimate: rows below the last
/// archive are gone, and they are older than any snapshot that matters here).
fn log_bytes_since(sql: &SqlStorage, from: Index) -> Result<u64> {
    let r = rows(sql, "SELECT COALESCE(SUM(length(row)), 0) FROM log WHERE idx > ?", vec![i(from.0)])?;
    Ok(r.first().map(|r| crate::host::int(&r[0]).unwrap_or(0) as u64).unwrap_or(0))
}

/// Starts a snapshot of the tier if one is due and this member writes it. Whether a job started.
fn begin(
    h: &Rc<Host<Repo>>,
    sql: &SqlStorage,
    t: Tier,
    store: Box<dyn TierStore>,
    core: &dyn TierStore,
) -> Result<bool> {
    let (chain, me, leader, place) = {
        let core = h.core.borrow();
        let Some(c) = core.as_ref() else { return Ok(false) };
        (c.join.chain, c.join.me.site, c.m.leader(), placement(c.m.latest_config()).clone())
    };
    let m = meta(&*store);
    let last = last_snapshot(core, t);
    let since = last.map_or(Index(0), |l| l.0);
    let forced = take_force(sql, t)?;
    let bytes = log_bytes_since(sql, since)?;
    if !forced && !snapshot_due(last, m.applied, m.at, bytes.min(SNAPSHOT_BYTES)) {
        return Ok(false);
    }
    // The writer is named by the index the snapshot would be at. A group with no leader known waits.
    let Some(leader) = leader else { return Ok(false) };
    if snapshot_writer(m.applied, &place, leader) != Some(me) && !forced {
        return Ok(false);
    }
    let mut s = store.clone();
    let job = SnapshotJob::begin(&mut s, schema(t), chain, crate::host::RELEASE.id, git::repo::REPO_FORMAT_V1)
        .map_err(|e| Error::RustError(e.to_string()))?;
    let digest = job.want(&*s);
    let mut stmts = Vec::new();
    crate::tier::handle(&*s).seal(&mut stmts);
    for (q, a) in stmts {
        sql.exec(q, a)?;
    }
    crate::tier::handle(&*s).flushed();
    let key = git::R2Key::Snapshot { chain, tier: t, from: Index(0), to: job.to(), digest };
    let j = Job {
        cursor: job.cursor().clone(),
        to: job.to().0,
        to_at: job.to_at().0,
        key: key.to_string(),
        digest,
        uploads: Vec::new(),
        ended: false,
        completed: false,
        began_ms: now_ms(),
        batches: 0,
        batch_max_ms: 0,
        batch_sum_ms: 0,
        rows: 0,
        bytes: 0,
    };
    save_job(sql, t, &j, &frames_header())?;
    log_job(sql, t, &j, "began")?;
    Ok(true)
}

/// Test installs set a one-shot `snap_force` row to start a snapshot now, whatever the cadence (BF-20).
#[cfg(feature = "test-faults")]
fn take_force(sql: &SqlStorage, t: Tier) -> Result<bool> {
    sql.exec("CREATE TABLE IF NOT EXISTS snap_force (tier INTEGER PRIMARY KEY)", None)?;
    let n = rows(sql, "SELECT tier FROM snap_force WHERE tier = ?", vec![i(tier_int(t))])?.len();
    if n > 0 {
        sql.exec("DELETE FROM snap_force WHERE tier = ?", vec![i(tier_int(t))])?;
    }
    Ok(n > 0)
}

#[cfg(not(feature = "test-faults"))]
fn take_force(_: &SqlStorage, _: Tier) -> Result<bool> {
    Ok(false)
}

/// Asks the leader to commit `Archived` with the snapshot's key, through the group lookup.
async fn request_commit(h: &Rc<Host<Repo>>, t: Tier, job: &Job) -> Result<()> {
    let (chain, record) = {
        let core = h.core.borrow();
        let Some(c) = core.as_ref() else { return Ok(()) };
        (c.join.chain, c.join.record.clone())
    };
    let body = json!({ "tier": tier_int(t), "to": job.to, "toAt": job.to_at, "key": job.key, "digest": job.digest.hex(),
        "sites": job.uploads.iter().map(|u| u.site).collect::<Vec<_>>() });
    let (v, _) =
        crate::lookup::call(&h.env, chain, &record, "/snapshot_commit", &body).await.map_err(Error::RustError)?;
    if v.get("recorded").is_none() {
        alarm(h, format!("snapshot {t:?} at {} not committed: {v}", job.to));
    }
    Ok(())
}

/// The leader's half: HEAD the key in both buckets, then record `Archived` with it and wait for the entry. A key
/// in fewer than two buckets is refused, so a snapshot nobody holds twice is never named by an entry.
pub async fn commit(h: &Rc<Host<Repo>>, body: Value) -> Result<Value> {
    let key_s = body["key"].as_str().ok_or_else(|| Error::RustError("key".into()))?;
    let key: git::R2Key = key_s.parse().map_err(|e: git::ParseKeyError| Error::RustError(e.to_string()))?;
    let git::R2Key::Snapshot { to, digest, .. } = key else {
        return Ok(json!({ "refused": "not a snapshot key" }));
    };
    let sites: Vec<u8> = serde_json::from_value(body["sites"].clone())?;
    let mut held = 0;
    for s in &sites {
        if bucket(&h.env, *s)?.head(key_s).await?.is_some() {
            held += 1;
        }
    }
    if held < 2 {
        return Ok(json!({ "refused": format!("held in {held} buckets") }));
    }
    let at = Timestamp(body["toAt"].as_u64().unwrap_or(0));
    let a = git::Archived { key: key.clone(), from: Index(0), to, from_at: at, to_at: at, digest };
    // The record's request id is the key's, so a re-sent commit records once.
    let rid = raft::RequestId(graftyard_types::Ulid(u128::from_be_bytes(digest.0[..16].try_into().expect("16"))));
    let rx = h.wait_recorded(rid);
    let r = h.input(|m, t| m.record(t, rid, git::repo::REPO_FORMAT_V1, vec![git::repo::RepoEvent::Archived(a)]))?;
    if let Err(e) = r {
        return Ok(json!({ "error": crate::host::call_error_json(&e) }));
    }
    h.served();
    match crate::host::within(std::time::Duration::from_secs(10), rx).await {
        Some(Ok(Ok(index))) => Ok(json!({ "recorded": index.0 })),
        Some(Ok(Err(nl))) => Ok(json!({ "error": crate::host::nl_json(&nl) })),
        _ => Ok(json!({ "busy": true })),
    }
}

/// Deletes from both buckets every snapshot this writer put whose row the tier no longer keeps.
async fn retain(h: &Rc<Host<Repo>>, sql: &SqlStorage, t: Tier, store: &dyn TierStore) -> Result<()> {
    let kept: Vec<u64> = git::tiers::snapshots(store, t).iter().map(|(to, _, _)| to.0).collect();
    let newest = kept.iter().max().copied().unwrap_or(0);
    for r in rows(sql, "SELECT key, to_idx, sites FROM snap_put WHERE tier = ?", vec![i(tier_int(t))])? {
        let to = crate::host::int(&r[1])? as u64;
        if kept.contains(&to) || to >= newest {
            continue;
        }
        let key = crate::host::text(&r[0])?.to_owned();
        let sites: Vec<u8> = serde_json::from_str(crate::host::text(&r[2])?)?;
        for s in sites {
            bucket(&h.env, s)?.delete(&key).await?;
        }
        sql.exec("DELETE FROM snap_put WHERE key = ?", vec![key.into()])?;
    }
    Ok(())
}

/// Each tier's `applied`, `fold_digest`, whether it is loaded, and, with `at`, its digest after entry `at`.
pub fn tiers_json(fold: &git::repo::RepoFold, at: Option<u64>) -> Value {
    let one = |s: &dyn TierStore, t: Tier| {
        let m = meta(s);
        json!({
            "loaded": s.loaded(),
            "applied": m.applied.0,
            "digest": fold_digest(s, t).hex(),
            "at": at.and_then(|a| digest_at(s, Index(a))).map(|d| d.hex()),
            "snapshots": git::tiers::snapshots(&*fold.core, t).iter().map(|(to, _, a)| json!({ "to": to.0, "key": a.key.to_string() })).collect::<Vec<_>>(),
            "running": m.snap.map(|s| s.to.0),
        })
    };
    json!({ "core": one(&*fold.core, Tier::Core), "metadata": one(&*fold.meta, Tier::Metadata) })
}

/// The `snap_log` rows, newest first, for BF-20.
pub fn log_json(sql: &SqlStorage) -> Result<Value> {
    let mut out = Vec::new();
    for r in rows(sql, "SELECT row FROM snap_log ORDER BY to_idx DESC, tier LIMIT 50", vec![])? {
        out.push(serde_json::from_str::<Value>(crate::host::text(&r[0])?)?);
    }
    Ok(json!(out))
}

/// Entries a member replays for the fold audit in one request [guess]: the digests every member keeps
/// (`graftyard_fold::DIGESTS_KEEP`), so the index the replay reaches is one every caught-up member can answer for.
const REPLAY_MAX: u64 = graftyard_fold::DIGESTS_KEEP;

/// The fold audit's refold (`design/SHARED.md`, "Folds", `fold_audit`): the stored log from genesis through this
/// member's `applied`, replayed into a fold in memory by the same `apply`, and each tier's `fold_digest` there. A chain
/// whose log no longer starts at 1, or is longer than `REPLAY_MAX`, is skipped with the reason: refolding it needs the
/// archive objects, which hold the entries' JSON, not the entries.
pub fn replay(h: &Rc<Host<Repo>>) -> Result<Value> {
    let (repo, owner, applied) = {
        let core = h.core.borrow();
        let c = core.as_ref().ok_or_else(|| Error::RustError("not joined".into()))?;
        let f = &c.m.group_fold().fold;
        (f.repo, c.join.owner, meta(&*f.core).applied)
    };
    if applied.0 > REPLAY_MAX {
        return Ok(json!({ "skip": format!("{} entries, past the {REPLAY_MAX} one request replays", applied.0) }));
    }
    let sql = h.state.storage().sql();
    let mut fold = git::repo::RepoFold::with_tiers(
        repo,
        owner,
        Box::new(graftyard_fold::MemStore::default()),
        Box::new(graftyard_fold::MemStore::default()),
    );
    let mut next = 1;
    for r in rows(&sql, "SELECT idx, row FROM log WHERE idx <= ? ORDER BY idx", vec![i(applied.0)])? {
        let idx = crate::host::int(&r[0])? as u64;
        if idx != next {
            return Ok(json!({ "skip": format!("the stored log starts at {idx}, not {next}") }));
        }
        let e: raft::LogEntry<git::repo::RepoEvent> =
            raft::codec::from_bytes(blob(&r[1])?).map_err(|e| Error::RustError(e.to_string()))?;
        raft::Fold::apply_entry(&mut fold, &e);
        next += 1;
    }
    if next != applied.0 + 1 {
        return Ok(json!({ "skip": format!("the stored log ends at {}, not {}", next - 1, applied.0) }));
    }
    Ok(json!({
        "index": applied.0,
        "core": fold_digest(&*fold.core, Tier::Core).hex(),
        "metadata": fold_digest(&*fold.meta, Tier::Metadata).hex(),
    }))
}

/// The fold audit's member half (`design/SHARED.md`, "Folds", `fold_audit`): every row of a tier refolded, and its
/// digest against the one `apply` kept. A long tier takes several calls: `from` is the (table, key) to resume at.
pub fn refold_page(fold: &git::repo::RepoFold, t: Tier, state: Option<Value>, budget: usize) -> Result<Value> {
    let store: &dyn TierStore = match t {
        Tier::Core => &*fold.core,
        Tier::Metadata => &*fold.meta,
    };
    if !store.loaded() {
        return Ok(json!({ "loading": true }));
    }
    let schema = schema(t);
    #[derive(Serialize, Deserialize)]
    struct State {
        table: usize,
        after: Option<Vec<graftyard_fold::Val>>,
        sum: [u8; 32],
        rows: u64,
        applied: u64,
    }
    let m = meta(store);
    let mut st: State = match state {
        Some(v) if !v.is_null() => serde_json::from_value(v)?,
        _ => State { table: 0, after: None, sum: [0; 32], rows: 0, applied: m.applied.0 },
    };
    if st.applied != m.applied.0 {
        return Ok(json!({ "moved": true, "applied": m.applied.0 }));
    }
    let mut left = budget.max(1);
    while let Some(def) = schema.tables.get(st.table) {
        if left == 0 {
            return Ok(json!({ "state": st }));
        }
        let want = left.min(1_000);
        // `scan` starts at `after` inclusive, so one more row is read and the repeat dropped.
        let from = st.after.clone().unwrap_or_default();
        let batch: Vec<_> = store
            .scan(def.name, &from, want + 1)
            .into_iter()
            .filter(|(k, _)| st.after.as_ref() != Some(k))
            .take(want)
            .collect();
        let n = batch.len();
        for (k, row) in batch {
            graftyard_fold::add_row(&mut st.sum, def.name, &k, &row);
            st.after = Some(k);
            st.rows += 1;
        }
        left -= n;
        if n < want {
            st.table += 1;
            st.after = None;
        }
    }
    let refold = graftyard_fold::digest_with(&schema, &m, &st.sum);
    Ok(
        json!({ "done": true, "applied": m.applied.0, "rows": st.rows, "refold": refold.hex(), "kept": fold_digest(store, t).hex() }),
    )
}

/// Test installs: `seed` puts `n` comment rows of `bytes` each into the metadata tier at `written`, the core tier's
/// `applied`, keeping the row-hash sum (`graftyard_fold::seed`). The rows come from no entry; every member must be
/// seeded alike, which BF-20's driver does and checks by `fold_digest`. `force` starts a snapshot of a tier now.
#[cfg(feature = "test-faults")]
pub fn bench_seed(h: &Rc<Host<Repo>>, body: &Value) -> Result<Value> {
    let sql = h.state.storage().sql();
    if let Some(t) = body.get("force").and_then(Value::as_u64) {
        sql.exec("CREATE TABLE IF NOT EXISTS snap_force (tier INTEGER PRIMARY KEY)", None)?;
        sql.exec("INSERT OR REPLACE INTO snap_force (tier) VALUES (?)", vec![i(t)])?;
        h.arm_in(BUSY_MS);
        return Ok(json!({ "forced": t }));
    }
    let start = body["start"].as_u64().ok_or_else(|| Error::RustError("start".into()))?;
    let n = body["n"].as_u64().unwrap_or(1_000);
    let bytes = body["bytes"].as_u64().unwrap_or(1_800) as usize;
    let core = h.core.borrow();
    let c = core.as_ref().ok_or_else(|| Error::RustError("not joined".into()))?;
    let fold = &c.m.group_fold().fold;
    if !fold.meta.loaded() {
        return Ok(json!({ "loading": true }));
    }
    let written = meta(&*fold.meta).applied;
    let mut m = crate::tier::handle(&*fold.meta).clone();
    drop(core);
    let rows: Vec<_> = (start..start + n)
        .map(|k| {
            let body: String = std::iter::repeat_n(char::from(b'a' + (k % 26) as u8), bytes).collect();
            let rec = git::tiers::CommentRow {
                on: git::issues::CommentTarget::Issue(git::issues::IssueNumber(1 + (k % 200_000) as u32)),
                author: None,
                body: body.into(),
                reply_to: None,
                resolved: false,
                created: Timestamp(k),
                updated: Timestamp(k),
            };
            let b = postcard::to_allocvec(&rec).expect("a row encodes");
            (vec![graftyard_fold::Val::Int(k as i64 + (1 << 40))], vec![graftyard_fold::Val::Blob(b.into())])
        })
        .collect();
    graftyard_fold::seed(&mut m, &git::tiers::METADATA, "comment", written, rows);
    let mut stmts = Vec::new();
    m.seal(&mut stmts);
    let s = h.state.storage();
    crate::host::commit_tx(&s, stmts).map_err(Error::RustError)?;
    m.flushed();
    Ok(json!({ "written": written.0, "digest": fold_digest(&m, Tier::Metadata).hex(), "dbBytes": sql.database_size() }))
}
