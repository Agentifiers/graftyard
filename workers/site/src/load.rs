//! One loader for a repository member's two tiers, shared by `InstallSnapshot` and the fold audit (`design/DESIGN.md`,
//! Shape, "Packs"; `design/SHARED.md`, "Folds", `fold_audit`; `decisions/DECISIONS.md`, "F48").
//!
//! - **A job.** Each tier from a committed snapshot in R2, then log entries applied to both tiers up to `through`, into
//!   scratch tables (`crate::tier::SCRATCH`) beside the member's own. Entries come from the member's stored log, or
//!   from archive objects (`R2Key::Archive`, `graftyard_fold::encode_archive_row`) for what the log no longer holds.
//! - **Batches.** The member's alarm runs one batch at a time: at most `CHUNK` bytes per R2 range read, `BATCH_MS` and
//!   `BATCH_BYTES` per batch. Each batch's rows and the job's cursor commit in one transaction, so a reset object
//!   resumes where the last batch ended, and memory holds one batch, never the fold.
//! - **Install.** A follower or learner sent `InstallSnapshot` loads the named snapshots and archives, rebuilds the
//!   `request` rows of archived entries inside retention, and calls `Member::install`; its `Write::Install` swaps the
//!   scratch tables in, in the transaction that drops the log (`host::SqlStore`). The member then acks the round.
//! - **Source.** A member whose stored log was truncated after an archive (`Host::truncate`) names its install source
//!   (`set_source`): the newest committed snapshot of each tier at or below the archive's end, the archives from there
//!   or from `REQUEST_RETENTION` before it, and the config and id block there. Only a leader sends it.
//! - **Audit.** The fold audit refolds a chain from each tier's newest committed snapshot, or with `deep` the core tier
//!   from its oldest, through the member's `applied`, and reports each tier's `fold_digest` there; with `deep` it also
//!   checks the refold against every later core snapshot's digest on the way. `crate::fold_audit` compares the report
//!   with every voter's digest at that index.

use std::rc::Rc;

use graftyard_fold::{
    self as fold, ARCHIVE_HEADER, Restore, RestoreCursor, Schema, SnapshotFrame, check_archive_header,
    decode_archive_some, decode_some, fold_digest, frames_header,
};
use graftyard_git as git;
use graftyard_raft::{self as raft, EntryBody, Fold as _, Index, InstallSnapshot, InstallSource, InstallWanted, codec};
use graftyard_types::{ChainId, Digest, SiteId, Tier};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::Digest as _;
use smallvec::SmallVec;
use worker::*;

use crate::family::{Family, Repo, Stmt};
use crate::host::{Host, blob, commit_tx, err, i, int, rows, text};
use crate::tier::TierSql;
use crate::{SiteEnv, now_ms};

type Entry = raft::LogEntry<git::repo::RepoEvent>;

/// The largest R2 range one read takes [guess]: a snapshot frame holds at most 500 rows of a few KB.
const CHUNK: u64 = 1 << 20;
/// A batch stops after this long, this many bytes read, or this many tier rows and entries [guess]. Its rows are
/// buffered as rows and again as statements until its transaction, beside the member's own state: a `repo:65536`
/// member's isolate stands at 111 MB of wasm memory, and a 4 MB batch of its log took the isolate past 128 MB.
const BATCH_MS: u64 = 2_000;
const BATCH_BYTES: u64 = 1 << 20;
const BATCH_ROWS: u64 = 20_000;
/// A batch that began this many times without its transaction committing is abandoned: its isolate died under it,
/// and resuming it again would reset every member in the isolate again (F47).
const BATCH_TRIES: u32 = 3;
const BUSY_MS: i64 = 20;
/// The largest snapshot object an audit restores into scratch tables [guess]. A larger tier is reported, not refolded:
/// its scratch copy would double the member's storage (the BF-20 metadata tier's snapshot is 2.4 GB, of a 4 GB tier).
const AUDIT_SNAPSHOT_MAX: u64 = 256 << 20;
/// How often the cron audits one chain [guess]: the design's daily digest comparison (`design/SHARED.md`, "Folds").
const AUDIT_EVERY_MS: u64 = 86_400_000;

#[derive(Clone, Debug, Serialize, Deserialize)]
struct TierLoad {
    /// The snapshot to start from, or none: the tier starts empty at index 0.
    key: Option<String>,
    /// Not loaded at all: a deep audit refolds the core tier alone.
    #[serde(default)]
    skip: bool,
    to: u64,
    /// The site whose bucket holds the key, once found, and the object's size.
    site: Option<u8>,
    size: u64,
    /// The next byte of the object to read.
    offset: u64,
    restore: Option<RestoreCursor>,
    restored: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Arch {
    from: u64,
    to: u64,
    key: String,
    site: Option<u8>,
    size: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
enum Kind {
    Install { from: SiteId, install: Box<InstallSnapshot> },
    Audit { deep: bool },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Job {
    kind: Kind,
    chain: ChainId,
    through: u64,
    tiers: [TierLoad; 2],
    archives: Vec<Arch>,
    /// The archive being read and the next byte of it.
    arch: usize,
    arch_off: u64,
    /// The next entry to read.
    next: u64,
    /// Scratch tables made for this job.
    prepared: bool,
    /// Entry `through`'s time, once read: an install keeps the `request` rows inside retention of it.
    through_at: Option<u64>,
    /// Audit with `deep`: core snapshots to check the refold against on the way, `(to, digest)`.
    checks: Vec<(u64, Digest)>,
    segments: Vec<Value>,
    began_ms: u64,
    /// Batches begun since the last one that committed (`BATCH_TRIES`).
    #[serde(default)]
    tries: u32,
    batches: u32,
    batch_max_ms: u64,
    bytes: u64,
    rows: u64,
    entries: u64,
    mem_max_mb: f64,
}

impl Job {
    fn install(&self) -> bool {
        matches!(self.kind, Kind::Install { .. })
    }
}

fn schema(t: usize) -> Schema {
    [git::tiers::CORE, git::tiers::METADATA][t]
}

fn tier_of(t: usize) -> Tier {
    [Tier::Core, Tier::Metadata][t]
}

/// This isolate's wasm linear memory, which only grows: its high-water mark.
pub fn wasm_mem_mb() -> f64 {
    #[cfg(target_arch = "wasm32")]
    return core::arch::wasm32::memory_size::<0>() as f64 * 65_536.0 / 1e6;
    #[cfg(not(target_arch = "wasm32"))]
    0.0
}

fn load_job(sql: &SqlStorage) -> Result<Option<Job>> {
    match rows(sql, "SELECT row FROM load_job WHERE id = 1", vec![])?.first() {
        Some(r) => Ok(Some(serde_json::from_str(text(&r[0])?)?)),
        None => Ok(None),
    }
}

fn job_stmt(job: &Job) -> Result<Stmt> {
    Ok(("INSERT OR REPLACE INTO load_job (id, row) VALUES (1, ?)", vec![serde_json::to_string(job)?.into()]))
}

fn save_job(h: &Rc<Host<Repo>>, job: &Job) -> Result<()> {
    commit_tx(&h.state.storage(), vec![job_stmt(job)?]).map_err(err)
}

fn note(h: &Rc<Host<Repo>>, m: String) {
    console_error!("{m}");
    let mut s = h.stats.borrow_mut();
    if s.alarms.len() < 50 {
        s.alarms.push(format!("{} {m}", now_ms()));
    }
}

const SCRATCH_REQUEST: &str = "CREATE TABLE IF NOT EXISTS i_request (rid TEXT PRIMARY KEY, idx INTEGER NOT NULL, at INTEGER NOT NULL, events BLOB NOT NULL)";

/// Drops a job's scratch tables, made again empty when `fresh`.
fn reset_scratch(sql: &SqlStorage, fresh: bool) -> Result<()> {
    for s in [git::tiers::CORE, git::tiers::METADATA] {
        for q in crate::tier::drop_scratch(&s) {
            sql.exec(&q, None)?;
        }
    }
    sql.exec("DROP TABLE IF EXISTS i_request", None)?;
    if fresh {
        crate::tier::scratch_tiers(sql, false)?;
        sql.exec(SCRATCH_REQUEST, None)?;
    }
    Ok(())
}

/// The statements `Write::Install` adds: the scratch tables become the member's fold and `request` table, and any
/// snapshot or load job of the old fold is dropped.
pub fn install_stmts(stmts: &mut Vec<Stmt>) {
    for s in [git::tiers::CORE, git::tiers::METADATA] {
        stmts.extend(crate::tier::swap_in(&s).into_iter().map(|q| (q, vec![])));
    }
    for q in [
        "DROP TABLE IF EXISTS request",
        "ALTER TABLE i_request RENAME TO request",
        "CREATE INDEX IF NOT EXISTS request_at ON request (at)",
        "DELETE FROM snap_run",
        "DELETE FROM snap_buf",
        "DELETE FROM load_job",
        "DELETE FROM load_want",
    ] {
        stmts.push((q, vec![]));
    }
}

// ---- R2 ----

fn bucket(env: &Env, s: u8) -> Result<Bucket> {
    env.bucket(&format!("BUCKET_{s}"))
}

/// The site whose bucket holds `key`, this site's first, and the object's size.
async fn find(env: &Env, key: &str) -> Result<(u8, u64)> {
    let site = SiteEnv::read(env)?;
    let mut order: Vec<u8> = vec![site.id.0];
    order.extend(site.sites.0.iter().map(|s| s.id.0).filter(|s| *s != site.id.0));
    for s in order {
        if let Some(o) = bucket(env, s)?.head(key).await? {
            return Ok((s, o.size()));
        }
    }
    Err(err(format!("{key} is in no bucket")))
}

async fn get_range(env: &Env, site: u8, key: &str, offset: u64, length: u64) -> Result<Vec<u8>> {
    let o = bucket(env, site)?
        .get(key)
        .range(Range::OffsetWithLength { offset, length })
        .execute()
        .await?
        .ok_or_else(|| err(format!("{key} is gone from bucket {site}")))?;
    o.body().ok_or_else(|| err("no body"))?.bytes().await
}

/// `length` bytes at `offset` of an object of `size`, read again longer while the first frame or row past `start`
/// is not complete in it: its 4-byte length says how much it needs.
async fn read_whole(env: &Env, site: u8, key: &str, offset: u64, size: u64, start: usize) -> Result<Vec<u8>> {
    let mut len = CHUNK.min(size - offset);
    loop {
        let b = get_range(env, site, key, offset, len).await?;
        if b.len() < start + 4 {
            return Ok(b);
        }
        let need = (start + 4 + u32::from_be_bytes(b[start..start + 4].try_into().expect("4")) as usize) as u64;
        if need <= b.len() as u64 || offset + len >= size {
            return Ok(b);
        }
        len = need.min(size - offset);
    }
}

// ---- jobs ----

/// The leader sent `InstallSnapshot`: start loading it, or keep loading the same one.
/// The leader re-sends it every CALL_DEADLINE while the load runs; a repeat of the running one only wakes it. The
/// job answers the round it started for: if the leader has moved on, its next `InstallSnapshot` finds the member
/// holding `through` and is answered at once (`Member::on_install`).
pub fn want(h: &Rc<Host<Repo>>, w: InstallWanted) {
    let sql = h.state.storage().sql();
    let same = load_job(&sql).ok().flatten().is_some_and(|j| match &j.kind {
        Kind::Install { install, .. } => install.source == w.install.source,
        Kind::Audit { .. } => false,
    });
    // The newest request for the same source, from any term, is the one the finished load answers (`finish_install`).
    let newest = || -> Result<()> {
        let row = serde_json::to_string(&(w.from, &w.install))?;
        sql.exec("INSERT OR REPLACE INTO load_want (id, row) VALUES (1, ?)", vec![row.into()])?;
        Ok(())
    };
    let r =
        if same { newest() } else { install_job(h, w.clone()).and_then(|j| save_job(h, &j)).and_then(|()| newest()) };
    match r {
        Ok(()) => h.arm_in(BUSY_MS),
        Err(e) => note(h, format!("install not started: {e}")),
    }
}

fn snapshot_to(key: &str) -> Result<u64> {
    match key.parse::<git::R2Key>() {
        Ok(git::R2Key::Snapshot { to, .. }) => Ok(to.0),
        _ => Err(err(format!("{key} is not a snapshot key"))),
    }
}

fn tier_load(key: Option<String>) -> Result<TierLoad> {
    let to = key.as_deref().map(snapshot_to).transpose()?.unwrap_or(0);
    Ok(TierLoad { key, to, skip: false, site: None, size: 0, offset: 0, restore: None, restored: false })
}

fn install_job(h: &Rc<Host<Repo>>, w: InstallWanted) -> Result<Job> {
    let chain = h.core.borrow().as_ref().map(|c| c.join.chain).ok_or_else(|| err("not joined"))?;
    let s = &w.install.source;
    let archives = s
        .archives
        .iter()
        .map(|k| match k.parse::<git::R2Key>() {
            Ok(git::R2Key::Archive { from, to, .. }) => {
                Ok(Arch { from: from.0, to: to.0, key: k.to_string(), site: None, size: 0 })
            }
            _ => Err(err(format!("{k} is not an archive key"))),
        })
        .collect::<Result<Vec<_>>>()?;
    let tiers = [tier_load(Some(s.core.to_string()))?, tier_load(s.metadata.as_ref().map(|k| k.to_string()))?];
    let next = archives.first().map_or(s.through.0 + 1, |a| a.from);
    Ok(new_job(
        Kind::Install { from: w.from, install: Box::new(w.install.clone()) },
        chain,
        s.through.0,
        tiers,
        archives,
        next,
    ))
}

fn new_job(kind: Kind, chain: ChainId, through: u64, tiers: [TierLoad; 2], archives: Vec<Arch>, next: u64) -> Job {
    Job {
        kind,
        chain,
        through,
        tiers,
        archives,
        arch: 0,
        arch_off: 0,
        next,
        prepared: false,
        through_at: None,
        checks: Vec::new(),
        segments: Vec::new(),
        began_ms: now_ms(),
        tries: 0,
        batches: 0,
        batch_max_ms: 0,
        bytes: 0,
        rows: 0,
        entries: 0,
        mem_max_mb: wasm_mem_mb(),
    }
}

/// The archives the member's applied `Archived` entries named that hold entries from `from` on.
fn stored_archives(sql: &SqlStorage, from: u64) -> Result<Vec<Arch>> {
    rows(sql, "SELECT from_idx, to_idx, key FROM archive_keys WHERE to_idx >= ? ORDER BY from_idx", vec![i(from)])?
        .iter()
        .map(|r| {
            Ok(Arch { from: int(&r[0])? as u64, to: int(&r[1])? as u64, key: text(&r[2])?.into(), site: None, size: 0 })
        })
        .collect()
}

fn audit_job(h: &Rc<Host<Repo>>, deep: bool) -> Result<Job> {
    let core = h.core.borrow();
    let c = core.as_ref().ok_or_else(|| err("not joined"))?;
    let f = &c.m.group_fold().fold;
    if !f.meta.loaded() {
        return Err(err("the metadata tier is loading"));
    }
    let through = fold::meta(&*f.core).applied;
    let mut tiers = Vec::new();
    let mut checks = Vec::new();
    for t in 0..2 {
        let snaps: Vec<_> =
            git::tiers::snapshots(&*f.core, tier_of(t)).into_iter().filter(|(to, _, _)| *to <= through).collect();
        let start = if deep && t == 0 { snaps.first() } else { snaps.last() };
        if deep && t == 0 {
            checks = snaps.iter().skip(1).map(|(to, _, a)| (to.0, a.digest)).collect();
        }
        tiers.push(tier_load(start.map(|(_, _, a)| a.key.to_string()))?);
    }
    let mut tiers: [TierLoad; 2] = tiers.try_into().expect("two tiers");
    if deep {
        tiers[1] = TierLoad { skip: true, restored: true, ..tier_load(None)? };
    }
    let next = if deep { tiers[0].to + 1 } else { tiers[0].to.min(tiers[1].to) + 1 };
    let archives = stored_archives(&h.state.storage().sql(), next)?;
    let mut job = new_job(Kind::Audit { deep }, c.join.chain, through.0, tiers, archives, next);
    job.checks = checks;
    Ok(job)
}

/// After restore: the stored install source goes back to the member, and a job left running resumes.
pub fn restored(h: &Rc<Host<Repo>>) {
    let sql = h.state.storage().sql();
    if let Ok(r) = rows(&sql, "SELECT row FROM install_source WHERE id = 1", vec![])
        && let Some(r) = r.first()
        && let Ok(b) = blob(&r[0])
        && let Ok(src) = codec::from_bytes::<InstallSource>(b)
        && let Some(c) = h.core.borrow_mut().as_mut()
    {
        c.m.set_install(Some(src));
    }
    if load_job(&sql).ok().flatten().is_some() {
        h.arm_in(BUSY_MS);
    }
}

pub async fn on_alarm(h: &Rc<Host<Repo>>) {
    h.settled().await;
    let sql = h.state.storage().sql();
    let job = match load_job(&sql) {
        Ok(Some(j)) => j,
        Ok(None) => return,
        Err(e) => return note(h, format!("load job unreadable: {e}")),
    };
    let install = job.install();
    let loaded = job.tiers.iter().all(|t| t.restored) && job.next > job.through;
    if job.tries >= BATCH_TRIES {
        let e = format!("{} batches began without committing: the isolate died under them", job.tries);
        note(h, format!("{} abandoned: {e}", if install { "install" } else { "fold audit" }));
        let _ = report(h, &json!({ "error": e, "job": job_json(&job), "at": now_ms() }));
        let _ = sql.exec("DELETE FROM load_job", None);
        let _ = reset_scratch(&sql, false);
        return;
    }
    // Counted before the batch and cleared by its commit, so a batch that kills its isolate is seen by the next.
    let mut marked = job.clone();
    marked.tries += 1;
    if let Err(e) = save_job(h, &marked) {
        return note(h, format!("load job not saved: {e}"));
    }
    match batch(h, &sql, job).await {
        // A load that only waits for the leader's next request tries again at its pace.
        Ok(true) if loaded => h.arm_in(raft::CALL_DEADLINE.as_millis() as i64),
        Ok(true) => h.arm_in(BUSY_MS),
        Ok(false) => {}
        Err(e) => {
            note(h, format!("{} failed: {e}", if install { "install" } else { "fold audit" }));
            if !install {
                let _ = report(h, &json!({ "error": e.to_string(), "at": now_ms() }));
            }
            let _ = sql.exec("DELETE FROM load_job", None);
            let _ = reset_scratch(&sql, false);
        }
    }
}

fn report(h: &Rc<Host<Repo>>, v: &Value) -> Result<()> {
    h.state.storage().sql().exec(
        "INSERT OR REPLACE INTO load_report (id, at, row) VALUES (1, ?, ?)",
        vec![i(now_ms()), v.to_string().into()],
    )?;
    Ok(())
}

/// One batch. Whether more batches are needed.
async fn batch(h: &Rc<Host<Repo>>, sql: &SqlStorage, mut job: Job) -> Result<bool> {
    let t0 = now_ms();
    if !job.prepared {
        reset_scratch(sql, true)?;
        job.prepared = true;
        save_job(h, &job)?;
        return Ok(true);
    }
    let (mut core, mut meta) = crate::tier::scratch_tiers(sql, job.install())?;
    let tiers = h.core.borrow().as_ref().map(|c| c.m.group_fold().fold.tiers()).ok_or_else(|| err("not joined"))?;
    let mut stmts: Vec<Stmt> = Vec::new();
    let mut bytes = 0u64;
    let rows0 = job.rows + job.entries;
    let more = |bytes: u64, job: &Job| {
        now_ms() - t0 < BATCH_MS && bytes < BATCH_BYTES && job.rows + job.entries - rows0 < BATCH_ROWS
    };
    for t in 0..2 {
        while !job.tiers[t].restored && more(bytes, &job) {
            let store = if t == 0 { &mut core } else { &mut meta };
            bytes += restore_some(h, &mut job, t, store).await?;
            // The member's Raft inputs run between reads, as between a snapshot writer's (`crate::snap`).
            Delay::from(std::time::Duration::ZERO).await;
        }
    }
    if job.tiers.iter().all(|t| t.restored) {
        while job.next <= job.through && more(bytes, &job) {
            let (es, n) = next_entries(h, sql, &mut job).await?;
            bytes += n;
            for e in es {
                apply_one(&tiers, &mut job, &mut core, &mut meta, &e, &mut stmts)?;
            }
            Delay::from(std::time::Duration::ZERO).await;
        }
    }
    core.seal(&mut stmts);
    meta.seal(&mut stmts);
    let ms = now_ms() - t0;
    job.tries = 0;
    job.batches += 1;
    job.batch_max_ms = job.batch_max_ms.max(ms);
    job.bytes += bytes;
    job.mem_max_mb = job.mem_max_mb.max(wasm_mem_mb());
    let done = job.tiers.iter().all(|t| t.restored) && job.next > job.through;
    stmts.push(job_stmt(&job)?);
    commit_tx(&h.state.storage(), stmts).map_err(err)?;
    core.flushed();
    meta.flushed();
    if !done {
        return Ok(true);
    }
    match job.kind.clone() {
        Kind::Audit { deep } => finish_audit(h, sql, &job, &core, &meta, deep).map(|()| false),
        Kind::Install { from, install } => finish_install(h, sql, &job, &core, &meta, from, *install),
    }
}

/// One range of tier `t`'s snapshot into its scratch tables. The bytes read.
async fn restore_some(h: &Rc<Host<Repo>>, job: &mut Job, t: usize, store: &mut TierSql) -> Result<u64> {
    let chain = job.chain;
    let tl = &mut job.tiers[t];
    let Some(key) = tl.key.clone() else {
        tl.restored = true;
        return Ok(0);
    };
    if tl.site.is_none() {
        let (s, size) = find(&h.env, &key).await?;
        (tl.site, tl.size) = (Some(s), size);
        if !matches!(job.kind, Kind::Install { .. }) && size > AUDIT_SNAPSHOT_MAX {
            let tl = &mut job.tiers[t];
            (tl.skip, tl.restored) = (true, true);
            return Ok(0);
        }
    }
    let tl = &mut job.tiers[t];
    let site = tl.site.expect("found");
    let start = if tl.offset == 0 { frames_header().len() } else { 0 };
    let b = read_whole(&h.env, site, &key, tl.offset, tl.size, start).await?;
    if start > 0 && b.get(..start) != Some(&frames_header()[..]) {
        return Err(err(format!("{key} is not a snapshot frame stream")));
    }
    let (frames, used) = decode_some(&b[start..]).map_err(|e| err(e.to_string()))?;
    if frames.is_empty() {
        return Err(err(format!("{key}: no whole frame at byte {}", tl.offset)));
    }
    let s = schema(t);
    let mut r = match tl.restore.take() {
        Some(c) => Restore::resume(s, c),
        None => Restore::begin(store, s, (chain, Index(tl.to))).map_err(|e| err(e.to_string()))?,
    };
    let mut ended = false;
    for f in &frames {
        r.frame(store, f).map_err(|e| err(e.to_string()))?;
        match f {
            SnapshotFrame::Rows { rows, .. } => job.rows += rows.len() as u64,
            SnapshotFrame::End(_) => ended = true,
        }
    }
    let tl = &mut job.tiers[t];
    tl.offset += (start + used) as u64;
    if ended {
        r.finish_rows(store).map_err(|e| err(e.to_string()))?;
        tl.restored = true;
    } else {
        tl.restore = Some(r.cursor());
    }
    Ok((start + used) as u64)
}

/// The next entries from `job.next`: from the stored log while it holds them (an audit), else from the archives. The
/// entries and the bytes read.
async fn next_entries(h: &Rc<Host<Repo>>, sql: &SqlStorage, job: &mut Job) -> Result<(Vec<Entry>, u64)> {
    let mut out = Vec::new();
    let mut bytes = 0u64;
    if !job.install() {
        let q = "SELECT idx, row FROM log WHERE idx >= ? AND idx <= ? ORDER BY idx";
        for r in sql.exec(q, vec![i(job.next), i(job.through)])?.raw() {
            let r = r?;
            let idx = int(&r[0])? as u64;
            if idx != job.next + out.len() as u64 {
                break;
            }
            let b = blob(&r[1])?;
            bytes += b.len() as u64;
            out.push(codec::from_bytes::<Entry>(b).map_err(|e| err(e.to_string()))?);
            if bytes >= CHUNK / 4 || out.len() >= 64 {
                break;
            }
        }
        if !out.is_empty() {
            return Ok((out, bytes));
        }
    }
    let Some(a) = job.archives.get_mut(job.arch) else {
        return Err(err(format!("no stored row and no archive holds entry {}", job.next)));
    };
    if a.to < job.next || (a.site.is_some() && job.arch_off >= a.size) {
        job.arch += 1;
        job.arch_off = 0;
        return Ok((out, 0));
    }
    if a.from > job.next {
        return Err(err(format!("archive {} starts after entry {}", a.key, job.next)));
    }
    if a.site.is_none() {
        let (s, size) = find(&h.env, &a.key).await?;
        (a.site, a.size) = (Some(s), size);
    }
    let start = if job.arch_off == 0 { ARCHIVE_HEADER } else { 0 };
    let b = read_whole(&h.env, a.site.expect("found"), &a.key, job.arch_off, a.size, start).await?;
    if start > 0 {
        check_archive_header(&b).map_err(|e| err(format!("{}: {e}", a.key)))?;
    }
    let (rows, used, _) = decode_archive_some(&b[start..]);
    if rows.is_empty() {
        return Err(err(format!("{}: no whole entry at byte {}", a.key, job.arch_off)));
    }
    for r in rows {
        let e: Entry = codec::from_bytes(r).map_err(|e| err(e.to_string()))?;
        if e.index.0 >= job.next {
            out.push(e);
        }
    }
    job.arch_off += (start + used) as u64;
    Ok((out, (start + used) as u64))
}

fn apply_one(
    tiers: &git::tiers::RepoTiers,
    job: &mut Job,
    core: &mut TierSql,
    meta: &mut TierSql,
    e: &Entry,
    stmts: &mut Vec<Stmt>,
) -> Result<()> {
    if e.index.0 != job.next {
        return Err(err(format!("entry {:?} where {} was next", e.index, job.next)));
    }
    for (t, store) in [(Tier::Core, &mut *core), (Tier::Metadata, &mut *meta)] {
        if job.tiers[t as usize].skip {
            continue;
        }
        let applied = fold::meta(&*store).applied;
        if e.index.0 == applied.0 + 1 {
            fold::apply(tiers, store, t, e).map_err(|x| err(format!("{t:?} tier at {:?}: {x}", e.index)))?;
        } else if e.index > applied {
            return Err(err(format!("{t:?} tier at {applied:?} cannot apply {:?}", e.index)));
        }
    }
    // An install keeps every archived request row; the ones past retention of `through` go at the end.
    if job.install()
        && let EntryBody::Events(evs) = &e.body
    {
        for (rid, r) in &e.requests {
            let row: Vec<git::repo::RepoEvent> =
                evs[r.start as usize..r.end as usize].iter().map(git::repo::RepoFold::row_event).collect();
            stmts.push((
                "INSERT OR IGNORE INTO i_request (rid, idx, at, events) VALUES (?, ?, ?, ?)",
                vec![rid.to_string().into(), i(e.index.0), i(e.at.0), codec::to_bytes(&row).into()],
            ));
        }
    }
    if e.index.0 == job.through {
        job.through_at = Some(e.at.0);
    }
    if let Some(&(to, want)) = job.checks.iter().find(|c| c.0 == e.index.0) {
        let got = fold_digest(&*core, Tier::Core);
        job.segments.push(json!({ "to": to, "want": want.hex(), "got": got.hex(), "ok": got == want }));
    }
    job.entries += 1;
    job.next += 1;
    Ok(())
}

fn job_json(job: &Job) -> Value {
    json!({
        "through": job.through,
        "next": job.next,
        "tiers": job.tiers.iter().map(|t| json!({ "key": t.key, "to": t.to, "offset": t.offset, "size": t.size, "restored": t.restored, "skipped": t.skip })).collect::<Vec<_>>(),
        "archives": job.archives.len(),
        "batches": job.batches,
        "batchMaxMs": job.batch_max_ms,
        "bytes": job.bytes,
        "rows": job.rows,
        "entries": job.entries,
        "ms": now_ms() - job.began_ms,
        "memMaxMb": job.mem_max_mb,
        "segments": job.segments,
    })
}

fn finish_audit(
    h: &Rc<Host<Repo>>,
    sql: &SqlStorage,
    job: &Job,
    core: &TierSql,
    meta: &TierSql,
    deep: bool,
) -> Result<()> {
    let mut v = job_json(job);
    v["kind"] = json!("audit");
    v["deep"] = json!(deep);
    v["index"] = json!(job.through);
    let d = |t: usize, s: &TierSql| (!job.tiers[t].skip).then(|| fold_digest(s, tier_of(t)).hex());
    v["core"] = json!(d(0, core));
    v["metadata"] = json!(d(1, meta));
    v["at"] = json!(now_ms());
    report(h, &v)?;
    sql.exec("DELETE FROM load_job", None)?;
    reset_scratch(sql, false)?;
    console_log!("fold audit refold {v}");
    Ok(())
}

fn finish_install(
    h: &Rc<Host<Repo>>,
    sql: &SqlStorage,
    job: &Job,
    core: &TierSql,
    meta: &TierSql,
    from: SiteId,
    install: InstallSnapshot,
) -> Result<bool> {
    let s = &install.source;
    for (t, store) in [(Tier::Core, core), (Tier::Metadata, meta)] {
        let m = fold::meta(store);
        if m.applied != s.through || m.last != s.through_digest || m.term != s.through_term {
            return Err(err(format!("the loaded {t:?} tier stands at {:?}, not the install's through", m.applied)));
        }
    }
    let at = job.through_at.ok_or_else(|| err("entry through was not read"))?;
    let retention = crate::host::request_retention(&h.env);
    let cut = at.saturating_sub(retention.as_millis() as u64);
    sql.exec("DELETE FROM i_request WHERE at < ?", vec![i(cut)])?;
    let kept = rows(sql, "SELECT count(*) FROM i_request", vec![])?.first().map_or(Ok(0), |r| int(&r[0]))?;
    // A fold over the member's own table names, which hold the loaded rows once `Write::Install`'s transaction ran.
    let gf = {
        let core = h.core.borrow();
        let c = core.as_ref().ok_or_else(|| err("not joined"))?;
        let f = Repo::load_fold(sql, &c.join)?;
        crate::tier::handle(&*f.meta).set_loaded(true);
        let reqs = h.request_store().ok_or_else(|| err("no request table"))?;
        let mut gf = raft::GroupFold::from_parts(f, s.through, Some(s.config.clone()), s.id_block, Box::new(reqs));
        gf.set_retention(retention);
        gf
    };
    let mut v = job_json(job);
    v["kind"] = json!("install");
    v["requestRows"] = json!(kept);
    v["core"] = json!(fold_digest(core, Tier::Core).hex());
    v["metadata"] = json!(fold_digest(meta, Tier::Metadata).hex());
    let through = s.through;
    // The leader's newest `InstallSnapshot` for this source, whose term and round the member answers.
    let (from, install) = match rows(sql, "SELECT row FROM load_want WHERE id = 1", vec![])?.first() {
        Some(r) => serde_json::from_str::<(SiteId, InstallSnapshot)>(text(&r[0])?)?,
        None => (from, install),
    };
    let w = InstallWanted { from, install };
    match h.input(|m, t| m.install(t, w, gf))? {
        Ok(()) => v["installed"] = json!(true),
        // Not the term or leader of the request it answers (a restart, an election): the loaded tables stay, and the
        // install is tried again once the leader's next `InstallSnapshot` updated `load_want`.
        Err(raft::InstallError::NotNeeded)
            if h.core.borrow().as_ref().is_some_and(|c| c.m.state().commit < through) =>
        {
            return Ok(true);
        }
        Err(raft::InstallError::NotNeeded) => {
            v["installed"] = json!("notNeeded");
            sql.exec("DELETE FROM load_job", None)?;
            reset_scratch(sql, false)?;
        }
        Err(raft::InstallError::Mismatch { what }) => return Err(err(format!("install refused: {what}"))),
    }
    v["at"] = json!(now_ms());
    report(h, &v)?;
    h.stats.borrow_mut().installs += 1;
    console_log!("install {v}");
    Ok(false)
}

// ---- the leader's side ----

/// After the stored log was truncated below `to`: the install source a leader sends a member whose next entry is at
/// or below `to`. Kept in `install_source`, since it names the config as of `to`, which a later config change
/// replaces.
pub async fn set_source(h: &Rc<Host<Repo>>, to: Index) {
    match source(h, to) {
        Ok(src) => {
            let _ = h.state.storage().sql().exec(
                "INSERT OR REPLACE INTO install_source (id, row) VALUES (1, ?)",
                vec![codec::to_bytes(&src).into()],
            );
            if let Some(c) = h.core.borrow_mut().as_mut() {
                c.m.set_install(Some(src));
            }
        }
        Err(e) => note(h, format!("no install source at {to:?}: {e}")),
    }
}

fn source(h: &Rc<Host<Repo>>, through: Index) -> std::result::Result<InstallSource, String> {
    let sql = h.state.storage().sql();
    // Entry `through`'s term, digest, and time: from its stored row, or, on a member that installed at `through`, from
    // `log_base` and the archive that ends there.
    let (term, digest, at) = match crate::host::stored_entry::<Repo>(&sql, through) {
        Some(e) => (e.term, raft::digest(&e), e.at.0),
        None => {
            let q = "SELECT b.term, b.digest, a.to_at FROM log_base b JOIN archive_keys a ON a.to_idx = b.idx WHERE b.idx = ?";
            let r = rows(&sql, q, vec![i(through.0)]).map_err(|e| e.to_string())?;
            let r = r.first().ok_or("no stored entry and no installed base at through")?;
            let d: [u8; 32] = blob(&r[1]).map_err(|e| e.to_string())?.try_into().map_err(|_| "log_base digest")?;
            (raft::Term(int(&r[0]).map_err(|e| e.to_string())? as u64), raft::Digest(d), int(&r[2]).unwrap_or(0) as u64)
        }
    };
    let core = h.core.borrow();
    let c = core.as_ref().ok_or("not joined")?;
    let gf = c.m.group_fold();
    let snap = |t| git::tiers::snapshots(&*gf.fold.core, t).into_iter().rfind(|(to, _, _)| *to <= through);
    let cs = snap(Tier::Core).ok_or("no core snapshot at or below through")?;
    let ms = snap(Tier::Metadata);
    let low = cs.0.0.min(ms.as_ref().map_or(cs.0.0, |m| m.0.0));
    let cut = at.saturating_sub(gf.retention().as_millis() as u64);
    let q = "SELECT from_idx, to_idx, to_at, key FROM archive_keys WHERE to_idx <= ? ORDER BY from_idx";
    let mut archives: Vec<(u64, u64, String)> = Vec::new();
    for r in rows(&sql, q, vec![i(through.0)]).map_err(|e| e.to_string())? {
        let (from, to, at) = (int(&r[0]).unwrap_or(0) as u64, int(&r[1]).unwrap_or(0) as u64, int(&r[2]).unwrap_or(0));
        if to > low || at as u64 >= cut {
            archives.push((from, to, text(&r[3]).map_err(|e| e.to_string())?.into()));
        }
    }
    if archives.last().map(|a| a.1) != Some(through.0) || archives.first().is_none_or(|a| a.0 > low + 1) {
        return Err(format!("the archives do not cover {}..={}", low + 1, through.0));
    }
    if archives.windows(2).any(|p| p[1].0 != p[0].1 + 1) {
        return Err("the archives are not contiguous".into());
    }
    let config = gf.config().filter(|(i, _)| *i <= through).ok_or("a config committed after through")?.clone();
    // The id block as of `through`: the member's, unless a later entry still in the log changed it.
    let later = rows(&sql, "SELECT row FROM log WHERE idx > ? ORDER BY idx", vec![i(through.0)])
        .map_err(|e| e.to_string())?
        .iter()
        .filter_map(|r| codec::from_bytes::<Entry>(blob(&r[0]).ok()?).ok())
        .any(|e| matches!(e.body, EntryBody::IdBlock(_)));
    if later {
        return Err("an id block committed after through".into());
    }
    Ok(InstallSource {
        core: cs.2.key.to_string().into(),
        metadata: ms.map(|m| m.2.key.to_string().into()),
        archives: archives.into_iter().map(|a| a.2.into()).collect::<SmallVec<_>>(),
        through,
        through_term: term,
        through_digest: digest,
        config,
        id_block: gf.id_block(),
    })
}

/// The leader's archive object for `from..=to` of its stored log, put create-only in the first two buckets of the
/// group's record: the entries as the log stores them (`graftyard_fold::encode_archive_row`), so an installing member
/// applies them as they were committed. The edge's archive route records `Archived` with the key (BF-18, BF-29).
/// The whole object is built in memory, so a range is at most what an isolate holds [limit, not the design's].
#[cfg(feature = "test-faults")]
pub async fn archive_put(h: &Rc<Host<Repo>>, from: u64, to: u64) -> Result<Value> {
    let (chain, record, commit) = {
        let core = h.core.borrow();
        let c = core.as_ref().ok_or_else(|| err("not joined"))?;
        if c.m.role() != raft::Role::Leader {
            return Ok(json!({ "error": { "notLeader": crate::host::nl_json(&c.m.not_leader()) } }));
        }
        (c.join.chain, c.join.record.clone(), c.m.state().commit.0)
    };
    if from == 0 || to < from || to > commit {
        return Ok(json!({ "refused": format!("{from}..={to} is not a committed range (commit {commit})") }));
    }
    let sql = h.state.storage().sql();
    let mut buf = graftyard_fold::archive_header();
    let (mut first_at, mut last_at, mut next) = (0, 0, from);
    for r in sql.exec("SELECT idx, row FROM log WHERE idx >= ? AND idx <= ? ORDER BY idx", vec![i(from), i(to)])?.raw()
    {
        let r = r?;
        if int(&r[0])? as u64 != next {
            return Ok(json!({ "refused": format!("the stored log lacks entry {next}") }));
        }
        let b = blob(&r[1])?;
        let e: Entry = codec::from_bytes(b).map_err(|e| err(e.to_string()))?;
        if next == from {
            first_at = e.at.0;
        }
        last_at = e.at.0;
        graftyard_fold::encode_archive_row(b, &mut buf);
        next += 1;
    }
    if next != to + 1 {
        return Ok(json!({ "refused": format!("the stored log ends at {}", next - 1) }));
    }
    let digest = Digest(sha2::Sha256::digest(&buf).into());
    let key = git::R2Key::Archive { chain, from: Index(from), to: Index(to), digest };
    let mut put = Vec::new();
    for s in record.sites.iter().take(2) {
        let b = git::host::R2(bucket(&h.env, s.0)?);
        git::Bucket::put_new(&b, &key.to_string(), &buf, digest, &[]).await.map_err(|e| err(e.0))?;
        put.push(s.0);
    }
    Ok(
        json!({ "key": key.to_string(), "digest": digest.hex(), "fromAt": first_at, "toAt": last_at, "bytes": buf.len(), "sites": put }),
    )
}

// ---- routes ----

/// `/fold {"audit": {..}}`: the last report and the running job. With `start`, a job starts when none runs and the
/// last report is older than `AUDIT_EVERY_MS`, or at once with `force`; `deep` refolds the core tier from its oldest
/// snapshot.
pub fn audit_route(h: &Rc<Host<Repo>>, body: &Value) -> Result<Value> {
    let sql = h.state.storage().sql();
    let job = load_job(&sql)?;
    let last = rows(&sql, "SELECT at, row FROM load_report WHERE id = 1", vec![])?;
    let last = last.first().map(|r| Ok::<_, Error>((int(&r[0])? as u64, serde_json::from_str::<Value>(text(&r[1])?)?)));
    let last = last.transpose()?;
    let mut started = false;
    let due = last.as_ref().is_none_or(|(at, _)| now_ms() >= at + AUDIT_EVERY_MS);
    let force = body["force"].as_bool().unwrap_or(false);
    if body["cancel"].as_bool().unwrap_or(false) && job.as_ref().is_some_and(|j| !j.install()) {
        sql.exec("DELETE FROM load_job", None)?;
        reset_scratch(&sql, false)?;
    }
    let job = load_job(&sql)?;
    // The leader refolds, so the cron's lookup follows `notLeader` to it; any member answers a read or a cancel.
    let start = body["start"].as_bool().unwrap_or(false);
    if let Some(nl) = h.core.borrow().as_ref().filter(|c| c.m.role() != raft::Role::Leader).map(|c| c.m.not_leader())
        && start
    {
        return Ok(json!({ "error": { "notLeader": crate::host::nl_json(&nl) } }));
    }
    if start && job.is_none() && (due || force) {
        let j = audit_job(h, body["deep"].as_bool().unwrap_or(false))?;
        save_job(h, &j)?;
        h.arm_in(BUSY_MS);
        started = true;
    }
    Ok(json!({
        "last": last.map(|(_, v)| v),
        "running": load_job(&sql)?.map(|j| { let mut v = job_json(&j); v["kind"] = json!(if j.install() { "install" } else { "audit" }); v }),
        "started": started,
        "isolateMemMb": wasm_mem_mb(),
    }))
}

/// `/fold {"requestsAt": n}`: the `request` rows at or below `n`, counted and hashed, so BF-29 compares an installed
/// member's rows with the leader's.
pub fn requests_json(h: &Rc<Host<Repo>>, at: u64) -> Result<Value> {
    let sql = h.state.storage().sql();
    let mut sum = sha2::Sha256::new();
    let mut n = 0u64;
    for r in sql.exec("SELECT rid, idx FROM request WHERE idx <= ? ORDER BY rid", vec![i(at)])?.raw() {
        let r = r?;
        sum.update(format!("{} {}\n", text(&r[0])?, int(&r[1])?));
        n += 1;
    }
    Ok(json!({ "at": at, "rows": n, "digest": Digest(sum.finalize().into()).hex() }))
}
