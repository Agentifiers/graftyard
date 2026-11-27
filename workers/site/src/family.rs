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
    /// The fold's rows held in memory until their transaction runs, and their bytes (`crate::tier::TierSql::buffered`).
    fn buffered(_fold: &Self::Fold) -> (usize, usize) {
        (0, 0)
    }
    /// The rows the fold's read memos hold and their bytes (`crate::tier`, "Read memo").
    fn memo(_fold: &Self::Fold) -> (usize, usize) {
        (0, 0)
    }
    /// Test installs: the fold's stored reads SQLite ran and those its read memos answered (`/stats` `foldReads`).
    #[cfg(feature = "test-faults")]
    fn fold_reads(_fold: &Self::Fold) -> (u64, u64) {
        (0, 0)
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
    /// Whether a purge may retire this member (`graftyard_git::purge`): only a repository chain whose fold holds the
    /// `Deleted` tombstone, or its metadata child, which holds no core tier and is purged on its parent's.
    fn purgeable(_fold: &Self::Fold) -> bool {
        false
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
    /// For a granted whole-tier or narrow read `q` at `now`: the ms until its answer may be rebuilt (`dump_wait`,
    /// `narrow_wait`), or `None`. `floor` is the grant's (`raft::ReadGrant::floor`), `every` the member's dump rebuild
    /// spacing (`dump_every`).
    fn heavy_wait(_fold: &Self::Fold, _q: &Value, _floor: u64, _now: u64, _every: u64) -> Option<u64> {
        None
    }
    /// What a `fresh` read answers for `q` as serialized JSON bytes shared across reads, when the family keeps them
    /// (a repository's packed dump, `dump_bytes`, and narrow answers at or above the grant's `floor`,
    /// `narrow_bytes`); else `None` and the host serializes `read_json`.
    fn read_bytes(_fold: &Self::Fold, _q: &Value, _floor: u64) -> Option<std::rc::Rc<[u8]>> {
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
    /// What `after_ok` needs of an admitted command, kept with its ticket until it resolves, or `None` when it does
    /// nothing for this command. Never the whole command: that held every inline pack and reply render a second time.
    fn marked(_cmd: &Self::C) -> Option<Self::C> {
        None
    }
    /// A repository chain's network (`graftyard_git::tiers::network`), which keys its packs; `None` for every other
    /// family.
    fn network(_fold: &Self::Fold, _chain: ChainId) -> Option<graftyard_types::NetworkId> {
        None
    }
    /// After an `ok` outcome, on the leader: the work a command's commit owes outside the log, such as the
    /// repository's `Committed` pack markers. `cmd` is what `marked` kept. Returns how many puts failed.
    async fn after_ok(
        _env: worker::Env,
        _chain: ChainId,
        _network: Option<graftyard_types::NetworkId>,
        _cmd: Self::C,
        _acked: raft::Acked<Out<Self>>,
    ) -> usize {
        0
    }
    /// The stored packs a `request` row's events name, for the sweeper's `RequestSource` (`{"q": {"request": id}}`).
    fn row_packs(_events: &[Self::Ev]) -> Value {
        json!([])
    }
    /// The archives an applied entry commits, in order: members record each range (`archive_key`) and truncate their
    /// stored log through its `to` once they can HEAD its key in two buckets.
    fn archived(_e: &raft::LogEntry<Self::Ev>) -> Vec<ArchivedRange> {
        Vec::new()
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
    /// Rows an answer built from the fold lacked (`crate::split::take_missed`): read them where the family keeps
    /// them, so the answer can be built again.
    async fn fetch_missed(_host: &std::rc::Rc<crate::host::Host<Self>>, _missed: &Value) -> Result<(), String>
    where
        Self: Sized,
    {
        Ok(())
    }
    /// After restore built the core: the stored install source, and a load job to resume.
    fn restored(_host: &std::rc::Rc<crate::host::Host<Self>>)
    where
        Self: Sized,
    {
    }
    /// Whether this member's group has background work only its leader does now (`Host::keep_led`): a repository's
    /// split step, archive, or promotion.
    fn needs_leader(_host: &std::rc::Rc<crate::host::Host<Self>>) -> bool
    where
        Self: Sized,
    {
        false
    }
    /// Whether this member may start a campaign or take a transfer now, beside the Raft core's own rules: a repository
    /// parent member whose install dropped its metadata tables may not (`crate::load::meta_dropped`).
    fn may_lead(_host: &std::rc::Rc<crate::host::Host<Self>>) -> bool
    where
        Self: Sized,
    {
        true
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
    /// An owner's Actions shard (`crate::actions`).
    Actions,
    /// A fork network's chain (`graftyard_git::network`, AM3-WP2a).
    Network,
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
            // A repository's metadata child (`crate::split`) is hosted by the repository family.
            ChainId::RepoMeta(_) => Some(Kind::Repo),
            ChainId::Actions(..) => Some(Kind::Actions),
            ChainId::Network(_) => Some(Kind::Network),
        }
    }
}

// ---- the repository chain ----

/// `repo:<id>` members: `crates/git`'s `Push` on `RepoFold` (`tests/m1/results/m1-wp6.md`, "What the member host's
/// `Repo` slot must provide").
pub struct Repo;

fn repo_of(join: &Join) -> Option<RepoId> {
    match join.chain {
        ChainId::Repo(r) | ChainId::RepoMeta(r) => Some(r),
        _ => None,
    }
}

/// How many repositories' packed dumps one isolate keeps: about 150 KB to 1.2 MB each at journey 9.
const DUMPS_MAX: usize = 8;

/// Which fold a packed dump was built from: the core tier's `applied`, whether the metadata tier was loaded, which
/// decides whether the dump holds its rows, and whether the fold is the repository's metadata child, whose log is its
/// own and whose indexes are not the repository's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DumpAt {
    applied: u64,
    loaded: bool,
    child: bool,
}

impl DumpAt {
    fn of(fold: &git::repo::RepoFold) -> DumpAt {
        DumpAt { applied: graftyard_fold::meta(&*fold.core).applied.0, loaded: fold.meta.loaded(), child: fold.child }
    }

    /// Whether a dump built at `self` answers a read of `fold` granted at `floor`: built from the same kind of fold
    /// (`loaded`, `child`) at or above the floor. Every applied entry is committed and the fold is a function of the
    /// committed log, so a dump built at index i by any member of the repository in this isolate is the answer every
    /// member gives at i, and one at or above the floor holds every entry acknowledged before the read arrived, as
    /// `narrow_hit`'s rule.
    fn covers(self, fold: &git::repo::RepoFold, floor: u64) -> bool {
        let now = DumpAt::of(fold);
        self.loaded == now.loaded && self.child == now.child && self.applied >= floor
    }
}

/// A cached dump's state and its answer bytes (`DUMPS`).
type Dump = (DumpAt, std::rc::Rc<[u8]>);

thread_local! {
    /// The newest packed whole-tier dump per repository (`dump_bytes`), with the state it was built at, as the answer's
    /// serialized bytes, which every read whose floor it covers shares, whichever input granted it. A dump took up to 7.8 s of CPU on a 500-PR
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

/// The bytes this isolate's read caches hold, shared by every repository in it: packed dumps and narrow answers
/// (`/stats` `memSplit`).
pub fn cache_bytes() -> serde_json::Value {
    let dumps = DUMPS.with(|d| d.borrow().values().map(|(_, b)| b.len()).sum::<usize>());
    let narrow = NARROW
        .with(|n| n.borrow().values().flat_map(|v| v.iter()).map(|(t, _, _, b)| t.len() + b.len()).sum::<usize>());
    json!({ "dumps": dumps, "narrow": narrow })
}

/// The cached dump of `fold`'s repository that answers a read granted at `floor` (`DumpAt::covers`).
fn dump_hit(fold: &git::repo::RepoFold, floor: u64) -> Option<std::rc::Rc<[u8]>> {
    let repo = fold.repo?;
    DUMPS.with(|d| d.borrow().get(&repo).filter(|(at, _)| at.covers(fold, floor)).map(|(_, b)| b.clone()))
}

/// Whether `q` is a packed whole-tier read (`{"rows": true, "packed": true}`).
fn is_dump(q: &Value) -> bool {
    q.get("rows").is_some_and(Value::is_boolean) && q.get("packed").and_then(Value::as_bool) == Some(true)
}

/// The answer to a packed whole-tier read of `fold` granted at `floor` (`raft::ReadGrant::floor`), as serialized JSON
/// bytes: the newest cached dump of the repository when it was built at or above the floor (`DumpAt::covers`), else
/// built now at the fold's state and cached. A learner's waiters for one rebuild are all answered by it, though an
/// entry applied while they waited: required to stand at the tiers' state, every one but the first found it stale
/// and the next rebuild past its deadline, and shed (B0-33 m61, `tests/am1/results/b033-m61-why.md`). `None` for any
/// other query.
pub fn dump_bytes(fold: &git::repo::RepoFold, q: &Value, floor: u64) -> Option<std::rc::Rc<[u8]>> {
    use graftyard_api::hosted::{Budget, TierRows};
    if !is_dump(q) {
        return None;
    }
    if let Some(b) = dump_hit(fold, floor) {
        return Some(b);
    }
    let at = DumpAt::of(fold);
    let mut budget = Budget::READ;
    let (core, c_ok) = TierRows::dump(&fold.core, &git::tiers::CORE, &mut budget);
    let meta = fold.meta.loaded().then(|| TierRows::dump(&fold.meta, &git::tiers::METADATA, &mut budget));
    let complete = c_ok && meta.as_ref().is_none_or(|(_, ok)| *ok);
    let mut rows = TierRows { index: at.applied, core, meta: meta.map(|(m, _)| m), complete, picks: None };
    // A split parent's slimmed rows, filled from the child's; a dump that missed a child row is not kept.
    let m0 = crate::split::mark();
    crate::split::fill_answer(fold, &mut rows);
    if let Some(miss) = crate::split::since(m0) {
        return Some(serde_json::to_vec(&miss).unwrap_or_default().into());
    }
    // `packed`: one JSON string, not a `Value` tree of 18 times its size (`TierRows::packed`).
    let bytes: std::rc::Rc<[u8]> = serde_json::to_vec(&rows.packed()).unwrap_or_else(|_| b"null".to_vec()).into();
    BUILDS.with(|b| b.set(b.get() + 1));
    if let Some(repo) = fold.repo {
        DUMPS.with(|d| {
            let mut d = d.borrow_mut();
            if d.len() >= DUMPS_MAX && !d.contains_key(&repo) {
                d.clear();
            }
            d.insert(repo, (at, bytes.clone()));
        });
    }
    Some(bytes)
}

/// One repository's newest narrow answers, by query text and whether the metadata tier was loaded: each with the
/// applied index it was built at.
type NarrowAnswers = Vec<(String, u64, bool, std::rc::Rc<[u8]>)>;

thread_local! {
    /// The newest narrow API answer per repository and query (`narrow_bytes`), served to any read whose floor it is
    /// at or above. Every applied entry is committed, and the fold is a function of the committed log, so an answer
    /// built at index `i` by any member of the repository in this isolate is the answer every member gives at `i`.
    static NARROW: std::cell::RefCell<std::collections::HashMap<RepoId, NarrowAnswers>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
    /// Narrow answers this isolate built (`/stats` `narrowBuilds`).
    static NARROW_BUILDS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Narrow answers this isolate has built.
pub fn narrow_builds() -> u64 {
    NARROW_BUILDS.with(std::cell::Cell::get)
}

/// Queries kept per repository [guess]: journey 9's API reads are a handful of distinct queries.
const NARROW_PER_STATE: usize = 32;

/// How often a member builds one narrow API answer (one query of one repository) at most [guess]: an answer reads
/// only the page it renders but selects over the whole table (500 PRs' rows for journey 9's lists), about 2 to 4 ms
/// natively on a 500-PR repository (`tests/am1/results/narrow-reads-3.md`). Reads whose floor the newest answer
/// already covers never wait for it; one whose floor is newer waits for the next build and is answered at the
/// member's state then (`raft::Member::fresh_after`), with no second read-index round (`crate::member`).
pub const NARROW_EVERY_MS: u64 = 100;

thread_local! {
    /// When each repository's member last built each narrow answer, by query (`narrow_wait`).
    static NARROW_BUILT: std::cell::RefCell<std::collections::HashMap<(RepoId, String), u64>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

/// The cached answer to query `text` of `repo` built at `floor` or later with the metadata tier as `loaded`.
fn narrow_hit(repo: RepoId, text: &str, floor: u64, loaded: bool) -> Option<std::rc::Rc<[u8]>> {
    NARROW.with(|n| {
        n.borrow().get(&repo).and_then(|v| {
            v.iter().find(|(t, at, l, _)| t == text && *at >= floor && *l == loaded).map(|(_, _, _, b)| b.clone())
        })
    })
}

/// For a narrow API read of `fold` granted at `floor`, at `now`: `None` to serve it now, because a cached answer is at
/// or above its floor or a build is due (which is recorded); else the ms until one is, as `dump_wait` spaces dumps.
pub fn narrow_wait(fold: &git::repo::RepoFold, q: &Value, floor: u64, now: u64) -> Option<u64> {
    let repo = fold.repo?;
    let text = q.to_string();
    if narrow_hit(repo, &text, floor, fold.meta.loaded()).is_some() {
        return None;
    }
    NARROW_BUILT.with(|b| {
        let mut b = b.borrow_mut();
        let k = (repo, text);
        match b.get(&k) {
            Some(at) if now < at + NARROW_EVERY_MS => Some(at + NARROW_EVERY_MS - now),
            _ => {
                if b.len() >= 256 {
                    b.clear();
                }
                b.insert(k, now);
                None
            }
        }
    })
}

/// The answer to a narrow API read (`{"rows": {"api": ..}}`, `graftyard_api::narrow::answer`) granted at `floor`, as
/// serialized bytes: the newest cached answer to the same query when it was built at `floor` or later, else built
/// now at the fold's state and cached. `None` for any other query.
pub fn narrow_bytes(fold: &git::repo::RepoFold, q: &Value, floor: u64) -> Option<std::rc::Rc<[u8]>> {
    q.get("rows")?.get("api")?;
    let (repo, text, loaded) = (fold.repo?, q.to_string(), fold.meta.loaded());
    if let Some(b) = narrow_hit(repo, &text, floor, loaded) {
        return Some(b);
    }
    let at = graftyard_fold::meta(&*fold.core).applied.0;
    let v = <Repo as Family>::read_json(fold, q);
    let bytes: std::rc::Rc<[u8]> = serde_json::to_vec(&v).unwrap_or_else(|_| b"null".to_vec()).into();
    NARROW_BUILDS.with(|b| b.set(b.get() + 1));
    if crate::split::is_miss(&v) {
        return Some(bytes);
    }
    NARROW.with(|n| {
        let mut n = n.borrow_mut();
        if n.len() >= DUMPS_MAX && !n.contains_key(&repo) {
            n.clear();
        }
        let v = n.entry(repo).or_default();
        // A build happens only when no cached answer covers the read's floor, so it is the newest one this isolate
        // has for the query.
        match v.iter().position(|(t, _, l, _)| *t == text && *l == loaded) {
            Some(i) => v[i] = (text, at, loaded, bytes.clone()),
            None if v.len() < NARROW_PER_STATE => v.push((text, at, loaded, bytes.clone())),
            None => {}
        }
    });
    Some(bytes)
}

/// Every account `fold`'s whole tiers name (`graftyard_api::hosted::accounts`), which a narrow API answer needs to
/// tell the edge the logins a whole-tier read knew: each tier's derived index, kept by `apply` (`TierSql::named`).
/// A fold over other stores (the unit tests' `MemStore`) scans.
fn named_accounts(fold: &git::repo::RepoFold) -> std::collections::BTreeSet<AccountId> {
    let sql = |t: &dyn graftyard_fold::TierStore| t.as_any().downcast_ref::<crate::tier::TierSql>().map(|h| h.named());
    match (sql(&*fold.core), sql(&*fold.meta)) {
        (Some(c), Some(m)) => c.into_iter().chain(m).collect(),
        _ => graftyard_api::hosted::accounts(&fold.core, &fold.meta),
    }
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

/// For a packed whole-tier read of `fold` granted at `floor`, at `now`: `None` to serve it now, because a cached dump
/// covers the floor (`DumpAt::covers`) or a rebuild is due (which is recorded); else the ms until one is. This only
/// spaces the rebuilds, by `every` ms (`dump_every`); what a read answers is never below its floor.
pub fn dump_wait(fold: &git::repo::RepoFold, floor: u64, now: u64, every: u64) -> Option<u64> {
    let repo = fold.repo?;
    if dump_hit(fold, floor).is_some() {
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
/// rows once its metadata tier is loaded, the core-only access, agent, and actions rows, and the edge's `ls-refs`
/// read (`graftyard_git::ls_refs::read`, HEAD and its pages from the core tier), and a git cache shard's `ranges` read
/// on its own site's member (core rows only, `decisions/DECISIONS.md`, `catchup-local`). Paged `refs`, `dep`, `prsync`,
/// `mergequeue`, `tiers`, and `settings` reads stay on the leader.
pub fn follower_query(q: &Value, meta_loaded: bool) -> bool {
    if q.get("lsRefs").is_some() || *q == serde_json::json!({ "ranges": true }) {
        return true;
    }
    match q.get("rows") {
        Some(Value::Bool(true)) => meta_loaded,
        Some(r) if graftyard_api::hosted::MergeOf::of_query(r).is_some() => meta_loaded,
        Some(r) if r.get("api").is_some() => meta_loaded,
        // A metadata child's rows (`crate::split::child_rows`): any member whose restored tier is loaded.
        Some(r) if r.get("child").is_some() => meta_loaded,
        // A command read that needs the metadata tier (`CommandSpec::loaded`): a loaded member, as its whole-tier read.
        Some(r) if r.get("command").and_then(|c| c.get("loaded")) == Some(&Value::Bool(true)) => meta_loaded,
        Some(r) => r.get("access").is_some() || r.get("agent").is_some() || r.get("actions").is_some(),
        None => false,
    }
}

/// Drops every cached dump, for a member whose fold storage refused a read.
pub fn forget_dumps() {
    DUMPS.with(|d| d.borrow_mut().clear());
    NARROW.with(|n| n.borrow_mut().clear());
}

/// Whether a repository member's metadata tier serves reads and takes `apply`'s writes: once it stands where the
/// core tier does, or on a metadata child once its seed's restore finished (`crate::split`).
/// A parent member whose install dropped its metadata tables (`dropped`, `crate::load::meta_dropped`) is never loaded,
/// whatever its tier's meta row says, until the install's `Write::Install` clears the mark.
pub(crate) fn meta_loaded(fold: &git::repo::RepoFold, caught_up: bool, dropped: bool) -> bool {
    !dropped && if fold.child { crate::split::child_loaded(fold) } else { caught_up }
}

/// `Repo::read_json` before a split parent's fills are checked (`crate::split`).
fn repo_read(fold: &git::repo::RepoFold, q: &Value) -> Value {
    // The owner and enterprise `deps` read (`RepoSettings`, else the owner the group was joined with), for the
    // `Audit` consumer's transfer plans.
    if q.get("settings").is_some() {
        return json!({ "owner": fold.authz_owner(), "enterprise": fold.authz_enterprise(),
            "created": git::tiers::created(&*fold.core).is_some(),
            "deleted": git::tiers::deleted(&*fold.core).is_some() });
    }
    // The `Webhooks` consumer's committed cursor and the applied index, for BF-57's cursor lag.
    if q.get("hookCursor").is_some() {
        return json!({ "cursor": git::hooks::hook_cursor(&*fold.core),
            "applied": graftyard_fold::meta(&*fold.core).applied.0 });
    }
    if let Some(prs) = q.get("prsync") {
        return crate::prsync::read_json(fold, prs);
    }
    if let Some(base) = q.get("mergequeue") {
        return crate::merge_queue::read_json(fold, base);
    }
    // A metadata child's state and rows (`crate::split`), which its parent's leader and members read.
    if q.get("split").is_some() {
        return crate::split::child_view(fold);
    }
    if let Some(keys) = q.get("rows").and_then(|r| r.get("child")) {
        return crate::split::child_rows(fold, keys);
    }
    // Test installs: metadata rows by key, as a reply reads them (b030-split's smoke).
    #[cfg(feature = "test-faults")]
    if let Some(keys) = q.get("rows").and_then(|r| r.get("keys")) {
        let keys: Vec<(String, Vec<graftyard_fold::Val>)> = serde_json::from_value(keys.clone()).unwrap_or_default();
        return crate::split::with_filled(fold, |m| {
            json!(keys.iter().map(|(t, k)| (t, k, m.get(t, k))).collect::<Vec<_>>())
        });
    }
    // After a split, every read of the metadata tier sees each slimmed field filled from the child
    // (`crate::split::with_filled`, `fill_answer`).
    if let Some(of) = q.get("rows").and_then(graftyard_api::hosted::MergeOf::of_query) {
        let mut t = crate::split::with_filled(fold, |m| {
            let meta: Option<&dyn graftyard_fold::Read> = fold.meta.loaded().then_some(m);
            graftyard_api::hosted::TierRows::merge_rows(&fold.core, meta, of)
        });
        t.index = graftyard_fold::meta(&*fold.core).applied.0;
        return serde_json::to_value(t).unwrap_or(Value::Null);
    }
    // A narrow API read (`graftyard_api::narrow`): the rows one REST or GraphQL reply reads, not both whole tiers.
    if let Some(spec) = q.get("rows").and_then(|r| r.get("api")) {
        let index = graftyard_fold::meta(&*fold.core).applied.0;
        // The answer's metadata rows are recorded as `Filled` reads them.
        return crate::split::with_filled(fold, |m| {
            let meta: Option<&dyn graftyard_fold::Read> = fold.meta.loaded().then_some(m);
            graftyard_api::narrow::answer(&fold.core, meta, index, spec, &|| named_accounts(fold))
        });
    }
    // A command read (`hosted::CommandSpec`, `command-rows`): the access rows, the one task the command names and its
    // leases, and the login picks, with `"command": true` so the edge knows this member answered it.
    if let Some(spec) = q.get("rows").filter(|r| r.get("access").is_some()).and_then(|r| r.get("command")) {
        let Ok(spec) = serde_json::from_value::<graftyard_api::hosted::CommandSpec>(spec.clone()) else {
            return json!({ "error": "not a command read spec" });
        };
        let index = graftyard_fold::meta(&*fold.core).applied.0;
        let rows = crate::split::with_filled(fold, |m| {
            let meta: Option<&dyn graftyard_fold::Read> = fold.meta.loaded().then_some(m);
            graftyard_api::hosted::TierRows::command_rows(&fold.core, meta, index, &spec, &|| named_accounts(fold))
        });
        let mut v = serde_json::to_value(rows).unwrap_or(Value::Null);
        v["command"] = json!(true);
        return v;
    }
    if let Some(r) = q.get("rows").filter(|r| r.get("access").is_some()) {
        // A push's read gate and its agent rows in one read (`{"access": true, "agent": true}`): both sets, and
        // `"agent": true` beside them, so an edge whose member predates this reads the agent rows on their own.
        let both = r.get("agent").is_some();
        let mut core = graftyard_api::hosted::TierRows::access_rows(&fold.core);
        if both {
            core.extend(graftyard_api::hosted::TierRows::agent_rows(&fold.core));
        }
        let index = graftyard_fold::meta(&*fold.core).applied.0;
        let mut v = serde_json::to_value(graftyard_api::hosted::TierRows {
            index,
            core,
            meta: None,
            complete: true,
            picks: None,
        })
        .unwrap_or(Value::Null);
        if both {
            v["agent"] = json!(true);
        }
        return v;
    }
    let core_only = match q.get("rows") {
        Some(r) if r.get("agent").is_some() => Some(graftyard_api::hosted::TierRows::agent_rows(&fold.core)),
        Some(r) if r.get("actions").is_some() => Some(graftyard_api::hosted::TierRows::actions_rows(&fold.core)),
        _ => None,
    };
    if let Some(core) = core_only {
        let index = graftyard_fold::meta(&*fold.core).applied.0;
        return serde_json::to_value(graftyard_api::hosted::TierRows {
            index,
            core,
            meta: None,
            complete: true,
            picks: None,
        })
        .unwrap_or(Value::Null);
    }
    // The packed dump, as `read_bytes` shares it (`dump_bytes`), at or above the fold's own state.
    if let Some(b) = dump_bytes(fold, q, graftyard_fold::meta(&*fold.core).applied.0) {
        return serde_json::from_slice(&b).unwrap_or(Value::Null);
    }
    if q.get("rows").is_some() {
        use graftyard_api::hosted::{Budget, TierRows};
        let mut budget = Budget::READ;
        let (core, c_ok) = TierRows::dump(&fold.core, &git::tiers::CORE, &mut budget);
        let meta = fold.meta.loaded().then(|| TierRows::dump(&fold.meta, &git::tiers::METADATA, &mut budget));
        let complete = c_ok && meta.as_ref().is_none_or(|(_, ok)| *ok);
        let index = graftyard_fold::meta(&*fold.core).applied.0;
        let mut rows = TierRows { index, core, meta: meta.map(|(m, _)| m), complete, picks: None };
        crate::split::fill_answer(fold, &mut rows);
        return serde_json::to_value(rows).unwrap_or(Value::Null);
    }
    if q.get("ranges").is_some() {
        // `fanout`: the committed k, which the network's first shard reads on every catch-up (`crate::fanout`).
        // `compactions`: every committed roll, which the shard's `ChainLog::compactions` reads the produced packs of
        // in place of their consumed promotions (`decisions/DECISIONS.md`, `roll-up`).
        // `default_branch`: the shard's HEAD, as the edge's ls-refs names it (G26), `refs/heads/main` with no
        // `Settings` row (`graftyard_git::tiers::default_branch`).
        let default_branch = git::tiers::default_branch(&*fold.core).display();
        return json!({
            "promoted": fold.promoted(),
            "compactions": fold.compactions(),
            "archived": fold.archived(),
            "first": fold.first_index().0,
            "fanout": fold.fanout_k(),
            "default_branch": default_branch,
        });
    }
    if q.get("tiers").is_some() {
        return crate::snap::tiers_json(fold, q.get("at").and_then(Value::as_u64));
    }
    // The edge's v2 `ls-refs`: HEAD and every page from this one fold (`graftyard_git::ls_refs::read`).
    if let Some(l) = q.get("lsRefs") {
        return git::ls_refs::read(fold, l);
    }
    // The refs: `{"refs": {..}}` one page of them, `{}` all of them only while they fit one page
    // (`graftyard_git::group::refs_read`), so no reply grows with the ref count.
    git::group::refs_read(fold, q)
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
    /// `GRAFTYARD_AUDIT` is set. 32.18 is `IssueOp::CreateCheck`, sent once `FormatsEnable` enabled it. 32.16 is
    /// M4-WP1's hook commands, which the edge sends once `FormatsEnable` enabled it, and the `Webhooks` consumer's
    /// `HookCursorCmd`, which only a leader of this release commits, for a repository that has a hook. 32.21 is
    /// M4-WP3's `RunChecksCmd`, which only `RunOwner` sends, from a release whose Actions leaders run it. 32.22 is a
    /// review comment outside a review, whose entry carries `ReviewEvent::SingleComment`, sent once `FormatsEnable` enabled it.
    /// 32.24 is R24's `RunChecksCmd` with `workflow_job` steps, which `RunOwner` sends only where `GRAFTYARD_APP_HOOKS` is set.
    /// 32.25 is `RepoEvent::Compacted`, which only a leader of this release records itself. 32.26 is a push over SSH
    /// (M6-WP2), whose principal is an SSH key, sent once `FormatsEnable` enabled it. 32.28 is a Graftyard-run create
    /// (AM3-WP3), sent once `FormatsEnable` enabled it. 32.29 is a REST issue create or an App's commit status
    /// (`bf77-routes`), sent once `FormatsEnable` enabled it. 32.33 is a title or body edit or an assignment change
    /// that may pause or resume a Graftyard-run task (`host-pause`), sent once `FormatsEnable` enabled it. A
    /// `test-format` build also reads
    /// BM-5's `REPO_FORMAT_TEST`, which no other release reads.
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
            git::issue_ops::call::REPO_FORMAT_CHECKS,
            git::split::REPO_FORMAT_SPLIT,
            git::hooks::REPO_FORMAT_HOOKS,
            git::watch::REPO_FORMAT_WATCH,
            git::issue_ops::call::REPO_FORMAT_REVIEWS,
            git::run_checks::REPO_FORMAT_ACTIONS,
            git::secrets::REPO_FORMAT_SECRETS,
            git::workflow_job::REPO_FORMAT_WORKFLOW_JOB,
            git::repo::REPO_FORMAT_ROLLUP,
            git::repo::REPO_FORMAT_SSH,
            git::repo::REPO_FORMAT_RUNS,
            git::issue_ops::call::REPO_FORMAT_ROUTES,
            git::issue_ops::call::REPO_FORMAT_ASSIGN,
            git::issue_ops::call::REPO_FORMAT_ISSUE_COMMENT,
            git::issue_ops::call::REPO_FORMAT_HOST_PAUSE,
            git::issue_ops::call::REPO_FORMAT_FORKS,
            git::issue_ops::call::REPO_FORMAT_ISSUE_EDIT,
            git::issue_ops::call::REPO_FORMAT_LABEL_UPDATE,
            git::issue_ops::call::REPO_FORMAT_MILESTONES,
            git::issue_ops::call::REPO_FORMAT_APP_AUTHOR,
            #[cfg(feature = "test-format")]
            git::repo::REPO_FORMAT_TEST,
        ]
    }
    /// The repository and its owner, from the join, over the member's two tier table sets (`crate::tier`). A member
    /// joined with no owner holds none, and every push to it is refused `Forbidden` (`may_write` needs the owner's
    /// `Authz`). The metadata tier is attached by `load_fold`, never on a core request's path.
    /// A metadata child (`repo-meta:<id>`) is hosted the same way; its tier is the parent's seed (`crate::split`).
    fn new_fold(sql: &SqlStorage, join: &Join) -> git::repo::RepoFold {
        let (core, meta) = crate::tier::repo_tiers(sql);
        let child = matches!(join.chain, ChainId::RepoMeta(_));
        let mut f = git::repo::RepoFold::with_tiers(repo_of(join), join.owner, Box::new(core), Box::new(meta));
        f.child = child;
        if child {
            crate::tier::handle(&*f.meta).set_loaded(crate::split::child_loaded(&f));
        }
        f
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
    fn buffered(fold: &git::repo::RepoFold) -> (usize, usize) {
        let (c, m) = (crate::tier::handle(&*fold.core).buffered(), crate::tier::handle(&*fold.meta).buffered());
        (c.0 + m.0, c.1 + m.1)
    }
    fn memo(fold: &git::repo::RepoFold) -> (usize, usize) {
        let (c, m) = (crate::tier::handle(&*fold.core).memo_size(), crate::tier::handle(&*fold.meta).memo_size());
        (c.0 + m.0, c.1 + m.1)
    }
    #[cfg(feature = "test-faults")]
    fn fold_reads(fold: &git::repo::RepoFold) -> (u64, u64) {
        let (c, m) = (crate::tier::handle(&*fold.core).read_counts(), crate::tier::handle(&*fold.meta).read_counts());
        (c.0 + m.0, c.1 + m.1)
    }
    /// Feeds the activity feed's ring each applied entry (`crate::feed`), and marks a gap or an install, which stop
    /// the member following until it catches up. The ring waits for the owner's `Authz` (`crate::feed::Feed::owner`,
    /// read on the first poll) and keeps the entries meanwhile.
    fn stepped(m: &raft::Member<Self::C>, join: &Join, from: raft::Index, to: raft::Index, out: &raft::Out<Self::C>) {
        let ChainId::Repo(repo) = join.chain else { return };
        let f = crate::feed::of(&join.me.do_name, m.boot());
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
        // Until a poll read the owner, nothing is kept: that poll starts the ring at the member's applied entry
        // (`crate::feed::Feed::owner_read`). An install's entries are not in the log; `crate::load::finish_install`
        // restarted the ring there.
        let crate::feed::Feed { owner, ring, .. } = &mut *f;
        if let Some(owner) = owner.as_ref() {
            let empty = graftyard_fold::MemStore::default();
            let meta: &dyn graftyard_fold::Read = if fold.meta.loaded() { &fold.meta } else { &empty };
            for i in from.0 + 1..=to.0 {
                let Some(e) = m.entry_at(raft::Index(i)) else { continue };
                let evs: &[git::repo::RepoEvent] = match &e.body {
                    raft::EntryBody::Events(evs) => evs,
                    _ => &[],
                };
                ring.feed(repo, owner, &fold.core, meta, (e.index, e.at, evs));
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
        crate::tier::handle(&*fold.meta).set_loaded(meta_loaded(&fold, caught_up, crate::load::meta_dropped(sql)));
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
        let dropped = crate::load::meta_dropped(&sql);
        crate::tier::handle(&*fold.meta).set_loaded(meta_loaded(fold, meta == applied, dropped));
        Ok(())
    }
    fn tiered() -> bool {
        true
    }
    async fn on_alarm(host: &std::rc::Rc<crate::host::Host<Self>>) {
        // Each step's ms, logged when one is long (`crate::alarm_steps`).
        let mut steps = crate::alarm_steps::AlarmSteps::start(crate::now_ms());
        crate::split::on_alarm(host).await;
        steps.mark("split").await;
        crate::load::on_alarm(host).await;
        steps.mark("load").await;
        crate::prsync::on_alarm(host).await;
        steps.mark("prsync").await;
        crate::merge_queue::on_alarm(host).await;
        steps.mark("merge_queue").await;
        crate::hookseq::on_alarm(host).await;
        steps.mark("hookseq").await;
        crate::activity::on_alarm(host).await;
        steps.mark("activity").await;
        crate::actions_trigger::on_alarm(host).await;
        steps.mark("actions_trigger").await;
        crate::snap::on_alarm(host).await;
        steps.mark("snap").await;
        crate::archive::on_alarm(host);
        if let Some(line) = steps.line() {
            let name = host.core.borrow().as_ref().map(|c| c.join.me.do_name.to_string()).unwrap_or_default();
            worker::console_log!("{name} {line}");
        }
    }
    fn schema() -> &'static [&'static str] {
        crate::snap::SCHEMA
    }
    /// A leader's alarm runs the `PrSync` dispatcher at once; every member's checks for a due snapshot.
    fn fold_alarm_ms(leading: bool) -> Option<i64> {
        Some(if leading { crate::prsync::LEADER_ALARM_MS } else { crate::snap::IDLE_MS })
    }
    fn purgeable(fold: &git::repo::RepoFold) -> bool {
        git::purge::purgeable(fold)
    }
    fn route_paths() -> &'static [&'static str] {
        #[cfg(feature = "test-faults")]
        return &["/snapshot_commit", "/fold", "/bench_seed", "/feed", "/fanout", "/prsync", "/activity", "/split"];
        #[cfg(not(feature = "test-faults"))]
        &["/snapshot_commit", "/fold", "/feed", "/fanout", "/prsync", "/activity", "/split"]
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
            "/split" => Some(crate::split::route(host, body).await),
            // M4-WP2: a shard rebuild's documents and an `Inbox` rebuild's notices (`crate::activity::route`).
            "/activity" => Some(crate::activity::route(host, &body).await),
            "/fanout" => Some(crate::fanout::report(host, body).await),
            // The operator's replay of a chain `PrSync` parked (`design/SHARED.md`, "Event fan-out", Isolation).
            "/prsync" if body["replay"] == json!(true) => Some(crate::prsync::replay(host).await),
            // Test installs: ask the warm sites' pool ledgers for k at once, with no load behind it, and commit what
            // both granted, to drive fan-out without saturating a shard; `pool_down` (ms) first takes both ledgers down.
            #[cfg(feature = "test-faults")]
            "/bench_seed" if body.get("fanout").is_some() => Some(
                crate::fanout::force(host, body["fanout"].as_u64().unwrap_or(1) as u8, body["pool_down"].as_u64())
                    .await,
            ),
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
                // The fold audit's skip (`crate::fold_audit`): whether this member holds an entry past `since` that
                // writes the fold.
                if let Some(since) = body["since"].as_u64() {
                    v["moved"] = json!(crate::load::moved(raft::Index(since), c.m.log_base().index, c.m.log()));
                }
                // The fold audit's live-row check (`crate::live`): the last result, and a new check when one is due.
                if body["live"] == json!(true) {
                    drop(core);
                    v["live"] = crate::live::route(host)?;
                    return Ok(v);
                }
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
            // Test installs: this chain's `SPLIT_AT` in this isolate (`crate::split::set_split_at`).
            #[cfg(feature = "test-faults")]
            "/bench_seed" if body.get("splitAt").is_some() => {
                let chain = host.core.borrow().as_ref().map(|c| c.join.chain);
                host.arm_in(0);
                Some(
                    chain
                        .ok_or_else(|| worker::Error::RustError("not joined".into()))
                        .map(|c| crate::split::set_split_at(c, &body["splitAt"])),
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
            "/bench_seed" if body.get("prsyncFirstFor").is_some() => {
                Some(Ok(crate::prsync::first_for(host, body["prsyncFirstFor"].as_u64().unwrap_or(0))))
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
            // Test installs (read-memo): `{"memo": false}` turns this member's fold read memos off, `true` back on, so a
            // row can read the same member both ways; either drops what they held.
            #[cfg(feature = "test-faults")]
            "/bench_seed" if body.get("memo").is_some() => Some((|| {
                let core = host.core.borrow();
                let c = core.as_ref().ok_or_else(|| crate::host::err("member not joined"))?;
                let on = body["memo"].as_bool().unwrap_or(true);
                let f = &c.m.group_fold().fold;
                crate::tier::handle(&*f.core).set_memo(on);
                crate::tier::handle(&*f.meta).set_memo(on);
                Ok(json!({ "memo": on }))
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
            // Test installs (`seal-batch`'s probe): per-call against per-row, and per-transaction against per-byte,
            // SQLite cost on this member's own storage, in scratch tables dropped before it answers.
            #[cfg(feature = "test-faults")]
            "/bench_seed" if body.get("sqlProbe").is_some() => Some(crate::sqlprobe::run(host, &body["sqlProbe"])),
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
            git::repo::RepoOut::Counts(o) => json!({ "Counts": o }),
            git::repo::RepoOut::RunChecks(o) => json!({ "RunChecks": o }),
            git::repo::RepoOut::Issue(o) => {
                let m0 = crate::split::mark();
                // `leader-render`: a first send's render carries no owner `Authz`; the entry's dependency fills it.
                let mut source = None;
                let reply = match serde_json::from_value::<graftyard_api::hosted::Render>(o.render.clone()) {
                    Ok(mut r) => {
                        source = graftyard_api::hosted::fill_authz(&mut r, o.authz.as_deref());
                        crate::split::with_filled(fold, |m| {
                            graftyard_api::hosted::render_ok(&r, &a.map(|_| o.out.clone()), &fold.core, m)
                        })
                    }
                    Err(e) => graftyard_api::hosted::WireReply {
                        status: 500,
                        headers: Vec::new(),
                        body: Some(json!({ "message": e.to_string() })),
                    },
                };
                let mut v = json!({ "issue": { "out": format!("{:?}", o.out), "reply": reply } });
                if let Some(s) = source {
                    v["issue"]["authz"] = json!(s);
                }
                // A reply that missed a child row says so; the member reads the rows and renders it again.
                if let Some(mut miss) = crate::split::since(m0) {
                    v["missedChild"] = miss["missedChild"].take();
                }
                v
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
    /// true` (`TierRows::packed`); with `{"q": {"rows": {"api": spec}}}`, only the rows one API reply reads
    /// (`graftyard_api::narrow::answer`); with `{"q": {"rows": {"access":
    /// true}}}`, only the core rows the edge's read gate reads (`TierRows::access_rows`); with `{"q": {"rows":
    /// {"agent": true}}}`, only the core rows an agent push reads (`TierRows::agent_rows`); with `{"q": {"rows":
    /// {"actions": true}}}`, only the core rows `acquirejob` reads (`TierRows::actions_rows`); with `{"q":
    /// {"ranges": true}}`, the last `Promoted` and `Archived` and the first index still in the log, what the shard's
    /// `ChainLog` reads; with `{"q": {"tiers": true}}`, each tier's `applied` and `fold_digest`, and the digest after
    /// `{"at": index}` while the tier keeps it (`graftyard_fold::digest_at`), for the fold audit.
    fn follower_serves(fold: &git::repo::RepoFold, q: &Value) -> bool {
        follower_query(q, fold.meta.loaded())
    }
    fn heavy_wait(fold: &git::repo::RepoFold, q: &Value, floor: u64, now: u64, every: u64) -> Option<u64> {
        if q.get("rows").and_then(|r| r.get("api")).is_some() {
            return narrow_wait(fold, q, floor, now);
        }
        // Only a whole-tier dump is spaced: a merge's rows (`{"rows": {"merge": id}}`) carry `packed` too, but build
        // no dump, and spaced as one they waited out a rebuild they never made and took its slot (`merge-p99`).
        is_dump(q).then(|| dump_wait(fold, floor, now, every)).flatten()
    }
    fn read_bytes(fold: &git::repo::RepoFold, q: &Value, floor: u64) -> Option<std::rc::Rc<[u8]>> {
        dump_bytes(fold, q, floor).or_else(|| narrow_bytes(fold, q, floor))
    }
    fn read_json(fold: &git::repo::RepoFold, q: &Value) -> Value {
        let m0 = crate::split::mark();
        let v = repo_read(fold, q);
        crate::split::since(m0).unwrap_or(v)
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
        let e = git::group::decode_commit(body, crate::host::cached_release().id).err()?;
        serde_json::to_value(e).ok()
    }
    fn marked(cmd: &Self::C) -> Option<Self::C> {
        cmd.marked()
    }
    fn network(fold: &Self::Fold, chain: ChainId) -> Option<graftyard_types::NetworkId> {
        match chain {
            ChainId::Repo(r) => Some(git::tiers::network(&*fold.core, r)),
            _ => None,
        }
    }
    /// The committing leader marks the stored pack an ok push names, in each bucket its `sites` names, as
    /// `graftyard_git::mark_committed`, and the blob an ok asset upload or LFS `verify` names, as
    /// `graftyard_git::blob::mark_blob`. A push whose every ref was refused marks nothing.
    async fn after_ok(
        env: worker::Env,
        chain: ChainId,
        network: Option<graftyard_types::NetworkId>,
        cmd: Self::C,
        acked: raft::Acked<Out<Self>>,
    ) -> usize {
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
            Ok(pack) => {
                git::mark_committed(
                    &refs,
                    network.unwrap_or(graftyard_types::NetworkId::of_root(repo)),
                    pack,
                    chain,
                    acked.index,
                )
                .await
            }
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
    fn archived(e: &raft::LogEntry<Self::Ev>) -> Vec<ArchivedRange> {
        let raft::EntryBody::Events(evs) = &e.body else { return Vec::new() };
        evs.iter()
            .filter_map(|ev| match ev {
                git::repo::RepoEvent::Archived(a) if matches!(a.key, git::R2Key::Archive { .. }) => {
                    Some(ArchivedRange { key: a.key.to_string(), from: a.from.0, to: a.to.0, to_at: a.to_at.0 })
                }
                _ => None,
            })
            .collect()
    }
    async fn truncated(host: &std::rc::Rc<crate::host::Host<Self>>, to: raft::Index) {
        crate::load::set_source(host, to).await;
    }
    fn install_wanted(host: &std::rc::Rc<crate::host::Host<Self>>, w: raft::InstallWanted) {
        crate::load::want(host, w);
    }
    fn restored(host: &std::rc::Rc<crate::host::Host<Self>>) {
        crate::load::restored(host);
        crate::split::restored(host);
    }
    fn needs_leader(host: &std::rc::Rc<crate::host::Host<Self>>) -> bool {
        crate::split::leader_due(host) || crate::archive::leader_due(host)
    }
    async fn fetch_missed(host: &std::rc::Rc<crate::host::Host<Self>>, missed: &Value) -> Result<(), String> {
        crate::split::fetch(host, missed).await
    }
    fn may_lead(host: &std::rc::Rc<crate::host::Host<Self>>) -> bool {
        !crate::load::meta_dropped(&host.state.storage().sql())
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
pub(crate) fn store_blob<T: Serialize>(fold: &T, stmts: &mut Vec<Stmt>) {
    stmts.push(("INSERT OR REPLACE INTO fold (id, row) VALUES (1, ?)", vec![codec::to_bytes(fold).into()]));
}

pub(crate) fn load_blob<T: DeserializeOwned>(sql: &SqlStorage) -> worker::Result<Option<T>> {
    match crate::host::rows(sql, "SELECT row FROM fold WHERE id = 1", vec![])?.first() {
        Some(r) => Ok(Some(
            codec::from_bytes(crate::host::blob(&r[0])?).map_err(|e| crate::host::err(format!("fold row: {e}")))?,
        )),
        None => Ok(None),
    }
}

/// 401 for a credential refusal, 403 for a permission refusal, 422 for the rest.
pub(crate) fn refusal(e: &impl Debug, status: u16) -> Value {
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
    /// Format 4 is a fork's `RepoOwner` row with its network (AM3-WP2a), sent only once `FormatsEnable` enabled it.
    fn reads() -> Vec<Format> {
        vec![id::NAMES_FORMAT, id::NAMES_FORMAT_V2, id::NAMES_FORMAT_V3, id::NAMES_FORMAT_V4]
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
        // A fork's network from its `RepoOwner` row (format 0x10.4), `null` for a root: the edge's id-only routes.
        if let Some(r) = q.get("repoNetwork").and_then(Value::as_i64) {
            out.insert("repoNetwork".into(), json!(f.repo_network(graftyard_types::RepoId(DbId(r)))));
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

    /// Format 3 is the `Audit` cursor's, as for `Names`. Format 4 is `token/scoped`'s narrowed `ghu_` (AM2-WP1), and 5
    /// the lifeboat enroll that revokes the account's earlier lifeboat tokens (M6-WP3).
    fn reads() -> Vec<Format> {
        vec![id::TOKENS_FORMAT, id::TOKENS_FORMAT_V2, id::TOKENS_FORMAT_V3, id::TOKENS_FORMAT_V4, id::TOKENS_FORMAT_V5]
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
        // The lifeboat front's only credential read (`decisions/DECISIONS.md`, "`lifeboat-bundle`"): a `LifeboatRead`
        // row's account, never a `Stored` row's.
        if let Some(h) = hex_of("lifeboat") {
            out.insert("lifeboat".into(), json!(f.lifeboat(&h.0).map(|a| a.0.0)));
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
        // consumer's abort; 0x12.12 M4-WP3's `SealKey` and org secret and variable ops, which the edge sends only once
        // `FormatsEnable` enabled it; 0x12.13 `PATCH /app/hook/config`'s `SetAppHook`, likewise; 0x12.14 the Actions
        // App's installation, which `acquirejob` commits only once `FormatsEnable` enabled it (`gh-token`); 0x12.15
        // BF-76's substitution policy, which the edge sends only once `FormatsEnable` enabled it (`pin-map`); 0x12.16
        // AM3-WP2a's `CreateFork`, likewise.
        vec![
            id::AUTHZ_FORMAT,
            id::AUTHZ_FORMAT_V2,
            id::AUTHZ_FORMAT_V3,
            id::AUTHZ_FORMAT_V4,
            id::AUTHZ_FORMAT_V11,
            id::AUTHZ_FORMAT_V12,
            id::AUTHZ_FORMAT_V13,
            id::AUTHZ_FORMAT_V14,
            id::AUTHZ_FORMAT_V15,
            id::AUTHZ_FORMAT_V16,
        ]
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
    /// `{"repo": name}`; `{"app": id}`: the App record's slug, callbacks, and how many client secrets it holds, so a
    /// script can see that `graftyard apps init` registered it (`tests/am1/rest/apps.mjs` refuses then); always the
    /// opened kind, the name count, and the id cursor.
    fn read_json(f: &id::AuthzFold, q: &Value) -> Value {
        use id::AuthzRead;
        let app = q.get("app").and_then(Value::as_i64).map(|a| {
            f.app(id::AppId(DbId(a))).map(|r| {
                json!({ "slug": r.slug.as_str(), "callbacks": r.callbacks.iter().map(|u| &*u.0).collect::<Vec<_>>(),
                        "client_secrets": r.client_secrets.len() })
            })
        });
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
            "app": app,
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
    /// once `FormatsEnable` enabled it. 0x14.3 is M4-WP2's `SetRecord` and the `Counts` consumer's `CountsCursor`, on
    /// the same terms: the edge sends `SetRecord` only once 0x14.3 is enabled, and the consumer runs only under
    /// `GRAFTYARD_ACTIVITY` (`crate::activity`). 0x14.4 is AM2-WP1's `McpGrant` rows, sent by the edge only once
    /// 0x14.4 is enabled.
    fn reads() -> Vec<Format> {
        vec![id::IDENTITY_FORMAT, id::IDENTITY_FORMAT_V2, id::IDENTITY_FORMAT_V3, id::IDENTITY_FORMAT_V4]
    }
    /// A leader's alarm right after it applies entries runs the `Counts` consumer (`crate::activity`).
    fn fold_alarm_ms(leading: bool) -> Option<i64> {
        leading.then_some(git::watch::COUNTS_EVERY_MS as i64)
    }
    async fn on_alarm(host: &std::rc::Rc<crate::host::Host<Self>>) {
        crate::activity::counts_on_alarm(host).await;
    }
    fn route_paths() -> &'static [&'static str] {
        &["/inbox"]
    }
    /// `/inbox`: the account's `Inbox` object (`crate::inbox`) through its `Identity` leader, which holds the account's
    /// record and `UserRows`: `{"op": "view" | "snapshot" | "delete" | "rebuild"}`. The operator's way to an `Inbox`,
    /// with no binding of its own on the caller (BF-5, BF-67).
    async fn route(
        host: &std::rc::Rc<crate::host::Host<Self>>,
        path: &str,
        body: Value,
    ) -> Option<worker::Result<Value>> {
        (path == "/inbox").then_some(())?;
        Some(crate::inbox::route(host, body).await)
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
    /// `Done` for most ops, and a grant's row for the 0x14.4 ops, which the edge renders.
    fn out_json(_fold: &Self::Fold, a: &raft::Acked<id::IdentityOut>) -> Value {
        serde_json::to_value(&a.out).unwrap_or(Value::Null)
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
    fn read_json(f: &id::IdentityFold, q: &Value) -> Value {
        // `{"user": true}`: the account's `UserRows` (format 0x14.3) in the raft codec, as hex, for the edge's
        // `/notifications` and `/subscription` replies.
        if q.get("user").is_some() {
            return json!({ "user": crate::host::hex(&codec::to_bytes(f.user())) });
        }
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

/// `network:<id>` members: the fork network's chain (`graftyard_git::network`; `build/BUILD-PLAN.md`, AM3-WP2a), a
/// core-tier fold of the network's forks, placed under the root repository owner's record (`design/DESIGN.md`, Shape,
/// `Placement`: "each network chain whose root repository it holds"). Only the `Audit` finisher writes it, after a
/// fork's commit point; `{"forks": parent}` lists a repository's direct forks for `GET /forks`, and `{}` the count.
pub struct Network;

impl Family for Network {
    type Ev = git::network::NetworkEvent;
    type Fold = git::network::NetworkFold;
    type C = git::network::NetworkCmd;

    /// 0x21.1, the family's first format: the finisher sends it only once `FormatsEnable` enabled it.
    fn reads() -> Vec<Format> {
        vec![git::network::NETWORK_FORMAT]
    }
    fn new_fold(_sql: &SqlStorage, join: &Join) -> git::network::NetworkFold {
        match join.chain {
            ChainId::Network(n) => git::network::NetworkFold::new(n),
            _ => git::network::NetworkFold::default(),
        }
    }
    fn store_fold(fold: &git::network::NetworkFold, _: &[&raft::LogEntry<Self::Ev>], stmts: &mut Vec<Stmt>) {
        store_blob(fold, stmts);
    }
    fn load_fold(sql: &SqlStorage, join: &Join) -> worker::Result<git::network::NetworkFold> {
        Ok(load_blob(sql)?.unwrap_or_else(|| Self::new_fold(sql, join)))
    }
    fn out_json(_fold: &Self::Fold, a: &raft::Acked<git::network::NetworkOut>) -> Value {
        serde_json::to_value(a.out).unwrap_or(Value::Null)
    }
    fn err_json(e: &git::network::NetworkErr) -> Value {
        match e {
            git::network::NetworkErr::Forbidden => refusal(e, 403),
            _ => refusal(e, 422),
        }
    }
    fn read_json(f: &git::network::NetworkFold, q: &Value) -> Value {
        if let Some(p) = q.get("forks").and_then(Value::as_i64) {
            return json!({ "forks": f.forks_of(RepoId(DbId(p))).map(|r| r.0.0).collect::<Vec<_>>() });
        }
        json!({ "network": f.network.map(|n| n.0.0), "forks": f.forks.len() })
    }
    fn fold_bytes(f: &git::network::NetworkFold) -> Vec<u8> {
        codec::to_bytes(f)
    }
    fn dep(chain: ChainId, _bytes: &[u8]) -> Result<IdentityDep, String> {
        Err(format!("the network chain reads no dependency ({chain})"))
    }
    /// The root repository's `RepoOwner` row names the owner whose record places the chain.
    fn source(join: &Join) -> Source {
        match (join.chain, join.owner) {
            (ChainId::Network(n), Some(owner)) => Source::Repo { repo: n.root(), owner },
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
