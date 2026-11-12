//! The command families a member hosts, picked by the kind of its chain (`design/DESIGN.md`, Shape, "Chains"):
//! `Names`, `Tokens`, `Ids`, and `Authz` from `graftyard_identity`, and the test register on `Repo` chains until the
//! repository fold exists (the `Repo` slot, `RepoSlot`).
//!
//! A family says how its fold is stored in the member's SQLite, how its commands, outputs, and refusals look on the
//! member's JSON routes, what a `fresh` read answers, and how a dependency fold read from another group decodes.

use std::fmt::Debug;

use graftyard_git as git;
use graftyard_identity::{self as id, IdentityDep};
use graftyard_raft::{self as raft, Command, Format, codec};
use graftyard_types::{AccountId, ChainId, DbId, RepoId};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use worker::{SqlStorage, SqlStorageValue};

use crate::member::Join;
#[cfg(feature = "test-faults")]
use crate::register::{REG_FORMAT, Reg, RegCmd, RegErr, RegEvent};

pub type Stmt = (&'static str, Vec<SqlStorageValue>);

/// A log range an applied `Archived` put in an archive object.
#[derive(Clone, Debug)]
pub struct ArchivedRange {
    pub key: String,
    pub from: u64,
    pub to: u64,
    pub to_at: u64,
}
pub type Out<F> = <<F as Family>::C as Command>::Out;
pub type Err<F> = <<F as Family>::C as Command>::Err;
pub type Dep<F> = <<F as Family>::C as Command>::Dep;

/// Where a member reads the record it bootstraps from, before it stores the bootstrap entry
/// (`design/DESIGN.md`, Shape, `bootstrap`: "after it reads the same record generation from the record's source
/// itself (the `Names` leader, or genesis)").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// The system chain's record in the genesis view (`SiteEnv::system_record`).
    Genesis,
    /// The owner's `Placement` row on `Names` shard `account_shard(owner)`.
    Owner(AccountId),
    /// A repository chain: its `RepoOwner` row on `Names` shard `name_shard(Repo(repo))` must name the owner the
    /// member was joined with, and the record is that owner's `Placement` row.
    Repo { repo: RepoId, owner: AccountId },
    /// The `Release` group: the signed genesis the member was joined with, accepted against this script's own trust
    /// anchor, names the group's record.
    Anchor,
    /// The test register trusts the record its creator sent.
    Given,
}

pub trait Family: 'static {
    type Ev: raft::Record + Clone + Debug + PartialEq + Serialize + DeserializeOwned;
    type Fold: raft::Fold<Event = Self::Ev> + PartialEq;
    type C: Command<Fold = Self::Fold> + DeserializeOwned;

    /// The formats this release decodes for the family.
    fn reads() -> Vec<Format>;
    /// The fold of a member that has applied nothing, from what it was joined with. A tiered fold's tiers live in
    /// `sql` (`crate::tier`), and are empty here.
    fn new_fold(sql: &SqlStorage, join: &Join) -> Self::Fold;
    /// Statements storing the fold after `entries` applied, inside `Write::Apply`'s transaction.
    fn store_fold(fold: &Self::Fold, entries: &[&raft::LogEntry<Self::Ev>], stmts: &mut Vec<Stmt>);
    /// Every statement of the input's transaction has run (`host::flush`).
    fn flushed(_fold: &Self::Fold) {}
    /// A read of the fold's own tables that storage refused, with its cause (`crate::tier::TierSql::lost`): what was
    /// read since may lack rows, so the member must reset before anything it computed leaves.
    fn lost(_fold: &Self::Fold) -> Option<String> {
        None
    }
    /// The fold keeps its own tables beside the log (`crate::tier`): restore does not replay the log into a second
    /// fold to compare, which would write those tables; a tier refuses an entry out of order instead.
    fn tiered() -> bool {
        false
    }
    /// How soon after applying entries the member's alarm runs the family's fold work (`on_alarm`), if it has any;
    /// `leading` when the member leads.
    fn fold_alarm_ms(_leading: bool) -> Option<i64> {
        None
    }
    /// The family's own tables beside the member's.
    fn schema() -> &'static [&'static str] {
        &[]
    }
    /// The paths `route` serves.
    fn route_paths() -> &'static [&'static str] {
        &[]
    }
    /// The family's own member routes: `None` for a path it does not serve.
    async fn route(
        _host: &std::rc::Rc<crate::host::Host<Self>>,
        _path: &str,
        _body: Value,
    ) -> Option<worker::Result<Value>>
    where
        Self: Sized,
    {
        None
    }
    /// After an input that applied entries `from+1..=to` (or wrote nothing but stepped): a repository member feeds
    /// its activity-feed ring (`crate::feed`). Synchronous, inside the input, so the ring sees the tiers the input left.
    fn stepped(
        _m: &raft::Member<Self::C>,
        _join: &Join,
        _from: raft::Index,
        _to: raft::Index,
        _out: &raft::Out<Self::C>,
    ) {
    }
    /// The family's work on the member's alarm, before the Raft tick: a repository member's snapshot batches and
    /// metadata load (`crate::snap`).
    async fn on_alarm(_host: &std::rc::Rc<crate::host::Host<Self>>)
    where
        Self: Sized,
    {
    }
    /// Brings the family's own tables to this release's shape, first thing at restore.
    fn prepare(_sql: &SqlStorage) -> worker::Result<()> {
        Ok(())
    }
    fn load_fold(sql: &SqlStorage, join: &Join) -> worker::Result<Self::Fold>;
    /// Brings a fold that keeps its own tables up to the member's `applied` from the stored log, at restore, or
    /// refuses to restore; `apply` would otherwise meet an entry its tables do not follow.
    fn catch_up(_storage: &worker::Storage, _fold: &Self::Fold, _applied: raft::Index) -> worker::Result<()> {
        Ok(())
    }
    /// A committed output's JSON. It takes the whole `Acked`, so a family can render from it what only `&Acked` may
    /// build, as the repository's `report_status` bytes (`graftyard_git::group::PushReport::of`), and the member's
    /// fold at or after the output's index, which an issue command's reply reads.
    fn out_json(fold: &Self::Fold, a: &raft::Acked<Out<Self>>) -> Value;
    /// `{"err": .., "status": 401 | 403 | 422}`.
    fn err_json(e: &Err<Self>) -> Value;
    /// Before the member admits a command: work the command needs outside the fold, such as an issue command's
    /// `BlobStore::confirm`.
    async fn admit(_env: &worker::Env, _chain: ChainId, _cmd: &mut Self::C) {}
    /// What a `fresh` read answers for query `q`.
    fn read_json(fold: &Self::Fold, q: &Value) -> Value;
    /// For a granted whole-tier read `q` at `now`: the ms until its dump may be rebuilt (`dump_wait`), or `None`.
    /// `every` is the member's rebuild spacing (`dump_every`).
    fn heavy_wait(_fold: &Self::Fold, _q: &Value, _now: u64, _every: u64) -> Option<u64> {
        None
    }
    /// What a `fresh` read answers for `q` as serialized JSON bytes shared across reads, when the family keeps them
    /// (a repository's packed dump, `dump_bytes`); else `None` and the host serializes `read_json`.
    fn read_bytes(_fold: &Self::Fold, _q: &Value) -> Option<std::rc::Rc<[u8]>> {
        None
    }
    /// Whether a follower serves read `q` by read-index (`design/DESIGN.md`, "Follower read"); else it names the leader.
    fn follower_serves(_fold: &Self::Fold, _q: &Value) -> bool {
        true
    }
    /// The fold in the crate's codec, for another group's dependency read (`/dep`).
    fn fold_bytes(fold: &Self::Fold) -> Vec<u8>;
    /// A dependency fold read from `chain`'s leader.
    fn dep(chain: ChainId, bytes: &[u8]) -> Result<Dep<Self>, String>;
    /// The refusal that asks the host to lease an id block and re-send.
    fn needs_block(_e: &Err<Self>) -> bool {
        false
    }
    fn source(join: &Join) -> Source;
    /// The answer to a `/commit` or `/outcome_by_request` body whose command does not decode: `Some` names the
    /// format this release does not read (`{"unsupported": ..}`), and nothing is touched. `None` answers 400.
    fn unreadable(_body: &[u8]) -> Option<Value> {
        None
    }
    /// After an `ok` outcome, on the leader: the work a command's commit owes outside the log, such as the
    /// repository's `Committed` pack markers. Returns how many puts failed.
    async fn after_ok(_env: worker::Env, _chain: ChainId, _cmd: Self::C, _acked: raft::Acked<Out<Self>>) -> usize {
        0
    }
    /// The stored packs a `request` row's events name, for the sweeper's `RequestSource` (`{"q": {"request": id}}`).
    fn row_packs(_events: &[Self::Ev]) -> Value {
        json!([])
    }
    /// The archive an applied entry commits: members record its range (`archive_key`) and truncate their stored log
    /// through `to` once they can HEAD the key in two buckets.
    fn archived(_e: &raft::LogEntry<Self::Ev>) -> Option<ArchivedRange> {
        None
    }
    /// After the stored log was truncated below `to`: a family that hosts `InstallSnapshot` names its source
    /// (`crate::load::set_source`).
    async fn truncated(_host: &std::rc::Rc<crate::host::Host<Self>>, _to: raft::Index)
    where
        Self: Sized,
    {
    }
    /// The member must install a snapshot before it can follow (`Out::install`): a family that hosts it loads one
    /// (`crate::load::want`). A family that does not never names an install source, so its members are never sent one.
    fn install_wanted(_host: &std::rc::Rc<crate::host::Host<Self>>, _w: raft::InstallWanted)
    where
        Self: Sized,
    {
    }
    /// After restore built the core: the stored install source, and a load job to resume.
    fn restored(_host: &std::rc::Rc<crate::host::Host<Self>>)
    where
        Self: Sized,
    {
    }
    /// `Write::Install`'s statements that make the loaded tables the member's fold.
    fn install_stmts(_stmts: &mut Vec<Stmt>) {}
    /// Family fields of a log entry on the harness's `/log` and the shard's `/entries`.
    fn entry_json(_e: &raft::LogEntry<Self::Ev>) -> serde_json::Map<String, Value> {
        serde_json::Map::new()
    }
    /// Test installs: more fields of a log entry on the harness's `/log` only.
    #[cfg(feature = "test-faults")]
    fn log_fields(_e: &raft::LogEntry<Self::Ev>) -> serde_json::Map<String, Value> {
        serde_json::Map::new()
    }
    /// Extra fold state for the BF-12 audit.
    #[cfg(feature = "test-faults")]
    fn audit(_sql: &SqlStorage, _fold: &Self::Fold, _prefix: &str) -> worker::Result<Value> {
        Ok(Value::Null)
    }
}

/// The family a member runs, from the chain kind it was joined for. `None` for kinds no member hosts yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Names,
    Tokens,
    Ids,
    Authz,
    Identity,
    Repo,
    Release,
    /// Test installs only: a `repo:` chain joined with no owner hosts the test register, for M1-WP2's rows and the
    /// linearizability run.
    #[cfg(feature = "test-faults")]
    Register,
}

impl Kind {
    pub fn of(join: &Join) -> Option<Kind> {
        match join.chain {
            ChainId::Names(_) => Some(Kind::Names),
            ChainId::Tokens(_) => Some(Kind::Tokens),
            ChainId::Ids => Some(Kind::Ids),
            ChainId::Authz(_) => Some(Kind::Authz),
            ChainId::Repo(_) if join.owner.is_some() => Some(Kind::Repo),
            #[cfg(feature = "test-faults")]
            ChainId::Repo(_) => Some(Kind::Register),
            ChainId::Release => Some(Kind::Release),
            #[cfg(not(feature = "test-faults"))]
            ChainId::Repo(_) => Some(Kind::Repo),
            ChainId::Identity(_) => Some(Kind::Identity),
            ChainId::Network(_) | ChainId::Actions(..) | ChainId::RepoMeta(_) => None,
        }
    }
}

// ---- the repository chain ----

/// `repo:<id>` members: `crates/git`'s `Push` on `RepoFold` (`tests/m1/results/m1-wp6.md`, "What the member host's
/// `Repo` slot must provide").
pub struct Repo;

fn repo_of(join: &Join) -> Option<RepoId> {
    match join.chain {
        ChainId::Repo(r) => Some(r),
        _ => None,
    }
}

/// How many repositories' packed dumps one isolate keeps: about 150 KB to 1.2 MB each at journey 9.
const DUMPS_MAX: usize = 8;

/// The state of both tiers a packed dump was built at: each tier's `applied` and row-hash `sum`, and whether the
/// metadata tier was loaded, which decides whether the dump holds its rows.
type DumpKey = (u64, [u8; 32], bool, u64, [u8; 32]);
/// A cached dump's key and its answer bytes (`DUMPS`).
type Dump = (DumpKey, std::rc::Rc<[u8]>);

thread_local! {
    /// The last packed whole-tier dump per repository (`dump_bytes`), by `DumpKey`, as the answer's serialized bytes,
    /// which every read at that state shares, whichever input granted it. A dump took up to 7.8 s of CPU on a 500-PR
    /// repository, and 200 API reads/s mostly read the same tiers at the same index (`tests/am1/results/
    /// lm1-memory.md`); a copy per input that granted reads was what `HEAVY_READS` bounded.
    static DUMPS: std::cell::RefCell<std::collections::HashMap<RepoId, Dump>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
    /// Packed dumps this isolate built (`/stats` `dumpBuilds`).
    static BUILDS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Packed dumps this isolate has built.
pub fn dump_builds() -> u64 {
    BUILDS.with(std::cell::Cell::get)
}

/// The answer to a packed whole-tier read of `fold` (`{"rows": true, "packed": true}`), as serialized JSON bytes: the
/// cached ones when the tiers stand where they were built, else built now and cached. The key is read from the tiers'
/// own meta rows after the read's grant, so a hit answers exactly what a rebuild would. `None` for any other query.
pub fn dump_bytes(fold: &git::repo::RepoFold, q: &Value) -> Option<std::rc::Rc<[u8]>> {
    use graftyard_api::hosted::{ROWS_MAX, TierRows};
    if !q.get("rows").is_some_and(Value::is_boolean) || q.get("packed").and_then(Value::as_bool) != Some(true) {
        return None;
    }
    let key = dump_key(fold);
    if let Some(repo) = fold.repo
        && let Some(b) = DUMPS.with(|d| d.borrow().get(&repo).filter(|(at, _)| *at == key).map(|(_, b)| b.clone()))
    {
        return Some(b);
    }
    let mut budget = ROWS_MAX;
    let (core, c_ok) = TierRows::dump(&fold.core, &git::tiers::CORE, &mut budget);
    let meta = fold.meta.loaded().then(|| TierRows::dump(&fold.meta, &git::tiers::METADATA, &mut budget));
    let complete = c_ok && meta.as_ref().is_none_or(|(_, ok)| *ok);
    let index = graftyard_fold::meta(&*fold.core).applied.0;
    let rows = TierRows { index, core, meta: meta.map(|(m, _)| m), complete };
    // `packed`: one JSON string, not a `Value` tree of 18 times its size (`TierRows::packed`).
    let bytes: std::rc::Rc<[u8]> = serde_json::to_vec(&rows.packed()).unwrap_or_else(|_| b"null".to_vec()).into();
    BUILDS.with(|b| b.set(b.get() + 1));
    if let Some(repo) = fold.repo {
        DUMPS.with(|d| {
            let mut d = d.borrow_mut();
            if d.len() >= DUMPS_MAX && !d.contains_key(&repo) {
                d.clear();
            }
            d.insert(repo, (key, bytes.clone()));
        });
    }
    Some(bytes)
}

/// How often a learner rebuilds its packed dump at most [guess]: a learner never votes, so a rebuild delays no
/// AppendEntries reply or confirm of the group's, only its own reads and applies; a rebuild of B0-29's 1.08 MB dump
/// took 80 to 100 ms of a learner's CPU, so this keeps rebuilds under about 20% of it. A granted read whose rebuild
/// is not due waits for it (`crate::member`), it is not shed (`tests/am1/results/learner-dumps.md`).
pub const LEARNER_DUMP_EVERY_MS: u64 = 500;

/// The rebuild spacing of the member at address `me`: DUMP_EVERY_MS on a voter, whose rebuilds run on the thread
/// that answers its AppendEntries and confirms, LEARNER_DUMP_EVERY_MS on a learner.
pub fn dump_every(me: graftyard_types::SiteId) -> u64 {
    if graftyard_types::learner_slot(me).is_some() { LEARNER_DUMP_EVERY_MS } else { DUMP_EVERY_MS }
}

fn dump_key(fold: &git::repo::RepoFold) -> DumpKey {
    let (c, m) = (graftyard_fold::meta(&*fold.core), graftyard_fold::meta(&*fold.meta));
    (c.applied.0, c.sum, fold.meta.loaded(), m.applied.0, m.sum)
}

/// How often a repository leader rebuilds its packed dump at most [guess]: in the 500-agent smoke the tiers changed
/// with nearly every entry, so the cache missed and each rebuild of a 513 KB dump ran inside the input that granted
/// the read, up to 1.2 s of CPU in one `/raft` input on the apac leader, which then stepped down on `CommitErrors`
/// (`tests/am1/results/lm1-memory.md`, "Rebuilds").
pub const DUMP_EVERY_MS: u64 = 1_000;

thread_local! {
    /// When each repository's leader last let a whole-tier read rebuild its dump (`dump_wait`).
    static REBUILT: std::cell::RefCell<std::collections::HashMap<RepoId, u64>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

/// For a packed whole-tier read of `fold` at `now`: `None` to serve it now, because the cached dump is at the tiers'
/// state or a rebuild is due (which is recorded); else the ms until one is. Either way the read itself still takes the
/// lease and reads the tiers' meta rows after it, so what it answers is fresh; this only spaces the rebuilds, by
/// `every` ms (`dump_every`).
pub fn dump_wait(fold: &git::repo::RepoFold, now: u64, every: u64) -> Option<u64> {
    let repo = fold.repo?;
    let key = dump_key(fold);
    if DUMPS.with(|d| d.borrow().get(&repo).is_some_and(|(k, _)| *k == key)) {
        return None;
    }
    REBUILT.with(|r| {
        let mut r = r.borrow_mut();
        match r.get(&repo) {
            Some(at) if now < at + every => Some(at + every - now),
            _ => {
                if r.len() >= 64 {
                    r.clear();
                }
                r.insert(repo, now);
                None
            }
        }
    })
}

/// The repository reads a follower serves by read-index (`design/DESIGN.md`, "Follower read"): whole-tier and merge
/// rows once its metadata tier is loaded, and the core-only access and agent rows. Refs, `ranges`, `dep`, `prsync`,
/// `mergequeue`, `tiers`, and `settings` reads stay on the leader.
pub fn follower_query(q: &Value, meta_loaded: bool) -> bool {
    match q.get("rows") {
        Some(Value::Bool(true)) => meta_loaded,
        Some(r) if r.get("merge").is_some() => meta_loaded,
        Some(r) => r.get("access").is_some() || r.get("agent").is_some(),
        None => false,
    }
}

/// Drops every cached dump, for a member whose fold storage refused a read.
pub fn forget_dumps() {
    DUMPS.with(|d| d.borrow_mut().clear());
}

impl Family for Repo {
    type Ev = git::repo::RepoEvent;
    type Fold = git::repo::RepoFold;
    type C = git::repo::RepoCmd;

    /// 32.1; 32.3 for the `PrSync` consumer's commands and the `TestMerged` and `ConsumerCursor` events; and 32.5 for
    /// issue commands and the events of tags 17 to 20 they write. 32.2 waits for a release to enable it through
    /// `FormatsEnable`. This test build reads 32.3 before any `FormatsEnable`, since only its own site scripts write it
    /// (`tests/m2/results/m2-wp5.md`, "Design gaps"). It reads 32.5 so that a later `FormatsEnable` can let the edge
    /// send it; until then the edge sends none (`design/SHARED.md`, the two-release rule, step 1: "Release N reads the
    /// format"). It reads 32.4, the merge commands', on the same terms as 32.5: the edge sends one only once
    /// `FormatsEnable` enabled it, and only this release's own `MergeQueue` consumer writes one before that. 32.6
    /// (collaborators and invitations), 32.7 (a push under a `ghs_` token), 32.8 (AM1-WP2's agent commands), and 32.9
    /// (an issue command under a `ghs_` token) are read on the same terms, as are 32.11 (`CancelInvitation`) and 32.12
    /// (AM1-WP3's task commands and any command whose token carries a `task` claim), and 32.13 (`rest:repos`'
    /// `InitRepo` and `DeleteRepo`). 32.15 is `RepoEvent::Fanout`, which only a leader of this release records itself;
    /// 32.17 holds F59's finisher steps, which only this release's own `Audit` consumer sends, once
    /// `GRAFTYARD_AUDIT` is set.
    fn reads() -> Vec<Format> {
        vec![
            git::repo::REPO_FORMAT_V1,
            git::prsync::REPO_FORMAT_V3,
            git::merge::REPO_FORMAT_V4,
            git::issue_ops::call::REPO_FORMAT_ISSUES,
            git::issue_ops::call::REPO_FORMAT_COLLABORATORS,
            git::repo::REPO_FORMAT_STATELESS,
            git::repo::REPO_FORMAT_AGENT,
            git::issue_ops::call::REPO_FORMAT_APP_CALLS,
            git::transfer::REPO_FORMAT_TRANSFER,
            git::issue_ops::call::REPO_FORMAT_INVITATIONS,
            git::repo::REPO_FORMAT_TASKS,
            git::repo::REPO_FORMAT_REPOS,
            git::repo::REPO_FORMAT_LEASES,
            git::repo::REPO_FORMAT_FANOUT,
            git::lifecycle::REPO_FORMAT_LIFECYCLE,
        ]
    }
    /// The repository and its owner, from the join, over the member's two tier table sets (`crate::tier`). A member
    /// joined with no owner holds none, and every push to it is refused `Forbidden` (`may_write` needs the owner's
    /// `Authz`). The metadata tier is attached by `load_fold`, never on a core request's path.
    fn new_fold(sql: &SqlStorage, join: &Join) -> git::repo::RepoFold {
        let (core, meta) = crate::tier::repo_tiers(sql);
        git::repo::RepoFold::with_tiers(repo_of(join), join.owner, Box::new(core), Box::new(meta))
    }
    /// The tiers' writes, buffered by `apply`, as statements in the input's transaction (`crate::tier::TierSql`).
    fn store_fold(fold: &git::repo::RepoFold, _entries: &[&raft::LogEntry<Self::Ev>], stmts: &mut Vec<Stmt>) {
        crate::tier::handle(&*fold.core).seal(stmts);
        crate::tier::handle(&*fold.meta).seal(stmts);
    }
    fn flushed(fold: &git::repo::RepoFold) {
        crate::tier::handle(&*fold.core).flushed();
        crate::tier::handle(&*fold.meta).flushed();
    }
    fn lost(fold: &git::repo::RepoFold) -> Option<String> {
        crate::tier::handle(&*fold.core).lost().or_else(|| crate::tier::handle(&*fold.meta).lost())
    }
    /// Feeds the activity feed's ring each applied entry (`crate::feed`), and marks a gap or an install, which stop
    /// the member following until it catches up. The ring waits for the owner's `Authz` (`crate::feed::Feed::owner`,
    /// read on the first poll) and keeps the entries meanwhile.
    fn stepped(m: &raft::Member<Self::C>, join: &Join, from: raft::Index, to: raft::Index, out: &raft::Out<Self::C>) {
        let Some(repo) = repo_of(join) else { return };
        let f = crate::feed::of(repo);
        let mut f = f.borrow_mut();
        let gap = out.early.iter().chain(&out.messages).any(|e| matches!(&e.msg, raft::Message::AppendRes(r) if !r.ok));
        let ok = out.early.iter().chain(&out.messages).any(|e| matches!(&e.msg, raft::Message::AppendRes(r) if r.ok));
        let was = (f.gap, f.installing);
        if gap {
            f.gap = true;
        } else if ok {
            f.gap = false;
        }
        if !out.install.is_empty() {
            f.installing = true;
        } else if to > from {
            f.installing = false;
        }
        if let Some(e) = m.entry_at(m.state().commit) {
            f.leader_at = f.leader_at.max(e.at);
        }
        if let Some(e) = m.entry_at(to) {
            f.applied_at = f.applied_at.max(e.at);
        }
        let fold = &m.group_fold().fold;
        // An install's input applies through the snapshot, whose entries this member never holds: entries waiting
        // from before it are past what the ring serves. `crate::load::finish_install` restarted the ring there.
        if to > from && m.entry_at(to).is_none() {
            f.pending.clear();
        }
        for i in from.0 + 1..=to.0 {
            let Some(e) = m.entry_at(raft::Index(i)) else { continue };
            let evs: Vec<git::repo::RepoEvent> = match &e.body {
                raft::EntryBody::Events(evs) => evs.clone(),
                _ => Vec::new(),
            };
            f.pending.push((e.index, e.at, evs));
        }
        // A member nobody polls keeps at most FEED_RING's count waiting; older entries are past what the ring keeps.
        let over = f.pending.len().saturating_sub(git::feed::FEED_RING.0);
        f.pending.drain(..over);
        if let Some(owner) = f.owner.clone() {
            let empty = graftyard_fold::MemStore::default();
            let meta: &dyn graftyard_fold::Read = if fold.meta.loaded() { &fold.meta } else { &empty };
            for (i, at, evs) in std::mem::take(&mut f.pending) {
                f.ring.feed(repo, &owner, &fold.core, meta, (i, at, &evs));
            }
        }
        if to > from || (gap, !out.install.is_empty()) != (false, false) || was != (f.gap, f.installing) {
            f.fed(crate::now_ms());
        }
    }
    /// The core tier, and the metadata tier marked loaded only if it stands where the core tier does; otherwise the
    /// alarm loads it (`crate::snap`).
    /// Before any cursor is open: `DROP TABLE` under an open statement fails with SQLITE_LOCKED, and `load` reads
    /// the log with cursors it stops early.
    fn prepare(sql: &SqlStorage) -> worker::Result<()> {
        for schema in [git::tiers::CORE, git::tiers::METADATA] {
            let made = crate::tier::reshape(sql, &schema)?;
            if !made.is_empty() {
                worker::console_log!("fold tables made again in this release's shape: {made:?}");
            }
        }
        Ok(())
    }
    fn load_fold(sql: &SqlStorage, join: &Join) -> worker::Result<git::repo::RepoFold> {
        let fold = Self::new_fold(sql, join);
        let caught_up = graftyard_fold::meta(&*fold.meta).applied == graftyard_fold::meta(&*fold.core).applied;
        crate::tier::handle(&*fold.meta).set_loaded(caught_up);
        Ok(fold)
    }
    /// A member that applied entries before its chain had tier tables (an M1 group on an M2 build) has a core tier
    /// behind `applied`. Its next `apply` panicked, and a panic resets the wasm instance of every member in the
    /// isolate (`tests/m2/results/m2-wp1.md`, "2026-11-04"). The stored log replays into the core tier here, 8 entries
    /// (at most 1.5 MB each) per transaction. A log that no longer holds the missing entries refuses the restore. The
    /// metadata tier is then loaded by the alarm (`crate::snap`).
    fn catch_up(storage: &worker::Storage, fold: &git::repo::RepoFold, applied: raft::Index) -> worker::Result<()> {
        use worker::Error;
        let from = graftyard_fold::meta(&*fold.core).applied;
        if from >= applied {
            return Ok(());
        }
        let (sql, tiers) = (storage.sql(), fold.tiers());
        let mut core = crate::tier::handle(&*fold.core).clone();
        let mut next = from.0 + 1;
        while next <= applied.0 {
            let q = "SELECT idx, row FROM log WHERE idx >= ? AND idx <= ? ORDER BY idx LIMIT 8";
            let page = crate::host::rows(&sql, q, vec![crate::host::i(next), crate::host::i(applied.0)])?;
            let mut gap = page.is_empty();
            for r in &page {
                let idx = crate::host::int(&r[0])? as u64;
                if idx != next {
                    gap = true;
                    break;
                }
                let e: raft::LogEntry<git::repo::RepoEvent> =
                    codec::from_bytes(crate::host::blob(&r[1])?).map_err(|e| Error::RustError(e.to_string()))?;
                graftyard_fold::apply(&tiers, &mut core, graftyard_types::Tier::Core, &e)
                    .map_err(|e| Error::RustError(format!("core tier catch-up at {idx}: {e}")))?;
                next += 1;
            }
            let mut stmts = Vec::new();
            core.seal(&mut stmts);
            crate::host::commit_tx(storage, stmts).map_err(Error::RustError)?;
            core.flushed();
            if gap {
                break;
            }
        }
        if next != applied.0 + 1 {
            return Err(Error::RustError(format!(
                "core tier at {from:?} behind applied {applied:?}, and the stored log does not hold entry {next}"
            )));
        }
        let meta = graftyard_fold::meta(&*fold.meta).applied;
        crate::tier::handle(&*fold.meta).set_loaded(meta == applied);
        Ok(())
    }
    fn tiered() -> bool {
        true
    }
    async fn on_alarm(host: &std::rc::Rc<crate::host::Host<Self>>) {
        crate::load::on_alarm(host).await;
        crate::prsync::on_alarm(host).await;
        crate::merge_queue::on_alarm(host).await;
        crate::snap::on_alarm(host).await;
        crate::archive::on_alarm(host);
    }
    fn schema() -> &'static [&'static str] {
        crate::snap::SCHEMA
    }
    /// A leader's alarm runs the `PrSync` dispatcher at once; every member's checks for a due snapshot.
    fn fold_alarm_ms(leading: bool) -> Option<i64> {
        Some(if leading { crate::prsync::LEADER_ALARM_MS } else { crate::snap::IDLE_MS })
    }
    fn route_paths() -> &'static [&'static str] {
        #[cfg(feature = "test-faults")]
        return &["/snapshot_commit", "/fold", "/bench_seed", "/feed", "/fanout", "/prsync"];
        #[cfg(not(feature = "test-faults"))]
        &["/snapshot_commit", "/fold", "/feed", "/fanout", "/prsync"]
    }
    /// `/snapshot_commit`, the leader's half of a snapshot (`crate::snap::commit`); `/fold`, each tier's digests (at
    /// `at`), the fold audit's log replay (`replay`), a paged refold of one tier's rows (`refold`), and the snapshot
    /// log (`log`); and on test installs `/bench_seed`.
    async fn route(
        host: &std::rc::Rc<crate::host::Host<Self>>,
        path: &str,
        body: Value,
    ) -> Option<worker::Result<Value>> {
        match path {
            "/feed" => Some(crate::feed::route(host, body).await),
            "/fanout" => Some(crate::fanout::report(host, body).await),
            // The operator's replay of a chain `PrSync` parked (`design/SHARED.md`, "Event fan-out", Isolation).
            "/prsync" if body["replay"] == json!(true) => Some(crate::prsync::replay(host).await),
            // Test installs: commit k at once, with no load behind it, to drive fan-out without saturating a shard.
            #[cfg(feature = "test-faults")]
            "/bench_seed" if body.get("fanout").is_some() => {
                Some(crate::fanout::force(host, body["fanout"].as_u64().unwrap_or(1) as u8).await)
            }
            "/snapshot_commit" => Some(crate::snap::commit(host, body).await),
            "/fold" => Some((|| {
                let core = host.core.borrow();
                let c = core.as_ref().ok_or_else(|| crate::host::err("member not joined"))?;
                let fold = &c.m.group_fold().fold;
                let tier = if body["tier"].as_u64() == Some(1) {
                    graftyard_types::Tier::Metadata
                } else {
                    graftyard_types::Tier::Core
                };
                if let Some(a) = body.get("audit") {
                    drop(core);
                    return crate::load::audit_route(host, a);
                }
                if let Some(at) = body.get("requestsAt").and_then(Value::as_u64) {
                    drop(core);
                    return crate::load::requests_json(host, at);
                }
                let mut v = match body.get("refold") {
                    Some(st) => crate::snap::refold_page(
                        fold,
                        tier,
                        Some(st.clone()),
                        body["budget"].as_u64().unwrap_or(20_000) as usize,
                    )?,
                    None => crate::snap::tiers_json(fold, body["at"].as_u64()),
                };
                if body.get("log").is_some() {
                    v["log"] = crate::snap::log_json(&host.state.storage().sql())?;
                    v["dbBytes"] = json!(host.state.storage().sql().database_size());
                }
                Ok(v)
            })()),
            // Test installs: B0-30's storage readings (`crate::b030`).
            #[cfg(feature = "test-faults")]
            "/bench_seed" if body.get("b030").is_some() => Some(crate::b030::route(host, &body["b030"]).await),
            // Test installs: one fold audit run, the cron's own, from this member's site.
            #[cfg(feature = "test-faults")]
            "/bench_seed" if body.get("audit").is_some() => {
                let only = body["audit"].as_str().and_then(|c| c.parse().ok());
                Some(crate::fold_audit::run(&host.env, only, body["force"].as_bool().unwrap_or(false)).await)
            }
            // Test installs: the leader's archive object of a stored log range (BF-29); the edge records `Archived`.
            #[cfg(feature = "test-faults")]
            "/bench_seed" if body.get("archive").is_some() => {
                let a = &body["archive"];
                Some(
                    crate::load::archive_put(host, a["from"].as_u64().unwrap_or(0), a["to"].as_u64().unwrap_or(0))
                        .await,
                )
            }
            // Test installs: this isolate's archive cap in place of `LOG_MAX_BYTES` (`crate::archive`).
            #[cfg(feature = "test-faults")]
            "/bench_seed" if body.get("archiveCap").is_some() => {
                host.arm_in(0);
                Some(Ok(crate::archive::set_cap(&body["archiveCap"])))
            }
            #[cfg(feature = "test-faults")]
            "/bench_seed" if body.get("openPr").is_some() => Some(crate::prsync::open_pr(host, &body["openPr"]).await),
            #[cfg(feature = "test-faults")]
            "/bench_seed"
                if body.get("mqKill").is_some() || body.get("mqHold").is_some() || body.get("mqHeld").is_some() =>
            {
                Some(Ok(crate::merge_queue::fault(host, &body)))
            }
            #[cfg(feature = "test-faults")]
            "/bench_seed" if body.get("record").is_some() => {
                Some(crate::merge_queue::record(host, &body["record"]).await)
            }
            #[cfg(feature = "test-faults")]
            "/bench_seed" if body.get("prsyncDrop").is_some() => {
                Some(Ok(crate::prsync::drop_first(host, body["prsyncDrop"].as_u64().unwrap_or(1) as u32)))
            }
            // Test installs: the core tier's next stored read fails in SQLite, as when the object has moved ("Network
            // connection lost"), so a request can show the member resets alone instead of panicking.
            #[cfg(feature = "test-faults")]
            "/bench_seed" if body.get("failRead").is_some() => Some((|| {
                let core = host.core.borrow();
                let c = core.as_ref().ok_or_else(|| crate::host::err("member not joined"))?;
                crate::tier::handle(&*c.m.group_fold().fold.core).fail_next_read();
                Ok(json!({ "failRead": true }))
            })()),
            // Test installs (L0-16): `{"metaUnload": true}` holds this member's metadata tier unloaded and counts
            // every read of it; `false` releases it (the alarm then loads it). Either answers the reads counted.
            #[cfg(feature = "test-faults")]
            "/bench_seed" if body.get("metaUnload").is_some() => Some((|| {
                let core = host.core.borrow();
                let c = core.as_ref().ok_or_else(|| crate::host::err("member not joined"))?;
                let m = crate::tier::handle(&*c.m.group_fold().fold.meta);
                let reads = match body["metaUnload"].as_bool() {
                    Some(h) => m.hold_unloaded(h),
                    None => m.reads_while_held().unwrap_or(0),
                };
                if body["metaUnload"] == false {
                    host.arm_in(0);
                }
                Ok(
                    json!({ "metaUnload": body["metaUnload"], "metaReads": reads, "loaded": m.reads_while_held().is_none() }),
                )
            })()),
            #[cfg(feature = "test-faults")]
            "/bench_seed" if body.get("load").is_some() => Some(crate::snap::bench_load(host, &body["load"]).await),
            // Test installs: the object's alarm as storage holds it, and the host's own idea of it.
            #[cfg(feature = "test-faults")]
            "/bench_seed" if body.get("alarm").is_some() => Some(
                host.state
                    .storage()
                    .get_alarm()
                    .await
                    .map(|at| json!({ "alarm": at, "armedAt": host.armed_at(), "now": crate::now_ms() })),
            ),
            #[cfg(feature = "test-faults")]
            "/bench_seed" => Some(crate::snap::bench_seed(host, &body)),
            _ => None,
        }
    }
    /// A push's `PushReport::of(&acked)`: the outcomes and the `report_status` bytes only an `Acked` can render. A
    /// sync's output is `{"Sync": SyncOut}`. An issue command's `{"issue": {"out", "reply"}}`, the client's reply
    /// rendered from the `&Acked` over this member's tiers, which have applied the entry
    /// (`graftyard_api::hosted::render_ok`).
    fn out_json(fold: &git::repo::RepoFold, a: &raft::Acked<Out<Self>>) -> Value {
        match &a.out {
            git::repo::RepoOut::Push(_) => serde_json::to_value(git::group::PushReport::of(a)).unwrap_or(Value::Null),
            git::repo::RepoOut::Sync(o) => json!({ "Sync": o }),
            git::repo::RepoOut::Transfer(o) => json!({ "Transfer": o }),
            git::repo::RepoOut::HookCursor(o) => json!({ "HookCursor": o }),
            git::repo::RepoOut::Lifecycle(o) => json!({ "Lifecycle": o }),
            git::repo::RepoOut::Issue(o) => {
                let reply = match serde_json::from_value::<graftyard_api::hosted::Render>(o.render.clone()) {
                    Ok(r) => graftyard_api::hosted::render_ok(&r, &a.map(|_| o.out.clone()), &fold.core, &fold.meta),
                    Err(e) => graftyard_api::hosted::WireReply {
                        status: 500,
                        headers: Vec::new(),
                        body: Some(json!({ "message": e.to_string() })),
                    },
                };
                json!({ "issue": { "out": format!("{:?}", o.out), "reply": reply } })
            }
            git::repo::RepoOut::Merge(o) => {
                let reply = (!o.render.is_null()).then(|| {
                    match serde_json::from_value::<graftyard_api::hosted::Render>(o.render.clone()) {
                        Ok(r) => graftyard_api::hosted::render_merge_ok(&r, &a.map(|_| o.out.clone()), &fold.core),
                        Err(e) => graftyard_api::hosted::WireReply {
                            status: 500,
                            headers: Vec::new(),
                            body: Some(json!({ "message": e.to_string() })),
                        },
                    }
                });
                json!({ "merge": { "out": o.out, "reply": reply } })
            }
        }
    }
    fn err_json(e: &Err<Self>) -> Value {
        use git::repo::RepoErr;
        match e {
            RepoErr::Push(e) => {
                let w = git::group::RefusedWire::of(e, 0);
                json!({ "err": w.err, "status": w.status })
            }
            RepoErr::Transfer(e) => json!({ "err": format!("{e:?}"), "status": 409 }),
            RepoErr::Lifecycle(e) => json!({ "err": format!("{e:?}"), "status": 409 }),
            RepoErr::Issue { err, render } => {
                let reply = serde_json::from_value::<graftyard_api::hosted::Render>(render.clone())
                    .map(|r| graftyard_api::hosted::render_err(&r, err))
                    .ok();
                json!({ "err": format!("{err:?}"), "status": reply.as_ref().map_or(422, |r| r.status), "issue": reply })
            }
            // `refusal` is what the `MergeQueue` consumer reads back for `on_refused`.
            RepoErr::Merge { n, err, render } => {
                let reply = serde_json::from_value::<graftyard_api::hosted::Render>(render.clone())
                    .map(|r| graftyard_api::hosted::render_merge_err(&r, err))
                    .ok();
                let refusal = git::merge_call::MergeRefusal { n: *n, err: err.clone() };
                json!({ "err": format!("{err:?}"), "status": reply.as_ref().map_or(422, |r| r.status),
                    "merge": { "refusal": refusal, "reply": reply } })
            }
        }
    }
    fn needs_block(e: &Err<Self>) -> bool {
        matches!(e, git::repo::RepoErr::Issue { err: git::issue_ops::IssueErr::NeedIdBlock, .. })
    }
    /// An issue command that names a blob: the `DurableBlob` again, from a HEAD of every bucket of the placement
    /// (`BlobStore::confirm`), so `decide` refuses it unless two buckets in two regions hold the bytes (BF-25).
    async fn admit(env: &worker::Env, chain: ChainId, cmd: &mut Self::C) {
        let (git::repo::RepoCmd::Issue(c), ChainId::Repo(repo)) = (cmd, chain) else { return };
        let Some(b) = c.op.blob().copied() else { return };
        let Ok(site) = crate::SiteEnv::read(env) else { return };
        let buckets: Vec<(graftyard_types::SiteId, git::host::R2)> = site
            .sites
            .0
            .iter()
            .filter_map(|s| env.bucket(&format!("BUCKET_{}", s.id.0)).ok().map(|b| (s.id, git::host::R2(b))))
            .collect();
        let refs: Vec<(graftyard_types::SiteId, &git::host::R2)> = buckets.iter().map(|(s, b)| (*s, b)).collect();
        let store = git::blob::BlobStore { sites: &site.sites, buckets: &refs, copies: git::Copies::TwoRegions };
        match store.confirm(repo, b.oid, b.bytes).await {
            Ok(d) => c.confirm(d),
            // `decide` refuses the command `Unavailable`; the client's reply does not say why.
            Err(e) => worker::console_log!("blob {} of {repo:?} not confirmed: {e:?}", b.oid.hex()),
        }
    }
    /// The refs as `[[name, oid]]`, what `graftyard_git::group::ReadValue` reads, from the core tier, while they fit one
    /// page, and with `{"q": {"refs": {..}}}` one page of them (`graftyard_git::group::refs_read`); with `{"q":
    /// {"rows": true}}`, both tiers' rows at this index, at most `graftyard_api::hosted::ROWS_MAX`
    /// (`graftyard_api::hosted::TierRows`), which the edge's `Repo` view reads, as one JSON string with `"packed":
    /// true` (`TierRows::packed`); with `{"q": {"rows": {"access":
    /// true}}}`, only the core rows the edge's read gate reads (`TierRows::access_rows`); with `{"q": {"rows":
    /// {"agent": true}}}`, only the core rows an agent push reads (`TierRows::agent_rows`); with `{"q":
    /// {"ranges": true}}`, the last `Promoted` and `Archived` and the first index still in the log, what the shard's
    /// `ChainLog` reads; with `{"q": {"tiers": true}}`, each tier's `applied` and `fold_digest`, and the digest after
    /// `{"at": index}` while the tier keeps it (`graftyard_fold::digest_at`), for the fold audit.
    fn follower_serves(fold: &git::repo::RepoFold, q: &Value) -> bool {
        follower_query(q, fold.meta.loaded())
    }
    fn heavy_wait(fold: &git::repo::RepoFold, q: &Value, now: u64, every: u64) -> Option<u64> {
        (q.get("packed").and_then(Value::as_bool) == Some(true)).then(|| dump_wait(fold, now, every)).flatten()
    }
    fn read_bytes(fold: &git::repo::RepoFold, q: &Value) -> Option<std::rc::Rc<[u8]>> {
        dump_bytes(fold, q)
    }
    fn read_json(fold: &git::repo::RepoFold, q: &Value) -> Value {
        // The owner and enterprise `deps` read (`RepoSettings`, else the owner the group was joined with), for the
        // `Audit` consumer's transfer plans.
        if q.get("settings").is_some() {
            return json!({ "owner": fold.authz_owner(), "enterprise": fold.authz_enterprise(),
                "created": git::tiers::created(&*fold.core).is_some(),
                "deleted": git::tiers::deleted(&*fold.core).is_some() });
        }
        if let Some(prs) = q.get("prsync") {
            return crate::prsync::read_json(fold, prs);
        }
        if let Some(base) = q.get("mergequeue") {
            return crate::merge_queue::read_json(fold, base);
        }
        if let Some(id) = q.get("rows").and_then(|r| r.get("merge")).and_then(Value::as_i64) {
            let meta: Option<&dyn graftyard_fold::Read> = fold.meta.loaded().then_some(&fold.meta);
            let mut t = graftyard_api::hosted::TierRows::merge_rows(&fold.core, meta, DbId(id));
            t.index = graftyard_fold::meta(&*fold.core).applied.0;
            return serde_json::to_value(t).unwrap_or(Value::Null);
        }
        if q.get("rows").and_then(|r| r.get("access")).is_some() {
            let core = graftyard_api::hosted::TierRows::access_rows(&fold.core);
            let index = graftyard_fold::meta(&*fold.core).applied.0;
            return serde_json::to_value(graftyard_api::hosted::TierRows { index, core, meta: None, complete: true })
                .unwrap_or(Value::Null);
        }
        if q.get("rows").and_then(|r| r.get("agent")).is_some() {
            let core = graftyard_api::hosted::TierRows::agent_rows(&fold.core);
            let index = graftyard_fold::meta(&*fold.core).applied.0;
            return serde_json::to_value(graftyard_api::hosted::TierRows { index, core, meta: None, complete: true })
                .unwrap_or(Value::Null);
        }
        // The packed dump, as `read_bytes` shares it (`dump_bytes`).
        if let Some(b) = dump_bytes(fold, q) {
            return serde_json::from_slice(&b).unwrap_or(Value::Null);
        }
        if q.get("rows").is_some() {
            use graftyard_api::hosted::{ROWS_MAX, TierRows};
            let mut budget = ROWS_MAX;
            let (core, c_ok) = TierRows::dump(&fold.core, &git::tiers::CORE, &mut budget);
            let meta = fold.meta.loaded().then(|| TierRows::dump(&fold.meta, &git::tiers::METADATA, &mut budget));
            let complete = c_ok && meta.as_ref().is_none_or(|(_, ok)| *ok);
            let index = graftyard_fold::meta(&*fold.core).applied.0;
            let rows = TierRows { index, core, meta: meta.map(|(m, _)| m), complete };
            return serde_json::to_value(rows).unwrap_or(Value::Null);
        }
        if q.get("ranges").is_some() {
            // `fanout`: the committed k, which the network's first shard reads on every catch-up (`crate::fanout`).
            return json!({
                "promoted": fold.promoted(),
                "archived": fold.archived(),
                "first": fold.first_index().0,
                "fanout": fold.fanout_k(),
            });
        }
        if q.get("tiers").is_some() {
            return crate::snap::tiers_json(fold, q.get("at").and_then(Value::as_u64));
        }
        // The refs: `{"refs": {..}}` one page of them, `{}` all of them only while they fit one page
        // (`graftyard_git::group::refs_read`), so no reply grows with the ref count.
        git::group::refs_read(fold, q)
    }
    fn fold_bytes(_fold: &git::repo::RepoFold) -> Vec<u8> {
        Vec::new()
    }
    fn dep(chain: ChainId, bytes: &[u8]) -> Result<IdentityDep, String> {
        identity_dep(chain, bytes)
    }
    /// `decode_commit`'s mapping: a `cmd` in a format this release does not read, or bytes that are no `Push` at
    /// all (format 0.0), answers `Unsupported` and touches nothing. An issue command names its format first.
    fn unreadable(body: &[u8]) -> Option<Value> {
        let v: Value = serde_json::from_slice(body).ok()?;
        if let Some(f) = ["/cmd/issue/format", "/cmd/merge/format", "/cmd/transfer/format"]
            .iter()
            .find_map(|p| v.pointer(p))
            .and_then(Value::as_array)
        {
            let n = |i: usize| f.get(i).and_then(Value::as_u64).unwrap_or(0) as u16;
            return Some(json!({ "unsupported": { "family": n(0), "version": n(1) } }));
        }
        let e = git::group::decode_commit(body, crate::host::RELEASE.id).err()?;
        serde_json::to_value(e).ok()
    }
    /// The committing leader marks the stored pack an ok push names, in each bucket its `sites` names, as
    /// `graftyard_git::mark_committed`, and the blob an ok asset upload or LFS `verify` names, as
    /// `graftyard_git::blob::mark_blob`. A push whose every ref was refused marks nothing.
    async fn after_ok(env: worker::Env, chain: ChainId, cmd: Self::C, acked: raft::Acked<Out<Self>>) -> usize {
        use git::repo::{RepoCmd, RepoOut};
        let ChainId::Repo(repo) = chain else { return 0 };
        let mark = match (&cmd, &acked.out) {
            (RepoCmd::Push(p), RepoOut::Push(o)) if o.refs.iter().any(|(_, o)| *o == git::repo::RefOutcome::Ok) => {
                match &p.pack {
                    Some(pack @ git::PackRef::Stored { .. }) => Ok(pack),
                    _ => return 0,
                }
            }
            // The blob the member confirmed, whose sites are the buckets that hold it; the edge's names none for a
            // `verify`.
            (RepoCmd::Issue(c), RepoOut::Issue(_)) => match c.confirmed {
                Some(d) => Err(d.blob()),
                None => return 0,
            },
            _ => return 0,
        };
        let Ok(site) = crate::SiteEnv::read(&env) else { return 1 };
        let buckets: Vec<(graftyard_types::SiteId, git::host::R2)> = site
            .sites
            .0
            .iter()
            .filter_map(|s| env.bucket(&format!("BUCKET_{}", s.id.0)).ok().map(|b| (s.id, git::host::R2(b))))
            .collect();
        let refs: Vec<(graftyard_types::SiteId, &git::host::R2)> = buckets.iter().map(|(s, b)| (*s, b)).collect();
        match mark {
            Ok(pack) => git::mark_committed(&refs, graftyard_types::NetworkId(repo.0), pack, chain, acked.index).await,
            Err(b) => git::blob::mark_blob(&refs, repo, &b, chain, acked.index).await,
        }
    }
    /// A `Promoted` names its pack too, so the sweeper repairs its marker as any other (`crate::archive`).
    fn row_packs(events: &[git::repo::RepoEvent]) -> Value {
        let promoted = events.iter().filter_map(|ev| match ev {
            git::repo::RepoEvent::Promoted(p) => Some(p.pack.clone()),
            _ => None,
        });
        json!(git::repo::packs_of(events).into_iter().chain(promoted).collect::<Vec<_>>())
    }
    fn archived(e: &raft::LogEntry<Self::Ev>) -> Option<ArchivedRange> {
        let raft::EntryBody::Events(evs) = &e.body else { return None };
        evs.iter().find_map(|ev| match ev {
            git::repo::RepoEvent::Archived(a) if matches!(a.key, git::R2Key::Archive { .. }) => {
                Some(ArchivedRange { key: a.key.to_string(), from: a.from.0, to: a.to.0, to_at: a.to_at.0 })
            }
            _ => None,
        })
    }
    async fn truncated(host: &std::rc::Rc<crate::host::Host<Self>>, to: raft::Index) {
        crate::load::set_source(host, to).await;
    }
    fn install_wanted(host: &std::rc::Rc<crate::host::Host<Self>>, w: raft::InstallWanted) {
        crate::load::want(host, w);
    }
    fn restored(host: &std::rc::Rc<crate::host::Host<Self>>) {
        crate::load::restored(host);
    }
    fn install_stmts(stmts: &mut Vec<Stmt>) {
        crate::load::install_stmts(stmts);
    }
    /// What the shard's `ChainLog` and the BF-16/17 drivers read: the packs the entry names, its ref updates, and
    /// its canonical length.
    fn entry_json(e: &raft::LogEntry<Self::Ev>) -> serde_json::Map<String, Value> {
        let mut m = serde_json::Map::new();
        let evs: &[git::repo::RepoEvent] = match &e.body {
            raft::EntryBody::Events(evs) => evs,
            _ => &[],
        };
        let packs = git::repo::packs_of(evs);
        m.insert("packs".into(), json!(packs));
        // `LogPacks.promoted`: the shard loads a promoted pack like a pack the entry names.
        if let Some(p) = evs.iter().find_map(|ev| match ev {
            git::repo::RepoEvent::Promoted(p) => Some(p),
            _ => None,
        }) {
            m.insert("promoted".into(), json!(p));
        }
        m.insert("ref_updates".into(), json!(evs.iter().map(|ev| ev.updates().len()).sum::<usize>()));
        // `LogPacks.refs`: a shard's catch-up applies these instead of reading the whole ref set.
        m.insert("refs".into(), json!(git::repo::entry_ref_changes(evs)));
        m.insert("bytes".into(), json!(raft::encode_entry(e).len()));
        m
    }
    /// BF-24's driver times a head push's entry to its `TestMerged`'s on the leader's clock: the first 8 ref names an
    /// entry updates, and each `TestMerged`.
    #[cfg(feature = "test-faults")]
    fn log_fields(e: &raft::LogEntry<Self::Ev>) -> serde_json::Map<String, Value> {
        let mut m = serde_json::Map::new();
        // L0-12's hosted stall reads which entry carried the recorded stall, of any body (a term's `Noop` too).
        if let Some(x) = e.extend {
            m.insert("extend".into(), json!({ "since": x.since.0, "byMs": x.by.as_millis() as u64 }));
        }
        let raft::EntryBody::Events(evs) = &e.body else { return m };
        let refs: Vec<Value> = evs
            .iter()
            .flat_map(|ev| ev.updates())
            .take(8)
            .map(|u| json!([u.name.display(), u.new.map(|o| o.hex())]))
            .collect();
        m.insert("refs".into(), json!(refs));
        // M2-WP3's audits: every event's kind, and each base-branch update with its expected value and the event it
        // rides in, so BF-21 can check that a base moves only in whole landings and only forward.
        let kinds: Vec<Value> = evs
            .iter()
            .map(|ev| match ev {
                git::repo::RepoEvent::Merged { n, .. } => json!(["Merged", n.0]),
                git::repo::RepoEvent::Queue(q) => json!([
                    "Queue",
                    match q {
                        git::queue::QueueEvent::Enqueued { n, .. } => format!("Enqueued {}", n.0),
                        git::queue::QueueEvent::GroupBuilt { entries, .. } => {
                            format!("GroupBuilt {:?}", entries.iter().map(|e| e.pr.0).collect::<Vec<_>>())
                        }
                        git::queue::QueueEvent::Ejected { n, reason, .. } => format!("Ejected {} {reason:?}", n.0),
                        git::queue::QueueEvent::Landed { through, .. } => format!("Landed {}", through.0),
                    }
                ]),
                other => json!([format!("{other:?}").split([' ', '(', '{']).next().unwrap_or("").to_owned()]),
            })
            .collect();
        m.insert("kinds".into(), json!(kinds));
        let moves: Vec<Value> = evs
            .iter()
            .enumerate()
            .flat_map(|(i, ev)| ev.updates().into_iter().map(move |u| (i, u)))
            .filter(|(_, u)| !u.name.as_bytes().starts_with(b"refs/heads/gh-readonly-queue/"))
            .filter(|(_, u)| u.name.as_bytes().starts_with(b"refs/heads/"))
            .map(|(i, u)| json!([i, u.name.display(), u.expect.map(|o| o.hex()), u.new.map(|o| o.hex())]))
            .collect();
        m.insert("branchMoves".into(), json!(moves));
        let merged: Vec<Value> = evs
            .iter()
            .filter_map(|ev| match ev {
                git::repo::RepoEvent::TestMerged { n, head, result, .. } => Some(json!({
                    "n": n.0,
                    "head": head.hex(),
                    "clean": matches!(result, git::issues::MergeabilityResult::Clean { .. }),
                })),
                _ => None,
            })
            .collect();
        m.insert("testMerged".into(), json!(merged));
        // AM1's hosted rows: each stored provenance source, so B0-37 counts linked and claimed trailers.
        let prov: Vec<Value> = evs
            .iter()
            .flat_map(|ev| match ev {
                git::repo::RepoEvent::Provenance { list, .. } => list.iter().collect::<Vec<_>>(),
                _ => Vec::new(),
            })
            .map(|p| json!([p.refname.display(), p.commit.hex(), p.sources]))
            .collect();
        m.insert("provenance".into(), json!(prov));
        m
    }
    /// A member joined with no owner holds a fold no push can write (`may_write` needs the owner), so it is trusted
    /// with the record it was sent: the CLI's latency probe joins `repo:0` that way.
    fn source(join: &Join) -> Source {
        match (repo_of(join), join.owner) {
            (Some(repo), Some(owner)) => Source::Repo { repo, owner },
            _ => Source::Given,
        }
    }
}

// ---- the test register (test installs only) ----

#[cfg(feature = "test-faults")]
pub struct Register;

#[cfg(feature = "test-faults")]
impl Family for Register {
    type Ev = RegEvent;
    type Fold = Reg;
    type C = RegCmd;

    fn reads() -> Vec<Format> {
        vec![REG_FORMAT]
    }
    fn new_fold(_sql: &SqlStorage, _join: &Join) -> Reg {
        Reg::default()
    }
    /// One `reg` row per key an applied entry touched.
    fn store_fold(fold: &Reg, entries: &[&raft::LogEntry<RegEvent>], stmts: &mut Vec<Stmt>) {
        let mut keys = std::collections::BTreeSet::new();
        for e in entries {
            if let raft::EntryBody::Events(evs) = &e.body {
                keys.extend(evs.iter().map(|ev| ev.key().to_owned()));
            }
        }
        for k in keys {
            let v = fold.0.get(&k).cloned().unwrap_or_default();
            stmts.push(("INSERT OR REPLACE INTO reg (k, v) VALUES (?, ?)", vec![k.into(), v.into()]));
        }
    }
    fn load_fold(sql: &SqlStorage, _join: &Join) -> worker::Result<Reg> {
        let mut reg = Reg::default();
        for r in crate::host::rows(sql, "SELECT k, v FROM reg", vec![])? {
            reg.0.insert(crate::host::text(&r[0])?.to_owned(), crate::host::text(&r[1])?.to_owned());
        }
        Ok(reg)
    }
    fn out_json(_fold: &Self::Fold, a: &raft::Acked<bool>) -> Value {
        json!(a.out)
    }
    fn err_json(e: &RegErr) -> Value {
        match e {
            RegErr::Stale => json!({ "err": "stale", "status": 422 }),
        }
    }
    fn read_json(fold: &Reg, q: &Value) -> Value {
        let key = q.get("key").and_then(Value::as_str).unwrap_or_default();
        json!(fold.0.get(key))
    }
    fn fold_bytes(_fold: &Reg) -> Vec<u8> {
        Vec::new()
    }
    fn dep(chain: ChainId, _bytes: &[u8]) -> Result<Reg, String> {
        Err(format!("the register reads no dependency ({chain})"))
    }
    fn source(_join: &Join) -> Source {
        Source::Given
    }
    #[cfg(feature = "test-faults")]
    fn audit(sql: &SqlStorage, fold: &Reg, prefix: &str) -> worker::Result<Value> {
        let mut disk = serde_json::Map::new();
        for r in crate::host::rows(
            sql,
            "SELECT k, v FROM reg WHERE substr(k, 1, length(?)) = ?",
            vec![prefix.into(), prefix.into()],
        )? {
            disk.insert(crate::host::text(&r[0])?.to_owned(), crate::host::text(&r[1])?.into());
        }
        let values: serde_json::Map<String, Value> =
            fold.0.iter().filter(|(k, _)| k.starts_with(prefix)).map(|(k, v)| (k.clone(), v.clone().into())).collect();
        Ok(json!({ "values": values, "diskValues": disk }))
    }
}

// ---- identity ----

/// The identity folds are stored whole, in the crate's codec, in one `fold` row written with the entries' apply.
/// Per-table rows come with the folds' own storage layout (`design/SHARED.md`, "Folds").
fn store_blob<T: Serialize>(fold: &T, stmts: &mut Vec<Stmt>) {
    stmts.push(("INSERT OR REPLACE INTO fold (id, row) VALUES (1, ?)", vec![codec::to_bytes(fold).into()]));
}

fn load_blob<T: DeserializeOwned>(sql: &SqlStorage) -> worker::Result<Option<T>> {
    match crate::host::rows(sql, "SELECT row FROM fold WHERE id = 1", vec![])?.first() {
        Some(r) => Ok(Some(
            codec::from_bytes(crate::host::blob(&r[0])?).map_err(|e| crate::host::err(format!("fold row: {e}")))?,
        )),
        None => Ok(None),
    }
}

/// 401 for a credential refusal, 403 for a permission refusal, 422 for the rest.
fn refusal(e: &impl Debug, status: u16) -> Value {
    json!({ "err": format!("{e:?}"), "status": status })
}

fn identity_dep(chain: ChainId, bytes: &[u8]) -> Result<IdentityDep, String> {
    let e = |x: codec::CodecError| format!("{chain}: {x}");
    match chain {
        ChainId::Names(_) => Ok(IdentityDep::Names(codec::from_bytes(bytes).map_err(e)?)),
        ChainId::Tokens(_) => Ok(IdentityDep::Tokens(codec::from_bytes(bytes).map_err(e)?)),
        ChainId::Authz(_) => Ok(IdentityDep::Authz(codec::from_bytes(bytes).map_err(e)?)),
        other => Err(format!("{other} is not an identity dependency")),
    }
}

fn q_id(q: &Value, k: &str) -> Option<AccountId> {
    q.get(k).and_then(Value::as_i64).map(|v| AccountId(DbId(v)))
}

fn cursor_json(c: &id::IdCursor) -> Value {
    json!({ "next": c.peek().map(|d| d.0), "remaining": c.remaining() })
}

pub struct Names;

impl Family for Names {
    type Ev = id::NamesEvent;
    type Fold = id::NamesFold;
    type C = id::NamesCmd;

    /// Format 3 is the `Audit` cursor's, written only by this release's own consumer once `GRAFTYARD_AUDIT` is set.
    fn reads() -> Vec<Format> {
        vec![id::NAMES_FORMAT, id::NAMES_FORMAT_V2, id::NAMES_FORMAT_V3]
    }
    /// A leader's alarm a minute after it applies entries runs the `Audit` consumer (`crate::audit`).
    fn fold_alarm_ms(leading: bool) -> Option<i64> {
        leading.then_some(id::audit::FINISH_AFTER.as_millis() as i64 + 1_000)
    }
    async fn on_alarm(host: &std::rc::Rc<crate::host::Host<Self>>) {
        crate::audit::on_alarm(host).await;
    }
    fn new_fold(_sql: &SqlStorage, join: &Join) -> id::NamesFold {
        match join.chain {
            ChainId::Names(s) => id::NamesFold::new(s),
            _ => id::NamesFold::default(),
        }
    }
    fn store_fold(fold: &id::NamesFold, _: &[&raft::LogEntry<id::NamesEvent>], stmts: &mut Vec<Stmt>) {
        store_blob(fold, stmts);
    }
    fn load_fold(sql: &SqlStorage, join: &Join) -> worker::Result<id::NamesFold> {
        Ok(load_blob(sql)?.unwrap_or_else(|| Self::new_fold(sql, join)))
    }
    fn out_json(_fold: &Self::Fold, a: &raft::Acked<id::NamesOut>) -> Value {
        serde_json::to_value(&a.out).unwrap_or(Value::Null)
    }
    fn err_json(e: &id::NamesErr) -> Value {
        match e {
            id::NamesErr::Unauthorized(_) => refusal(e, 401),
            id::NamesErr::Forbidden => refusal(e, 403),
            _ => refusal(e, 422),
        }
    }
    /// `{"placement": owner}`, `{"login": key}`, `{"account": id}`, `{"accountLogins": [id, ..]}`, `{"repoOwner": id}`,
    /// `{"ids": true}`, `{"slug": s}`, `{"app": id}`, `{"client": id}`, `{"installation": id}`.
    fn read_json(f: &id::NamesFold, q: &Value) -> Value {
        use id::NamesRead;
        let mut out = serde_json::Map::new();
        if let Some(o) = q_id(q, "placement") {
            out.insert("placement".into(), json!(f.placement(o)));
        }
        if let Some(k) = q.get("login").and_then(Value::as_str)
            && let Ok(l) = id::Login::parse(k)
        {
            out.insert("login".into(), json!(f.login(&l.key())));
        }
        if let Some(a) = q_id(q, "account") {
            out.insert("account".into(), json!(f.account(a)));
        }
        // The logins of the accounts this shard holds, by id: the edge renders other accounts from them (F50).
        if let Some(ids) = q.get("accountLogins").and_then(Value::as_array) {
            let m: serde_json::Map<String, Value> = ids
                .iter()
                .filter_map(Value::as_i64)
                .filter_map(|i| Some((i.to_string(), json!(f.account_login(AccountId(DbId(i)))?.as_str()))))
                .collect();
            out.insert("accountLogins".into(), Value::Object(m));
        }
        if let Some(r) = q.get("repoOwner").and_then(Value::as_i64) {
            out.insert("repoOwner".into(), json!(f.repo_owner(graftyard_types::RepoId(DbId(r)))));
        }
        if q.get("ids").is_some() {
            out.insert("ids".into(), cursor_json(&f.ids));
        }
        // M3's routes to an owner: an App by slug or id, a client id, an installation.
        if let Some(s) = q.get("slug").and_then(Value::as_str).and_then(|s| id::Slug::try_from(s.to_owned()).ok()) {
            out.insert("slug".into(), json!(f.slug(&s)));
        }
        if let Some(a) = q.get("app").and_then(Value::as_i64) {
            out.insert("app".into(), json!(f.app_owner(id::AppId(DbId(a)))));
        }
        if let Some(c) = q.get("client").and_then(Value::as_str).and_then(|c| id::ClientId::try_from(c.to_owned()).ok())
        {
            out.insert("client".into(), json!(f.client(&c)));
        }
        if let Some(i) = q.get("installation").and_then(Value::as_i64) {
            out.insert("installation".into(), json!(f.installation(id::InstallationId(DbId(i)))));
        }
        out.insert("logins".into(), json!(f.logins.len()));
        out.insert("placements".into(), json!(f.placements.len()));
        Value::Object(out)
    }
    fn fold_bytes(f: &id::NamesFold) -> Vec<u8> {
        codec::to_bytes(f)
    }
    fn dep(chain: ChainId, bytes: &[u8]) -> Result<IdentityDep, String> {
        identity_dep(chain, bytes)
    }
    fn needs_block(e: &id::NamesErr) -> bool {
        matches!(e, id::NamesErr::NeedIdBlock)
    }
    fn source(_join: &Join) -> Source {
        Source::Genesis
    }
}

pub struct Tokens;

impl Family for Tokens {
    type Ev = id::TokensEvent;
    type Fold = id::TokensFold;
    type C = id::TokensCmd;

    /// Format 3 is the `Audit` cursor's, as for `Names`.
    fn reads() -> Vec<Format> {
        vec![id::TOKENS_FORMAT, id::TOKENS_FORMAT_V2, id::TOKENS_FORMAT_V3]
    }
    /// A leader's alarm a minute after it applies entries runs the `Audit` consumer (`crate::audit`).
    fn fold_alarm_ms(leading: bool) -> Option<i64> {
        leading.then_some(id::audit::FINISH_AFTER.as_millis() as i64 + 1_000)
    }
    async fn on_alarm(host: &std::rc::Rc<crate::host::Host<Self>>) {
        crate::audit::on_alarm(host).await;
    }
    fn new_fold(_sql: &SqlStorage, join: &Join) -> id::TokensFold {
        match join.chain {
            ChainId::Tokens(s) => id::TokensFold::new(s),
            _ => id::TokensFold::default(),
        }
    }
    fn store_fold(fold: &id::TokensFold, _: &[&raft::LogEntry<id::TokensEvent>], stmts: &mut Vec<Stmt>) {
        store_blob(fold, stmts);
    }
    fn load_fold(sql: &SqlStorage, join: &Join) -> worker::Result<id::TokensFold> {
        Ok(load_blob(sql)?.unwrap_or_else(|| Self::new_fold(sql, join)))
    }
    fn out_json(_fold: &Self::Fold, _a: &raft::Acked<()>) -> Value {
        Value::Null
    }
    fn err_json(e: &id::TokensErr) -> Value {
        match e {
            id::TokensErr::Unauthorized(_) => refusal(e, 401),
            id::TokensErr::Forbidden => refusal(e, 403),
            _ => refusal(e, 422),
        }
    }
    /// `{"token": "<64 hex>"}`; for M3, `{"verifyingKey": kid}`, `{"jti": "<ULID>"}`, `{"sshKey": "<64 hex>"}`, and
    /// `{"code": "<64 hex>"}`. A code row comes with its `expires`, which the caller checks against its own clock.
    fn read_json(f: &id::TokensFold, q: &Value) -> Value {
        use id::TokensRead;
        let hex_of = |k: &str| q.get(k).and_then(Value::as_str).and_then(graftyard_types::Digest::parse_hex);
        let row = hex_of("token").map(|h| (h, f.token(&h.0)));
        let mut out = serde_json::Map::new();
        out.insert(
            "token".into(),
            json!(row.and_then(|(h, r)| r.map(|r| json!({
                "account": r.account.0.0,
                "revoked": r.revoked,
                "kind": r.kind,
                "expires": r.expires,
                "fields": f.fields(&h.0),
            })))),
        );
        out.insert("tokens".into(), json!(f.tokens.len()));
        if let Some(k) = q.get("verifyingKey").and_then(Value::as_u64) {
            let row = f.verifying_key(id::Kid(k as u32));
            out.insert("verifyingKey".into(), json!(row.map(|r| json!({ "key": r.key.0, "retired": r.retired }))));
        }
        if let Some(j) = q.get("jti").and_then(Value::as_str).and_then(|s| s.parse().ok()) {
            out.insert("jti".into(), json!(f.more.jtis.get(&id::Jti(j))));
        }
        if let Some(fp) = hex_of("sshKey") {
            let fp = id::Fingerprint(fp.0);
            out.insert("sshKey".into(), json!(f.ssh_key(&fp)));
            out.insert("sshRemoved".into(), json!(f.ssh_removed(&fp)));
        }
        if let Some(h) = hex_of("code") {
            out.insert("code".into(), json!(f.more.codes.get(&h.0)));
        }
        Value::Object(out)
    }
    fn fold_bytes(f: &id::TokensFold) -> Vec<u8> {
        codec::to_bytes(f)
    }
    fn dep(chain: ChainId, bytes: &[u8]) -> Result<IdentityDep, String> {
        identity_dep(chain, bytes)
    }
    fn source(_join: &Join) -> Source {
        Source::Genesis
    }
}

pub struct Authz;

impl Family for Authz {
    type Ev = id::AuthzEvent;
    type Fold = id::AuthzFold;
    type C = id::AuthzCmd;

    fn reads() -> Vec<Format> {
        // 0x12.3 is AM1-WP2's `AgentApp` and `OrgAgents` ops, which the edge sends only once `FormatsEnable` enabled it;
        // 0x12.4 the `Audit` cursor's, written only by this release's own consumer once `GRAFTYARD_AUDIT` is set;
        // 0x12.11 F59's create with its settings, which the edge sends only once `FormatsEnable` enabled it, and the
        // consumer's abort.
        vec![id::AUTHZ_FORMAT, id::AUTHZ_FORMAT_V2, id::AUTHZ_FORMAT_V3, id::AUTHZ_FORMAT_V4, id::AUTHZ_FORMAT_V11]
    }
    /// A leader's alarm a minute after it applies entries runs the `Audit` consumer (`crate::audit`): a transfer's
    /// `Reserved` name and an enterprise change commit on this chain.
    fn fold_alarm_ms(leading: bool) -> Option<i64> {
        leading.then_some(id::audit::FINISH_AFTER.as_millis() as i64 + 1_000)
    }
    async fn on_alarm(host: &std::rc::Rc<crate::host::Host<Self>>) {
        crate::audit::on_alarm(host).await;
    }
    /// `/finish`, the in-band first attempt at what a commit point just left owed (`crate::audit::finish_now`).
    fn route_paths() -> &'static [&'static str] {
        &["/finish"]
    }
    async fn route(
        host: &std::rc::Rc<crate::host::Host<Self>>,
        path: &str,
        body: Value,
    ) -> Option<worker::Result<Value>> {
        match path {
            "/finish" if crate::audit::enabled(&host.env) => Some(crate::audit::finish_now(host, body).await),
            "/finish" => Some(Ok(json!({ "error": "GRAFTYARD_AUDIT is not set" }))),
            _ => None,
        }
    }
    fn new_fold(_sql: &SqlStorage, join: &Join) -> id::AuthzFold {
        match join.chain {
            ChainId::Authz(o) => id::AuthzFold::new(o),
            _ => id::AuthzFold::default(),
        }
    }
    fn store_fold(fold: &id::AuthzFold, _: &[&raft::LogEntry<id::AuthzEvent>], stmts: &mut Vec<Stmt>) {
        store_blob(fold, stmts);
    }
    fn load_fold(sql: &SqlStorage, join: &Join) -> worker::Result<id::AuthzFold> {
        Ok(load_blob(sql)?.unwrap_or_else(|| Self::new_fold(sql, join)))
    }
    fn out_json(_fold: &Self::Fold, a: &raft::Acked<id::AuthzOut>) -> Value {
        serde_json::to_value(&a.out).unwrap_or(Value::Null)
    }
    fn err_json(e: &id::AuthzErr) -> Value {
        match e {
            id::AuthzErr::Unauthorized(_) => refusal(e, 401),
            id::AuthzErr::Forbidden => refusal(e, 403),
            _ => refusal(e, 422),
        }
    }
    /// `{"repo": name}`; always the opened kind, the name count, and the id cursor.
    fn read_json(f: &id::AuthzFold, q: &Value) -> Value {
        use id::AuthzRead;
        let repo = q
            .get("repo")
            .and_then(Value::as_str)
            .and_then(|n| id::RepoName::parse(n).ok())
            .and_then(|n| f.resolve(&n.key()))
            .map(|(r, live)| json!({ "repo": r.0.0, "live": live }));
        json!({
            "opened": f.opened.map(|k| format!("{k:?}")),
            "names": f.names.len(),
            "repo": repo,
            "ids": cursor_json(&f.ids),
        })
    }
    fn fold_bytes(f: &id::AuthzFold) -> Vec<u8> {
        codec::to_bytes(f)
    }
    fn dep(chain: ChainId, bytes: &[u8]) -> Result<IdentityDep, String> {
        identity_dep(chain, bytes)
    }
    fn needs_block(e: &id::AuthzErr) -> bool {
        matches!(e, id::AuthzErr::NeedIdBlock)
    }
    fn source(join: &Join) -> Source {
        match join.chain {
            ChainId::Authz(o) => Source::Owner(o),
            _ => Source::Given,
        }
    }
}

/// `identity:<account>` members: the account's own record, sessions, and credential list (M3-WP3). The chain sits
/// under the owner's record, as `Authz` does (`design/DESIGN.md`, Shape, `PlacementRecord`: "It covers the owner's
/// `Authz`, `Identity`, `Actions`, and repository chains").
pub struct Identity;

impl Family for Identity {
    type Ev = id::IdentityEvent;
    type Fold = id::IdentityFold;
    type C = id::IdentityCmd;

    /// 0x14.2 is `AddKey` with its `KeyText` (BF-35), read on the same terms as every M3 format: the edge sends it only
    /// once `FormatsEnable` enabled it.
    fn reads() -> Vec<Format> {
        vec![id::IDENTITY_FORMAT, id::IDENTITY_FORMAT_V2]
    }
    fn new_fold(_sql: &SqlStorage, join: &Join) -> id::IdentityFold {
        match join.chain {
            ChainId::Identity(a) => id::IdentityFold::new(a),
            _ => id::IdentityFold::default(),
        }
    }
    fn store_fold(fold: &id::IdentityFold, _: &[&raft::LogEntry<id::IdentityEvent>], stmts: &mut Vec<Stmt>) {
        store_blob(fold, stmts);
    }
    fn load_fold(sql: &SqlStorage, join: &Join) -> worker::Result<id::IdentityFold> {
        Ok(load_blob(sql)?.unwrap_or_else(|| Self::new_fold(sql, join)))
    }
    fn out_json(_fold: &Self::Fold, _a: &raft::Acked<()>) -> Value {
        Value::Null
    }
    fn err_json(e: &id::IdentityErr) -> Value {
        match e {
            id::IdentityErr::Unauthorized(_) | id::IdentityErr::Suspended => refusal(e, 401),
            id::IdentityErr::Forbidden => refusal(e, 403),
            _ => refusal(e, 422),
        }
    }
    /// The created capture, the credential list with titles (what `GET /user/keys` lists), and the session count.
    /// The password hash and second factors never leave the member.
    fn read_json(f: &id::IdentityFold, _q: &Value) -> Value {
        let credentials: Vec<Value> = f
            .credentials
            .iter()
            .map(|(c, title)| match c {
                id::CredentialRef::Token { hash, kind } => {
                    json!({ "token": graftyard_types::Digest(*hash).hex(), "kind": kind, "title": title.as_ref() })
                }
                id::CredentialRef::SshKey(fp) => json!({
                    "sshKey": graftyard_types::Digest(fp.0).hex(), "title": title.as_ref(),
                    "key": f.more.key_text.get(fp).map(AsRef::<str>::as_ref),
                }),
            })
            .collect();
        json!({
            "account": f.account.map(|a| a.0.0),
            "created": f.created,
            "credentials": credentials,
            "sessions": f.sessions.len(),
            "password": f.password.is_some(),
        })
    }
    fn fold_bytes(f: &id::IdentityFold) -> Vec<u8> {
        codec::to_bytes(f)
    }
    fn dep(chain: ChainId, bytes: &[u8]) -> Result<IdentityDep, String> {
        identity_dep(chain, bytes)
    }
    fn source(join: &Join) -> Source {
        match join.chain {
            ChainId::Identity(a) => Source::Owner(a),
            _ => Source::Given,
        }
    }
}

pub struct Ids;

impl Family for Ids {
    type Ev = id::IdsEvent;
    type Fold = id::IdsFold;
    type C = id::IdsCmd;

    fn reads() -> Vec<Format> {
        vec![id::IDS_FORMAT]
    }
    /// One cell, cell 0, on the test install.
    fn new_fold(_sql: &SqlStorage, _join: &Join) -> id::IdsFold {
        id::IdsFold::new(0)
    }
    fn store_fold(fold: &id::IdsFold, _: &[&raft::LogEntry<id::IdsEvent>], stmts: &mut Vec<Stmt>) {
        store_blob(fold, stmts);
    }
    fn load_fold(sql: &SqlStorage, join: &Join) -> worker::Result<id::IdsFold> {
        Ok(load_blob(sql)?.unwrap_or_else(|| Self::new_fold(sql, join)))
    }
    fn out_json(_fold: &Self::Fold, a: &raft::Acked<id::IdBlock>) -> Value {
        serde_json::to_value(a.out).unwrap_or(Value::Null)
    }
    fn err_json(e: &id::IdsErr) -> Value {
        refusal(e, 422)
    }
    fn read_json(f: &id::IdsFold, _q: &Value) -> Value {
        json!({ "cell": f.cell, "nextBlock": f.next_block })
    }
    fn fold_bytes(f: &id::IdsFold) -> Vec<u8> {
        codec::to_bytes(f)
    }
    fn dep(chain: ChainId, bytes: &[u8]) -> Result<IdentityDep, String> {
        identity_dep(chain, bytes)
    }
    fn source(_join: &Join) -> Source {
        Source::Genesis
    }
}
