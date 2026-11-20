//! One loader for a repository member's two tiers, shared by `InstallSnapshot` and the fold audit (`design/DESIGN.md`,
//! Shape, "Packs"; `design/SHARED.md`, "Folds", `fold_audit`; `decisions/DECISIONS.md`, "F48").
//!
//! - **A job.** Each tier from a committed snapshot in R2, then log entries applied to both tiers up to `through`.
//!   Entries come from the member's stored log, or from archive objects (`R2Key::Archive`,
//!   `graftyard_fold::encode_archive_row`) for what the log no longer holds.
//! - **Batches.** The member's alarm runs one batch at a time: at most `CHUNK` bytes per R2 range read, `BATCH_MS` and
//!   `BATCH_BYTES` per batch. Each batch's rows and the job's cursor commit in one transaction, so a reset object
//!   resumes where the last batch ended, and memory holds one batch, never the fold.
//! - **Install.** A follower or learner sent `InstallSnapshot` loads the named snapshots and archives into scratch
//!   tables (`crate::tier::SCRATCH`) beside the member's own, rebuilds the `request` rows of archived entries inside
//!   retention, and calls `Member::install`; its `Write::Install` swaps the scratch tables in, in the transaction that
//!   drops the log (`host::SqlStore`). The member then acks the round.
//! - **Source.** A member whose stored log was truncated after an archive (`Host::truncate`) names its install source
//!   (`set_source`): the newest committed snapshot of each tier at or below the archive's end, the archives from there
//!   or from `REQUEST_RETENTION` before it, and the config and id block there. Only a leader sends it.
//! - **Audit.** The fold audit refolds a chain from each tier's newest committed snapshot, or with `deep` the core tier
//!   from its oldest, through the member's `applied`, and reports each tier's `fold_digest` there; with `deep` it also
//!   checks the refold against every later core snapshot's digest on the way. `crate::fold_audit` compares the report
//!   with every voter's digest at that index. It writes no tier table (`decisions/DECISIONS.md`, "`refold-stream`"):
//!   each snapshot is checked against its trailer and staged as pages (`graftyard_fold::Pager`, `audit_page`), and the
//!   entries apply through a `graftyard_fold::SnapView` over them, whose changes each batch saves in `audit_overlay`.
//!   The job's end, its failure, and a restore with no audit running delete both tables' rows.

use std::rc::Rc;

use graftyard_fold::{
    self as fold, ARCHIVE_HEADER, Key, Pager, Restore, RestoreCursor, Schema, SnapView, SnapshotFrame, Store, Stored,
    Val, check_archive_header, decode_archive_some, decode_some, fold_digest, frames_header, key_bytes,
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
/// How long an install waits before it tries a failed batch again (`retry_batch`) [guess: past a transient R2 error].
const RETRY_MS: i64 = 5_000;
const BUSY_MS: i64 = 20;
/// The share of the room between the member's SQLite and its `SPLIT_AT` an audit's staged pages may take [guess]: the
/// pages are the snapshot's bytes, and a member the audit took past `SPLIT_AT` would record `MetaSplit`.
const STAGE_SHARE: u64 = 2;
/// An audit batch saves its `SnapView` changes once they pass this many rows [guess], so memory holds at most this
/// many changed rows whatever an entry or a batch rewrites.
const AUDIT_CHANGES: usize = 20_000;
/// How often the cron audits one chain [guess]: the design's daily digest comparison (`design/SHARED.md`, "Folds").
const AUDIT_EVERY_MS: u64 = 86_400_000;

#[derive(Clone, Debug, Serialize, Deserialize)]
struct TierLoad {
    /// The snapshot to start from, or none: the tier starts empty at index 0.
    key: Option<String>,
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
    /// An audit job this release made, which stages pages (`refold-stream`). One an older release left running loads
    /// scratch tables and is dropped, not resumed.
    #[serde(default)]
    paged: bool,
    /// An audit's staged pages and saved changes, the rows its refold wrote.
    #[serde(default)]
    pages: u64,
    #[serde(default)]
    saved: u64,
    /// The member's own metadata tables still to drop, one a batch, before the load starts (`drop_due`).
    #[serde(default)]
    drop: Vec<String>,
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

/// Deletes an audit's staged pages and saved changes (`graftyard_fold::AUDIT_CLEAR`).
fn clear_audit(sql: &SqlStorage) -> Result<()> {
    for q in fold::AUDIT_CLEAR {
        sql.exec(q, None)?;
    }
    Ok(())
}

/// Ends a job: its row goes, with an install's scratch tables or an audit's staged rows.
fn end_job(sql: &SqlStorage, install: bool) -> Result<()> {
    sql.exec("DELETE FROM load_job", None)?;
    if install { reset_scratch(sql, false) } else { clear_audit(sql) }
}

/// Drops an install's scratch tables, made again empty when `fresh`.
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
        "DELETE FROM live_run",
        "DELETE FROM load_job",
        "DELETE FROM load_want",
        "DELETE FROM meta_dropped",
    ] {
        stmts.push((q, vec![]));
    }
}

// ---- R2 ----

fn bucket(env: &Env, s: u8) -> Result<Bucket> {
    env.bucket(&format!("BUCKET_{s}"))
}

/// The site whose bucket holds `key`, this site's first, and the object's size.
pub(crate) async fn find(env: &Env, key: &str) -> Result<(u8, u64)> {
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
pub(crate) async fn read_whole(
    env: &Env,
    site: u8,
    key: &str,
    offset: u64,
    size: u64,
    start: usize,
) -> Result<Vec<u8>> {
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

/// The leader sent `InstallSnapshot`: start loading it, keep loading the running job (`next_job`), or ignore a late one.
/// The leader re-sends it every CALL_DEADLINE while the load runs; a repeat of the running one only wakes it. The
/// job answers the round it started for: if the leader has moved on, its next `InstallSnapshot` finds the member
/// holding `through` and is answered at once (`Member::on_install`).
pub fn want(h: &Rc<Host<Repo>>, w: InstallWanted) {
    let sql = h.state.storage().sql();
    let running = load_job(&sql).ok().flatten();
    let same = running.as_ref().is_some_and(|j| match &j.kind {
        Kind::Install { install, .. } => install.source == w.install.source,
        Kind::Audit { .. } => false,
    });
    // The newest request for the same source, from any term, is the one the finished load answers (`finish_install`).
    let newest = || -> Result<()> {
        let row = serde_json::to_string(&(w.from, &w.install))?;
        sql.exec("INSERT OR REPLACE INTO load_want (id, row) VALUES (1, ?)", vec![row.into()])?;
        Ok(())
    };
    // Whether the job takes this request's round (`newest`): not when it is a late delivery the job ignores.
    let start = || -> Result<bool> {
        let chain = h.core.borrow().as_ref().map(|c| c.join.chain).ok_or_else(|| err("not joined"))?;
        let job = match next_job(running.clone(), install_job(chain, &w)?) {
            Next::Ignore => return Ok(false),
            Next::Extend(j) => j,
            Next::Restart(j, why) => {
                if running.as_ref().is_some_and(Job::install) {
                    note(h, format!("install restarted: a source at {}: {why}", w.install.source.through.0));
                }
                j
            }
        };
        save_job(h, &job)?;
        Ok(true)
    };
    let r = if same { newest() } else { start().and_then(|take| if take { newest() } else { Ok(()) }) };
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
    Ok(TierLoad { key, to, site: None, size: 0, offset: 0, restore: None, restored: false })
}

fn install_job(chain: ChainId, w: &InstallWanted) -> Result<Job> {
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

/// What a new install source does to the running job (`next_job`).
enum Next {
    /// A late delivery from the job's own leader: the job, and the round it answers, stay as they are.
    Ignore,
    /// The running job, carried on to the new source's `through` and round.
    Extend(Job),
    /// The new source's job from the start, and why.
    Restart(Job, &'static str),
}

/// The job a new install source runs (`design/DESIGN.md`, Shape, `CHILD_GROUP_AT`, Installs). A running install keeps
/// the snapshots it started from: its snapshots plus the archives after them reach any later `through` with the same
/// fold a newer snapshot would give. On B0-30's short runs a restart at each new leader snapshot meant an install
/// under load never finished (G21, `tests/am1/results/b030-split.md`, "Phases (b030r5)").
fn next_job(running: Option<Job>, new: Job) -> Next {
    let Some(mut old) = running.filter(|j| j.install() && j.chain == new.chain) else {
        return Next::Restart(new, "no install runs");
    };
    if new.through < old.through {
        // The same leader in the same term and boot sends sources in `through` order, so an earlier one was queued
        // behind the job's (b030r5: 39 such at 06:54:48Z). Another leader's earlier `through` is a round the job
        // cannot answer.
        let round = |j: &Job| match &j.kind {
            Kind::Install { from, install } => Some((*from, install.term, install.boot)),
            Kind::Audit { .. } => None,
        };
        return if round(&old) == round(&new) {
            Next::Ignore
        } else {
            Next::Restart(new, "another leader names an earlier through")
        };
    }
    let end = old.archives.last().map_or(old.through, |a| a.to);
    let more: Vec<Arch> = new.archives.iter().filter(|a| a.to > end).cloned().collect();
    if more.first().is_some_and(|a| a.from != end + 1) || (more.is_empty() && new.through > end) {
        return Next::Restart(new, "its archives do not continue the job's");
    }
    old.archives.extend(more);
    old.through = new.through;
    old.kind = new.kind;
    Next::Extend(old)
}

/// Whether a failed batch of a job tried `tries` times, this one counted, is tried again: an install's, until
/// `BATCH_TRIES`. A failed read of R2 threw away a 9-minute install on b030r5 (07:57:38Z, "Network connection lost").
/// An audit ends at its first failure, as it did before.
fn retry_batch(install: bool, tries: u32) -> bool {
    install && tries < BATCH_TRIES
}

/// Whether an install drops the member's own metadata tables before it loads (`design/DESIGN.md`, Shape,
/// `CHILD_GROUP_AT`, Installs): only an install, and only on a member below the source's `through`, which its own tier
/// cannot serve: it was sent the install because the leader no longer holds its next entry. Its tier then holds nothing
/// the group needs from it, whether or not it applied `MetaDropped`, and keeping it beside the scratch load is what took
/// site c past `CHILD_GROUP_AT` on B0-30's rerun (5.70 GB own plus 4.48 GB of scratch) and to the platform's limit on
/// the short run, severed below `MetaDropped` (10.70 GB).
fn drop_due(install: bool, applied: u64, through: u64) -> bool {
    install && applied < through
}

/// Whether this member dropped its metadata tables for an install that has not finished: its tier is unloaded, it
/// serves no metadata read, it does not lead, and `crate::snap` replays nothing into it.
pub fn meta_dropped(sql: &SqlStorage) -> bool {
    rows(sql, "SELECT at FROM meta_dropped WHERE id = 1", vec![]).is_ok_and(|r| !r.is_empty())
}

fn unload_meta(h: &Rc<Host<Repo>>) {
    if let Some(c) = h.core.borrow().as_ref() {
        crate::tier::handle(&*c.m.group_fold().fold.meta).set_loaded(false);
    }
    crate::family::forget_dumps();
}

fn new_job(kind: Kind, chain: ChainId, through: u64, tiers: [TierLoad; 2], archives: Vec<Arch>, next: u64) -> Job {
    let paged = matches!(kind, Kind::Audit { .. });
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
        paged,
        pages: 0,
        saved: 0,
        drop: Vec::new(),
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
    let tiers: [TierLoad; 2] = tiers.try_into().map_err(|_| err("two tiers"))?;
    let next = tiers[0].to.min(tiers[1].to) + 1;
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
    let job = load_job(&sql).ok().flatten();
    if job.is_some() {
        h.arm_in(BUSY_MS);
    }
    if !keeps_audit_rows(job.as_ref())
        && let Err(e) = clear_audit(&sql)
    {
        note(h, format!("audit pages not cleared: {e}"));
    }
}

/// Whether a restored member keeps the staged pages and saved changes it finds: only while the audit that wrote them
/// still runs. Any others were left by an object reset between an audit's last batch and its end, or by a failed
/// clear, and no job will read them.
fn keeps_audit_rows(job: Option<&Job>) -> bool {
    job.is_some_and(|j| !j.install())
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
    if !install && !job.paged {
        note(h, "fold audit from an older release dropped: its scratch tables go, and the next run refolds".into());
        let _ = sql.exec("DELETE FROM load_job", None);
        let _ = reset_scratch(&sql, false);
        return;
    }
    let loaded = job.tiers.iter().all(|t| t.restored) && job.next > job.through;
    if job.tries >= BATCH_TRIES {
        let e = format!("{} batches began without committing: the isolate died under them", job.tries);
        note(h, format!("{} abandoned: {e}", if install { "install" } else { "fold audit" }));
        let _ = report(h, &json!({ "error": e, "job": job_json(&job), "at": now_ms() }));
        let _ = end_job(&sql, install);
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
        Err(e) if retry_batch(install, marked.tries) => {
            h.full(&e.to_string());
            note(h, format!("install batch failed ({} of {BATCH_TRIES}), retrying: {e}", marked.tries));
            h.arm_in(RETRY_MS);
        }
        Err(e) => {
            h.full(&e.to_string());
            note(h, format!("{} failed: {e}", if install { "install" } else { "fold audit" }));
            if !install {
                let _ = report(h, &json!({ "error": e.to_string(), "at": now_ms() }));
            }
            let _ = end_job(&sql, install);
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
        if job.install() {
            reset_scratch(sql, true)?;
        } else {
            clear_audit(sql)?;
        }
        job.prepared = true;
        let applied = h.core.borrow().as_ref().map_or(u64::MAX, |c| c.m.state().applied.0);
        let drop = drop_due(job.install(), applied, job.through);
        let mut stmts = Vec::new();
        if drop {
            // Marked in the job's own transaction, and unloaded before anything awaits, so no input applies to, reads,
            // or snapshots a tier whose tables then go.
            job.drop = crate::tier::live_tables(&git::tiers::METADATA);
            stmts.push(("INSERT OR REPLACE INTO meta_dropped (id, at) VALUES (1, ?)", vec![i(now_ms())]));
        }
        stmts.push(job_stmt(&job)?);
        commit_tx(&h.state.storage(), stmts).map_err(err)?;
        if drop {
            unload_meta(h);
            note(h, format!("install: dropping {} metadata tables before the load", job.drop.len()));
        }
        return Ok(true);
    }
    // One of the member's own metadata tables a batch, dropped and made again empty, so a read of it finds an empty
    // table, never a missing one.
    if let Some(t) = job.drop.pop() {
        let mut stmts: Vec<Stmt> = crate::tier::drop_live(&t).into_iter().map(|q| (q, vec![])).collect();
        stmts.push(job_stmt(&job)?);
        commit_tx(&h.state.storage(), stmts).map_err(err)?;
        return Ok(true);
    }
    let tiers = h.core.borrow().as_ref().map(|c| c.m.group_fold().fold.tiers()).ok_or_else(|| err("not joined"))?;
    let mut stmts: Vec<Stmt> = Vec::new();
    let mut bytes = 0u64;
    let rows0 = job.rows + job.entries;
    let more = |bytes: u64, job: &Job| {
        now_ms() - t0 < BATCH_MS && bytes < BATCH_BYTES && job.rows + job.entries - rows0 < BATCH_ROWS
    };
    if !job.install() {
        return audit_batch(h, sql, job, &tiers, t0, more).await;
    }
    let (mut core, mut meta) = crate::tier::scratch_tiers(sql, true)?;
    for t in 0..2 {
        while !job.tiers[t].restored && more(bytes, &job) {
            let store = if t == 0 { &mut core } else { &mut meta };
            let (frames, n) = read_frames(h, &mut job, t).await?;
            bytes += n;
            restore_frames(&mut job, t, store, &frames)?;
            // The member's Raft inputs run between reads, as between a snapshot writer's (`crate::snap`).
            Delay::from(std::time::Duration::ZERO).await;
        }
    }
    if job.tiers.iter().all(|t| t.restored) {
        while job.next <= job.through && more(bytes, &job) {
            let (es, n) = next_entries(h, sql, &mut job).await?;
            bytes += n;
            for e in es {
                let mut stores = [(Tier::Core, &mut core as &mut dyn Store), (Tier::Metadata, &mut meta)];
                apply_one(&tiers, &mut job, &mut stores, &e)?;
                install_requests(&e, &mut stmts);
            }
            Delay::from(std::time::Duration::ZERO).await;
        }
    }
    core.seal(&mut stmts);
    meta.seal(&mut stmts);
    let done = end_batch(&mut job, t0, bytes);
    stmts.push(job_stmt(&job)?);
    commit_tx(&h.state.storage(), stmts).map_err(err)?;
    core.flushed();
    meta.flushed();
    if !done {
        return Ok(true);
    }
    match job.kind.clone() {
        Kind::Install { from, install } => finish_install(h, sql, &job, &core, &meta, from, *install),
        Kind::Audit { .. } => Err(err("an audit job in the install path")),
    }
}

/// The batch's counters; whether the job is done.
fn end_batch(job: &mut Job, t0: u64, bytes: u64) -> bool {
    let ms = now_ms() - t0;
    job.tries = 0;
    job.batches += 1;
    job.batch_max_ms = job.batch_max_ms.max(ms);
    job.bytes += bytes;
    job.mem_max_mb = job.mem_max_mb.max(wasm_mem_mb());
    job.tiers.iter().all(|t| t.restored) && job.next > job.through
}

/// One batch of an audit (`decisions/DECISIONS.md`, "`refold-stream`"): snapshot frames checked and staged as
/// `audit_page` rows, then entries applied through a `SnapView` per tier over the staged pages and the saved changes.
/// The batch's pages, its changes, and the job's cursor commit in one transaction, so a reset object resumes at the
/// last batch and no change of an entry is kept unless the batch that applied it committed.
async fn audit_batch(
    h: &Rc<Host<Repo>>,
    sql: &SqlStorage,
    mut job: Job,
    tiers: &git::tiers::RepoTiers,
    t0: u64,
    more: impl Fn(u64, &Job) -> bool,
) -> Result<bool> {
    let mut stmts: Vec<Stmt> = Vec::new();
    let mut bytes = 0u64;
    let mut pages = 0u64;
    if !job.tiers.iter().any(|t| t.restored || t.offset > 0) {
        for tl in job.tiers.iter_mut() {
            if let (Some(key), None) = (tl.key.clone(), tl.site) {
                let (s, size) = find(&h.env, &key).await?;
                (tl.site, tl.size) = (Some(s), size);
            }
        }
        let need: u64 = job.tiers.iter().map(|t| t.size).sum();
        let db = crate::split::db_bytes(h).max(0.0) as u64;
        let split_at = crate::split::split_at(job.chain);
        if !stage_fits(db, need, split_at) {
            return Err(err(format!(
                "too large to stage: {need} bytes of snapshot on a member of {db} bytes, SPLIT_AT {split_at} (F52)"
            )));
        }
    }
    for t in 0..2 {
        while !job.tiers[t].restored && more(bytes, &job) {
            let (frames, n) = read_frames(h, &mut job, t).await?;
            bytes += n;
            let mut pager = Pager::new(fold::PAGE_BYTES);
            restore_frames(&mut job, t, &mut pager, &frames)?;
            for p in pager.take() {
                pages += 1;
                stmts.push((
                    fold::PAGE_PUT,
                    vec![i(t as u64), p.table.to_string().into(), p.last.into(), p.first.into(), p.rows.into()],
                ));
            }
            if let Some(m) = pager.meta() {
                stmts.push(saved_stmt(t, fold::META, &[], Some(m)));
            }
            Delay::from(std::time::Duration::ZERO).await;
        }
    }
    let mut saved = 0u64;
    // Entries apply in a batch that staged nothing, so the views read only committed pages, and a batch that dies
    // after its stage leaves the job's `tries` mark in place.
    if job.tiers.iter().all(|t| t.restored) && stmts.is_empty() {
        let staged = [SqlStaged::new(sql, 0), SqlStaged::new(sql, 1)];
        let mut views = [SnapView::new(&staged[0], fold::CACHE_PAGES), SnapView::new(&staged[1], fold::CACHE_PAGES)];
        while job.next <= job.through && more(bytes, &job) && views.iter().all(|v| v.changed() < AUDIT_CHANGES) {
            let (es, n) = next_entries(h, sql, &mut job).await?;
            bytes += n;
            for e in es {
                let [c, m] = &mut views;
                let mut stores = [(Tier::Core, c as &mut dyn Store), (Tier::Metadata, m)];
                apply_one(tiers, &mut job, &mut stores, &e)?;
            }
            if let Some(e) = staged.iter().find_map(|s| s.lost.borrow().clone()) {
                return Err(err(format!("audit pages unreadable: {e}")));
            }
            Delay::from(std::time::Duration::ZERO).await;
        }
        for (t, v) in views.iter_mut().enumerate() {
            for ((table, key), row) in v.take() {
                saved += 1;
                stmts.push(saved_stmt(t, &table, &key, row.as_ref()));
            }
        }
    }
    job.pages += pages;
    job.saved += saved;
    let done = end_batch(&mut job, t0, bytes);
    stmts.push(job_stmt(&job)?);
    commit_tx(&h.state.storage(), stmts).map_err(err)?;
    if !done {
        return Ok(true);
    }
    finish_audit(h, sql, &job)?;
    Ok(false)
}

/// Whether an audit may stage `need` bytes of snapshot pages on a member of `db` bytes: within `1 / STAGE_SHARE` of the
/// room left below `split_at`, so staging never starts a split.
fn stage_fits(db: u64, need: u64, split_at: u64) -> bool {
    need <= split_at.saturating_sub(db) / STAGE_SHARE
}

fn saved_stmt(t: usize, table: &str, key: &[Val], row: Option<&Stored>) -> Stmt {
    let r = postcard::to_allocvec(&(key, row)).unwrap_or_default();
    (fold::SAVED_PUT, vec![i(t as u64), table.to_string().into(), key_bytes(key).into(), r.into()])
}

/// One tier's staged pages and saved changes in the member's SQLite. A read SQLite refuses is kept in `lost` and
/// answers nothing; the batch checks `lost` after each read of entries and fails before it commits.
struct SqlStaged {
    sql: SqlStorage,
    tier: u64,
    lost: std::cell::RefCell<Option<String>>,
}

impl SqlStaged {
    fn new(sql: &SqlStorage, tier: u64) -> SqlStaged {
        SqlStaged { sql: sql.clone(), tier, lost: std::cell::RefCell::new(None) }
    }

    fn q(&self, q: &str, mut args: Vec<SqlStorageValue>) -> Vec<Vec<SqlStorageValue>> {
        args.insert(0, i(self.tier));
        match rows(&self.sql, q, args) {
            Ok(r) => r,
            Err(e) => {
                self.lost.borrow_mut().get_or_insert(e.to_string());
                Vec::new()
            }
        }
    }

    fn decode<T: serde::de::DeserializeOwned>(&self, v: &SqlStorageValue) -> Option<T> {
        match blob(v).ok().and_then(|b| postcard::from_bytes(b).ok()) {
            Some(x) => Some(x),
            None => {
                self.lost.borrow_mut().get_or_insert("a staged row does not decode".into());
                None
            }
        }
    }
}

impl fold::Staged for SqlStaged {
    fn page(&self, table: &str, from: &[u8]) -> Option<fold::PageAt> {
        let r = self.q(fold::PAGE_FIND, vec![table.into(), from.to_vec().into()]);
        let r = r.first()?;
        Some(fold::PageAt { first: blob(&r[0]).ok()?.to_vec(), last: blob(&r[1]).ok()?.to_vec() })
    }
    fn rows(&self, table: &str, last: &[u8]) -> Vec<fold::Row> {
        let r = self.q(fold::PAGE_ROWS, vec![table.into(), last.to_vec().into()]);
        r.first().and_then(|r| self.decode(&r[0])).unwrap_or_default()
    }
    fn saved(&self, table: &str, key: &[Val]) -> Option<Option<Stored>> {
        let r = self.q(fold::SAVED_GET, vec![table.into(), key_bytes(key).into()]);
        let (_, row): (Key, Option<Stored>) = self.decode(&r.first()?[0])?;
        Some(row)
    }
    fn saved_scan(&self, table: &str, from: &[Val], limit: usize) -> Vec<(Key, Option<Stored>)> {
        let n = limit.min(i64::MAX as usize) as u64;
        let r = self.q(fold::SAVED_SCAN, vec![table.into(), key_bytes(from).into(), i(n)]);
        r.iter().filter_map(|r| self.decode(&r[0])).collect()
    }
}

/// The next whole frames of tier `t`'s snapshot from its offset, and the bytes read.
async fn read_frames(h: &Rc<Host<Repo>>, job: &mut Job, t: usize) -> Result<(Vec<SnapshotFrame>, u64)> {
    let tl = &mut job.tiers[t];
    let Some(key) = tl.key.clone() else { return Ok((Vec::new(), 0)) };
    if tl.site.is_none() {
        let (s, size) = find(&h.env, &key).await?;
        (tl.site, tl.size) = (Some(s), size);
    }
    let site = tl.site.ok_or_else(|| err("found"))?;
    let start = if tl.offset == 0 { frames_header().len() } else { 0 };
    let b = read_whole(&h.env, site, &key, tl.offset, tl.size, start).await?;
    if start > 0 && b.get(..start) != Some(&frames_header()[..]) {
        return Err(err(format!("{key} is not a snapshot frame stream")));
    }
    let (frames, used) = decode_some(&b[start..]).map_err(|e| err(e.to_string()))?;
    if frames.is_empty() {
        return Err(err(format!("{key}: no whole frame at byte {}", tl.offset)));
    }
    tl.offset += (start + used) as u64;
    Ok((frames, (start + used) as u64))
}

/// Frames of tier `t`'s snapshot into `store` through `Restore`, which checks them against the trailer: an install's
/// scratch tables, or an audit's `Pager`. With no snapshot the tier starts empty at 0.
fn restore_frames(job: &mut Job, t: usize, store: &mut dyn Store, frames: &[SnapshotFrame]) -> Result<()> {
    let chain = job.chain;
    let tl = &mut job.tiers[t];
    if tl.key.is_none() {
        tl.restored = true;
        return Ok(());
    }
    let s = schema(t);
    let mut r = match tl.restore.take() {
        Some(c) => Restore::resume(s, c),
        None => Restore::begin(store, s, (chain, Index(tl.to))).map_err(|e| err(e.to_string()))?,
    };
    let mut ended = false;
    for f in frames {
        r.frame(store, f).map_err(|e| err(e.to_string()))?;
        match f {
            SnapshotFrame::Rows { rows, .. } => job.rows += rows.len() as u64,
            SnapshotFrame::End(_) => ended = true,
        }
    }
    let tl = &mut job.tiers[t];
    if ended {
        r.finish_rows(store).map_err(|e| err(e.to_string()))?;
        tl.restored = true;
    } else {
        tl.restore = Some(r.cursor());
    }
    Ok(())
}

/// The next entries from `job.next`: from the stored log while it holds them (an audit), else from the archives. The
/// entries and the bytes read.
async fn next_entries(h: &Rc<Host<Repo>>, sql: &SqlStorage, job: &mut Job) -> Result<(Vec<Entry>, u64)> {
    let mut out = Vec::new();
    let mut bytes = 0u64;
    if !job.install() {
        // A page read to its end: a cursor left open holds the log table, and the job's last `DROP TABLE` of its
        // scratch tables then fails with SQLITE_LOCKED.
        let q = "SELECT idx, row FROM log WHERE idx >= ? AND idx <= ? ORDER BY idx LIMIT 8";
        for r in rows(sql, q, vec![i(job.next), i(job.through)])? {
            let idx = int(&r[0])? as u64;
            if idx != job.next + out.len() as u64 {
                break;
            }
            let b = blob(&r[1])?;
            bytes += b.len() as u64;
            out.push(codec::from_bytes::<Entry>(b).map_err(|e| err(e.to_string()))?);
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
    stores: &mut [(Tier, &mut dyn Store); 2],
    e: &Entry,
) -> Result<()> {
    if e.index.0 != job.next {
        return Err(err(format!("entry {:?} where {} was next", e.index, job.next)));
    }
    for (t, store) in stores.iter_mut() {
        let applied = fold::try_meta(&**store).map_err(|x| err(x.to_string()))?.applied;
        if e.index.0 == applied.0 + 1 {
            fold::apply(tiers, &mut **store, *t, e).map_err(|x| err(format!("{t:?} tier at {:?}: {x}", e.index)))?;
        } else if e.index > applied {
            return Err(err(format!("{t:?} tier at {applied:?} cannot apply {:?}", e.index)));
        }
    }
    if e.index.0 == job.through {
        job.through_at = Some(e.at.0);
    }
    if let Some(&(to, want)) = job.checks.iter().find(|c| c.0 == e.index.0) {
        let got = fold_digest(&*stores[0].1, Tier::Core);
        job.segments.push(json!({ "to": to, "want": want.hex(), "got": got.hex(), "ok": got == want }));
    }
    job.entries += 1;
    job.next += 1;
    Ok(())
}

/// An install keeps every archived request row; the ones past retention of `through` go at the end.
fn install_requests(e: &Entry, stmts: &mut Vec<Stmt>) {
    if let EntryBody::Events(evs) = &e.body {
        for (rid, r) in &e.requests {
            let row: Vec<git::repo::RepoEvent> =
                evs[r.start as usize..r.end as usize].iter().map(git::repo::RepoFold::row_event).collect();
            stmts.push((
                "INSERT OR IGNORE INTO i_request (rid, idx, at, events) VALUES (?, ?, ?, ?)",
                vec![rid.to_string().into(), i(e.index.0), i(e.at.0), codec::to_bytes(&row).into()],
            ));
        }
    }
}

fn job_json(job: &Job) -> Value {
    json!({
        "through": job.through,
        "next": job.next,
        "tiers": job.tiers.iter().map(|t| json!({ "key": t.key, "to": t.to, "offset": t.offset, "size": t.size, "restored": t.restored })).collect::<Vec<_>>(),
        "pages": job.pages,
        "saved": job.saved,
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

fn finish_audit(h: &Rc<Host<Repo>>, sql: &SqlStorage, job: &Job) -> Result<()> {
    let deep = matches!(job.kind, Kind::Audit { deep: true });
    let staged = [SqlStaged::new(sql, 0), SqlStaged::new(sql, 1)];
    let d = |t: usize| fold_digest(&SnapView::new(&staged[t], 1), tier_of(t)).hex();
    let mut v = job_json(job);
    v["kind"] = json!("audit");
    v["deep"] = json!(deep);
    v["index"] = json!(job.through);
    v["core"] = json!(d(0));
    v["metadata"] = json!(d(1));
    if let Some(e) = staged.iter().find_map(|s| s.lost.borrow().clone()) {
        return Err(err(format!("audit pages unreadable: {e}")));
    }
    v["at"] = json!(now_ms());
    commit_tx(&h.state.storage(), finish_stmts(&v, now_ms())).map_err(err)?;
    console_log!("fold audit refold {v}");
    Ok(())
}

/// An audit's end, in one transaction: its report, and its job, staged pages and saved changes gone.
fn finish_stmts(report: &Value, at: u64) -> Vec<Stmt> {
    let mut stmts: Vec<Stmt> = vec![("DELETE FROM load_job", vec![])];
    stmts.extend(fold::AUDIT_CLEAR.iter().map(|q| (*q, vec![])));
    stmts.push((
        "INSERT OR REPLACE INTO load_report (id, at, row) VALUES (1, ?, ?)",
        vec![i(at), report.to_string().into()],
    ));
    stmts
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
    let retention = crate::host::request_retention(&h.env, true);
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
    // The feed's ring restarts at `through` with the leases the loaded tier holds there, so a cursor from before gets
    // `gone` and a lease the next entry releases reports `Released`. Read here, from the loaded tables: the install's
    // own input, which the feed also sees (`Repo::stepped`), runs before its table swap is flushed and would read the
    // member's tables from before the install (LF-13 on graft-m3c: one `Released` missing on F in each run).
    let fed = h
        .core
        .borrow()
        .as_ref()
        .and_then(|c| c.m.group_fold().fold.repo.map(|r| (r, crate::feed::of(&c.join.me.do_name, c.m.boot()))));
    if let Some((repo, f)) = fed {
        let mut f = f.borrow_mut();
        if let Some(owner) = f.owner.clone() {
            f.ring.skip(repo, &owner, core, (through, graftyard_types::Timestamp(at)));
        }
    }
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
            let mut stmts: Vec<Stmt> = vec![("DELETE FROM load_job", vec![])];
            // The member caught up by AppendEntries from a leader that still stored its next entries, after it dropped
            // its own metadata tables: the loaded tier at `through` becomes its tier, still unloaded, and
            // `crate::snap::load_meta` replays the stored log above `through` into it before it serves.
            if meta_dropped(sql) {
                stmts.extend(crate::tier::swap_in(&git::tiers::METADATA).into_iter().map(|q| (q, vec![])));
                stmts.push(("DELETE FROM meta_dropped", vec![]));
                v["metadataKept"] = json!(true);
            }
            commit_tx(&h.state.storage(), stmts).map_err(err)?;
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
    // The id block as of `through`: the member's, unless a later entry still in the log changed it. An `IdBlock`
    // entry carries no events, so only small rows are read: reading every row above `through` into memory took a
    // leader with 120 MB of log past its isolate's memory on the first archive (B0-30), and `LOG_MAX_BYTES` is 2 GiB.
    let q = "SELECT row FROM log WHERE idx > ? AND length(row) < 4096 ORDER BY idx";
    let later = rows(&sql, q, vec![i(through.0)])
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

/// Whether an entry above `ok` may have written a tier beyond its meta row: one with events or an id block, or one
/// that extends leases after a stall. A no-op or a config entry runs only `on_entry`'s time-based pruning. With `ok`
/// below the entries held in memory (`base`), the member cannot tell, and answers that the chain moved.
pub fn moved<E>(ok: Index, base: Index, log: &[raft::LogEntry<E>]) -> bool {
    ok < base
        || log.iter().any(|e| {
            e.index > ok && (e.extend.is_some() || matches!(e.body, EntryBody::Events(_) | EntryBody::IdBlock(_)))
        })
}

// ---- routes ----

/// `/fold {"audit": {..}}`: the last report and the running job. With `start`, a job starts when none runs and the
/// last report is older than `AUDIT_EVERY_MS`, or at once with `force`; `deep` refolds the core tier from its oldest
/// snapshot.
///
/// Any member also answers, for the cron's skip (`crate::fold_audit`, `decisions/DECISIONS.md`, "audit-moved"),
/// `deleted`, whether its fold holds the repository's tombstone, and `seen`, what the cron last marked on it with
/// `{"seen": {..}}`; `{"seen": null}` clears it. A tombstoned chain starts no refold without `force`.
pub fn audit_route(h: &Rc<Host<Repo>>, body: &Value) -> Result<Value> {
    let sql = h.state.storage().sql();
    match body.get("seen") {
        Some(Value::Null) => {
            sql.exec("DELETE FROM audit_seen", None)?;
        }
        Some(v) => {
            sql.exec("INSERT OR REPLACE INTO audit_seen (id, row) VALUES (1, ?)", vec![v.to_string().into()])?;
        }
        None => {}
    }
    let job = load_job(&sql)?;
    let last = rows(&sql, "SELECT at, row FROM load_report WHERE id = 1", vec![])?;
    let last = last.first().map(|r| Ok::<_, Error>((int(&r[0])? as u64, serde_json::from_str::<Value>(text(&r[1])?)?)));
    let last = last.transpose()?;
    let mut started = false;
    let due = last.as_ref().is_none_or(|(at, _)| now_ms() >= at + AUDIT_EVERY_MS);
    let force = body["force"].as_bool().unwrap_or(false);
    if body["cancel"].as_bool().unwrap_or(false) && job.as_ref().is_some_and(|j| !j.install()) {
        end_job(&sql, false)?;
    }
    let job = load_job(&sql)?;
    // The leader refolds, so the cron's lookup follows `notLeader` to it; any member answers a read or a cancel.
    let start = body["start"].as_bool().unwrap_or(false);
    if let Some(nl) = h.core.borrow().as_ref().filter(|c| c.m.role() != raft::Role::Leader).map(|c| c.m.not_leader())
        && start
    {
        return Ok(json!({ "error": { "notLeader": crate::host::nl_json(&nl) } }));
    }
    // A tombstoned repository keeps its rows only until its purge, and nothing writes them: no refold until then.
    let deleted = h.core.borrow().as_ref().is_some_and(|c| git::purge::purgeable(&c.m.group_fold().fold));
    if start && job.is_none() && ((due && !deleted) || force) {
        let j = audit_job(h, body["deep"].as_bool().unwrap_or(false))?;
        save_job(h, &j)?;
        h.arm_in(BUSY_MS);
        started = true;
    }
    let seen = rows(&sql, "SELECT row FROM audit_seen WHERE id = 1", vec![])?;
    let seen = seen.first().map(|r| serde_json::from_str::<Value>(text(&r[0])?).map_err(Error::from)).transpose()?;
    Ok(json!({
        "deleted": deleted,
        "seen": seen,
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

#[cfg(test)]
mod tests {
    use graftyard_types::{ChainId, DbId, RepoId, SiteId};

    use super::{Arch, Kind, Next, TierLoad, next_job, tier_load};

    const CHAIN: ChainId = ChainId::Repo(RepoId(DbId(1048590)));

    fn arch(from: u64, to: u64) -> Arch {
        Arch { from, to, key: format!("archive/{from}-{to}"), site: None, size: 0 }
    }

    /// An install job as `install_job` builds it from a source: the snapshot keys, `through`, and its archives.
    fn job(core: &str, meta: Option<&str>, through: u64, archives: Vec<Arch>) -> super::Job {
        let tiers: [TierLoad; 2] = [tier_load(Some(core.into())).unwrap(), tier_load(meta.map(Into::into)).unwrap()];
        let next = archives.first().map_or(through + 1, |a| a.from);
        super::Job {
            kind: Kind::Install { from: SiteId(0), install: Box::new(fake_install(through)) },
            chain: CHAIN,
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
            began_ms: 0,
            tries: 0,
            batches: 0,
            batch_max_ms: 0,
            bytes: 0,
            rows: 0,
            entries: 0,
            mem_max_mb: 0.0,
            paged: false,
            pages: 0,
            saved: 0,
            drop: Vec::new(),
        }
    }

    /// An install job whose source runs through `through`, with no archives.
    pub(super) fn job_for(through: u64) -> super::Job {
        job(&core(), Some(&meta()), through, vec![])
    }

    fn fake_install(through: u64) -> graftyard_raft::InstallSnapshot {
        use graftyard_raft::{Digest, GroupConfig, Index, InstallSnapshot, InstallSource, Term};
        use graftyard_types::Timestamp;
        let p: graftyard_types::Placement =
            serde_json::from_value(serde_json::json!({ "voters": [graftyard_types::Member::new(CHAIN, SiteId(0), graftyard_types::MemberKind::Full, 0, 1)], "learners": [], "home": 0, "generation": 1 }))
                .unwrap();
        InstallSnapshot {
            term: Term(1),
            boot: 0,
            round: 0,
            sent_at: Timestamp(0),
            lead_seq: 0,
            source: InstallSource {
                core: "c".into(),
                metadata: None,
                archives: Default::default(),
                through: Index(through),
                through_term: Term(1),
                through_digest: Digest([0; 32]),
                config: (Index(1), GroupConfig::Normal(Box::new(p))),
                id_block: None,
            },
        }
    }

    fn snap(tier: graftyard_types::Tier, to: u64, d: u8) -> String {
        let (from, to, digest) =
            (graftyard_types::Index(0), graftyard_types::Index(to), graftyard_types::Digest([d; 32]));
        graftyard_git::R2Key::Snapshot { chain: CHAIN, tier, from, to, digest }.to_string()
    }
    fn core() -> String {
        snap(graftyard_types::Tier::Core, 200_000, 1)
    }
    fn meta() -> String {
        snap(graftyard_types::Tier::Metadata, 159_387, 2)
    }
    fn meta2() -> String {
        snap(graftyard_types::Tier::Metadata, 201_500, 3)
    }

    /// The job a source from leader `from` in `term` builds.
    fn from(mut j: super::Job, from: u8, term: u64) -> super::Job {
        if let Kind::Install { from: f, install } = &mut j.kind {
            *f = SiteId(from);
            install.term = graftyard_raft::Term(term);
        }
        j
    }

    fn extended(n: Next) -> super::Job {
        match n {
            Next::Extend(j) => j,
            Next::Ignore => panic!("ignored"),
            Next::Restart(_, why) => panic!("restarted: {why}"),
        }
    }

    fn restarted(n: Next) -> (super::Job, &'static str) {
        match n {
            Next::Restart(j, why) => (j, why),
            _ => panic!("not restarted"),
        }
    }

    /// G21: b030r5's site c restarted at each new leader snapshot and never finished under load. A newer source that
    /// names other core and metadata snapshots, from another leader, extends the running job: its scratch tables,
    /// offsets and pinned snapshot keys stay, it gains only the archives past its own, and it answers the newer round.
    #[test]
    fn a_newer_snapshot_mid_install_extends_the_pinned_job() {
        let mut running = job(&core(), Some(&meta()), 200_121, vec![arch(159_388, 199_900), arch(199_901, 200_121)]);
        running.prepared = true;
        running.tiers[1].offset = 2_740_000_000;
        running.batches = 1_517;
        let newer = from(
            job(
                &snap(graftyard_types::Tier::Core, 200_900, 4),
                Some(&meta2()),
                200_984,
                vec![arch(199_901, 200_121), arch(200_122, 200_519), arch(200_520, 200_984)],
            ),
            1,
            2,
        );
        let j = extended(next_job(Some(running), newer));
        assert!(j.prepared, "the scratch tables stay");
        assert_eq!((j.tiers[1].offset, j.batches), (2_740_000_000, 1_517), "the load's progress stays");
        assert_eq!([j.tiers[0].key.clone(), j.tiers[1].key.clone()], [Some(core()), Some(meta())], "pinned");
        assert_eq!(j.through, 200_984);
        let ranges: Vec<_> = j.archives.iter().map(|a| (a.from, a.to)).collect();
        assert_eq!(ranges, [(159_388, 199_900), (199_901, 200_121), (200_122, 200_519), (200_520, 200_984)]);
        match &j.kind {
            Kind::Install { from, install } => {
                assert_eq!((from.0, install.term.0, install.source.through.0), (1, 2, 200_984), "the newer round")
            }
            Kind::Audit { .. } => panic!(),
        }
    }

    /// G21: b030r5's 39 restarts at 06:54:48Z were deliveries from the job's own leader queued behind it, `through`
    /// 5,607 then 5,623 then 5,603. An earlier `through` from the same leader, term and boot changes nothing.
    #[test]
    fn a_late_source_from_the_same_leader_changes_nothing() {
        let running = || {
            let mut j = job(&core(), Some(&meta()), 200_121, vec![arch(159_388, 200_121)]);
            j.prepared = true;
            j
        };
        let late = job(&core(), Some(&meta()), 200_000, vec![arch(159_388, 200_000)]);
        assert!(matches!(next_job(Some(running()), late), Next::Ignore));
        let older_snapshots = job(&snap(graftyard_types::Tier::Core, 150_000, 5), None, 199_999, vec![]);
        assert!(matches!(next_job(Some(running()), older_snapshots), Next::Ignore), "whatever it names");
    }

    /// The restarts left: no install runs, the newer archives leave a gap after the job's or none reaches `through`, or
    /// another leader or term names an earlier `through`, a round the job cannot answer. Each starts the new source's
    /// job from fresh scratch tables.
    #[test]
    fn only_a_gap_or_another_leaders_earlier_source_restarts() {
        let running = || {
            let mut j = job(&core(), Some(&meta()), 200_121, vec![arch(159_388, 200_121)]);
            j.prepared = true;
            j
        };
        let cases = [
            ("a gap", job(&core(), Some(&meta()), 201_600, vec![arch(200_500, 201_600)]), "do not continue"),
            (
                "no archive to through",
                job(&core(), Some(&meta()), 201_600, vec![arch(159_388, 200_121)]),
                "do not continue",
            ),
            ("another leader", from(job(&core(), Some(&meta()), 200_000, vec![]), 2, 1), "earlier through"),
            ("another term", from(job(&core(), Some(&meta()), 200_000, vec![]), 0, 3), "earlier through"),
        ];
        for (case, new, why) in cases {
            let (j, said) = restarted(next_job(Some(running()), new));
            assert!(said.contains(why), "{case}: {said}");
            assert!(!j.prepared, "{case}: a restart starts from fresh scratch tables");
        }
        let (_, said) = restarted(next_job(None, job(&core(), Some(&meta()), 1, vec![])));
        assert_eq!(said, "no install runs");
    }

    /// `refold-stream`: no staged page or saved change outlives its audit. The audit's end deletes both in the
    /// transaction that writes its report; a restored member keeps them only while an audit job still runs, so an
    /// object reset between an audit's last batch and its end, or a job that failed, leaves none behind.
    #[test]
    fn an_audits_staged_rows_go_at_its_end_and_on_a_restore_without_it() {
        let stmts = super::finish_stmts(&serde_json::json!({ "kind": "audit" }), 1);
        let qs: Vec<&str> = stmts.iter().map(|(q, _)| *q).collect();
        for q in graftyard_fold::AUDIT_CLEAR {
            assert!(qs.contains(&q), "the end's transaction runs {q}");
        }
        assert!(qs.contains(&"DELETE FROM load_job") && qs.iter().any(|q| q.contains("load_report")));
        let mut audit = job_for(10);
        audit.kind = Kind::Audit { deep: false };
        assert!(super::keeps_audit_rows(Some(&audit)), "a running audit resumes on its pages");
        assert!(!super::keeps_audit_rows(None), "no job: the pages are left over");
        assert!(!super::keeps_audit_rows(Some(&job_for(10))), "an install's job reads no audit page");
    }

    /// An audit stages pages only within half the room left below `SPLIT_AT`, so its own pages never start a split.
    /// BF-20's 2.4 GB metadata snapshot on its 4.5 GB member of 7 GiB is still too large (F52); `repo:65536`'s 31.6 MB
    /// core snapshot on a small member fits.
    #[test]
    fn an_audit_stages_only_within_half_the_room_below_split_at() {
        let split_at = graftyard_git::split::SPLIT_AT;
        assert!(super::stage_fits(4_000_000, 31_600_000, split_at));
        assert!(!super::stage_fits(4_500_000_000, 2_400_000_000, split_at));
        assert!(super::stage_fits(0, split_at / 2, split_at) && !super::stage_fits(0, split_at / 2 + 1, split_at));
        assert!(!super::stage_fits(split_at + 1, 1, split_at), "a member past SPLIT_AT stages nothing");
    }

    /// G21: one "Network connection lost" at 07:57:38Z threw away b030r5's 9-minute install. A failed install batch is
    /// tried again until its `BATCH_TRIES`th failure in a row; an audit's first failure ends it.
    #[test]
    fn a_failed_install_batch_is_retried_before_it_is_abandoned() {
        assert!(super::retry_batch(true, 1) && super::retry_batch(true, super::BATCH_TRIES - 1));
        assert!(!super::retry_batch(true, super::BATCH_TRIES), "abandoned on the last try");
        assert!(!super::retry_batch(false, 1), "an audit");
    }
}

/// (i-3)'s pure rules: which install drops the member's own metadata tables, that a member with them dropped is never
/// loaded, and that the fold audit compares no digest of an unloaded tier.
#[cfg(test)]
mod drop_tests {
    use graftyard_fold::{MemStore, Store, TierStore};
    use graftyard_git::repo::RepoFold;
    use graftyard_raft::{EntryBody, Fold as _, LogEntry, PREV_ZERO, Term, TierSet};
    use graftyard_types::{AccountId, DbId, Index, RepoId, Timestamp};

    use super::drop_due;

    /// Every install on a member below the source's `through` drops its tier, whether or not the member applied
    /// `MetaDropped`: an audit refolds beside a tier it must keep, and a member at or past `through` was not sent the
    /// install for want of entries.
    #[test]
    fn every_install_below_through_drops_its_tier() {
        assert!(drop_due(true, 199_808, 200_984), "B0-30's second-run site c, past MetaDropped");
        assert!(drop_due(true, 6_889, 9_120), "b030r4's site c, severed below MetaDropped");
        assert!(!drop_due(false, 199_808, 200_984), "an audit");
        assert!(!drop_due(true, 200_984, 200_984), "at through");
        assert!(!drop_due(true, 201_000, 200_984), "past through");
    }

    /// An install abandoned after the drop (b030r4's site c: the platform's size limit killed three batches, the
    /// object reset) leaves the member marked: unloaded on restore, serving no metadata or narrow `api` read, barred
    /// from leading. The next source starts a fresh job, which drops again (the drop is idempotent) and whose
    /// `Write::Install` swaps the loaded tier in and clears the mark.
    #[test]
    fn an_install_abandoned_after_the_drop_stays_unloaded_and_the_next_one_finishes() {
        use crate::family::{follower_query, meta_loaded};
        let f = fold_at_1(true);
        // The abandon deletes the job and drops only scratch tables (`reset_scratch`), so the mark, which bars a
        // campaign, `keep_led`, and a transfer (`Family::may_lead`), stays, and so do the emptied live tables.
        for schema in [graftyard_git::tiers::CORE, graftyard_git::tiers::METADATA] {
            for q in crate::tier::drop_scratch(&schema) {
                assert!(q.starts_with("DROP TABLE IF EXISTS \"i") || q.starts_with("DROP TABLE IF EXISTS i"), "{q}");
                assert!(!q.contains("meta_dropped"), "{q}");
            }
        }
        assert!(!meta_loaded(&f, true, true), "unloaded on restore, though its meta row stands where the core's does");
        for q in [serde_json::json!({ "rows": true }), serde_json::json!({ "rows": { "api": { "issues": {} } } })] {
            assert!(!follower_query(&q, false), "{q}: no metadata read from the half-empty tier");
        }
        assert!(follower_query(&serde_json::json!({ "rows": { "access": {} } }), false), "core reads still served");
        // The next install starts from fresh scratch tables and drops again.
        let next = super::tests::job_for(9_120);
        let super::Next::Restart(j, _) = super::next_job(None, next) else { panic!("a new job") };
        assert!(!j.prepared);
        assert!(drop_due(j.install(), 6_890, j.through));
        for t in crate::tier::live_tables(&graftyard_git::tiers::METADATA) {
            let q = crate::tier::drop_live(&t);
            assert!(q[0].starts_with("DROP TABLE IF EXISTS"), "{t}: a second drop of a dropped table succeeds");
        }
        // It finishes: `Write::Install` swaps the scratch tier in and lifts the mark in the same transaction.
        let mut stmts = Vec::new();
        super::install_stmts(&mut stmts);
        let qs: Vec<&str> = stmts.iter().map(|(q, _)| *q).collect();
        assert!(qs.contains(&"DELETE FROM meta_dropped"));
        assert!(qs.iter().any(|q| q.starts_with("ALTER TABLE \"im_")), "the scratch metadata tier is swapped in");
    }

    /// A tier that is loaded, or not, whatever its rows.
    #[derive(Clone, Debug, Default)]
    struct Tier(MemStore, bool);
    impl Store for Tier {
        fn get(&self, t: &str, k: &[graftyard_fold::Val]) -> Option<graftyard_fold::Stored> {
            self.0.get(t, k)
        }
        fn put(&mut self, t: &str, k: graftyard_fold::Key, r: graftyard_fold::Stored) {
            self.0.put(t, k, r)
        }
        fn delete(&mut self, t: &str, k: &[graftyard_fold::Val]) {
            self.0.delete(t, k)
        }
        fn scan(
            &self,
            t: &str,
            f: &[graftyard_fold::Val],
            n: usize,
        ) -> Vec<(graftyard_fold::Key, graftyard_fold::Stored)> {
            self.0.scan(t, f, n)
        }
    }
    impl TierStore for Tier {
        fn clone_box(&self) -> Box<dyn TierStore> {
            Box::new(self.clone())
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
        fn loaded(&self) -> bool {
            self.1
        }
    }

    fn fold_at_1(meta_loaded: bool) -> RepoFold {
        let mut f = RepoFold::with_tiers(
            Some(RepoId(DbId(9))),
            Some(AccountId(DbId(1))),
            Box::new(Tier(MemStore::default(), true)),
            Box::new(Tier(MemStore::default(), true)),
        );
        f.apply_entry(&LogEntry {
            term: Term(1),
            index: Index(1),
            prev: PREV_ZERO,
            format: graftyard_git::repo::REPO_FORMAT_V1,
            tiers: TierSet::CORE.union(TierSet::METADATA),
            at: Timestamp(1),
            requests: Default::default(),
            extend: None,
            body: EntryBody::Events(vec![]),
        });
        if !meta_loaded {
            let m: &Tier = f.meta.as_any().downcast_ref().unwrap();
            f.meta = Box::new(Tier(m.0.clone(), false));
        }
        f
    }

    /// A member with its tables dropped is unloaded on restore whatever its tier's meta row says (a parent's tier
    /// whose meta row stands where the core's does, or a child's restored one).
    #[test]
    fn a_member_with_its_tables_dropped_is_never_loaded() {
        let f = fold_at_1(true);
        assert!(crate::family::meta_loaded(&f, true, false));
        assert!(!crate::family::meta_loaded(&f, true, true));
        let mut child = fold_at_1(true);
        child.child = true;
        assert!(crate::family::meta_loaded(&child, false, false), "a restored child");
        assert!(!crate::family::meta_loaded(&child, false, true));
    }

    /// The fold audit reads `at` per tier and compares only digests a member answers: an unloaded tier answers none,
    /// though its digest ring may still hold one (an install dropping it, or a tier still loading).
    #[test]
    fn an_unloaded_tier_answers_no_digest_for_the_audit() {
        let loaded = crate::snap::tiers_json(&fold_at_1(true), Some(1));
        assert!(loaded["core"]["at"].is_string() && loaded["metadata"]["at"].is_string());
        let v = crate::snap::tiers_json(&fold_at_1(false), Some(1));
        assert!(v["core"]["at"].is_string(), "{v}");
        assert!(v["metadata"]["at"].is_null(), "{v}");
        assert_eq!(v["metadata"]["loaded"], false);
    }
}

/// `decisions/DECISIONS.md`, "audit-moved": which entries send an idle chain back to a refold.
#[cfg(test)]
mod moved_tests {
    use graftyard_raft::{EntryBody, ExtendLeases, Format, GroupConfig, LogEntry, PREV_ZERO, Term, TierSet};
    use graftyard_types::{DbId, IdBlock, Index, Timestamp};

    use super::moved;

    fn entry(index: u64, body: EntryBody<u8>) -> LogEntry<u8> {
        LogEntry {
            term: Term(1),
            index: Index(index),
            prev: PREV_ZERO,
            format: Format { family: 0x20, version: 1 },
            tiers: TierSet::CORE,
            at: Timestamp(index),
            requests: Default::default(),
            extend: None,
            body,
        }
    }

    /// graft-m3f, 2026-11-18: an idle chain's log grows only by the no-ops of the terms the audit's own lookups
    /// started (`repo:65548`, 2,922 to 2,942 with no write). Those, and a config entry, leave the fold as it was; an
    /// entry with events, an id block, or a stall's lease extension does not, and neither does a match older than the
    /// entries the member holds, since it cannot see what came between.
    #[test]
    fn only_an_entry_that_writes_the_fold_moves_a_chain() {
        let chain = graftyard_types::ChainId::Repo(graftyard_types::RepoId(DbId(1)));
        let voter =
            graftyard_types::Member::new(chain, graftyard_types::SiteId(0), graftyard_types::MemberKind::Full, 0, 1);
        let p = serde_json::json!({ "voters": [voter], "learners": [], "home": 0, "generation": 1 });
        let config = || EntryBody::Config(GroupConfig::Normal(Box::new(serde_json::from_value(p.clone()).unwrap())));
        let log = vec![
            entry(5, EntryBody::Events(vec![1])),
            entry(6, EntryBody::Noop),
            entry(7, config()),
            entry(8, EntryBody::Noop),
        ];
        let base = Index(4);
        assert!(!moved(Index(5), base, &log), "no-ops and a config since the match");
        assert!(!moved(Index(8), base, &log), "nothing since");
        assert!(moved(Index(4), base, &log), "an events entry since");
        assert!(moved(Index(3), base, &log), "a match below the entries in memory");
        let mut stalled = log.clone();
        stalled.push(LogEntry {
            extend: Some(ExtendLeases { since: Timestamp(1), by: std::time::Duration::from_secs(9) }),
            ..entry(9, EntryBody::Noop)
        });
        assert!(moved(Index(8), base, &stalled), "a no-op that extends leases moves them");
        let mut ids = log;
        ids.push(entry(9, EntryBody::IdBlock(IdBlock::new(DbId(1 << 20), 4).unwrap())));
        assert!(moved(Index(8), base, &ids), "an id block moves the tiers' cursors");
    }
}
