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
    /// The archive an applied entry commits, `Archived`'s key and `to`: members truncate their stored log through `to`
    /// once they can HEAD the key in two buckets.
    fn archived(_e: &raft::LogEntry<Self::Ev>) -> Option<(String, u64)> {
        None
    }
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
            ChainId::Network(_) | ChainId::Identity(_) | ChainId::Actions(..) | ChainId::RepoMeta(_) => None,
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
    /// `FormatsEnable` enabled it, and only this release's own `MergeQueue` consumer writes one before that.
    fn reads() -> Vec<Format> {
        vec![
            git::repo::REPO_FORMAT_V1,
            git::prsync::REPO_FORMAT_V3,
            git::merge::REPO_FORMAT_V4,
            git::issue_ops::call::REPO_FORMAT_ISSUES,
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
        crate::prsync::on_alarm(host).await;
        crate::merge_queue::on_alarm(host).await;
        crate::snap::on_alarm(host).await;
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
        return &["/snapshot_commit", "/fold", "/bench_seed"];
        #[cfg(not(feature = "test-faults"))]
        &["/snapshot_commit", "/fold"]
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
                if body.get("replay").is_some() {
                    drop(core);
                    return crate::snap::replay(host);
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
            // Test installs: one fold audit run, the cron's own, from this member's site.
            #[cfg(feature = "test-faults")]
            "/bench_seed" if body.get("audit").is_some() => {
                let only = body["audit"].as_str().and_then(|c| c.parse().ok());
                Some(crate::fold_audit::run(&host.env, only).await)
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
    /// The refs as `[[name, oid]]`, what `graftyard_git::group::ReadValue` reads, from the core tier; with `{"q":
    /// {"rows": true}}`, both tiers' rows at this index, at most `graftyard_api::hosted::ROWS_MAX`
    /// (`graftyard_api::hosted::TierRows`), which the edge's `Repo` view reads; with `{"q":
    /// {"ranges": true}}`, the last `Promoted` and `Archived` and the first index still in the log, what the shard's
    /// `ChainLog` reads; with `{"q": {"tiers": true}}`, each tier's `applied` and `fold_digest`, and the digest after
    /// `{"at": index}` while the tier keeps it (`graftyard_fold::digest_at`), for the fold audit.
    fn read_json(fold: &git::repo::RepoFold, q: &Value) -> Value {
        if let Some(prs) = q.get("prsync") {
            return crate::prsync::read_json(fold, prs);
        }
        if let Some(base) = q.get("mergequeue") {
            return crate::merge_queue::read_json(fold, base);
        }
        if q.get("rows").is_some() {
            use graftyard_api::hosted::{ROWS_MAX, TierRows};
            let mut budget = ROWS_MAX;
            let (core, c_ok) = TierRows::dump(&fold.core, &git::tiers::CORE, &mut budget);
            let meta = fold.meta.loaded().then(|| TierRows::dump(&fold.meta, &git::tiers::METADATA, &mut budget));
            let complete = c_ok && meta.as_ref().is_none_or(|(_, ok)| *ok);
            let index = graftyard_fold::meta(&*fold.core).applied.0;
            return serde_json::to_value(TierRows { index, core, meta: meta.map(|(m, _)| m), complete })
                .unwrap_or(Value::Null);
        }
        if q.get("ranges").is_some() {
            return json!({ "promoted": fold.promoted(), "archived": fold.archived(), "first": fold.first_index().0 });
        }
        if q.get("tiers").is_some() {
            return crate::snap::tiers_json(fold, q.get("at").and_then(Value::as_u64));
        }
        json!(fold.refs().iter().map(|(n, o)| (n.display(), o.hex())).collect::<Vec<_>>())
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
        if let Some(f) =
            v.pointer("/cmd/issue/format").or_else(|| v.pointer("/cmd/merge/format")).and_then(Value::as_array)
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
    fn row_packs(events: &[git::repo::RepoEvent]) -> Value {
        json!(git::repo::packs_of(events))
    }
    fn archived(e: &raft::LogEntry<Self::Ev>) -> Option<(String, u64)> {
        let raft::EntryBody::Events(evs) = &e.body else { return None };
        evs.iter().find_map(|ev| match ev {
            git::repo::RepoEvent::Archived(a) if matches!(a.key, git::R2Key::Archive { .. }) => {
                Some((a.key.to_string(), a.to.0))
            }
            _ => None,
        })
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
        m.insert("bytes".into(), json!(raft::encode_entry(e).len()));
        m
    }
    /// BF-24's driver times a head push's entry to its `TestMerged`'s on the leader's clock: the first 8 ref names an
    /// entry updates, and each `TestMerged`.
    #[cfg(feature = "test-faults")]
    fn log_fields(e: &raft::LogEntry<Self::Ev>) -> serde_json::Map<String, Value> {
        let mut m = serde_json::Map::new();
        let raft::EntryBody::Events(evs) = &e.body else { return m };
        let refs: Vec<Value> = evs
            .iter()
            .flat_map(|ev| ev.updates())
            .take(8)
            .map(|u| json!([u.name.display(), u.new.map(|o| o.hex())]))
            .collect();
        m.insert("refs".into(), json!(refs));
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

    fn reads() -> Vec<Format> {
        vec![id::NAMES_FORMAT]
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
    /// `{"placement": owner}`, `{"login": key}`, `{"account": id}`, `{"repoOwner": id}`, `{"ids": true}`.
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
        if let Some(r) = q.get("repoOwner").and_then(Value::as_i64) {
            out.insert("repoOwner".into(), json!(f.repo_owner(graftyard_types::RepoId(DbId(r)))));
        }
        if q.get("ids").is_some() {
            out.insert("ids".into(), cursor_json(&f.ids));
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

    fn reads() -> Vec<Format> {
        vec![id::TOKENS_FORMAT]
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
    /// `{"token": "<64 hex>"}`.
    fn read_json(f: &id::TokensFold, q: &Value) -> Value {
        use id::TokensRead;
        let hash = q.get("token").and_then(Value::as_str).and_then(graftyard_types::Digest::parse_hex);
        let row = hash.and_then(|h| f.token(&h.0));
        json!({
            "token": row.map(|r| json!({ "account": r.account.0.0, "revoked": r.revoked })),
            "tokens": f.tokens.len(),
        })
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
        vec![id::AUTHZ_FORMAT]
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
