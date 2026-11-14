//! A repository's metadata child group on a site (`design/DESIGN.md`, Shape, `CHILD_GROUP_AT`;
//! `graftyard_git::split`, the pure half):
//!
//! - **Parent.** The leader's alarm reads `databaseSize` and the child's state, and records the step
//!   `graftyard_git::split::plan` names: `MetaSplit`, `MetaSeed`, the child's creation and its own `MetaSeed`,
//!   `MetaDropped`, then `MetaSlim` until the pass is done.
//! - **Child.** Each member of `repo-meta:<id>` restores the seed's metadata snapshot into its metadata tier in
//!   alarm-driven batches, one transaction each with its cursor, and marks the tier loaded at the trailer. No entry
//!   writes that tier after it (`RepoFold::apply_meta`), and the child answers only reads of its rows.
//! - **Fill.** A parent member that answers tier rows or renders a reply takes each slimmed field from the child's
//!   row of the same key, kept per isolate once read, since it never changes. A read that misses a row fetches the
//!   missing rows from the child and builds its answer again.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use graftyard_fold::{Read, Restore, RestoreCursor, SnapshotFrame, Val, decode_some, frames_header};
use graftyard_git as git;
use graftyard_git::split::{ChildView, SplitEntry, SplitState, SplitStep};
use graftyard_raft::{Index, Role};
use graftyard_types::{ChainId, RepoId};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use worker::wasm_bindgen_futures::spawn_local;
use worker::*;

use crate::family::Repo;
use crate::host::{Host, err, rows, text};
use crate::{SiteEnv, now_ms};

/// How often an idle parent leader checks its size [guess]: a split takes hours, a minute's delay none.
const CHECK_EVERY_MS: u64 = 60_000;
/// How long one alarm's run records slim entries before it yields [guess], as the archive drain does.
const RUN_FOR_MS: u64 = 20_000;
/// A child member's restore batch: at most this many bytes of the snapshot or this long [guess], as `crate::load`'s.
const RESTORE_BYTES: u64 = 2 << 20;
const RESTORE_MS: u64 = 2_000;
const BUSY_MS: i64 = 20;
/// Child rows one isolate keeps for fills, in bytes [guess]: a page of an API read is a few hundred rows.
const CACHE_BYTES: usize = 32 << 20;
/// Child rows one fetch asks for at most [guess]: about 2 MB of comment rows.
const FETCH_MAX: usize = 1_000;

/// Child rows by repository, table, and key, `None` for a key the child does not hold, and their bytes in all.
type Cache = (usize, HashMap<(RepoId, String, Vec<Val>), Option<Vec<Val>>>);

thread_local! {
    static RUNNING: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
    static CHECKED: RefCell<HashMap<String, u64>> = RefCell::new(HashMap::new());
    static CACHE: RefCell<Cache> = RefCell::new((0, HashMap::new()));
    /// The child rows the answer being built missed.
    static MISSED: RefCell<Vec<(RepoId, String, Vec<Val>)>> = const { RefCell::new(Vec::new()) };
    /// Test installs: `SPLIT_AT` by chain (`set_split_at`).
    #[cfg(feature = "test-faults")]
    static SPLIT_AT_FOR: RefCell<HashMap<ChainId, u64>> = RefCell::new(HashMap::new());
}

/// `ctx.storage.sql.databaseSize` as the `f64` the runtime returns: workers-rs 0.8.7's `database_size()` is
/// `as usize`, which saturates at 4 GiB on wasm32.
pub fn db_bytes(h: &Host<Repo>) -> f64 {
    let storage = h.state.storage();
    let raw: &worker::wasm_bindgen::JsValue = storage.as_raw().as_ref();
    let sql = js_sys::Reflect::get(raw, &"sql".into()).unwrap_or_default();
    js_sys::Reflect::get(&sql, &"databaseSize".into()).ok().and_then(|v| v.as_f64()).unwrap_or(-1.0)
}

/// Whether `fold`'s metadata tier holds slimmed rows: its parent recorded `MetaDropped`.
pub fn dropped(fold: &git::repo::RepoFold) -> bool {
    !fold.child && SplitState::of(&*fold.core).dropped
}

/// Whether the parent's slim pass is running: a metadata snapshot then waits (`crate::snap`), since each row the pass
/// rewrites would be copied to `snap_undo` first, up to the whole tier's text on a member already near SPLIT_AT.
pub fn slimming(fold: &git::repo::RepoFold) -> bool {
    !fold.child && git::split::slim_cursor(&fold.meta).is_some_and(|c| !c.done)
}

/// A child's metadata tier is loaded once its restore wrote the tier's meta row.
pub fn child_loaded(fold: &git::repo::RepoFold) -> bool {
    graftyard_fold::meta(&*fold.meta).applied > Index(0)
}

// ---- fills ----

fn cached(repo: RepoId, table: &str, key: &[Val]) -> Option<Option<Vec<Val>>> {
    CACHE.with(|c| c.borrow().1.get(&(repo, table.to_owned(), key.to_vec())).cloned())
}

fn keep(repo: RepoId, table: String, key: Vec<Val>, row: Option<Vec<Val>>) {
    let n = row.as_ref().map_or(0, |r| format!("{r:?}").len()) + 64;
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        if c.0 + n > CACHE_BYTES {
            *c = (0, HashMap::new());
        }
        c.0 += n;
        c.1.insert((repo, table, key), row);
    });
}

/// Fills the slimmed metadata rows of `rows` from the cache, and counts each row it lacks in `MISSED`.
pub fn fill_rows(repo: RepoId, rows: &mut [(String, graftyard_fold::Key, Vec<Val>)]) {
    for (t, k, cols) in rows.iter_mut() {
        if !git::split::slimmed(t, cols) {
            continue;
        }
        match cached(repo, t, k) {
            Some(c) => *cols = git::split::fill(t, cols, c.as_deref()),
            None => MISSED.with(|m| m.borrow_mut().push((repo, t.clone(), k.clone()))),
        }
    }
}

/// `fill_rows` over a tier-rows answer, when the parent dropped its text and the answer is whole (an incomplete one
/// is never served).
pub fn fill_answer(fold: &git::repo::RepoFold, t: &mut graftyard_api::hosted::TierRows) {
    if let (Some(repo), Some(meta), true) = (fold.repo, t.meta.as_mut(), t.complete && dropped(fold)) {
        fill_rows(repo, meta);
    }
}

/// `fold`'s metadata tier as a reply renders it: each slimmed field from the cache; a miss counted in `MISSED`.
pub fn with_filled<T>(fold: &git::repo::RepoFold, f: impl FnOnce(&dyn Read) -> T) -> T {
    let Some(repo) = fold.repo.filter(|_| dropped(fold)) else { return f(&fold.meta) };
    let missed = RefCell::new(Vec::new());
    let child = |t: &str, k: &[Val]| cached(repo, t, k);
    let r = f(&git::split::Filled { meta: &fold.meta, child: &child, missed: &missed });
    MISSED.with(|m| m.borrow_mut().extend(missed.into_inner().into_iter().map(|(t, k)| (repo, t, k))));
    r
}

/// Where `MISSED` stands now, for `since`.
pub fn mark() -> usize {
    MISSED.with(|m| m.borrow().len())
}

/// The child rows missed since `m0`, taken out of `MISSED`, as the answer that stands in for one built without them:
/// `{"missedChild": [[repo, table, key], ..]}`. The member reads them (`fetch`) and builds the answer again. Both
/// calls run in one synchronous stretch, so no other answer's misses fall between them.
pub fn since(m0: usize) -> Option<Value> {
    let m: Vec<_> = MISSED.with(|m| m.borrow_mut().drain(m0..).collect());
    (!m.is_empty()).then(|| json!({ "missedChild": m }))
}

#[cfg(test)]
pub fn take_missed_for_test() -> Vec<(RepoId, String, Vec<Val>)> {
    MISSED.with(|m| std::mem::take(&mut *m.borrow_mut()))
}

/// Whether an answer stands in for one that missed child rows.
pub fn is_miss(v: &Value) -> bool {
    v.get("missedChild").is_some()
}

/// Reads the missed rows from the child (any member whose tier is loaded, by read-index) into the cache.
pub async fn fetch(h: &Rc<Host<Repo>>, missed: &Value) -> std::result::Result<(), String> {
    let want: Vec<(RepoId, String, Vec<Val>)> =
        serde_json::from_value(missed["missedChild"].clone()).map_err(|e| e.to_string())?;
    let record = h.core.borrow().as_ref().map(|c| c.join.record.clone()).ok_or("not joined")?;
    let site = SiteEnv::read(&h.env).map_err(|e| e.to_string())?;
    let b = graftyard_lookup::Bindings { env: &h.env, sites: &site.sites };
    let mut by: HashMap<RepoId, Vec<(String, Vec<Val>)>> = HashMap::new();
    for (r, t, k) in want {
        let v = by.entry(r).or_default();
        if !v.contains(&(t.clone(), k.clone())) {
            v.push((t, k));
        }
    }
    for (repo, keys) in by {
        for chunk in keys.chunks(FETCH_MAX) {
            let q = json!({ "q": { "rows": { "child": chunk } } });
            let v = graftyard_lookup::read_spread(&b, ChainId::RepoMeta(repo), &record, &q)
                .await?
                .ok_or_else(|| format!("repo-meta:{} has no leader", repo.0.0))?
                .0;
            let got: Vec<(String, Vec<Val>, Option<Vec<Val>>)> =
                serde_json::from_value(v["value"].clone()).map_err(|e| format!("child rows: {e}: {v}"))?;
            for (t, k, row) in got {
                keep(repo, t, k, row);
            }
        }
    }
    Ok(())
}

/// The child's answer to `{"rows": {"child": [[table, key], ..]}}`: each row of its restored tier, or null.
pub fn child_rows(fold: &git::repo::RepoFold, keys: &Value) -> Value {
    if !fold.child || !child_loaded(fold) {
        return json!({ "error": "not a loaded metadata child" });
    }
    let keys: Vec<(String, Vec<Val>)> = serde_json::from_value(keys.clone()).unwrap_or_default();
    let out: Vec<(String, Vec<Val>, Option<Vec<Val>>)> = keys
        .into_iter()
        .filter(|(t, _)| git::split::SLIM_TABLES.contains(&t.as_str()))
        .map(|(t, k)| {
            let row = fold.meta.get(&t, &k);
            (t, k, row)
        })
        .collect();
    json!(out)
}

/// The child's answer to `{"split": true}`, which the parent's leader reads.
pub fn child_view(fold: &git::repo::RepoFold) -> Value {
    json!(ChildView { seeded: SplitState::of(&*fold.core).seed.is_some(), loaded: fold.child && child_loaded(fold) })
}

// ---- recording ----

/// Why a split entry was not recorded: this member does not lead (the JSON a lookup follows), or anything else.
enum NotRecorded {
    NotLeader(Value),
    Other(String),
}

impl From<NotRecorded> for String {
    fn from(e: NotRecorded) -> String {
        match e {
            NotRecorded::NotLeader(v) => format!("not leader: {v}"),
            NotRecorded::Other(s) => s,
        }
    }
}

/// Records one split entry on this member's group, as its leader, once the fold's own check passes.
pub async fn record(h: &Rc<Host<Repo>>, x: SplitEntry) -> std::result::Result<Index, String> {
    record_as(h, x).await.map_err(String::from)
}

async fn record_as(h: &Rc<Host<Repo>>, x: SplitEntry) -> std::result::Result<Index, NotRecorded> {
    let other = |s: String| NotRecorded::Other(s);
    use graftyard_raft::{Fold as _, Scratch as _};
    let ev = git::repo::RepoEvent::Split(x);
    {
        let core = h.core.borrow();
        let c = core.as_ref().ok_or_else(|| other("not joined".into()))?;
        c.m.group_fold().fold.scratch().check(&ev).map_err(|e| other(format!("refused: {e}")))?;
    }
    let rid = graftyard_lookup::request_id();
    let rx = h.wait_recorded(rid);
    match h.input(|m, t| m.record(t, rid, git::split::REPO_FORMAT_SPLIT, vec![ev])).map_err(|e| other(e.to_string()))? {
        Ok(()) => {}
        Err(e @ graftyard_raft::CallError::NotLeader(_)) => {
            return Err(NotRecorded::NotLeader(crate::host::call_error_json(&e)));
        }
        Err(e) => return Err(other(crate::host::call_error_json(&e).to_string())),
    }
    h.served();
    match crate::host::within(std::time::Duration::from_secs(10), rx).await {
        Some(Ok(Ok(i))) => Ok(i),
        Some(Ok(Err(nl))) => {
            Err(NotRecorded::NotLeader(crate::host::call_error_json(&graftyard_raft::CallError::NotLeader(nl))))
        }
        Some(Err(_)) => Err(other("member reset".into())),
        None => Err(other("the split entry did not commit in 10 s".into())),
    }
}

/// `/split` on a child's leader: `{"seed": MetaSeed}` from the parent. A seed already recorded answers `seeded`.
pub async fn route(h: &Rc<Host<Repo>>, body: Value) -> Result<Value> {
    let x: SplitEntry = serde_json::from_value(body["seed"].clone())?;
    let already = {
        let core = h.core.borrow();
        let c = core.as_ref().ok_or_else(|| err("not joined"))?;
        let f = &c.m.group_fold().fold;
        if !f.child {
            return Ok(json!({ "refused": "not a metadata child" }));
        }
        SplitState::of(&*f.core).seed
    };
    if let (Some((at, key)), SplitEntry::MetaSeed { at: a, key: k }) = (already, &x) {
        return Ok(json!({ "seeded": at == *a && key == *k }));
    }
    // A follower answers `{"error": {"notLeader": ..}}`, which the lookup follows to the leader.
    Ok(match record_as(h, x).await {
        Ok(i) => json!({ "recorded": i.0 }),
        Err(NotRecorded::NotLeader(e)) => json!({ "error": e }),
        Err(NotRecorded::Other(e)) => json!({ "failed": e }),
    })
}

// ---- the alarm ----

enum Who {
    Parent { repo: RepoId, key: String },
    Child,
}

fn who(h: &Rc<Host<Repo>>) -> Option<Who> {
    let core = h.core.borrow();
    let c = core.as_ref()?;
    match c.join.chain {
        ChainId::RepoMeta(_) => Some(Who::Child),
        ChainId::Repo(repo) if c.join.owner.is_some() && c.m.role() == Role::Leader => {
            Some(Who::Parent { repo, key: c.join.me.do_name.to_string() })
        }
        _ => None,
    }
}

pub async fn on_alarm(h: &Rc<Host<Repo>>) {
    match who(h) {
        Some(Who::Child) => {
            h.settled().await;
            match restore(h).await {
                Ok(true) => h.arm_in(BUSY_MS),
                Ok(false) => {}
                Err(e) => note(h, format!("split restore: {e}")),
            }
        }
        Some(Who::Parent { repo, key }) => parent(h, repo, key),
        None => {}
    }
}

/// After restore: a child whose restore has not finished resumes it.
pub fn restored(h: &Rc<Host<Repo>>) {
    let due = h.core.borrow().as_ref().is_some_and(|c| {
        let f = &c.m.group_fold().fold;
        f.child && SplitState::of(&*f.core).seed.is_some() && !child_loaded(f)
    });
    if due {
        h.arm_in(BUSY_MS);
    }
}

fn note(h: &Rc<Host<Repo>>, m: String) {
    console_error!("{m}");
    let mut s = h.stats.borrow_mut();
    if s.alarms.len() < 50 {
        s.alarms.push(format!("{} {m}", now_ms()));
    }
}

fn parent(h: &Rc<Host<Repo>>, repo: RepoId, key: String) {
    if RUNNING.with(|r| r.borrow().contains(&key)) {
        return;
    }
    if CHECKED.with(|c| c.borrow().get(&key).is_some_and(|at| now_ms() < at + CHECK_EVERY_MS)) {
        return;
    }
    RUNNING.with(|r| r.borrow_mut().insert(key.clone()));
    let w = Rc::downgrade(h);
    spawn_local(async move {
        let r = match w.upgrade() {
            Some(h) => run(&h, repo).await,
            None => Ok(false),
        };
        RUNNING.with(|r| r.borrow_mut().remove(&key));
        let Some(h) = w.upgrade() else { return };
        match r {
            Ok(true) => return h.arm_in(BUSY_MS),
            Ok(false) => {
                CHECKED.with(|c| c.borrow_mut().insert(key, now_ms()));
            }
            Err(e) => {
                CHECKED.with(|c| c.borrow_mut().insert(key, now_ms()));
                note(&h, format!("split: {e}"));
            }
        }
        // A split under way checks again on its own: a leader that serves nothing arms no alarm, and smoke 2's
        // parent waited on a child restored at 05:39Z with no alarm after 05:36Z (`tests/am1/results/b030-split.md`).
        if leader_due(&h) {
            h.arm_in(CHECK_EVERY_MS as i64);
        }
    });
}

/// The parent leader's steps until one waits, fails, or RUN_FOR_MS passed. Whether more is due at once.
async fn run(h: &Rc<Host<Repo>>, repo: RepoId) -> std::result::Result<bool, String> {
    let began = now_ms();
    let mut view: Option<ChildView> = None;
    loop {
        // The child is read only while the parent waits on it: seeded, not yet dropped.
        let waits = h.core.borrow().as_ref().is_some_and(|c| {
            let st = SplitState::of(&*c.m.group_fold().fold.core);
            st.seed.is_some() && !st.dropped
        });
        if waits && view.is_none() {
            view = read_child(h, repo).await;
        }
        let (step, record) = {
            let core = h.core.borrow();
            let c = core.as_ref().ok_or("not joined")?;
            if c.m.role() != Role::Leader {
                return Ok(false);
            }
            let f = &c.m.group_fold().fold;
            let bytes = db_bytes(h).max(0.0) as u64;
            (git::split::plan(&*f.core, &f.meta, bytes, split_at(c.join.chain), view.as_ref()), c.join.record.clone())
        };
        match step {
            SplitStep::Idle | SplitStep::WaitSnapshot => return Ok(false),
            SplitStep::Split => {
                record_on(h, SplitEntry::MetaSplit { child: ChainId::RepoMeta(repo) }).await?;
            }
            SplitStep::Seed { at, key } => {
                record_on(h, SplitEntry::MetaSeed { at, key }).await?;
            }
            SplitStep::Child { at, key } => {
                seed_child(h, repo, &record, SplitEntry::MetaSeed { at, key }).await?;
                return Ok(false);
            }
            SplitStep::Drop { seed } => {
                record_on(h, SplitEntry::MetaDropped { seed }).await?;
            }
            SplitStep::Slim => {
                let t0 = now_ms();
                record_on(h, SplitEntry::MetaSlim).await?;
                Delay::from(crate::host::paced(now_ms() - t0)).await;
            }
        }
        if now_ms() >= began + RUN_FOR_MS {
            return Ok(true);
        }
    }
}

/// Test installs: `SPLIT_AT` for one chain in this isolate (`/bench_seed {"splitAt": n}`), so a hosted smoke splits a
/// group of a few hundred MB. Only `MetaSplit` reads it; every later step follows the fold.
#[cfg(feature = "test-faults")]
pub fn set_split_at(chain: ChainId, v: &Value) -> Value {
    let at = v.as_u64();
    SPLIT_AT_FOR.with(|m| match at {
        Some(n) => m.borrow_mut().insert(chain, n),
        None => m.borrow_mut().remove(&chain),
    });
    CHECKED.with(|c| c.borrow_mut().clear());
    json!({ "splitAt": at })
}

fn split_at(chain: ChainId) -> u64 {
    #[cfg(feature = "test-faults")]
    if let Some(n) = SPLIT_AT_FOR.with(|m| m.borrow().get(&chain).copied()) {
        return n;
    }
    let _ = chain;
    git::split::SPLIT_AT
}

/// Whether a split of `fold`, at `bytes` of member SQLite, has a step only its leader takes: any but `Idle`. A child
/// has none. A campaign it starts still waits out every promise to a live leader (`Member::campaign`).
pub fn needs_leader(fold: &git::repo::RepoFold, bytes: u64, split_at: u64) -> bool {
    !fold.child && !matches!(git::split::plan(&*fold.core, &fold.meta, bytes, split_at, None), SplitStep::Idle)
}

/// `needs_leader` on this member, for `Host::keep_led`.
pub fn leader_due(h: &Rc<Host<Repo>>) -> bool {
    let bytes = db_bytes(h).max(0.0) as u64;
    let core = h.core.borrow();
    core.as_ref().is_some_and(|c| needs_leader(&c.m.group_fold().fold, bytes, split_at(c.join.chain)))
}

async fn record_on(h: &Rc<Host<Repo>>, x: SplitEntry) -> std::result::Result<(), String> {
    let what = format!("{x:?}");
    let i = record(h, x).await?;
    if !what.starts_with("MetaSlim") {
        console_log!("split: {what} at {i:?}");
    }
    Ok(())
}

async fn read_child(h: &Rc<Host<Repo>>, repo: RepoId) -> Option<ChildView> {
    let record = h.core.borrow().as_ref().map(|c| c.join.record.clone())?;
    let site = SiteEnv::read(&h.env).ok()?;
    let b = graftyard_lookup::Bindings { env: &h.env, sites: &site.sites };
    let q = json!({ "q": { "split": true } });
    match graftyard_lookup::call_opt(&b, ChainId::RepoMeta(repo), &record, "/read", q.to_string().as_bytes()).await {
        Ok(Some((v, _, _))) => serde_json::from_value(v["value"].clone()).ok(),
        _ => None,
    }
}

/// Creates the child on the parent's record if the lookup finds none, and records the parent's `MetaSeed` there.
async fn seed_child(
    h: &Rc<Host<Repo>>,
    repo: RepoId,
    record: &graftyard_types::PlacementRecord,
    seed: SplitEntry,
) -> std::result::Result<(), String> {
    let owner = h.core.borrow().as_ref().and_then(|c| c.join.owner).ok_or("no owner")?;
    let site = SiteEnv::read(&h.env).map_err(|e| e.to_string())?;
    let b = graftyard_lookup::Bindings { env: &h.env, sites: &site.sites };
    let chain = ChainId::RepoMeta(repo);
    graftyard_lookup::create(&b, chain, record, &json!({ "owner": owner })).await?;
    let (v, _) = graftyard_lookup::call(&b, chain, record, "/split", &json!({ "seed": seed })).await?;
    console_log!("split: child {chain} seed {v}");
    match v.get("failed").or_else(|| v.get("refused")) {
        Some(e) => Err(format!("the child refused its seed: {e}")),
        None => Ok(()),
    }
}

// ---- the child's restore ----

/// A child member's restore of the seed snapshot, as `split_job` stores it.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Job {
    key: String,
    at: u64,
    site: Option<u8>,
    size: u64,
    offset: u64,
    restore: Option<RestoreCursor>,
    rows: u64,
    began_ms: u64,
}

fn load_job(sql: &SqlStorage) -> Result<Option<Job>> {
    match rows(sql, "SELECT row FROM split_job WHERE id = 1", vec![])?.first() {
        Some(r) => Ok(Some(serde_json::from_str(text(&r[0])?)?)),
        None => Ok(None),
    }
}

/// One batch of the child's restore. Whether more batches are due.
async fn restore(h: &Rc<Host<Repo>>) -> Result<bool> {
    let (seed, meta, repo) = {
        let core = h.core.borrow();
        let Some(c) = core.as_ref() else { return Ok(false) };
        let f = &c.m.group_fold().fold;
        if !f.child || child_loaded(f) {
            return Ok(false);
        }
        let Some(seed) = SplitState::of(&*f.core).seed else { return Ok(false) };
        (seed, crate::tier::handle(&*f.meta).clone(), f.repo.ok_or_else(|| err("no repository"))?)
    };
    let sql = h.state.storage().sql();
    // Its own handle on the tier's tables, so no input seals a batch's rows apart from its cursor. No FTS5 rows: no
    // route serves search from them, and they would double the child's writes (`decisions/DECISIONS.md`, "A split
    // moves only the text `decide` never reads").
    let live = meta;
    let meta = crate::tier::TierSql::new(sql.clone(), &git::tiers::METADATA, None);
    let key = seed.1.to_string();
    let mut job = match load_job(&sql)? {
        Some(j) if j.key == key => j,
        _ => Job {
            key: key.clone(),
            at: seed.0.0,
            site: None,
            size: 0,
            offset: 0,
            restore: None,
            rows: 0,
            began_ms: now_ms(),
        },
    };
    if job.site.is_none() {
        let (s, size) = crate::load::find(&h.env, &key).await?;
        (job.site, job.size) = (Some(s), size);
    }
    let t0 = now_ms();
    let mut store = meta;
    let schema = git::tiers::METADATA;
    let mut r = match job.restore.take() {
        Some(c) => Restore::resume(schema, c),
        None => Restore::begin(&store, schema, (ChainId::Repo(repo), Index(job.at))).map_err(|e| err(e.to_string()))?,
    };
    let mut ended = false;
    let mut read = 0u64;
    while !ended && read < RESTORE_BYTES && now_ms() - t0 < RESTORE_MS && job.offset < job.size {
        let start = if job.offset == 0 { frames_header().len() } else { 0 };
        let b = crate::load::read_whole(&h.env, job.site.expect("found"), &key, job.offset, job.size, start).await?;
        if start > 0 && b.get(..start) != Some(&frames_header()[..]) {
            return Err(err(format!("{key} is not a snapshot frame stream")));
        }
        let (frames, used) = decode_some(&b[start..]).map_err(|e| err(e.to_string()))?;
        if frames.is_empty() {
            return Err(err(format!("{key}: no whole frame at byte {}", job.offset)));
        }
        for f in &frames {
            r.frame(&mut store, f).map_err(|e| err(e.to_string()))?;
            match f {
                SnapshotFrame::Rows { rows, .. } => job.rows += rows.len() as u64,
                SnapshotFrame::End(_) => ended = true,
            }
        }
        job.offset += (start + used) as u64;
        read += (start + used) as u64;
    }
    if ended {
        r.finish_rows(&mut store).map_err(|e| err(e.to_string()))?;
    } else if job.offset >= job.size {
        return Err(err(format!("{key} ended without its trailer")));
    } else {
        job.restore = Some(r.cursor());
    }
    let mut stmts = Vec::new();
    store.seal(&mut stmts);
    if ended {
        stmts.push(("DELETE FROM split_job", vec![]));
    } else {
        stmts.push((
            "INSERT OR REPLACE INTO split_job (id, row) VALUES (1, ?)",
            vec![serde_json::to_string(&job)?.into()],
        ));
    }
    crate::host::commit_tx(&h.state.storage(), stmts).map_err(err)?;
    store.flushed();
    if ended {
        live.set_loaded(true);
        console_log!(
            "split: child restored {key}: {} rows, {} bytes in {} ms",
            job.rows,
            job.size,
            now_ms() - job.began_ms
        );
    }
    Ok(!ended)
}

#[cfg(test)]
mod tests {
    use graftyard_fold::Val;
    use graftyard_git::issues::{CommentTarget, IssueNumber};
    use graftyard_git::repo::{RepoEvent, RepoFold};
    use graftyard_git::split::{REPO_FORMAT_SPLIT, SLIMMED, SplitEntry};
    use graftyard_git::tiers::{CommentRow, METADATA, get_rec};
    use graftyard_raft::{EntryBody, Fold as _, LogEntry, PREV_ZERO, TierSet, digest};
    use graftyard_types::{AccountId, ChainId, DbId, Index, RepoId, Tier, Timestamp};
    use serde_json::json;
    use smallvec::SmallVec;

    use crate::family::{Family, Repo};

    const REPO: RepoId = RepoId(DbId(41));

    fn row(k: i64, body: &str) -> (Vec<Val>, Vec<Val>) {
        let r = CommentRow {
            on: CommentTarget::Issue(IssueNumber(1)),
            author: None,
            body: body.into(),
            reply_to: None,
            resolved: false,
            created: Timestamp(1),
            updated: Timestamp(1),
        };
        (vec![Val::Int(k)], vec![Val::Blob(postcard::to_allocvec(&r).unwrap().into())])
    }

    /// A parent past its whole slim pass: comments 1 to 3 written at index 1, then the split's entries applied as
    /// every member applies them. Returns the fold and the next entry's `prev`.
    fn parent() -> RepoFold {
        let mut f = RepoFold::new(REPO, AccountId(DbId(1)));
        let mut prev = PREV_ZERO;
        let mut i = 0;
        let mut entry = |f: &mut RepoFold, ev: RepoEvent| {
            i += 1;
            let tiers = graftyard_git::tiers::tier_of(&ev).union(TierSet::CORE);
            let e = LogEntry {
                term: graftyard_raft::Term(1),
                index: Index(i),
                prev,
                format: REPO_FORMAT_SPLIT,
                tiers,
                at: Timestamp(i),
                requests: SmallVec::new(),
                extend: None,
                body: EntryBody::Events(vec![ev]),
            };
            prev = digest(&e);
            f.apply_entry(&e);
        };
        entry(&mut f, RepoEvent::Fanout { k: 1 });
        graftyard_fold::seed(
            &mut *f.meta,
            &METADATA,
            "comment",
            Index(1),
            (1..=3).map(|k| row(k, &format!("text {k}"))).collect(),
        );
        entry(&mut f, RepoEvent::Split(SplitEntry::MetaSplit { child: ChainId::RepoMeta(REPO) }));
        let key = graftyard_git::key::R2Key::Snapshot {
            chain: ChainId::Repo(REPO),
            tier: Tier::Metadata,
            from: Index(0),
            to: Index(1),
            digest: graftyard_types::Digest([1; 32]),
        };
        entry(&mut f, RepoEvent::Split(SplitEntry::MetaSeed { at: Index(1), key }));
        entry(&mut f, RepoEvent::Split(SplitEntry::MetaDropped { seed: Index(1) }));
        entry(&mut f, RepoEvent::Split(SplitEntry::MetaSlim));
        f
    }

    fn body(rows: &graftyard_api::hosted::TierRows, k: i64) -> Option<String> {
        let m = graftyard_api::hosted::TierRows::store(rows.meta.as_ref().unwrap());
        get_rec::<CommentRow>(&m, "comment", &[Val::Int(k)]).map(|r| r.body.to_string())
    }

    /// A whole-tier read of a slimmed parent answers `missedChild` naming exactly the slimmed rows, and keeps no
    /// dump of it; once the child's rows are held, the same read answers every text as before the split, a row the
    /// child lacks reads empty, and an unslimmed row is the parent's own.
    #[test]
    fn a_parent_read_answers_its_text_only_with_the_childs_rows() {
        let f = parent();
        assert_eq!(&*get_rec::<CommentRow>(&f.meta, "comment", &[Val::Int(2)]).unwrap().body, SLIMMED);
        let q = json!({ "rows": true, "ranges": true, "packed": true });
        let builds = crate::family::dump_builds();
        let miss = Repo::read_json(&f, &q);
        let mut want: Vec<_> = (1..=3).map(|k| json!([REPO, "comment", [Val::Int(k)]])).collect();
        let mut got = miss["missedChild"].as_array().cloned().unwrap_or_default();
        want.sort_by_key(|v| v.to_string());
        got.sort_by_key(|v| v.to_string());
        assert_eq!(got, want, "{miss}");
        assert_eq!(crate::family::dump_builds(), builds, "a missed dump is not kept");
        assert!(crate::split::take_missed_for_test().is_empty(), "a read leaves no misses behind");
        // The child holds rows 1 and 2 (row 3 it never had), and parent row 2 is not filled from another key.
        for (k, b) in [(1, "text 1"), (2, "text 2")] {
            let (key, cols) = row(k, b);
            super::keep(REPO, "comment".into(), key, Some(cols));
        }
        super::keep(REPO, "comment".into(), vec![Val::Int(3)], None);
        let v = Repo::read_json(&f, &q);
        assert!(v.get("missedChild").is_none(), "{v}");
        let rows = graftyard_api::hosted::TierRows::from_value(&v).unwrap();
        assert_eq!(body(&rows, 1).as_deref(), Some("text 1"));
        assert_eq!(body(&rows, 2).as_deref(), Some("text 2"));
        assert_eq!(body(&rows, 3).as_deref(), Some(""), "a slimmed row the child lacks reads empty, not SLIMMED");
        // A plain rows read fills the same way.
        let plain =
            graftyard_api::hosted::TierRows::from_value(&Repo::read_json(&f, &json!({ "rows": true }))).unwrap();
        assert_eq!(body(&plain, 2).as_deref(), Some("text 2"));
        super::CACHE.with(|c| *c.borrow_mut() = (0, Default::default()));
    }

    /// A split needs its leader while a step is due: at SPLIT_AT before `MetaSplit`, and through the slim pass; not
    /// under SPLIT_AT, not once the pass is done, and never on the child.
    #[test]
    fn a_split_needs_its_leader_only_while_a_step_is_due() {
        use super::needs_leader;
        let fresh = RepoFold::new(REPO, AccountId(DbId(1)));
        assert!(!needs_leader(&fresh, 100, 200));
        assert!(needs_leader(&fresh, 200, 200));
        let mut child = RepoFold::new(REPO, AccountId(DbId(1)));
        child.child = true;
        assert!(!needs_leader(&child, 300, 200));
        // `parent()`'s one `MetaSlim` covered its three rows, so the pass is done.
        let done = parent();
        assert!(super::git::split::slim_cursor(&done.meta).is_some_and(|c| c.done));
        assert!(!needs_leader(&done, 300, 200));
    }

    /// The child answers its restored rows by key, and the parent group answers none.
    #[test]
    fn only_a_loaded_child_answers_child_rows() {
        let mut c = RepoFold::new(REPO, AccountId(DbId(1)));
        c.child = true;
        let q =
            json!({ "rows": { "child": [["comment", [Val::Int(1)]], ["issue", [Val::Int(9)]], ["settings", []]] } });
        assert!(Repo::read_json(&c, &q).get("error").is_some(), "not restored yet");
        // The parent's metadata snapshot at its index 1, restored into the child's tier.
        let mut src = RepoFold::new(REPO, AccountId(DbId(1)));
        src.apply_entry(&LogEntry {
            term: graftyard_raft::Term(1),
            index: Index(1),
            prev: PREV_ZERO,
            format: REPO_FORMAT_SPLIT,
            tiers: TierSet::CORE.union(TierSet::METADATA),
            at: Timestamp(1),
            requests: SmallVec::new(),
            extend: None,
            body: EntryBody::Events(vec![RepoEvent::Fanout { k: 1 }]),
        });
        graftyard_fold::seed(&mut *src.meta, &METADATA, "comment", Index(1), vec![row(1, "kept")]);
        let mut job = graftyard_fold::SnapshotJob::begin(
            &mut *src.meta,
            METADATA,
            ChainId::Repo(REPO),
            graftyard_types::ReleaseId([0; 32]),
            REPO_FORMAT_SPLIT,
        )
        .unwrap();
        let mut r = graftyard_fold::Restore::begin(&*c.meta, METADATA, (ChainId::Repo(REPO), Index(1))).unwrap();
        while let Some(fr) = job.next(&*src.meta, 100) {
            r.frame(&mut *c.meta, &fr).unwrap();
        }
        r.finish(&mut *c.meta).unwrap();
        assert!(super::child_loaded(&c));
        let v = Repo::read_json(&c, &q);
        let got: Vec<(String, Vec<Val>, Option<Vec<Val>>)> = serde_json::from_value(v).unwrap();
        assert_eq!(got.len(), 2, "only the slimmed tables are served");
        assert_eq!(got[0].2, Some(row(1, "kept").1));
        assert_eq!(got[1].2, None);
        assert!(Repo::read_json(&parent(), &q).get("error").is_some());
        assert!(crate::family::follower_query(&q, true) && !crate::family::follower_query(&q, false));
    }
}
