//! The edge Worker's git over HTTPS (`build/BUILD-PLAN.md`, M1-WP6; `design/DESIGN.md`, journey 1 and "Usage
//! (caller's view)"): smart HTTP only, receive-pack staged, validated, placed, committed, and reported, and
//! upload-pack through the git cache shard with the clone cache in front.
//!
//! The edge mints one `RequestId` per client request and reuses it on every re-send. It reaches the site members
//! through each site's `MEMBER_<n>` and `REGISTRY_<n>` bindings and finds groups by the M1-WP1 lookup rule
//! (`graftyard_lookup`), with the member routes and JSON of `graftyard_git::group`:
//!
//! - `o/r` resolves through `Names` (the owner's login row, then its `Placement` row) and then the owner's `Authz`
//!   (`RepoName`); system chains' records come from the `Release` view, accepted against this script's trust anchor.
//! - A repository chain's group is created on first use from the owner's record; its members check the owner against
//!   the repository's `RepoOwner` row before they store anything.
//! - The git cache shard is the `GitShard` class of the owner record's home site (`SHARD_<n>`).
//!
//! Authentication is a stored token as the Basic password (any username) or as `Authorization: token` / `Bearer`, as
//! GitHub accepts, checked offline by prefix and CRC32 and then on its row, read through `fresh` on its `Tokens`
//! shard; the leader re-reads the row at `decide` either way. `POST /api/graftyard/v1/release/entries` takes signed
//! `Release` entries (`release`).
//!
//! With the `test-faults` feature only: the `/_bed/` driver routes, BF-16's `FORCE_FORMAT`, BF-12's `x-graft-kill`
//! on pushes, and the `GRAFTYARD_GENESIS` stand-in for the `Release` view.

use futures_util::StreamExt;
use graftyard_git::clone::{CloneKey, tags_digest};
use graftyard_git::compose::ChainCheck;
use graftyard_git::group::{CallErrorWire, CommitReq, PushOutcome, RefsPage, RefsReply};
use graftyard_git::host::R2;
use graftyard_git::refs::{Atomicity, ObjectId, RefName};
use graftyard_git::repo::{ATOMIC_REFS_MAX, Push, REPO_FORMAT_V1, REPO_FORMAT_V2};
use graftyard_git::smart::{
    self, BodyReader, Plan, PushCaps, ReceivePack, Refusal, Route, Service, cap_refusal, parse_receive_pack, parse_v2,
    receive_pack_advertisement, receive_pack_caps, upload_pack_v2_advertisement, wants_v2, wrap,
};
use graftyard_git::{
    Copies, INLINE_MAX, ObjectMeta, PART_SIZE, PackId, PackIndex, PackStore, Stage, Streamed, TeeError,
};
use graftyard_identity::{
    Actor, Credential, Login, LoginRow, NameKey, OpaqueToken, Principal, RepoName, ScopeSet, UserId, account_shard,
    name_shard,
};
use graftyard_raft::{COMMAND_MAX, Format, Record};
use graftyard_release::Anchor;
use graftyard_types::{
    AccountId, ChainId, DbId, Digest, NetworkId, PlacementRecord, RepoId, RequestId, SiteId, SiteTable, Timestamp, Ulid,
};
use serde::Deserialize;
use serde_json::json;
use worker::*;

mod actions;
mod activity;
mod agents;
mod api;
mod artifacts;
#[cfg(feature = "test-faults")]
mod bed_rows;
mod cache;
mod collab;
mod colo;
mod export;
mod hooks;
mod hosted;
mod ident;
mod keys;
mod lifeboat;
mod mcp;
mod oidc;
mod precreate;
mod purge;
mod rcache;
mod release;
mod repos;
mod secrets;
mod ssh;
mod transfer;
mod twirp;
mod ui;
mod usertok;

const AGENT: &str = "git/graftyard-m1";
/// `design/DESIGN.md`, Shape, `Unsupported`: "The edge renders it as 503 with `Retry-After: 5` [guess]".
const UNSUPPORTED_RETRY_AFTER: &str = "5";
/// A commit with no outcome (`no_outcome`): the REST routes' `Retry-After` for the same answer (`api::unavailable`).
const NO_OUTCOME_RETRY_AFTER: &str = "5";

fn now() -> Timestamp {
    Timestamp(Date::now().as_millis())
}

fn rand10() -> [u8; 10] {
    let mut b = [0u8; 10];
    let crypto: web_sys::Crypto = js_sys::Reflect::get(&js_sys::global(), &"crypto".into()).unwrap().into();
    crypto.get_random_values_with_u8_array(&mut b).unwrap();
    b
}

/// The edge's configuration, from the script's version.
#[derive(Clone)]
struct Config {
    plan: Plan,
    ssh_option: bool,
    /// The edge's own cap, below the plan's body limit.
    cap: u64,
    host: String,
    /// The formats `FormatsEnable` has enabled. A command is sent in its newest enabled format, never one the
    /// `Release` group has not enabled, so an edge on N+1 talks format k to leaders on N.
    enabled: Vec<Format>,
    /// Test fault for BF-16: send every push in this format whatever is enabled. Read only with `test-faults`.
    force_format: Option<Format>,
    /// BM-5's aim for `force_format` (`FORCE_FORMAT_SITE`): only a push to a chain whose leader this isolate last saw
    /// on this site is forced; the rest go in their own format. Read only with `test-faults`.
    force_site: Option<SiteId>,
    /// Multipart part size of the tee.
    part: usize,
}

fn parse_format(s: &str) -> Option<Format> {
    let (f, v) = s.split_once('.')?;
    Some(Format { family: f.parse().ok()?, version: v.parse().ok()? })
}

impl Config {
    fn read(env: &Env) -> Result<Config> {
        let var = |k: &str| env.var(k).map(|v| v.to_string()).ok();
        let plan = match var("PLAN").as_deref() {
            Some("business") => Plan::Business,
            Some("enterprise") => Plan::Enterprise,
            _ => Plan::Pro,
        };
        let cap = var("EDGE_CAP_BYTES").and_then(|v| v.parse().ok()).unwrap_or(plan.body_limit() * 9 / 10);
        if cap >= plan.body_limit() {
            return Err(Error::RustError(format!("EDGE_CAP_BYTES {cap} must sit below the plan limit")));
        }
        Ok(Config {
            plan,
            ssh_option: var("SSH_OPTION").as_deref() == Some("on"),
            cap,
            host: var("HOST").unwrap_or_else(|| "graftyard.dev".into()),
            enabled: var("FORMATS_ENABLED")
                .unwrap_or_else(|| "32.1".into())
                .split(',')
                .filter_map(parse_format)
                .collect(),
            #[cfg(feature = "test-faults")]
            force_format: var("FORCE_FORMAT").as_deref().and_then(parse_format),
            #[cfg(not(feature = "test-faults"))]
            force_format: None,
            #[cfg(feature = "test-faults")]
            force_site: var("FORCE_FORMAT_SITE").and_then(|v| v.parse().ok()).map(SiteId),
            #[cfg(not(feature = "test-faults"))]
            force_site: None,
            part: var("PART_BYTES").and_then(|v| v.parse().ok()).unwrap_or(PART_SIZE),
        })
    }

    /// The format a push is sent in: format 2 carries the client's agent, and is used only once enabled. A forced format
    /// applies to every push, or with `force_site` only to one whose chain's leader was last seen on that site.
    fn push_format(&self, leader: Option<SiteId>) -> Format {
        if let Some(f) = self.force_format.filter(|_| self.force_site.is_none_or(|s| leader == Some(s))) {
            return f;
        }
        if self.enabled.contains(&REPO_FORMAT_V2) { REPO_FORMAT_V2 } else { REPO_FORMAT_V1 }
    }
}

/// Every site's bucket, by its `BUCKET_<n>` binding, which the edge binds for every site of `GRAFTYARD_SITES`.
fn buckets(env: &Env, sites: &SiteTable) -> Result<Vec<(SiteId, R2)>> {
    sites.0.iter().map(|s| Ok((s.id, R2(env.bucket(&format!("BUCKET_{}", s.id.0))?)))).collect()
}

/// A repository the edge serves: its id, its owner, the owner's placement record, and its network once a read of its
/// own fixed it (`Repo::fix`).
#[derive(Clone, Debug)]
pub(crate) struct Repo {
    id: RepoId,
    owner: AccountId,
    record: PlacementRecord,
    net: std::cell::Cell<Option<NetworkId>>,
}

/// The same repository at the same placement, whether or not either has fixed its network yet.
impl PartialEq for Repo {
    fn eq(&self, o: &Repo) -> bool {
        (self.id, self.owner, &self.record) == (o.id, o.owner, &o.record)
    }
}

impl Repo {
    pub(crate) fn new(id: RepoId, owner: AccountId, record: PlacementRecord) -> Repo {
        Repo { id, owner, record, net: Default::default() }
    }

    /// The network the repository's packs, git cache shards, clone and archive caches are keyed by
    /// (`design/DESIGN.md`, Shape, `R2Key`, `shards_for`): every edge path names it through here, never from `id`.
    /// Once the read gate or the request's own read fixed it, that read's; before, the isolate's guess (`nets`),
    /// which only a lookup ahead of the gate uses and which `upload_pack` checks again.
    pub(crate) fn network(&self) -> NetworkId {
        self.net.get().unwrap_or_else(|| nets::get(self.id))
    }

    /// Fixes the network this request keys by to the one its own read found: `fork` from a fork's row, `None` for a
    /// root. A request that has fixed it never reads the isolate's map again, so an overflow that clears the map
    /// meanwhile cannot move this request's packs under the fork's own id (`fork-network-read`).
    pub(crate) fn fix(&self, fork: Option<NetworkId>) {
        let n = fork.unwrap_or(NetworkId::of_root(self.id));
        nets::learn(self.id, n);
        self.net.set(Some(n));
    }

    /// The fixed network when it is a fork's, what this request's gate entries carry; `None` for a root or before
    /// any read fixed it.
    fn fork(&self) -> Option<NetworkId> {
        self.net.get().filter(|n| n.root() != self.id)
    }
}

/// The forks' networks this isolate learned (`decisions/DECISIONS.md`, "`fork-network-read`"): from a fork's `fork`
/// row in the access rows every gate read and tier read already carries, from a shared gate entry's `net`, or from
/// `Names`' `RepoOwner` row on an id-only route. A network never changes, so an entry is right at any age. A root has
/// none and is its own id. It is a guess for the lookups ahead of the gate only: each request keys by the network
/// its own read fixed (`Repo::fix`), and each gate entry holds its own fill's, so clearing the map moves neither.
mod nets {
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    use graftyard_types::{NetworkId, RepoId};

    /// The most forks one isolate remembers; past it the map starts empty again.
    const ENTRIES_MAX: usize = 4_096;

    thread_local! {
        static FORKS: RefCell<BTreeMap<RepoId, NetworkId>> = const { RefCell::new(BTreeMap::new()) };
    }

    /// `repo`'s network: a fork's as learned, else its own id.
    pub(crate) fn get(repo: RepoId) -> NetworkId {
        fork(repo).unwrap_or(NetworkId::of_root(repo))
    }

    /// `repo`'s network when it is a learned fork's: what a shared gate entry carries.
    pub(crate) fn fork(repo: RepoId) -> Option<NetworkId> {
        FORKS.with(|m| m.borrow().get(&repo).copied())
    }

    /// Learns `repo`'s network; a root's own id stores nothing.
    pub(crate) fn learn(repo: RepoId, network: NetworkId) {
        if network.root() == repo {
            return;
        }
        FORKS.with(|m| {
            let mut m = m.borrow_mut();
            if m.len() >= ENTRIES_MAX && !m.contains_key(&repo) {
                m.clear();
            }
            m.insert(repo, network);
        });
    }
}

/// `o/r` through `Names` then `Authz` (`design/DESIGN.md`, Shape, the lookup rule: "the owner's `Names` rows for
/// `o/r`"): the owner's login row on `name_shard(Login(o))`, the owner's `Placement` row on `account_shard(owner)`, then
/// the repository's name on the owner's `Authz` chain. `None` for an unknown owner or repository, which git sees as 404.
/// The three reads are a chain; with this isolate's last login and `Placement` answers for the owner held, at any age,
/// they start at once on them (`chained`).
async fn resolve(c: &Ctx<'_>, owner: &str, repo: &str) -> Result<std::result::Result<Option<Repo>, String>> {
    resolve_on(c, owner, repo, None).await
}

/// `resolve`, guessing from the data center's link entry (`linked`) when this isolate holds no login and `Placement`
/// answer for the owner (`beside`). The entry is a guess at any age, never an answer: `chained` keeps a read made on
/// it only when the reads before it answer the same.
pub(crate) async fn resolve_on(
    c: &Ctx<'_>,
    owner: &str,
    repo: &str,
    linked: Option<&Repo>,
) -> Result<std::result::Result<Option<Repo>, String>> {
    let Ok(name) = RepoName::parse(repo) else { return Ok(Ok(None)) };
    let guess = held_guess(owner).or_else(|| linked.map(|r| (r.owner, r.record.clone())));
    // Boxed: `chained` holds each read twice (its guessed and its fresh run), and inline in every caller's future it
    // took the edge past wasm's 1 MiB stack on every request (mt16's first edge, `beside`).
    Ok(Box::pin(chained(guess, account_of(c, owner), |a| placement(c, a), |a, r| named(c, a, r, &name))).await)
}

/// This isolate's last login and `Placement` answers for `owner`, at any age: `resolve`'s guess.
fn held_guess(owner: &str) -> Option<(AccountId, PlacementRecord)> {
    let l = Login::parse(owner).ok()?;
    let account = rcache::last(&rcache::LOGINS, &l.key())?;
    Some((account, rcache::last(&rcache::PLACEMENTS, &account)?))
}

/// Starts the read gate's access rows on `guessed` beside the front (`beside`).
fn beside_start<'a>(c: &'a Ctx<'_>, guessed: Repo) -> Beside<'a> {
    use futures_util::FutureExt;
    let r = guessed.clone();
    let read = async move { api::timed(c, "beside_rows", api::guessed_rows(c, &r)).await };
    Beside { repo: guessed, start: now(), rows: Box::pin(futures_util::future::maybe_done(read.boxed_local())) }
}

/// Whether this isolate's or the data center's gate entry for `guessed` and this request's credential would answer
/// the gate now, so rows read beside the front would go unused. A credential it cannot key before `authenticate` (a `ghs_`, a session) counts
/// as answered, and reads nothing beside.
async fn gate_shared(c: &Ctx<'_>, req: &Request, guessed: &Repo) -> Result<bool> {
    let key = match token_of(req)? {
        None => None,
        Some(t) if t.starts_with("ghs_") => return Ok(true),
        Some(t) => match OpaqueToken::parse(&t) {
            Ok(tok) => Some(tok.hash()),
            Err(_) => return Ok(true),
        },
    };
    if gate::get(&(guessed.id, key)).is_some_and(|((at, _, foreign, _), _)| colo::servable(at, now(), foreign)) {
        return Ok(true);
    }
    let got = api::timed(c, "beside_match", colo::match_gate(&colo::gate_url(guessed.id, key, &guessed.record))).await;
    if let Some((_, Some(n))) = got {
        nets::learn(guessed.id, n);
    }
    Ok(got.is_some_and(|(at, _)| colo::servable(at, now(), true)))
}

/// `main`, polling `side` beside it until `main` answers, so rows read beside the front keep reading while it runs.
async fn driving<F: std::future::Future, S: std::future::Future<Output = ()>>(
    main: F,
    side: Option<std::pin::Pin<&mut S>>,
) -> F::Output {
    let mut main = std::pin::pin!(main);
    let mut side = side;
    std::future::poll_fn(|cx| {
        if let Some(s) = side.as_mut() {
            let _ = s.as_mut().poll(cx);
        }
        main.as_mut().poll(cx)
    })
    .await
}

/// The access rows read beside the front on a guessed repository (`beside`): the guess, when that read started, and
/// the read, polled beside the front and taken by the read gate only if it reads members.
struct Beside<'a> {
    repo: Repo,
    start: Timestamp,
    rows: std::pin::Pin<Box<futures_util::future::MaybeDone<futures_util::future::LocalBoxFuture<'a, GuessedRows>>>>,
}

type RowsRead = Result<std::result::Result<graftyard_fold::MemStore, String>>;

/// The rows read beside the front: `None` when the guessed group answered no leader, which is not created on a guess.
type GuessedRows = Result<std::result::Result<Option<graftyard_fold::MemStore>, String>>;

/// The access rows the gate decides on, given rows `started` on `guessed` beside the front: those, through `link`
/// (the rule `resolve`'s guesses follow) with `resolved` as the answer before them, so they are kept only when the
/// resolve named the same repository, owner and placement record, else read again on `resolved` by `read`. With
/// whether the started read was kept.
async fn beside_rows<T, E, F, G>(
    guessed: Repo,
    started: F,
    resolved: &Repo,
    read: impl Fn(Repo) -> G,
) -> (std::result::Result<Option<T>, E>, bool)
where
    F: std::future::Future<Output = std::result::Result<Option<T>, E>>,
    G: std::future::Future<Output = std::result::Result<Option<T>, E>>,
{
    use futures_util::future::Either;
    let again = std::cell::Cell::new(false);
    let next = |r: Repo| {
        again.set(true);
        Either::Right(read(r))
    };
    let got = link(Some((guessed, Either::Left(started))), std::future::ready(Ok(Some(resolved.clone()))), next).await;
    (got.map(|g| g.map(|(_, t)| t)), !again.get())
}

/// One link of a chain of reads: `next` is addressed by `prev`'s answer. With no `guess` they run in turn. With one,
/// `guess` is the guessed answer and `next`'s read on it, which runs beside `prev`; it is kept only if `prev` answered
/// what the guess said, else dropped once `prev` answers and read again on `prev`'s answer. So the pair is always the
/// one the reads in turn would give, a guessed read that was not kept counts for nothing, its error included, and the
/// answer is never the guess. A guessed read that is not kept is dropped before the read again starts: alive, a
/// write-class one held its key's batch (`rcache::batched`), and the read again waited out BATCH_STALE behind it
/// (B0-34 cw1: 16 of 19 cold first POSTs paid an `install` of about 2 s, `rcwait` 1,800 to 1,940 ms).
async fn link<A, T, E, F1, F2>(
    guess: Option<(A, F2)>,
    prev: F1,
    next: impl Fn(A) -> F2,
) -> std::result::Result<Option<(A, T)>, E>
where
    A: Clone + PartialEq,
    F1: std::future::Future<Output = std::result::Result<Option<A>, E>>,
    F2: std::future::Future<Output = std::result::Result<Option<T>, E>>,
{
    use futures_util::future::{Either, select};
    let Some((g, started)) = guess else {
        let Some(a) = prev.await? else { return Ok(None) };
        return Ok(next(a.clone()).await?.map(|t| (a, t)));
    };
    let (a, started) = match select(std::pin::pin!(prev), Box::pin(started)).await {
        Either::Left((a, started)) => (a, Either::Left(started)),
        Either::Right((t, prev)) => (prev.await, Either::Right(t)),
    };
    let Some(a) = a? else { return Ok(None) };
    if a != g {
        drop(started);
        return Ok(next(a.clone()).await?.map(|t| (a, t)));
    }
    let t = match started {
        Either::Left(running) => running.await,
        Either::Right(t) => t,
    };
    Ok(t?.map(|t| (a, t)))
}

/// A chain of three reads, each addressed by the answers before it (`resolve`'s login, `Placement`, and name): two
/// `link`s. With a `guess`, all three start at once, and a right guess costs the longest of the three reads, not
/// their sum.
async fn chained<A, R, T, E, F1, F2, F3>(
    guess: Option<(A, R)>,
    first: F1,
    second: impl Fn(A) -> F2,
    third: impl Fn(A, R) -> F3,
) -> std::result::Result<Option<T>, E>
where
    A: Copy + PartialEq,
    R: Clone + PartialEq,
    F1: std::future::Future<Output = std::result::Result<Option<A>, E>>,
    F2: std::future::Future<Output = std::result::Result<Option<R>, E>>,
    F3: std::future::Future<Output = std::result::Result<Option<T>, E>>,
{
    let (ga, gr) = guess.unzip();
    let (second, third) = (&second, &third);
    let rest = move |a: A| {
        let g = gr.clone().filter(|_| Some(a) == ga).map(|r| (r.clone(), third(a, r)));
        link(g, second(a), move |r| third(a, r))
    };
    let got = link(ga.map(|a| (a, rest(a))), first, rest).await?;
    Ok(got.map(|(_, (_, t))| t))
}

/// The repository `name` on `account`'s `Authz` chain, found by `record`: `resolve`'s third read.
async fn named(
    c: &Ctx<'_>,
    account: AccountId,
    record: PlacementRecord,
    name: &RepoName,
) -> std::result::Result<Option<Repo>, String> {
    // A read-class request resolves the name in the owner's `Authz` fold, which its read gate needs anyway: from
    // `rcache` when one is at most `READ_CACHE` old, else one read that the gate then reuses. A name the fold does
    // not hold is read again fresh below, so a repository created since resolves at once.
    if c.read_class.get()
        && let Ok(Some(f)) = authz_read(c, account, &record).await
        && let Some((r, _)) = graftyard_identity::AuthzRead::resolve(&f, &name.key())
    {
        return Ok(Some(Repo::new(r, account, record)));
    }
    let authz = ChainId::Authz(account);
    let b = c.bindings();
    // By read-index on the `Authz` group's learners and followers, as `authz_read`: one per request, it was about
    // 240 reads/s on the `Authz` leader at 500 agents once the permission reads had moved (`design/DESIGN.md`,
    // "Follower read").
    let q = json!({ "q": { "repo": name.as_str() } });
    let v = match authz_get(c, &b, authz, &record, &q).await {
        Ok(Some((v, _, _))) => v,
        // An owner whose `Authz` chain was never created holds no repository.
        Ok(None) => return Ok(None),
        Err(e) => return Err(e),
    };
    Ok(v.pointer("/value/repo/repo")
        .and_then(serde_json::Value::as_i64)
        .map(|r| Repo::new(RepoId(DbId(r)), account, record)))
}

/// A REST merge's guess at `owner/repo` (`api::rest_repo`), which it reads ahead on and keeps only if its own fresh
/// `resolve` names the same repository and record: this isolate's `rcache` entries at any age, as `resolve`'s own
/// guess (`held_guess`), else the data center's link entry (`beside`). With the link entry's key when this isolate
/// held no guess, for `put_link` once the resolve answers. A guess never answers a read, so its age does not matter
/// (`merge-front`); on main it was the isolate's entries within `READ_CACHE` only.
async fn merge_guess(c: &Ctx<'_>, owner: &str, repo: &str) -> (Option<Repo>, Option<String>) {
    if let Some(g) = held_repo(owner, repo) {
        return (Some(g), None);
    }
    let (Ok(login), Ok(name)) = (Login::parse(owner), RepoName::parse(repo)) else { return (None, None) };
    if colo_off(c) {
        return (None, None);
    }
    let url = colo::link_url(&login.key(), &name.key());
    (colo::match_link(&url).await, Some(url))
}

/// `owner/repo` from this isolate's login, `Placement` and `Authz` entries at any age (`rcache::last`): a guess.
fn held_repo(owner: &str, repo: &str) -> Option<Repo> {
    let name = RepoName::parse(repo).ok()?;
    let (account, record) = held_guess(owner)?;
    let (id, _) = graftyard_identity::AuthzRead::resolve(&rcache::last(&rcache::AUTHZ, &account)?, &name.key())?;
    Some(Repo::new(id, account, record))
}

/// Whether the caller may read `repo`, before the edge serves any of its bytes (`graftyard_identity::authorize_read`
/// over the repository's `settings` and `collaborator` rows and the owner's `Authz`, both read through `fresh`, at
/// once). `who` is the caller `authenticate` found, so the token's own reach is the one in `c.token_fields`.
/// `Ok(Err(why))` when either read failed: the caller answers 503, never a 404 that would read as "no such
/// repository".
async fn read_gate(
    c: &Ctx<'_>,
    repo: &Repo,
    who: Option<&Principal>,
    beside: Option<Beside<'_>>,
    share: colo::Share,
) -> Result<std::result::Result<graftyard_identity::Decision, String>> {
    let key = gate::key(repo.id, who, c.write.get());
    // The test arm with the share off (`x-graft-colo: off` with the bed token) keeps the isolate cache alone, under its
    // own keys, so it never takes an entry the shared arm copied (`colo-gate`'s flip compares the two in one row).
    #[cfg(feature = "test-faults")]
    let key = if c.colo_off { key.map(gate::off_key) } else { key };
    let Some(k) = key else {
        return Ok(match fresh_gate(c, repo, who, None).await {
            Ok((d, _, rows)) => {
                c.push_rows.replace(rows.map(|r| (r, false)));
                Ok(d)
            }
            Err(GateFail::Read(e)) => Err(e),
            Err(GateFail::Edge(e)) => return Err(e),
        });
    };
    // Every write keys and keeps its entries as a push's, whatever its share, so no write holds a read-only `Allow`
    // where a POST could take it.
    let share = colo::Share { push: c.write.get(), ..share };
    let share = if colo_off(c) { colo::Share { matches: false, puts: false, ..share } } else { share };
    let url = colo::gate_url(repo.id, k.1, &repo.record);
    let io =
        GateIo { c, repo, who, key: k, url, beside: std::cell::RefCell::new(beside), writable: Default::default() };
    // Boxed for the same reason: with `beside` the gate holds both its fresh read and the rows read beside the front.
    let g = Box::pin(colo::gate(&io, share)).await;
    // The agent rows the decision's own fill read, from the fresh read or the isolate entry that holds them; `None`
    // for a data-center copy or an older member, and `agents::push_context` then reads its own (`agent-rows-cache`).
    c.push_rows.replace(g.rows.map(|r| (r, g.served != colo::Served::Read)));
    match g.served {
        colo::Served::Read => {}
        colo::Served::Isolate(age) => c.timing.put("gate_iso", age as f64),
        colo::Served::Colo(age) => c.timing.put("gate_colo", age as f64),
    }
    if g.ahead && colo::begin(k) {
        reread_ahead(c, repo, who, k);
    }
    match g.decision {
        Ok(d) => Ok(Ok(d)),
        Err(GateFail::Read(e)) => Ok(Err(e)),
        Err(GateFail::Edge(e)) => Err(e),
    }
}

/// Whether this request is the test arm with the share off. Always false on a product edge.
fn colo_off(c: &Ctx<'_>) -> bool {
    #[cfg(feature = "test-faults")]
    return c.colo_off;
    #[cfg(not(feature = "test-faults"))]
    {
        let _ = c;
        false
    }
}

/// Why the read gate's fresh read gave no decision: a member read that failed (the caller answers 503), or the edge's
/// own error.
enum GateFail {
    Read(String),
    Edge(Error),
}

/// The read gate's effects for `colo::gate`: this request's clock, `gate`'s isolate entry, the data center's entry
/// at `url`, and `fresh_gate`.
struct GateIo<'a, 'c, 'b> {
    c: &'a Ctx<'c>,
    repo: &'a Repo,
    who: Option<&'a Principal>,
    key: gate::Key,
    url: String,
    beside: std::cell::RefCell<Option<Beside<'b>>>,
    /// Whether the last fresh read found the caller may also push here (`fresh_gate_on`, `colo::Share::push`).
    writable: std::cell::Cell<bool>,
}

impl colo::GateFx for GateIo<'_, '_, '_> {
    type Err = GateFail;
    type Rows = gate::Rows;
    fn now(&self) -> Timestamp {
        now()
    }
    /// The entry's own fill named its network, and the request keys by it from here (`Repo::fix`).
    fn held(&self) -> Option<colo::Held<gate::Rows>> {
        let (held, fork) = gate::get(&self.key)?;
        self.repo.fix(fork);
        Some(held)
    }
    /// Every hold follows a fill or a match that fixed the request's network, so the entry carries that one.
    fn hold(&self, at: Timestamp, d: graftyard_identity::Decision, foreign: bool, rows: Option<gate::Rows>) {
        gate::put(self.key, at, d, foreign, rows, self.repo.fork());
    }
    async fn matched(&self) -> Option<Timestamp> {
        let (at, fork) = api::timed(self.c, "gate_match", colo::match_gate(&self.url)).await?;
        self.repo.fix(fork);
        Some(at)
    }
    /// The network this request's own entry or fill fixed, never the isolate's map at put time.
    async fn share(&self, at: Timestamp) {
        let url = self.url.clone();
        self.c.ctx.wait_until(colo::put_gate(url, at, self.repo.fork()));
    }
    async fn read(&self) -> std::result::Result<colo::Fill<gate::Rows>, GateFail> {
        fresh_gate(self.c, self.repo, self.who, Some(&self.writable)).await
    }
    fn writable(&self) -> bool {
        self.writable.get()
    }
    async fn beside(
        &self,
    ) -> Option<(Timestamp, std::result::Result<(graftyard_identity::Decision, Option<Timestamp>), GateFail>)> {
        let Beside { repo: guessed, start, mut rows } = self.beside.borrow_mut().take()?;
        let started = async move {
            rows.as_mut().await;
            match rows.as_mut().take_output().expect("polled to the end") {
                Ok(r) => r,
                Err(e) => Err(e.to_string()),
            }
        };
        let (c, mismatch) = (self.c, guessed != *self.repo);
        // A guessed read answered after its resolve dropped it is read again through `fresh_gate`'s own read, which
        // creates a group that answers no leader, as before `beside`.
        let read = |_: Repo| std::future::ready(Ok(None));
        let (got, kept) = beside_rows(guessed, started, self.repo, read).await;
        let Ok(Some(store)) = got else {
            c.timing.put("gate_beside", 2.0);
            return match got {
                Err(e) if kept => Some((start, Err(GateFail::Read(e)))),
                _ => None,
            };
        };
        // `gate_beside`: 1 when the gate decided on the rows read beside the front, 2 when they were read again on
        // the resolved repository; `beside_bad` counts a kept read whose guess was not the resolved repository, which
        // must never happen (`beside`'s flip).
        c.timing.put("gate_beside", if kept { 1.0 } else { 2.0 });
        if kept && mismatch {
            c.timing.put("beside_bad", 1.0);
        }
        Some((start, fresh_gate_on(c, self.repo, self.who, std::future::ready(Ok(Ok(store))), None).await))
    }
}

/// `COLO_AHEAD`'s re-read, after this request: a fresh gate read in its own context that outlives the reply
/// (`waitUntil`), at most one per key in flight in this isolate. Its `Allow` replaces both entries with its own stamp,
/// and a refusal is held in the isolate and not put (`colo::reread`).
fn reread_ahead(c: &Ctx<'_>, repo: &Repo, who: Option<&Principal>, k: gate::Key) {
    let (env, js, cfg, sites, anchor, colo) = (
        c.env.clone(),
        worker::wasm_bindgen::JsCast::unchecked_into::<worker::worker_sys::Context>(
            worker::wasm_bindgen::JsValue::from(c.ctx.as_ref()),
        ),
        c.cfg.clone(),
        c.sites.clone(),
        c.anchor,
        c.colo,
    );
    let (repo, who, fields, ssh_front) = (repo.clone(), who.cloned(), c.token_fields.borrow().clone(), c.ssh_front);
    c.ctx.wait_until(async move {
        let ctx = Context::new(js);
        let bg = Ctx::background(&env, &ctx, &cfg, &sites, anchor, colo, fields, ssh_front);
        let url = colo::gate_url(repo.id, k.1, &repo.record);
        let io = GateIo {
            c: &bg,
            repo: &repo,
            who: who.as_ref(),
            key: k,
            url,
            beside: Default::default(),
            writable: Default::default(),
        };
        colo::reread(&io).await;
        colo::end(&k);
    });
}

/// The gate's fresh read: the repository's access rows and the owner's `Authz` (from `rcache` for a read-class
/// request), decided by `authorize_read`, the time of the oldest fold read under the decision (`reopened`), and a
/// push's agent rows read with it. `None` for a repository whose group is not created yet or was retired: hidden,
/// and never cached.
async fn fresh_gate(
    c: &Ctx<'_>,
    repo: &Repo,
    who: Option<&Principal>,
    writable: Option<&std::cell::Cell<bool>>,
) -> std::result::Result<colo::Fill<gate::Rows>, GateFail> {
    // A push that reads the access rows reads its agent rows in the same read. They ride with this decision into the
    // gate's isolate entry (`agent-rows-cache`), so a push served from that entry reads no rows either; only a member
    // that answered the access rows alone (an older release) leaves them `None`.
    let push = c.write.get() && c.cfg.enabled.contains(&graftyard_git::repo::REPO_FORMAT_AGENT);
    let agent_rows = std::cell::RefCell::new(None);
    let rows = async {
        if !push {
            return api::core_rows(c, repo, "access").await;
        }
        Ok(api::push_rows(c, repo).await?.map(|(core, agent)| {
            if agent {
                agent_rows.replace(Some(std::rc::Rc::new(core.clone())));
            }
            core
        }))
    };
    let (d, at) = fresh_gate_on(c, repo, who, rows, writable).await?;
    Ok((d, at, agent_rows.into_inner()))
}

/// `fresh_gate` on the access rows `rows` answers: its own read, or the rows read beside the front (`beside`). With
/// `writable`, also whether the caller may push here on the same rows and folds (`may_push`), set on each decision.
async fn fresh_gate_on(
    c: &Ctx<'_>,
    repo: &Repo,
    who: Option<&Principal>,
    rows: impl std::future::Future<Output = RowsRead>,
    writable: Option<&std::cell::Cell<bool>>,
) -> std::result::Result<(graftyard_identity::Decision, Option<Timestamp>), GateFail> {
    // A read-class request may decide on the owner's `Authz` from `rcache`, and the decision is then cached from that
    // fold's read, not from now, so a revocation still binds within one `READ_CACHE` (`reopened`).
    let at = now();
    let held = if c.read_class.get() { rcache::stamped(&rcache::AUTHZ, &repo.owner, at) } else { None };
    let fresh = || authz_read_as(c, repo.owner, &repo.record, false);
    let owner = async {
        match &held {
            Some((_, f)) => Ok(Some(f.clone())),
            None => fresh().await,
        }
    };
    // The two reads' own walls (`gate_rows`, `gate_authz`), so a slow gate names which read was slow (`gate-trace`).
    let (rows, owner) =
        futures_util::future::join(api::timed(c, "gate_rows", rows), api::timed(c, "gate_authz", owner)).await;
    let core = match rows.map_err(GateFail::Edge)? {
        Ok(core) => core,
        // A repository whose create has not committed `RepoOwner` yet, or whose group was retired: hidden from every
        // caller, as GitHub hides one that does not exist, and not cached, so it shows as soon as a create commits.
        Err(e) if graftyard_lookup::is_gone(&e) => return Ok((graftyard_identity::Decision::Hidden, None)),
        Err(e) => return Err(GateFail::Read(e)),
    };
    repo.fix(graftyard_git::tiers::fork(&core).map(|f| f.network));
    let access = graftyard_git::issue_ops::Access { repo: repo.id, owner: repo.owner, core: &core };
    let decide = |owner| {
        let owner = gate_owner(owner)?;
        let folds: [&dyn graftyard_identity::AuthzRead; 1] = [&owner];
        let fields = c.token_fields.borrow();
        if let Some(w) = writable {
            w.set(who.is_some_and(|p| may_push(p, fields.as_ref(), &folds, &access, &core)));
        }
        Ok(graftyard_identity::authorize_read(who, fields.as_ref(), &folds, &access))
    };
    let (d, stamp) = reopened(held.map(|(t, _)| t), at, decide(owner), fresh, decide).await.map_err(GateFail::Read)?;
    Ok((d, Some(stamp)))
}

/// The read gate's decision and the time it ages from. `held` is when the cached `Authz` it was first made on was read
/// (`None` for a fresh read started at `at`). A decision on a cached fold that does not allow is made again on a
/// `fresh` read, so a grant opens at once and only a revocation waits out the cache, as for API reads; one that allows
/// ages from the fold's read, so caching it again never stretches the bound past one `READ_CACHE`. A failed read is
/// not read again: a cache never holds an error, so it was a fresh read already.
async fn reopened<V, E, F: std::future::Future<Output = V>>(
    held: Option<Timestamp>,
    at: Timestamp,
    first: std::result::Result<graftyard_identity::Decision, E>,
    fresh: impl FnOnce() -> F,
    decide: impl FnOnce(V) -> std::result::Result<graftyard_identity::Decision, E>,
) -> std::result::Result<(graftyard_identity::Decision, Timestamp), E> {
    match (held, first) {
        (Some(_), Ok(d)) if d != graftyard_identity::Decision::Allow => Ok((decide(fresh().await)?, at)),
        (held, first) => Ok((first?, held.unwrap_or(at))),
    }
}

/// Whether `p` may push to the repository on the gate's rows and folds: `Push::decide`'s `may_write` (a `Write`
/// permission through the owner's `Authz` or a collaborator row, the token's own reach with `contents: write`, and a
/// repository not deleted). An advertisement puts its `Allow` for a POST only when this holds (`cold-write`, C); the
/// POST's leader decides it again on its own read either way.
fn may_push(
    p: &Principal,
    fields: Option<&graftyard_identity::TokenFields>,
    folds: &[&dyn graftyard_identity::AuthzRead],
    access: &graftyard_git::issue_ops::Access<'_>,
    core: &dyn graftyard_fold::Read,
) -> bool {
    use graftyard_identity::{Access, Decision::Allow, WriteAction};
    graftyard_git::tiers::deleted(core).is_none()
        && graftyard_identity::authorize_write(p, WriteAction::Push, folds, access) == Allow
        && graftyard_identity::token_allows(fields, &[("contents", Access::Write)], access) == Allow
}

/// The owner's `Authz` for the read gate, or why it failed. A read its members shed keeps its OVERLOADED reason, so
/// `api::read_failed` answers `Retry-After: 1` as for the access rows.
fn gate_owner(
    owner: std::result::Result<Option<graftyard_identity::AuthzFold>, String>,
) -> std::result::Result<graftyard_identity::AuthzFold, String> {
    match owner {
        Ok(Some(o)) => Ok(o),
        Ok(None) => Err("the owner's Authz chain did not answer".into()),
        Err(e) => Err(format!("the owner's Authz chain did not answer: {e}")),
    }
}

/// The read gate's per-isolate cache: a decision is reused for `READ_CACHE` (`design/DESIGN.md`, Shape, Identity:
/// "reads honor a revocation within 10 s"), so the three requests of one clone or fetch read the access rows and the
/// owner's `Authz` once. Decisions are keyed by the repository and the stored token's hash, an installation token's
/// `jti` (hashed with a domain prefix), or no token. A web session is not cached. An installation token's revocation
/// or narrowing is likewise honoured within `READ_CACHE`.
mod gate {
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    use graftyard_identity::{Actor, Credential, Decision, Principal, stateless::InstallationClaims};
    use graftyard_types::{NetworkId, RepoId, Timestamp};
    use sha2::{Digest, Sha256};

    /// The most decisions one isolate keeps; past it the cache starts empty again.
    const ENTRIES_MAX: usize = 4_096;

    pub(crate) type Key = (RepoId, Option<[u8; 32]>);
    /// A push gate's agent rows (`crate::api::push_rows`), shared by its entry and the requests it serves.
    pub(crate) type Rows = std::rc::Rc<graftyard_fold::MemStore>;
    /// The most agent-row bytes one isolate's entries hold, counted per entry [guess: a few hundred tasks a
    /// repository, about 100 B a task row]. An entry past it keeps its decision without rows, and its pushes read
    /// their own (`agent-rows-cache`).
    pub(crate) const ROWS_BYTES_MAX: usize = 8 << 20;

    struct Entry {
        at: Timestamp,
        d: Decision,
        foreign: bool,
        rows: Option<(Rows, usize)>,
        /// The fork's network the fill or match that made the decision named; `None` for a root.
        fork: Option<NetworkId>,
    }

    thread_local! {
        static CACHE: RefCell<BTreeMap<Key, Entry>> = const { RefCell::new(BTreeMap::new()) };
        static ROWS_BYTES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    }

    /// A store's bytes as counted against ROWS_BYTES_MAX: its keys' and columns' value lengths.
    pub(crate) fn rows_bytes_of(s: &graftyard_fold::MemStore) -> usize {
        use graftyard_fold::Val;
        let val = |v: &Val| match v {
            Val::Null => 1,
            Val::Int(_) => 8,
            Val::Text(t) => t.len(),
            Val::Blob(b) => b.len(),
        };
        s.0.iter()
            .map(|(t, rows)| {
                t.len()
                    + rows
                        .iter()
                        .map(|(k, r)| k.iter().map(val).sum::<usize>() + r.cols.iter().map(val).sum::<usize>())
                        .sum::<usize>()
            })
            .sum()
    }

    /// The agent-row bytes this isolate's entries hold now.
    #[cfg(test)]
    pub(crate) fn rows_bytes() -> usize {
        ROWS_BYTES.with(|b| b.get())
    }

    fn stateless_key(claims: &InstallationClaims) -> [u8; 32] {
        let mut hash = Sha256::new();
        hash.update(b"graftyard-read-gate-ghs\0");
        hash.update(claims.jti.0.0.to_be_bytes());
        hash.finalize().into()
    }

    /// A write's key (`push`) is another hash of the same credential, so its entries, in the isolate and in the data
    /// center (`colo::gate_url` hashes this credential), never serve a fetch and a fetch's never serve a push
    /// (`cold-write`, C).
    pub(crate) fn key(repo: RepoId, who: Option<&Principal>, push: bool) -> Option<Key> {
        let k = match who {
            None => (repo, None),
            Some(Principal { actor: Actor::User(_), credential: Credential::Stored(h), .. }) => (repo, Some(*h)),
            Some(Principal { credential: Credential::Stateless(claims), .. }) => (repo, Some(stateless_key(claims))),
            Some(_) => return None,
        };
        Some(if push { push_key(k) } else { k })
    }

    /// Anonymous stays distinct from every credential.
    fn push_key(k: Key) -> Key {
        let mut hash = Sha256::new();
        hash.update(b"graftyard-read-gate-push\0");
        match k.1 {
            Some(c) => {
                hash.update([1]);
                hash.update(c);
            }
            None => hash.update([0]),
        }
        (k.0, Some(hash.finalize().into()))
    }

    /// The decision held for `k` at any age, the time of the oldest fold read it was made on, whether it was copied
    /// from the data center's entry, and the agent rows its own fill read; `colo::gate` decides whether it may serve
    /// (`colo::servable`), and the rows serve only with it. Beside it, the fork's network its fill named.
    pub(crate) fn get(k: &Key) -> Option<(crate::colo::Held<Rows>, Option<NetworkId>)> {
        CACHE.with(|c| {
            c.borrow().get(k).map(|e| ((e.at, e.d, e.foreign, e.rows.as_ref().map(|(r, _)| r.clone())), e.fork))
        })
    }

    /// The share-off arm's key on a test install (`x-graft-colo: off`): the same credential and repository under
    /// another hash, so the arm never takes an entry the shared arm copied into this isolate.
    #[cfg(feature = "test-faults")]
    pub(crate) fn off_key(k: Key) -> Key {
        let mut hash = Sha256::new();
        hash.update(b"graftyard-read-gate-off\0");
        hash.update(k.1.unwrap_or([0; 32]));
        (k.0, Some(hash.finalize().into()))
    }

    /// Holds `d` for `k`, stamped `at` (the oldest fold read under it), with the agent rows the same fill read;
    /// `foreign` when copied from the data center. It replaces `k`'s entry whole, so no earlier fill's rows stay
    /// beside it, and with the fork's network that fill named.
    pub(crate) fn put(k: Key, at: Timestamp, d: Decision, foreign: bool, rows: Option<Rows>, fork: Option<NetworkId>) {
        CACHE.with(|c| {
            let mut c = c.borrow_mut();
            ROWS_BYTES.with(|b| {
                if c.len() >= ENTRIES_MAX {
                    c.clear();
                    b.set(0);
                }
                if let Some((_, n)) = c.remove(&k).and_then(|e| e.rows) {
                    b.set(b.get().saturating_sub(n));
                }
                let rows = rows.and_then(|r| {
                    let n = rows_bytes_of(&r);
                    (b.get() + n <= ROWS_BYTES_MAX).then(|| {
                        b.set(b.get() + n);
                        (r, n)
                    })
                });
                c.insert(k, Entry { at, d, foreign, rows, fork });
            });
        });
    }
}

/// Test installs: one Workers Logs line per push, `{"push": rid, "isolate", "open", "status", "timing"}`. `isolate` is
/// drawn once per isolate, so a run can group pushes by the isolate that served them, and `open` is how many pushes
/// that isolate held when this one started, itself included. B0-32 m3fr8 left two readings unproven for want of them:
/// that pushes decided stage-first before their own isolate's first `Stage` reply, and that the gaps between a push's
/// reads were its isolate's event loop (`tests/am1/results/lm1-rows-m3f.md`, "m3fr8 after the run").
#[cfg(feature = "test-faults")]
mod push_log {
    use std::cell::Cell;

    thread_local! {
        static ISOLATE: Cell<u64> = const { Cell::new(0) };
        static OPEN: Cell<u32> = const { Cell::new(0) };
    }

    /// This isolate's token, drawn on its first push.
    fn isolate() -> u64 {
        ISOLATE.with(|i| {
            if i.get() == 0 {
                let r = super::rand10();
                i.set(u64::from_le_bytes(r[..8].try_into().expect("8 bytes")) | 1);
            }
            i.get()
        })
    }

    /// A push starts: the count of pushes this isolate holds, this one included.
    pub(crate) fn opened() -> u32 {
        OPEN.with(|o| {
            o.set(o.get() + 1);
            o.get()
        })
    }

    pub(crate) fn closed() {
        OPEN.with(|o| o.set(o.get().saturating_sub(1)));
    }

    /// `client` is the driver's push id and `end` the leader's answer (`Ctx::push_end`), `null` for a push the edge
    /// answered itself before its commit (a `bound` refusal, a 4xx, a 503 before the send).
    pub(crate) fn line(
        rid: graftyard_types::RequestId,
        open: u32,
        status: u16,
        timing: &str,
        client: Option<&str>,
        end: Option<serde_json::Value>,
    ) -> String {
        let isolate = format!("{:016x}", isolate());
        serde_json::json!({ "push": rid.to_string(), "isolate": isolate, "open": open, "status": status, "timing": timing,
            "client": client, "end": end })
        .to_string()
    }
}

/// Groups whose leader answered this isolate's push `Stage` (`AdmitError::Stage`, `design/DESIGN.md`, journey 1, step
/// 1): for HOT_WINDOW after it, a push that would send an inline pack over INLINE_MAX_HOT stages it first, as the
/// leader would ask, instead of a commit the leader refuses and a second one. A group with no `Stage` here in the last
/// HOT_WINDOW is cool and its pushes commit inline as before, so a cool group's push never pays the bucket place; the
/// entry lapses on its own, and the first push after it learns again from the leader. An Agent-class push with an
/// inline pack over AGENT_INLINE_ENTRY stages first on any group, because the leader's per-entry budget answers such a
/// pack `Stage` whatever its group (`decisions/DECISIONS.md`, "`agent-stage-first`").
mod hot {
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    use graftyard_raft::admit::{AGENT_INLINE_ENTRY, HOT_WINDOW, INLINE_MAX_HOT};
    use graftyard_types::{RepoId, Timestamp};

    /// The most groups one isolate remembers; past it the map starts empty again.
    const ENTRIES_MAX: usize = 1_024;

    thread_local! {
        static STAGED: RefCell<BTreeMap<RepoId, Timestamp>> = const { RefCell::new(BTreeMap::new()) };
    }

    /// The leader answered a push to `repo` `Stage` at `now`.
    pub(crate) fn staged(repo: RepoId, now: Timestamp) {
        STAGED.with(|m| {
            let mut m = m.borrow_mut();
            if m.len() >= ENTRIES_MAX {
                m.clear();
            }
            m.insert(repo, now);
        });
    }

    /// Whether a push to `repo` at `now` with `inline` pack bytes stages its pack before its first commit; `agent` is
    /// whether the edge classes it Agent. Only a push sent with an admission class can get `Stage`, which the caller
    /// checks.
    pub(crate) fn stage_first(repo: RepoId, now: Timestamp, inline: usize, agent: bool) -> bool {
        let w = HOT_WINDOW.as_millis() as u64;
        (agent && inline > AGENT_INLINE_ENTRY)
            || inline > INLINE_MAX_HOT
                && STAGED.with(|m| m.borrow().get(&repo).is_some_and(|at| now.0.saturating_sub(at.0) <= w))
    }
}

#[cfg(test)]
mod hot_tests {
    use graftyard_raft::admit::{AGENT_INLINE_ENTRY, INLINE_MAX_HOT};
    use graftyard_types::{DbId, RepoId, Timestamp};

    use super::hot::{stage_first, staged};

    /// A cool group's push commits inline whatever its size; once the leader answered `Stage` it stages first, for
    /// HOT_WINDOW (5 s) and not a millisecond past it, and only packs over INLINE_MAX_HOT of a push with a class.
    #[test]
    fn only_a_group_that_answered_stage_within_hot_window_stages_first() {
        let (cool, hot) = (RepoId(DbId(301)), RepoId(DbId(302)));
        let big = INLINE_MAX_HOT + 1;
        for size in [1, INLINE_MAX_HOT, big, 1 << 20] {
            assert!(!stage_first(cool, Timestamp(10_000), size, false), "a cool group never stages first ({size} B)");
        }
        staged(hot, Timestamp(10_000));
        assert!(stage_first(hot, Timestamp(10_000), big, false));
        assert!(stage_first(hot, Timestamp(15_000), big, false), "at HOT_WINDOW");
        assert!(!stage_first(hot, Timestamp(15_001), big, false), "past HOT_WINDOW the group is cool again");
        assert!(!stage_first(hot, Timestamp(12_000), INLINE_MAX_HOT, false), "a pack the leader keeps inline");
        assert!(!stage_first(cool, Timestamp(12_000), big, false), "another group is not hot by this one");
    }

    /// The leader answers `Stage` to every Agent inline pack over AGENT_INLINE_ENTRY, hot group or not (B0-32 m3fwi3:
    /// 206 of 489 pushes committed twice), so such a push stages first on a cool group; a pack at the budget, and a
    /// Human or Bot push of any size, keeps its inline pack there.
    #[test]
    fn an_agent_pack_past_the_entry_budget_stages_first_on_a_cool_group() {
        let cool = RepoId(DbId(303));
        let now = Timestamp(50_000);
        assert!(stage_first(cool, now, 256 << 10, true), "B0-32's 256 KiB agent push");
        assert!(stage_first(cool, now, AGENT_INLINE_ENTRY + 1, true));
        assert!(!stage_first(cool, now, AGENT_INLINE_ENTRY, true), "at AGENT_INLINE_ENTRY the leader keeps it inline");
        assert!(!stage_first(cool, now, INLINE_MAX_HOT + 1, true), "past INLINE_MAX_HOT alone is the hot rule's");
        for size in [AGENT_INLINE_ENTRY + 1, 256 << 10, 1 << 20] {
            assert!(!stage_first(cool, now, size, false), "a push not classed Agent ({size} B)");
        }
    }
}

#[cfg(test)]
mod gate_tests {
    use std::collections::BTreeMap;

    use graftyard_identity::stateless::InstallationClaims;
    use graftyard_identity::{
        Actor, AppId, Credential, InstallationId, Jti, Kid, PermissionSet, Principal, RepoScope, ScopeSet, UserId,
        WebSessionId,
    };
    use graftyard_types::{AccountId, DbId, RepoId, Timestamp, Ulid};
    use sha2::{Digest, Sha256};

    use super::gate;

    fn claims(jti: u128) -> InstallationClaims {
        InstallationClaims {
            app: AppId(DbId(1)),
            installation: InstallationId(DbId(2)),
            repos: RepoScope::All,
            permissions: PermissionSet(BTreeMap::new()),
            jti: Jti(Ulid(jti)),
            iat: Timestamp(1_000),
            exp: Timestamp(2_000),
            kid: Kid(3),
            task: None,
        }
    }

    fn principal(credential: Credential) -> Principal {
        Principal {
            actor: Actor::User(UserId(AccountId(DbId(1)))),
            scopes: ScopeSet(0),
            credential,
            sso_orgs: smallvec::SmallVec::new(),
        }
    }

    fn stateless(jti: u128) -> Principal {
        principal(Credential::Stateless(Box::new(claims(jti))))
    }

    fn rows(tag: &str) -> gate::Rows {
        let mut s = graftyard_fold::MemStore::default();
        let row =
            graftyard_fold::Stored { written: Default::default(), cols: vec![graftyard_fold::Val::Text(tag.into())] };
        graftyard_fold::Store::put(&mut s, "agent_task", vec![graftyard_fold::Val::Text("k".into())], row);
        std::rc::Rc::new(s)
    }

    fn tag(r: &gate::Rows) -> String {
        match &graftyard_fold::Store::get(&**r, "agent_task", &[graftyard_fold::Val::Text("k".into())]).unwrap().cols[0]
        {
            graftyard_fold::Val::Text(t) => t.to_string(),
            _ => unreachable!(),
        }
    }

    /// `fork-network-read`: a fork's network comes from its `fork` row in the gate's access rows, and every path
    /// keys by it from then on, packs and the first shard's name included; a root's rows hold none and it keeps its
    /// own id, the bytes main's paths use. The request keys by what its own read fixed: another request that
    /// overflows the isolate's map meanwhile moves neither this request's network nor the network its gate entry
    /// carries (AM2's review of head 1: a cleared map put a fork's pack under `pack/<fork>/`).
    #[test]
    fn a_forks_network_is_fixed_by_its_own_read_and_a_root_keeps_its_own_id() {
        use graftyard_types::{NetworkId, PlacementName, PlacementRecord, SiteId};
        let record = PlacementRecord {
            name: PlacementName::Tri,
            sites: [SiteId(0), SiteId(1), SiteId(2)].into_iter().collect(),
            home: SiteId(0),
            generation: 1,
            moving_from: None,
            at: Timestamp(1),
        };
        let repo = |id: i64| super::Repo::new(RepoId(DbId(id)), AccountId(DbId(9)), record.clone());
        let (root, fork, base) = (repo(131_901), repo(131_902), NetworkId(DbId(131_900)));
        let mut rows = graftyard_fold::MemStore::default();
        let cols = |r: &graftyard_git::tiers::ForkRow| {
            vec![graftyard_fold::Val::Blob(postcard::to_allocvec(r).unwrap().into())]
        };
        let row = graftyard_git::tiers::ForkRow { network: base, fork_of: Some((base.root(), Default::default())) };
        graftyard_fold::Store::put(
            &mut rows,
            "fork",
            vec![],
            graftyard_fold::Stored { written: Default::default(), cols: cols(&row) },
        );
        let fixed = |r: &graftyard_fold::MemStore| graftyard_git::tiers::fork(r).map(|f| f.network);
        assert_eq!(fork.network(), NetworkId::of_root(fork.id), "before any read the edge knows no fork");
        fork.fix(fixed(&rows));
        root.fix(fixed(&graftyard_fold::MemStore::default()));
        assert_eq!(fork.network(), base);
        assert_eq!(graftyard_git::first_shard_name(fork.network(), SiteId(2)), "git#2#net:131900");
        assert_eq!(graftyard_git::R2Key::pack_prefix(fork.network()), "pack/131900/");
        assert_eq!(
            (root.network(), root.fork(), super::nets::fork(root.id)),
            (NetworkId::of_root(root.id), None, None)
        );
        assert_eq!(super::nets::fork(fork.id), Some(base));
        assert_eq!(repo(131_902).network(), base, "a later request's guess, before its own gate");
        // The fork's gate entry holds its fill's network.
        let k = (fork.id, Some([3; 32]));
        gate::put(k, Timestamp(1_000), graftyard_identity::Decision::Allow, false, None, fork.fork());
        // Another request learns enough forks to overflow the map, which clears it.
        for id in 0..5_000 {
            super::nets::learn(RepoId(DbId(200_000 + id)), NetworkId(DbId(1)));
        }
        assert_eq!(super::nets::fork(fork.id), None, "the map was cleared");
        assert_eq!(fork.network(), base, "this request still keys its packs by its network");
        assert_eq!(fork.fork(), Some(base), "and still shares it with its decision");
        assert_eq!(gate::get(&k).map(|(_, f)| f), Some(Some(base)), "the entry kept its own fill's network");
        let later = repo(131_902);
        assert_eq!(later.network(), NetworkId::of_root(later.id), "a new request has only the guess, until its gate");
    }

    /// `agent-rows-cache`: an entry's rows are its own fill's. A put replaces the whole entry, so a decision put
    /// without rows (a data-center copy) never keeps an earlier fill's rows beside it, and a new fill's rows replace
    /// the old; the bytes counted fall back with each replacement; and past `ROWS_BYTES_MAX` an entry keeps its
    /// decision with no rows, so its pushes read their own.
    #[test]
    fn a_gate_entry_holds_only_its_own_fills_rows_within_the_byte_cap() {
        let k = (RepoId(DbId(901)), Some([1; 32]));
        let base = gate::rows_bytes();
        gate::put(k, Timestamp(1_000), graftyard_identity::Decision::Allow, false, Some(rows("fill-1")), None);
        let one = gate::rows_bytes() - base;
        assert!(one > 0);
        let ((at, d, foreign, got), _) = gate::get(&k).unwrap();
        assert_eq!(
            (at, d, foreign, tag(&got.unwrap())),
            (Timestamp(1_000), graftyard_identity::Decision::Allow, false, "fill-1".into())
        );
        gate::put(k, Timestamp(2_000), graftyard_identity::Decision::Allow, true, None, None);
        assert_eq!(gate::get(&k).unwrap().0.3, None, "a copy's decision carries no earlier fill's rows");
        assert_eq!(gate::rows_bytes(), base, "the replaced rows' bytes are released");
        gate::put(k, Timestamp(3_000), graftyard_identity::Decision::Hidden, false, Some(rows("fill-3")), None);
        assert_eq!(tag(&gate::get(&k).unwrap().0.3.unwrap()), "fill-3");
        assert_eq!(gate::rows_bytes() - base, one);
        // A fill too large for what is left keeps its decision and no rows.
        let mut big = graftyard_fold::MemStore::default();
        let blob = vec![0u8; gate::ROWS_BYTES_MAX];
        let row =
            graftyard_fold::Stored { written: Default::default(), cols: vec![graftyard_fold::Val::Blob(blob.into())] };
        graftyard_fold::Store::put(&mut big, "agent_task", Vec::new(), row);
        let k2 = (RepoId(DbId(902)), Some([1; 32]));
        gate::put(k2, Timestamp(4_000), graftyard_identity::Decision::Allow, false, Some(std::rc::Rc::new(big)), None);
        assert_eq!(
            gate::get(&k2).map(|(e, _)| (e.1, e.3.is_none())),
            Some((graftyard_identity::Decision::Allow, true))
        );
        assert_eq!(gate::rows_bytes() - base, one, "and counts nothing for it");
    }

    #[test]
    fn stateless_keys_follow_the_jti() {
        let repo = RepoId(DbId(1));
        let first = stateless(10);
        let second = stateless(11);
        assert_eq!(gate::key(repo, Some(&first), false), gate::key(repo, Some(&first), false));
        assert_ne!(gate::key(repo, Some(&first), false), gate::key(repo, Some(&second), false));
    }

    #[test]
    fn stateless_keys_are_domain_separated_from_stored_token_hashes() {
        let repo = RepoId(DbId(1));
        let jti = 0x0001_0203_0405_0607_0809_0a0b_0c0d_0e0f;
        let stateless = stateless(jti);
        let unseparated: [u8; 32] = Sha256::digest(jti.to_be_bytes()).into();
        for hash in [[0; 32], unseparated] {
            let stored = principal(Credential::Stored(hash));
            assert_ne!(gate::key(repo, Some(&stateless), false), gate::key(repo, Some(&stored), false));
        }
    }

    /// `cold-write` C's key: a receive-pack's entry, in the isolate and in the data center, binds the credential
    /// and the repository and is never a fetch's. A fetch's `Allow` for the same token and repository, another token's
    /// push entry, another repository's, and anonymous all key apart, so none serves a POST.
    #[test]
    fn a_push_gate_key_misses_a_fetchs_allow_and_another_tokens() {
        let (repo, other_repo) = (RepoId(DbId(1)), RepoId(DbId(2)));
        let record = graftyard_types::PlacementRecord {
            name: graftyard_types::PlacementName::Tri,
            sites: [graftyard_types::SiteId(0), graftyard_types::SiteId(1), graftyard_types::SiteId(2)]
                .into_iter()
                .collect(),
            home: graftyard_types::SiteId(0),
            generation: 1,
            moving_from: None,
            at: Timestamp(1),
        };
        let (a, b) = (principal(Credential::Stored([7; 32])), principal(Credential::Stored([8; 32])));
        let url = |k: gate::Key| super::colo::gate_url(k.0, k.1, &record);
        let push = |r, p: Option<&Principal>| gate::key(r, p, true).unwrap();
        let post = push(repo, Some(&a));
        assert_eq!(post, push(repo, Some(&a)), "an advertisement and its POST share the entry");
        for (what, k) in [
            ("a fetch's Allow, same token and repository", gate::key(repo, Some(&a), false).unwrap()),
            ("another token's push entry", push(repo, Some(&b))),
            ("another token's fetch", gate::key(repo, Some(&b), false).unwrap()),
            ("another repository's push entry", push(other_repo, Some(&a))),
            ("an anonymous push entry", push(repo, None)),
            ("an anonymous fetch", gate::key(repo, None, false).unwrap()),
        ] {
            assert_ne!(post, k, "isolate key: {what}");
            assert_ne!(url(post), url(k), "data-center key: {what}");
        }
        let (s1, s2) = (stateless(10), stateless(11));
        assert_ne!(push(repo, Some(&s1)), push(repo, Some(&s2)), "a ghs_ push entry follows its jti");
        assert_ne!(push(repo, Some(&s1)), gate::key(repo, Some(&s1), false).unwrap());
    }

    #[test]
    fn sessions_stay_uncached_and_anonymous_stays_repository_scoped() {
        let repo = RepoId(DbId(1));
        let session = principal(Credential::Session(WebSessionId(Ulid(1))));
        assert_eq!(gate::key(repo, Some(&session), false), None);
        assert_eq!(gate::key(repo, None, false), Some((repo, None)));
    }
}

/// The `Retry-After` of an agent push refused while placement fails [guess]: R2's outages ran tens of minutes, so a
/// retry sooner only adds load.
const OUTAGE_RETRY_S: u32 = 60;

/// F91's A/B switch on a test install: the edge answers a v2 `ls-refs` from the group's refs (`b034-reads`) unless the
/// request says `x-graft-ls-refs: shard` with the bed token, when the shard answers it as before. On by default, so
/// one edge build serves both arms of the A/B and a product edge never reads the header.
#[cfg_attr(not(feature = "test-faults"), allow(dead_code))]
fn ls_refs_at_edge(header: Option<&str>, token: Option<&str>, secret: Option<&str>) -> bool {
    !matches!((header, token, secret), (Some("shard"), Some(t), Some(s)) if t == s)
}

/// The leader-sync A/B's arm B on a test install: `x-graft-lean` with the bed token makes this request's commits lean
/// (`graftyard_lookup::Hooks::lean`): held on the leader to their outcome, with no `/outcome_by_request` asks. Off by
/// default, so one edge build serves both arms and a product edge never reads the header.
#[cfg_attr(not(feature = "test-faults"), allow(dead_code))]
fn lean_asked(header: bool, token: Option<&str>, secret: Option<&str>) -> bool {
    header && matches!((token, secret), (Some(t), Some(s)) if t == s)
}

/// The driver's push id from `x-graft-client-push` as the push's log line carries it: at most 64 of
/// `[A-Za-z0-9._-]`, else none, so a header cannot write anything else into the line.
#[cfg_attr(not(feature = "test-faults"), allow(dead_code))]
fn client_push_id(h: Option<&str>) -> Option<String> {
    h.filter(|v| !v.is_empty() && v.len() <= 64 && v.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b)))
        .map(str::to_owned)
}

/// Whether this request sees placement failing. Only test installs can (`Ctx::place_fail`); a production edge has
/// no bucket probe yet, so it learns of an outage only from a failed `place_teed`.
fn placement_failing(c: &Ctx<'_>) -> bool {
    #[cfg(feature = "test-faults")]
    return c.place_fail;
    #[cfg(not(feature = "test-faults"))]
    {
        let _ = c;
        false
    }
}

/// The admission a command by `who` carries to its leader (`graftyard_git::agent::admit_class`), or `None` for an
/// anonymous Human command, which `Member::commit` takes as before. A user token with no App and no
/// `X-Graftyard-Agent-Session` is Human with its actor, without reading anything more, so a default request's reads
/// and client-visible replies do not change, and the leader counts its writes toward DEMOTE_PER_MIN. The leader's
/// `decide` classes the actor again from its own fresh read, so this class only orders admission.
pub(crate) async fn admission(
    c: &Ctx<'_>,
    who: &Principal,
    provenance: &[graftyard_git::agent::Provenance],
    push: bool,
    inline: usize,
) -> Option<graftyard_git::group::AdmitWire> {
    let app = c.token_app.get();
    let user = matches!(who.actor, Actor::User(_) | Actor::Anonymous);
    if user && app.is_none() && !c.agent_session {
        return graftyard_git::group::AdmitWire::of(graftyard_raft::admit::AdmitClass::Human, who, push, inline)
            .map(|a| graftyard_git::group::AdmitWire { placement_ok: !placement_failing(c), ..a });
    }
    let install = install_authz(c).await;
    let read = install.as_ref().map(|a| a as &dyn graftyard_identity::AuthzRead);
    let class = graftyard_git::agent::admit_class(who, read, app, provenance, c.agent_session);
    graftyard_git::group::AdmitWire::of(class, who, push, inline)
        .map(|a| graftyard_git::group::AdmitWire { placement_ok: !placement_failing(c), ..a })
}

/// Whether a push by `who` is held to AGENT_PUSH_BOUND: one `admission` may class Agent without reading anything, an
/// App's or a user token's with an App or `X-Graftyard-Agent-Session`. A plain user token's push is not.
fn bounded(c: &Ctx<'_>, who: &Principal) -> bool {
    matches!(who.actor, Actor::App { .. }) || c.token_app.get().is_some() || c.agent_session
}

/// What is left of the push's AGENT_PUSH_BOUND now, or `None` for a push with no bound.
fn budget_left(c: &Ctx<'_>) -> Option<std::time::Duration> {
    c.bound.get().map(|until| std::time::Duration::from_millis((until - js_sys::Date::now()).max(0.0) as u64))
}

/// At the read gate: an authenticated push the edge classes Agent (`agent`) whose gate read its members shed until its
/// AGENT_PUSH_BOUND ended answers `ng <ref> agent admission full, retry after 1s`, not 503. Any other gate failure, and
/// any push not classed Agent, keeps the 503 with `Retry-After`.
fn gate_ng(agent: bool, e: &str) -> Option<Refusal> {
    (agent && e.contains(graftyard_lookup::OVERLOADED)).then(|| Refusal::agent_full(1))
}

/// At a push's commit send: an Agent push with none of its AGENT_PUSH_BOUND left is refused in B0-32's shape, `ng <ref>
/// agent admission full, retry after 1s`, before anything reaches the group; one with some left sends it as `budget_ms`.
/// Any other class, and a push with no bound, sends none and waits as before.
fn bound_at_send(agent: bool, left: Option<std::time::Duration>) -> std::result::Result<Option<u64>, Refusal> {
    match left {
        Some(l) if agent && l.is_zero() => Err(Refusal::agent_full(1)),
        Some(l) if agent => Ok(Some(l.as_millis() as u64)),
        _ => Ok(None),
    }
}

/// What the leader's last advert (`graftyard_lookup::advert`) says of a write with `budget` left: send it, refuse it
/// at the edge (the advert is fresh and its wait passes the budget), hold it on a lapsed advert that said full
/// (edge-refusal-bound: one probe a chain an isolate per ADVERT_TTL goes through and the rest are refused, so an
/// isolate that refuses everything and hears no advert does not let a burst through when it lapses), or send it
/// because that advert is past ADVERT_STALE (`Stale`: the advert path is down, and the leader decides).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AdvertSays {
    Send,
    Refuse(graftyard_lookup::Advert),
    /// The lapsed advert and its age in ms.
    Hold(graftyard_lookup::Advert, u64),
    Stale(u64),
}

fn advert_says(h: graftyard_lookup::Heard, budget: std::time::Duration) -> AdvertSays {
    use graftyard_lookup::Heard;
    let past = |a: &graftyard_lookup::Advert| u128::from(a.wait_ms) > budget.as_millis();
    let stale = graftyard_raft::admit::ADVERT_STALE.as_millis() as u64;
    match h {
        Heard::Fresh(a) if past(&a) => AdvertSays::Refuse(a),
        Heard::Lapsed(a, age) if past(&a) && age <= stale => AdvertSays::Hold(a, age),
        Heard::Lapsed(a, age) if past(&a) => AdvertSays::Stale(age),
        _ => AdvertSays::Send,
    }
}

/// Whether a held write goes as its chain's probe: a request that already went as one (a push at its send checks after
/// `receive_pack`'s) always does, else the chain's first in ADVERT_TTL on this isolate (`graftyard_lookup::take_probe`).
/// Without the first, a probe push would be held at its own send by its own probe.
fn probe_goes(already: bool, chain: ChainId, now: u64) -> bool {
    already || graftyard_lookup::take_probe(chain, now)
}

/// Where a write meets the advert (edge-refusal-bound): a push at `receive_pack` and a REST, GraphQL or agent-API
/// write at the top of `read_repo` (`Check`), a push again at its first commit send (`Send`) and before its Stage
/// re-send (`Stage`), each with what is left of its bound there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum At {
    Check,
    Send,
    Stage,
}

/// `advert_says` for a caller of `class` (`admission`, `None` for Human): only an Agent-classed caller whose token
/// reaches the repository (`token_reaches`) is refused or counted; Human, Bot, and MergeQueue always send.
fn advert_verdict(says: AdvertSays, class: Option<graftyard_git::group::ClassWire>, reaches: bool) -> AdvertSays {
    match (says, class) {
        (s, Some(graftyard_git::group::ClassWire::Agent)) if reaches => s,
        _ => AdvertSays::Send,
    }
}

/// Whether the caller's token can reach repository `repo` of `owner` at all, from what `authenticate` already holds:
/// an installation token's repository list, or a scoped user token's owner and repository list. A token that cannot
/// is never refused by the advert before the read gate, so a 429 never stands where the gate would answer 404
/// (`decisions/DECISIONS.md`, "`edge-refusal`", disclosure).
fn token_reaches(
    who: &Principal,
    fields: Option<&graftyard_identity::TokenFields>,
    repo: RepoId,
    owner: AccountId,
) -> bool {
    use graftyard_identity::RepoScope;
    let listed = |s: &RepoScope| match s {
        RepoScope::All => true,
        RepoScope::Selected(r) => r.contains(&repo),
    };
    let claims = match &who.credential {
        Credential::Stateless(claims) => listed(&claims.repos),
        _ => true,
    };
    let scoped =
        fields.is_none_or(|f| f.resource_owner.is_none_or(|o| o == owner) && f.repos.as_ref().is_none_or(listed));
    claims && scoped
}

/// edge-refusal (`design/DESIGN.md`, Shape, the agent block): the retry hint to refuse a write by `who` to `repo`
/// with, when its group's leader advertised, within ADVERT_TTL, an agent wait longer than `budget` and the edge classes
/// the caller Agent. `None` sends the write as before. The advert is read first and costs nothing, so only a write the
/// advert already refuses reads the install's `Authz` (`admission`), and a request that reads none today, a user token
/// with no App and no session header, still reads none. `at` is where the write meets it (`At`).
///
/// edge-refusal-bound: on a lapsed advert that said full, at most ADVERT_STALE old, an Agent write is held: the
/// chain's first in each ADVERT_TTL on this isolate goes as the probe (`graftyard_lookup::take_probe`), and a push that
/// went as one stays one at its later checks; every other is refused on that advert's retry hint. Past ADVERT_STALE
/// it is sent. Each refusal, hold, probe and stale send logs an `edgefull` line.
pub(crate) async fn advert_refusal(
    c: &Ctx<'_>,
    repo: &Repo,
    who: &Principal,
    push: bool,
    at: At,
    budget: std::time::Duration,
) -> Option<u32> {
    let chain = ChainId::Repo(repo.id);
    let now = graftyard_lookup::now_ms();
    let heard = graftyard_lookup::advert(chain, now);
    #[cfg(feature = "test-faults")]
    let heard = c.advert_force.map_or(heard, |wait_ms| {
        graftyard_lookup::Heard::Fresh(graftyard_lookup::Advert { wait_ms, retry_after_s: 1 })
    });
    let says = advert_says(heard, budget);
    if says == AdvertSays::Send {
        return None;
    }
    let reaches = token_reaches(who, c.token_fields.borrow().as_ref(), repo.id, repo.owner);
    let class = if reaches { admission(c, who, &[], push, 0).await.map(|a| a.class) } else { None };
    let line = edgefull::Line { repo: repo.id, push, at, budget };
    match advert_verdict(says, class, reaches) {
        AdvertSays::Send => None,
        AdvertSays::Refuse(a) => {
            // B0-32's split of `full` by shape: an advert refusal is `advert` in `server-timing`, apart from `bound`.
            c.timing.put("advert", a.wait_ms as f64);
            worker::console_log!("{}", line.log(edgefull::What::Refused, a.wait_ms, 0));
            Some(a.retry_after_s.max(1))
        }
        AdvertSays::Hold(a, age) if probe_goes(c.probe.get(), chain, now) => {
            if !c.probe.replace(true) {
                worker::console_log!("{}", line.log(edgefull::What::Probe, a.wait_ms, age));
            }
            None
        }
        AdvertSays::Hold(a, age) => {
            c.timing.put("advert", a.wait_ms as f64);
            worker::console_log!("{}", line.log(edgefull::What::Held, a.wait_ms, age));
            Some(a.retry_after_s.max(1))
        }
        AdvertSays::Stale(age) => {
            worker::console_log!("{}", line.log(edgefull::What::Stale, 0, age));
            None
        }
    }
}

/// The `edgefull` log line (`decisions/DECISIONS.md`, "`edge-refusal`" and "`edge-refusal-bound`", their flips): one
/// per write the advert refused (`refused` on a fresh advert, `held` on a lapsed one), per probe, and per Agent write
/// sent on a full advert past ADVERT_STALE (`stale`), with where it met the advert (`at`), the advert's age, and this
/// isolate's running counts of each. `failOpen` is probes plus stale sends, the writes a lapsed full advert let through.
mod edgefull {
    use std::cell::Cell;

    use graftyard_types::RepoId;

    use super::At;

    #[derive(Clone, Copy)]
    pub(crate) enum What {
        Refused,
        Held,
        Probe,
        Stale,
    }

    thread_local! {
        static ISOLATE: Cell<u64> = const { Cell::new(0) };
        /// Running counts, in `What` order.
        static COUNTS: Cell<[u64; 4]> = const { Cell::new([0; 4]) };
    }

    fn isolate() -> u64 {
        ISOLATE.with(|i| {
            if i.get() == 0 {
                let r = super::rand10();
                i.set(u64::from_le_bytes(r[..8].try_into().expect("8 bytes")) | 1);
            }
            i.get()
        })
    }

    pub(crate) struct Line {
        pub(crate) repo: RepoId,
        pub(crate) push: bool,
        pub(crate) at: At,
        pub(crate) budget: std::time::Duration,
    }

    impl Line {
        /// Counts `what` and renders its line: `wait_ms` is the advert's wait (0 for a stale send, written null),
        /// `age_ms` its age (0 for a fresh one).
        pub(crate) fn log(&self, what: What, wait_ms: u64, age_ms: u64) -> String {
            let n = COUNTS.with(|c| {
                let mut n = c.get();
                n[what as usize] += 1;
                c.set(n);
                n
            });
            let at = match self.at {
                At::Check => "check",
                At::Send => "send",
                At::Stage => "stage",
            };
            let name = ["refused", "held", "probe", "stale"][what as usize];
            serde_json::json!({ "edgefull": name, "class": "agent",
                "push": self.push, "at": at, "repo": self.repo.0.0, "waitMs": (wait_ms > 0).then_some(wait_ms),
                "ageMs": age_ms, "budgetMs": self.budget.as_millis() as u64, "isolate": format!("{:016x}", isolate()),
                "refused": n[0], "held": n[1], "probe": n[2], "stale": n[3], "failOpen": n[2] + n[3] })
            .to_string()
        }
    }
}

/// The install's `Authz(Genesis.account)` fold, read as a `dep` through `fresh`: its `AgentApp` rows. `None` when it
/// cannot be read, which classes no App as an agent; the leader's `decide` reads the chain itself either way.
/// Read once per request: a later call answers the first one's result.
pub(crate) async fn install_authz(c: &Ctx<'_>) -> Option<graftyard_identity::AuthzFold> {
    if let Some(done) = c.install.borrow().as_ref() {
        return done.clone();
    }
    let read = async {
        let account = graftyard_identity::install_account();
        let guess = match rcache::last(&rcache::PLACEMENTS, &account) {
            Some(r) => Some(r),
            None => c.system_record(ChainId::Names(account_shard(account))).await.ok(),
        };
        let read = |r: PlacementRecord| async move { authz_read(c, account, &r).await };
        install_link(guess, placement(c, account), read).await.ok()?
    };
    // Boxed: the link holds two `Authz` reads, and inline in `fetch`'s future it took the edge past wasm's 1 MiB
    // stack on every request, as `beside` did (`tests/edgeboot/run.sh`).
    let got = Box::pin(read).await;
    c.install.replace(Some(got.clone()));
    got
}

/// The install's `graftyard-agent`: the `GraftyardRun` `AgentApp` on its own `Authz` chain whose slug is
/// `graftyard-agent`, the one App whose bot account an issue may be assigned to (F20). `None` when it is not
/// registered or the chain could not be read.
pub(crate) async fn run_agent(c: &Ctx<'_>) -> Option<graftyard_identity::AppId> {
    use graftyard_identity::AuthzRead;
    let install = install_authz(c).await?;
    install.more.more.agent_apps.values().find_map(|r| {
        (r.kind == graftyard_identity::AgentKind::GraftyardRun
            && install.app(r.app).is_some_and(|a| a.slug.as_str() == graftyard_git::task::AGENT_SLUG))
        .then_some(r.app)
    })
}

/// `install_authz`'s two reads: the fold read on the guessed record beside the `Placement` read, kept when the row
/// differs from the guess only in `at` (`kept_guess`), else read again on the row, as `link` does.
async fn install_link<T, E, F1, F2>(
    guess: Option<PlacementRecord>,
    placement: F1,
    read: impl Fn(PlacementRecord) -> F2,
) -> std::result::Result<Option<T>, E>
where
    F1: std::future::Future<Output = std::result::Result<Option<PlacementRecord>, E>>,
    F2: std::future::Future<Output = std::result::Result<Option<T>, E>>,
{
    let fresh = {
        let guess = guess.clone();
        async move { Ok(placement.await?.map(|r| kept_guess(guess.as_ref(), r))) }
    };
    let started = guess.map(|r| (r.clone(), read(r)));
    Ok(link(started, fresh, read).await?.map(|(_, t)| t))
}

/// The install's `Placement` answer as `link` compares it with the guess: the guess itself when the two differ only in
/// `at`, which no read uses, else the answer. The genesis `Names` record a cold isolate guesses is stamped `at` 1 and
/// the install's row a later `at` (5 on graft-m1), so compared whole the guess was dropped on every cold isolate and
/// the fold read waited behind the `Placement` read (B0-34 cw2: `install` median 199 ms past its `authz:1` read;
/// `decisions/DECISIONS.md`, "`cold-write`", cw3).
fn kept_guess(guess: Option<&PlacementRecord>, fresh: PlacementRecord) -> PlacementRecord {
    fn same(a: &PlacementRecord, b: &PlacementRecord) -> bool {
        // Destructured, so a field added to the record is compared or named here before this builds.
        let PlacementRecord { name, sites, home, generation, moving_from, at: _ } = a;
        *name == b.name
            && *sites == b.sites
            && *home == b.home
            && *generation == b.generation
            && match (moving_from, &b.moving_from) {
                (None, None) => true,
                (Some(x), Some(y)) => same(x, y),
                _ => false,
            }
    }
    match guess {
        Some(g) if same(g, &fresh) => g.clone(),
        _ => fresh,
    }
}

/// Whether a push by `who` reads the install's `Authz` anyway: under 32.8 every push asks `agents::agentish`, which
/// reads it, and otherwise `admission` reads it for any caller but a user token with no App and no session header.
fn needs_install(c: &Ctx<'_>, who: &Principal) -> bool {
    let user = matches!(who.actor, Actor::User(_) | Actor::Anonymous);
    c.cfg.enabled.contains(&graftyard_git::repo::REPO_FORMAT_AGENT)
        || !user
        || c.token_app.get().is_some()
        || c.agent_session
}

/// The account that holds `login`, from its `Names` row read through `fresh`.
pub(crate) async fn account_of(c: &Ctx<'_>, login: &str) -> std::result::Result<Option<AccountId>, String> {
    let Ok(l) = Login::parse(login) else { return Ok(None) };
    let read = async {
        let names = ChainId::Names(name_shard(&NameKey::Login(l.key())));
        let row = system_read(c, names, json!({ "login": login })).await?;
        Ok(match serde_json::from_value::<Option<LoginRow>>(row["login"].clone()) {
            Ok(Some(LoginRow::Claimed { account, .. })) => Some(account),
            _ => None,
        })
    };
    rcache::through(&rcache::LOGINS, l.key(), now(), c.read_class.get(), c.leader_reads.get(), &c.timing, read).await
}

/// `owner`'s `Authz` fold, read as a `dep` by read-index on the group's learners and followers
/// (`graftyard_lookup::read_spread`), never on to the leader once one sheds it: on a 500-agent row these reads, about
/// 125/s, queued every repository entry's `Authz` dep read on the leader and set the commit p99
/// (`design/DESIGN.md`, "Follower read"). `Err` names `graftyard_lookup::OVERLOADED` when the members shed it.
pub(crate) async fn authz_read(
    c: &Ctx<'_>,
    owner: AccountId,
    record: &PlacementRecord,
) -> std::result::Result<Option<graftyard_identity::AuthzFold>, String> {
    authz_read_as(c, owner, record, c.read_class.get()).await
}

/// `authz_read`, from `rcache` only when `cached` (a read-class request that has not been refused on it).
pub(crate) async fn authz_read_as(
    c: &Ctx<'_>,
    owner: AccountId,
    record: &PlacementRecord,
    cached: bool,
) -> std::result::Result<Option<graftyard_identity::AuthzFold>, String> {
    let read = async {
        let q = json!({ "q": { "dep": true } });
        let Some((v, _, _)) = authz_get(c, &c.bindings(), ChainId::Authz(owner), record, &q).await? else {
            return Ok(None);
        };
        let bytes = v["value"].as_str().and_then(graftyard_lookup::unhex).ok_or("authz dep: not hex")?;
        graftyard_raft::codec::from_bytes(&bytes).map(Some).map_err(|e| e.to_string())
    };
    rcache::through(&rcache::AUTHZ, owner, now(), cached, c.leader_reads.get(), &c.timing, read).await
}

/// One read of an owner's `Authz` group: on its leader for a REST merge (`Ctx::leader_reads`), else by read-index on
/// its learners and followers. Through them a merge's reads ran p50 451 to 489 ms at 500 agents, against 30 to 47 ms
/// for its rows on the repository's leader (`merge-p99`).
async fn authz_get(
    c: &Ctx<'_>,
    b: &graftyard_lookup::Bindings<'_>,
    chain: ChainId,
    record: &PlacementRecord,
    q: &serde_json::Value,
) -> std::result::Result<Option<(serde_json::Value, graftyard_lookup::Step, graftyard_lookup::Target)>, String> {
    if c.leader_reads.get() {
        graftyard_lookup::call_opt(b, chain, record, "/read", q.to_string().as_bytes()).await
    } else {
        spread_read(c, b, chain, record, q).await
    }
}

/// `graftyard_lookup::read_spread` for this request: a write asks again, backing off, while the members shed the read,
/// for up to `shed_retry_for`, so a short stall does not fail a push or a REST write; any other request answers 503 at
/// once.
pub(crate) async fn spread_read(
    c: &Ctx<'_>,
    b: &graftyard_lookup::Bindings<'_>,
    chain: ChainId,
    record: &PlacementRecord,
    q: &serde_json::Value,
) -> std::result::Result<Option<(serde_json::Value, graftyard_lookup::Step, graftyard_lookup::Target)>, String> {
    // A test install keeps each pass's member tries for `x-graft-spread` (`gate-trace`).
    #[cfg(feature = "test-faults")]
    let trace = Some(&c.timing.spread);
    #[cfg(not(feature = "test-faults"))]
    let trace = None;
    let retry = shed_retry_for(c.write.get(), budget_left(c));
    graftyard_lookup::read_spread_traced(b, chain, record, q, retry, trace).await
}

/// How long `spread_read` asks again for a shed read: not at all for a read-class request; SHED_RETRY_FOR for a write,
/// and no longer than what is left of a bounded push's AGENT_PUSH_BOUND. Past it the write answers 503 with
/// `Retry-After: 1` (`api::read_failed`), or `ng` for an Agent push (`gate_ng`): LOOKUP_FOR's 20 s held B0-33 m51's
/// writes 19 to 24 s each while their retries made the learners' reads ten times the clients'.
fn shed_retry_for(write: bool, left: Option<std::time::Duration>) -> std::time::Duration {
    match (write, left) {
        (false, _) => std::time::Duration::ZERO,
        (true, Some(left)) => left.min(graftyard_lookup::SHED_RETRY_FOR),
        (true, None) => graftyard_lookup::SHED_RETRY_FOR,
    }
}

/// `authz_read`, `None` when it cannot be read.
async fn authz_fold(c: &Ctx<'_>, owner: AccountId, record: &PlacementRecord) -> Option<graftyard_identity::AuthzFold> {
    authz_read(c, owner, record).await.ok().flatten()
}

/// The owner's `Placement` row on `account_shard(owner)`.
async fn placement(c: &Ctx<'_>, owner: AccountId) -> std::result::Result<Option<PlacementRecord>, String> {
    let read = async {
        let v = system_read(c, ChainId::Names(account_shard(owner)), json!({ "placement": owner.0.0 })).await?;
        serde_json::from_value(v["placement"].clone()).map_err(|e| e.to_string())
    };
    rcache::through(&rcache::PLACEMENTS, owner, now(), c.read_class.get(), c.leader_reads.get(), &c.timing, read).await
}

/// A `fresh` read of a system chain's group, found by its record in the `Release` view. A shard with no group holds
/// no rows, so it reads as empty (`null`). A miss under the edge's genesis binding is retried once under the view's
/// record when the view holds another genesis (`Ctx::rebind`).
async fn system_read(
    c: &Ctx<'_>,
    chain: ChainId,
    q: serde_json::Value,
) -> std::result::Result<serde_json::Value, String> {
    let body = json!({ "q": q }).to_string();
    loop {
        let record = c.system_record(chain).await?;
        match graftyard_lookup::call_opt(&c.bindings(), chain, &record, "/read", body.as_bytes()).await {
            Ok(Some((v, _, _))) => return Ok(v["value"].clone()),
            _ if c.rebind().await => continue,
            r => return r.map(|_| serde_json::Value::Null),
        }
    }
}

// ---- responses ----

/// Every response carries `X-GitHub-Request-Id` and `X-GitHub-Enterprise-Version`, from the shared wire layer
/// (`design/SHARED.md`, "Cross-cutting conventions").
fn headers(rid: RequestId) -> Result<Headers> {
    let h = Headers::new();
    h.set("x-github-request-id", &graftyard_wire::request_id_header(rid))?;
    h.set("x-github-enterprise-version", graftyard_wire::ENTERPRISE_VERSION)?;
    Ok(h)
}

fn git_headers(rid: RequestId, content_type: &str) -> Result<Headers> {
    let h = headers(rid)?;
    h.set("content-type", content_type)?;
    h.set("cache-control", "no-cache, max-age=0, must-revalidate")?;
    h.set("expires", "Fri, 01 Jan 1980 00:00:00 GMT")?;
    h.set("pragma", "no-cache")?;
    Ok(h)
}

fn text(rid: RequestId, status: u16, body: &str) -> Result<Response> {
    let h = headers(rid)?;
    h.set("content-type", "text/plain; charset=utf-8")?;
    Ok(Response::ok(body)?.with_status(status).with_headers(h))
}

fn unauthorized(rid: RequestId, body: &str) -> Result<Response> {
    let h = headers(rid)?;
    h.set("www-authenticate", "Basic realm=\"GitHub\"")?;
    h.set("content-type", "text/plain; charset=utf-8")?;
    Ok(Response::ok(body)?.with_status(401).with_headers(h))
}

/// A push no git cache shard could check: 503 with `Retry-After: 2`, before any byte of a git result, so git prints
/// the HTTP error and the push can be sent again. Its pack may already be in the buckets; the sweeper deletes it.
fn shard_unavailable(rid: RequestId, why: &str) -> Result<Response> {
    let h = headers(rid)?;
    h.set("retry-after", "2")?;
    h.set("content-type", "text/plain; charset=utf-8")?;
    Ok(Response::ok(format!("git cache: no shard could check the pack ({why}); retry\n"))?
        .with_status(503)
        .with_headers(h))
}

/// A fetch whose shard did not answer 200, `warming` most often: 503 with `Retry-After: 2`, as `shard_unavailable`
/// answers a push, so a client knows when to ask again. B0-33 erDA answered 215 of these with no `Retry-After`
/// (`tests/am1/results/edge-refusal.md`, "Row B2").
fn shard_read_failed(rid: RequestId, status: u16, why: &str) -> Result<Response> {
    let r = text(rid, 503, &format!("git cache: {status} {why}\n"))?;
    r.headers().set("retry-after", "2")?;
    Ok(r)
}

/// A composed answer whose chain checks did not end within `CHAIN_CHECK_WITHIN`: 503 with `Retry-After: 1`, before
/// any byte of a git result, so the client fails fast and tries again instead of waiting for bytes that may never come
/// (`decisions/DECISIONS.md`, "`chain-hold`").
fn chain_late(rid: RequestId) -> Result<Response> {
    let h = headers(rid)?;
    h.set("retry-after", "1")?;
    h.set("content-type", "text/plain; charset=utf-8")?;
    Ok(Response::ok("git cache: the clone's chain did not answer in time; retry\n")?.with_status(503).with_headers(h))
}

/// `Unsupported` (`design/DESIGN.md`, Shape, the write path): 503 with `Retry-After`, a retryable error, never a
/// refusal, and before any byte of a git result, so git prints the HTTP error and nothing it could read as a ref's fate.
fn unsupported(rid: RequestId, family: u16, version: u16) -> Result<Response> {
    let h = headers(rid)?;
    h.set("retry-after", UNSUPPORTED_RETRY_AFTER)?;
    h.set("content-type", "text/plain; charset=utf-8")?;
    let body = format!("The service is updating (record format {family}.{version}). Retry in a few seconds.\n");
    Ok(Response::ok(body)?.with_status(503).with_headers(h))
}

fn git_result(rid: RequestId, service: Service, body: Vec<u8>) -> Result<Response> {
    Ok(Response::from_bytes(body)?.with_headers(git_headers(rid, &service.result_type())?))
}

// ---- authentication ----

fn b64_decode(s: &str) -> Option<Vec<u8>> {
    graftyard_git::pack::b64::decode(s.trim())
}

/// The token in `Authorization`: Basic with the token as the password (any username), or `token`/`Bearer`.
pub(crate) fn token_of(req: &Request) -> Result<Option<String>> {
    let Some(a) = req.headers().get("authorization")? else { return Ok(None) };
    let (scheme, rest) = a.split_once(' ').unwrap_or((&a, ""));
    Ok(match scheme.to_ascii_lowercase().as_str() {
        "basic" => b64_decode(rest)
            .and_then(|b| String::from_utf8(b).ok())
            .and_then(|s| s.split_once(':').map(|(_, p)| p.to_owned())),
        "token" | "bearer" => Some(rest.trim().to_owned()),
        _ => None,
    })
}

#[derive(Clone, Deserialize)]
struct TokenRow {
    account: i64,
    revoked: bool,
    /// The row's optional fields; a user-to-server token's `app` is its App, for the admission class (AM1-WP2).
    #[serde(default)]
    fields: Option<graftyard_identity::TokenFields>,
    #[serde(default)]
    kind: Option<graftyard_identity::TokenKind>,
    #[serde(default)]
    expires: Option<Timestamp>,
}

/// The caller, or `None` for no credential or a refused one: a stored token's row read through `fresh` on its
/// `Tokens` shard, never a cache, or a `ghs_` token verified offline (`ident::stateless`). The leader re-reads the
/// row, or the `jti` row, at `decide` either way.
async fn authenticate(c: &Ctx<'_>, req: &Request) -> Result<Option<Principal>> {
    if c.ssh_front {
        return ssh::principal(c, req).await;
    }
    if graftyard_lookup::lifeboat() {
        // Only a `lifeboat:read` token is a caller on the lifeboat: no session, no `ghs_`, no main-host token.
        return match token_of(req)? {
            Some(t) => lifeboat::principal(c, &t).await,
            None => Ok(None),
        };
    }
    let Some(t) = token_of(req)? else { return ui::session(c, req).await };
    if t.starts_with("ghs_") {
        return ident::stateless(c, &t).await;
    }
    let Ok(tok) = OpaqueToken::parse(&t) else { return Ok(None) };
    let hash = tok.hash();
    let at = now();
    // A read-class request may use a usable row up to `READ_CACHE` old (`rcache`); a refused one is read again.
    let read = async {
        let chain = ChainId::Tokens(tok.shard());
        let v = system_read(c, chain, json!({ "token": Digest(hash).hex() })).await.map_err(Error::RustError)?;
        Ok::<_, Error>(serde_json::from_value::<Option<TokenRow>>(v["token"].clone())?)
    };
    let (read_class, decided, alone) = (c.read_class.get(), c.decided.get(), c.leader_reads.get());
    let row = rcache::through_bound(&rcache::TOKENS, hash, at, read_class, decided, alone, &c.timing, read).await?;
    // A `ghr_` only refreshes (`usertok`), and a lapsed token is no caller: `decide` sees no clock. Checked on every
    // use, so a cached row lapses at its `expires`.
    let row = row.filter(|r| !r.revoked && usertok::usable(r.kind, r.expires, now()));
    c.token_app.set(row.as_ref().and_then(|r| r.fields.as_ref()?.app));
    c.token_fields.replace(row.as_ref().and_then(|r| r.fields.clone()));
    Ok(row.map(|r| Principal {
        actor: Actor::User(UserId(AccountId(DbId(r.account)))),
        scopes: ScopeSet(1),
        credential: Credential::Stored(hash),
        sso_orgs: smallvec::SmallVec::new(),
    }))
}

// ---- the group and the shard ----

/// The network's home shard on the owner record's home site. A network id is its root repository's id; every M1
/// repository is a root.
fn shard(c: &Ctx<'_>, repo: &Repo) -> Result<Stub> {
    #[cfg(feature = "test-faults")]
    return shard_at(c, repo, c.shard);
    #[cfg(not(feature = "test-faults"))]
    shard_at(c, repo, None)
}

/// Site `at`'s shard of the network, the home's when `None`. Only test installs pick another site (BF-18).
fn shard_at(c: &Ctx<'_>, repo: &Repo, at: Option<SiteId>) -> Result<Stub> {
    let site = at.unwrap_or(repo.record.home);
    let region = c.sites.get(site).ok_or("unknown home site")?.region;
    c.env
        .durable_object(&format!("SHARD_{}", site.0))?
        .get_by_name_with_location_hint(&graftyard_git::first_shard_name(repo.network(), site), region.hint())
}

/// A repository chain's group, created on first use from the owner's record: its members join with the owner, which
/// each checks against the repository's `RepoOwner` row before it stores anything.
async fn ensure_group(c: &Ctx<'_>, repo: &Repo) -> std::result::Result<(), String> {
    graftyard_lookup::create(&c.bindings(), ChainId::Repo(repo.id), &repo.record, &json!({ "owner": repo.owner })).await
}

/// One page of the repository's refs whose names start with `prefix`, after `after` (`RefsPage::next`), from its
/// leader through `fresh` (`graftyard_git::group::refs_read`). No read answers every ref at once, so neither the
/// leader nor the edge holds a whole ref set of a large repository (`repo:65536`, 571,872 refs).
async fn refs_page(
    c: &Ctx<'_>,
    repo: &Repo,
    prefix: &[u8],
    after: Option<&str>,
) -> Result<std::result::Result<(u64, RefsPage), CallErrorWire>> {
    let chain = ChainId::Repo(repo.id);
    let q = graftyard_git::group::refs_query(prefix, after, graftyard_git::repo::REFS_PAGE_MAX);
    let mut tried_create = false;
    loop {
        match graftyard_lookup::call(&c.bindings(), chain, &repo.record, "/read", &q).await {
            Ok((v, _)) => {
                return Ok(match serde_json::from_value::<RefsReply>(v)? {
                    RefsReply::Page { value, index } => Ok((index, value)),
                    RefsReply::Error { error } => Err(error),
                });
            }
            Err(_) if !tried_create => {
                tried_create = true;
                ensure_group(c, repo).await.map_err(Error::RustError)?;
            }
            Err(e) => return Err(Error::RustError(e)),
        }
    }
}

/// Every ref whose name starts with `prefix`, read a page at a time, with the last page's index. For a ref set the
/// caller needs whole and that stays small (`refs/tags/` for the clone cache key, the test driver's routes).
async fn read_refs(
    c: &Ctx<'_>,
    repo: &Repo,
    prefix: &[u8],
) -> Result<std::result::Result<(u64, Vec<(String, String)>), CallErrorWire>> {
    let (mut out, mut after, mut index) = (Vec::new(), None, 0);
    loop {
        let (i, page) = match refs_page(c, repo, prefix, after.as_deref()).await? {
            Ok(p) => p,
            Err(e) => return Ok(Err(e)),
        };
        index = index.max(i);
        out.extend(page.refs);
        match page.next {
            Some(n) => after = Some(n),
            None => return Ok(Ok((index, out))),
        }
    }
}

/// receive-pack's advertisement for every ref, streamed a page of refs at a time as the leader answers them
/// (`graftyard_git::smart::receive_pack_advertisement` in pieces), so the edge never holds the whole ref set. The
/// first page is read before the response starts, so a group with no leader still answers 503.
async fn receive_pack_stream(
    c: &Ctx<'_>,
    repo: &Repo,
    caps: String,
) -> Result<std::result::Result<Response, Response>> {
    use graftyard_git::smart::{receive_pack_head, receive_pack_lines, receive_pack_tail};
    let first = match refs_page(c, repo, b"", None).await? {
        Ok((_, p)) => p,
        Err(CallErrorWire::Unsupported(u)) => return Ok(Err(unsupported(c.rid, u.family, u.version)?)),
        Err(_) => return Ok(Err(text(c.rid, 503, "the repository's group has no leader; retry\n")?)),
    };
    let parse = |refs: Vec<(String, String)>| -> Vec<(RefName, ObjectId)> {
        refs.iter()
            .filter_map(|(n, o)| Some((RefName::parse(n.as_bytes()).ok()?, ObjectId::parse_hex(o.as_bytes())?)))
            .collect()
    };
    if first.next.is_none() {
        let refs = parse(first.refs);
        return Ok(Ok(Response::from_bytes(receive_pack_advertisement(&refs, &caps))?));
    }
    let (env, sites, record, id) = (c.env.clone(), c.sites.clone(), repo.record.clone(), repo.id);
    let state = Some((parse(first.refs), first.next, true));
    let pages = futures_util::stream::unfold(state, move |st| {
        let (env, sites, record, caps) = (env.clone(), sites.clone(), record.clone(), caps.clone());
        async move {
            let (refs, next, first) = st?;
            let mut out = if first { receive_pack_head() } else { Vec::new() };
            out.extend(receive_pack_lines(&refs, &caps, first));
            let Some(after) = next else {
                out.extend(receive_pack_tail());
                return Some((Ok::<Vec<u8>, Error>(out), None));
            };
            let b = graftyard_lookup::Bindings { env: &env, sites: &sites };
            let q = graftyard_git::group::refs_query(b"", Some(&after), graftyard_git::repo::REFS_PAGE_MAX);
            let next = match graftyard_lookup::call(&b, ChainId::Repo(id), &record, "/read", &q).await {
                Ok((v, _)) => match serde_json::from_value::<RefsReply>(v) {
                    Ok(RefsReply::Page { value, .. }) => Some((parse(value.refs), value.next, false)),
                    // The stream ends without its flush, so the client sees a cut advertisement, not a short one.
                    _ => None,
                },
                Err(_) => None,
            };
            Some((Ok(out), next))
        }
    });
    Ok(Ok(Response::from_stream(pages)?))
}

/// The edge's `commit` (`graftyard_lookup::commit_hedged`): under one request id, to the leader only, a missed
/// receipt hedged by `suspect`. A reply that does not decode as `graftyard_git::group`'s shapes is answered as
/// `Unsupported` (503), never as a lost reply: a fallback that replays would turn a newer member's answer into
/// `fetch first` (`tests/m1/results/m1-wp6.md`, t5541 #20).
async fn commit(
    c: &Ctx<'_>,
    repo: &Repo,
    body: &serde_json::Value,
    first: Option<&serde_json::Value>,
    trace: Option<&std::cell::RefCell<Vec<serde_json::Value>>>,
    lookup: Option<&[u8]>,
) -> Result<std::result::Result<PushOutcome, CommitFail>> {
    // A group whose leader this isolate holds exists: no create probe first. With none held (a first commit, or after
    // `NotLeader` or a reply from an older term), the probe runs as before (`graftyard_lookup::knows_leader`).
    if !graftyard_lookup::knows_leader(ChainId::Repo(repo.id)) {
        let since = js_sys::Date::now();
        if let Err(e) = ensure_group(c, repo).await {
            return commit_result(Err(Err(e)));
        }
        // B0-32's count of commits that probed (`probe` in `server-timing`; a commit without one skipped it).
        c.timing.add("probe", since);
    }
    let bytes = body.to_string().into_bytes();
    let first = first.map(|f| f.to_string().into_bytes());
    // B0-32's receipt-rule counts on a test install: each `/commit` send, missed receipt (`busy` when the send stayed
    // unreported), `/outcome_by_request` ask while a send was open, and `suspect`, one `server-timing` entry each.
    #[cfg(feature = "test-faults")]
    let own = std::cell::RefCell::new(Vec::new());
    #[cfg(feature = "test-faults")]
    let trace = trace.or(Some(&own));
    let r = graftyard_lookup::commit_hedged(
        &c.bindings(),
        ChainId::Repo(repo.id),
        &repo.record,
        &bytes,
        c.colo,
        60_000,
        graftyard_lookup::Hooks {
            first: first.as_deref(),
            trace,
            lookup,
            #[cfg(feature = "test-faults")]
            lean: c.lean,
        },
    )
    .await;
    #[cfg(feature = "test-faults")]
    for n in trace.into_iter().flat_map(|t| t.borrow().clone()) {
        let name = match () {
            _ if n.get("send").is_some() => "send",
            _ if n.get("ask").is_some() => "ask",
            _ if n.get("suspect").is_some() => "suspect",
            _ if n.get("missed").is_some() && n.get("busy").is_some() => "busy",
            _ if n.get("missed").is_some() => "missed",
            _ => continue,
        };
        c.timing.put(name, 0.0);
    }
    commit_result(r)
}

/// Why a commit gave no `PushOutcome`: the member's error (`CallErrorWire`), or no outcome came in time
/// (`commit_hedged`'s `Err(Err(_))`, or the group's create probe failed). Each is a retryable 503, never a 500: B0-33
/// m59r33's four edge 500s were receive-pack commits that ended `no outcome: …: missed` under leader churn.
#[derive(Debug, serde::Serialize)]
#[serde(untagged)]
pub(crate) enum CommitFail {
    Wire(CallErrorWire),
    NoOutcome(String),
}

/// `commit_hedged`'s answer as `commit` returns it.
fn commit_result(
    r: std::result::Result<serde_json::Value, std::result::Result<serde_json::Value, String>>,
) -> Result<std::result::Result<PushOutcome, CommitFail>> {
    Ok(match r {
        Ok(done) => serde_json::from_value::<PushOutcome>(done).map_err(|_| CommitFail::Wire(unreadable_reply())),
        Err(Ok(error)) => {
            Err(CommitFail::Wire(serde_json::from_value::<CallErrorWire>(error).unwrap_or_else(|_| unreadable_reply())))
        }
        Err(Err(e)) => Err(CommitFail::NoOutcome(e)),
    })
}

/// A push whose commit gave no outcome in time: 503 with `Retry-After`, before any byte of a git result, as a group
/// with no leader is, so git prints the HTTP error and the push can be sent again.
fn no_outcome(rid: RequestId, why: &str) -> Result<Response> {
    let h = headers(rid)?;
    h.set("retry-after", NO_OUTCOME_RETRY_AFTER)?;
    h.set("content-type", "text/plain; charset=utf-8")?;
    Ok(Response::ok(format!("the push's commit gave no outcome ({why}); retry\n"))?.with_status(503).with_headers(h))
}

/// The body a push's commit asks `/outcome_by_request` with while its send is open (`graftyard_lookup::Hooks`): the
/// same request id and the push without its pack. The leader's replay of a push reads only its ref names
/// (`Push::replay`) and the `request` row holds the events, not the command, so the answer is the same, without the up
/// to INLINE_MAX bytes of an inline pack.
fn lookup_body(rid: RequestId, push: &Push) -> Vec<u8> {
    let cmd = Push { pack: None, ..push.clone() };
    serde_json::to_vec(&CommitReq { rid: rid.to_string(), cmd }).unwrap_or_default()
}

fn unreadable_reply() -> CallErrorWire {
    CallErrorWire::Unsupported(graftyard_git::group::UnsupportedWire { family: 0, version: 0 })
}

// ---- receive-pack ----

/// The cap refusal (`smart::cap_refusal`) against the body's first `smart::CAP_HEAD` bytes, inflated when the body
/// is gzip, with the commands they name or, when they name none, refusing every ref git reports.
fn over_cap(c: &Ctx<'_>, gz: bool, head: &[u8], path: &str) -> Result<Response> {
    let head = if gz { gunzip(head, c.cfg.cap).unwrap_or_else(|_| head.to_vec()) } else { head.to_vec() };
    let rp = parse_receive_pack(&head).unwrap_or(ReceivePack {
        updates: Vec::new(),
        caps: PushCaps { report_status: true, side_band_64k: true, ..PushCaps::default() },
        options: Vec::new(),
        pack_at: 0,
    });
    let r = cap_refusal(c.cfg.plan, c.cfg.ssh_option, c.cfg.cap, &c.cfg.host, path);
    git_result(c.rid, Service::ReceivePack, r.render(&rp))
}

/// Why a streamed body's rest stopped short: it passed the cap, or the request broke off.
enum Cut {
    OverCap,
    Broke(Error),
}

/// The rest of a body that streams (`smart::Read::Stream`): the pack bytes already read, then the request's chunks,
/// each counted against the cap.
struct Tail {
    pack: Vec<u8>,
    cap_head: Vec<u8>,
    rest: std::pin::Pin<Box<dyn futures_util::Stream<Item = std::result::Result<Vec<u8>, Cut>>>>,
}

impl Tail {
    fn new(s: graftyard_git::smart::Streaming, body: ByteStream, cap: u64) -> Tail {
        Tail { pack: s.pack, cap_head: s.cap_head, rest: Box::pin(capped(body, s.read, cap)) }
    }
}

/// `body`'s chunks, counted on from the `read` bytes before them, until the count passes `cap`.
fn capped(
    body: impl futures_util::Stream<Item = Result<Vec<u8>>>,
    read: u64,
    cap: u64,
) -> impl futures_util::Stream<Item = std::result::Result<Vec<u8>, Cut>> {
    let mut n = read;
    body.map(move |c| {
        let c = c.map_err(Cut::Broke)?;
        n += c.len() as u64;
        if n > cap { Err(Cut::OverCap) } else { Ok(c) }
    })
}

/// `r`, once a streamed body's rest is read to its end: an answer the edge gives before it reads the pack still
/// waits for the whole body, so a body over the cap is answered the cap refusal, as when the edge read every body
/// whole before it answered.
async fn after_body(c: &Ctx<'_>, tail: &mut Option<Tail>, path: &str, r: Result<Response>) -> Result<Response> {
    match drain(c, tail, path).await? {
        Some(cap) => Ok(cap),
        None => r,
    }
}

/// Reads a streamed body's rest to its end, keeping none of it: the cap refusal when it passed the cap.
async fn drain(c: &Ctx<'_>, tail: &mut Option<Tail>, path: &str) -> Result<Option<Response>> {
    let Some(t) = tail.take() else { return Ok(None) };
    let mut rest = t.rest;
    while let Some(x) = rest.next().await {
        match x {
            Ok(_) => {}
            Err(Cut::OverCap) => return over_cap(c, false, &t.cap_head, path).map(Some),
            Err(Cut::Broke(e)) => return Err(e),
        }
    }
    Ok(None)
}

/// A Worker isolate's memory, 128 MB [doc: Workers limits]: a body that inflates past it cannot be served.
const ISOLATE_BYTES: u64 = 128 << 20;

/// git sends a gzip-compressed receive-pack body only for small requests (`http.c`, below `postBuffer`); the edge
/// decompresses it after the cap check, which counts the bytes on the wire. The inflated body is held to the same
/// `cap` as one sent uncompressed, and never past the isolate's memory, so a small gzip bomb fails as a corrupt gzip
/// does instead of exhausting the isolate.
fn gunzip(b: &[u8], cap: u64) -> Result<Vec<u8>> {
    use std::io::Read;
    let max = cap.min(ISOLATE_BYTES);
    let mut out = Vec::new();
    flate2::read::GzDecoder::new(b)
        .take(max + 1)
        .read_to_end(&mut out)
        .map_err(|e| Error::RustError(format!("gzip: {e}")))?;
    if out.len() as u64 > max {
        return Err(Error::RustError(format!("gzip: the body inflates past {max} bytes")));
    }
    Ok(out)
}

struct Ctx<'a> {
    env: &'a Env,
    /// The request's context, for work that outlives the reply (`api::ask_merges`).
    ctx: &'a Context,
    cfg: &'a Config,
    rid: RequestId,
    /// The install's sites, from `GRAFTYARD_SITES`.
    sites: &'a SiteTable,
    /// This script's trust anchor, which the `Release` view is accepted against.
    anchor: Option<Anchor>,
    /// The client colo, packed as the `Colo` a `suspect` report carries.
    colo: u32,
    /// BF-12 on pushes: the `x-graft-kill` and `x-graft-down-ms` request headers.
    #[cfg(feature = "test-faults")]
    kill: Option<String>,
    #[cfg(feature = "test-faults")]
    down_ms: u64,
    /// BF-18: `x-graft-shard: N` with the bed token sends this request's git cache calls to site N's shard of the
    /// network, a second shard behind its own `IngestCursor`.
    #[cfg(feature = "test-faults")]
    shard: Option<SiteId>,
    /// B0-31's placement-failing half: `x-graft-place-fail` with the bed token makes this request see placement
    /// failing, as a red bucket probe would (`design/DESIGN.md`, Failure modes, pack buckets): no pack is placed, and
    /// the leader is told `placement_ok: false`.
    #[cfg(feature = "test-faults")]
    place_fail: bool,
    /// F91's A/B (`ls_refs_at_edge`): false when the request carries `x-graft-ls-refs: shard` with the bed token, so a
    /// v2 `ls-refs` goes to the shard as before `b034-reads`. True for every other request.
    #[cfg(feature = "test-faults")]
    ls_refs_edge: bool,
    /// The leader-sync A/B's arm B (`lean_asked`): this request's repository commits are lean.
    #[cfg(feature = "test-faults")]
    lean: bool,
    /// stale-log: the client's push id (`x-graft-client-push`, `client_push_id`) and the leader's answer to the push,
    /// for its `push_log` line, so a driver's per-push log joins the edge's answer to each push it sent.
    #[cfg(feature = "test-faults")]
    client_push: Option<String>,
    #[cfg(feature = "test-faults")]
    push_end: std::cell::RefCell<Option<serde_json::Value>>,
    /// `colo-gate`'s flip arm (`x-graft-colo: off` with the bed token): this request's read gate and chain checks keep
    /// the isolate's caches alone, as before the share, so one row measures both arms.
    #[cfg(feature = "test-faults")]
    colo_off: bool,
    /// `tests/edgeboot/run.sh` (`x-graft-link: <repo id>,<owner id>` with the bed token): this request's link guess, as
    /// if the data center's entry named that repository on the genesis `names` record, so a bed with no Cache API entry
    /// and no members still takes `beside`'s cold-guess path.
    #[cfg(feature = "test-faults")]
    link_guess: Option<(i64, i64)>,
    /// B0-33 row B's pre-load probe (`x-graft-advert: <ms>` with the bed token): this request reads a fresh advert of
    /// that wait, as if its group's leader had just advertised it, so a test install shows `advert_refusal` and its
    /// `advert` mark before any agent loads (`tests/am1/rest/a1.mjs`, `--advert-probe`).
    #[cfg(feature = "test-faults")]
    advert_force: Option<u64>,
    /// `authenticate`'s token's App, when it is a user-to-server token.
    token_app: std::cell::Cell<Option<graftyard_identity::AppId>>,
    /// `authenticate`'s token's optional fields: a `github_pat_`'s scope, which REST reads check (`usertok::reaches`).
    token_fields: std::cell::RefCell<Option<graftyard_identity::TokenFields>>,
    /// The request sent `X-Graftyard-Agent-Session`, which classes a user token's commands Agent and grants nothing.
    agent_session: bool,
    /// A read-class request (a REST `GET` or a GraphQL query on `api`, a git fetch, `git_class`): its token, name,
    /// placement, `Authz`, and login reads may use `rcache` entries up to `READ_CACHE` old. False for every other
    /// request, which reads them as before.
    read_class: std::cell::Cell<bool>,
    /// A write whose command's `decide` reads the caller's credential shard and answers a refusal with the front's
    /// 401 (`write-identity`; `git_bound`, `api::rest_bound`, `api::graphql_bound`, `agents::serve`): its token row
    /// and a `ghs_` `jti` row may come from `rcache` within `READ_CACHE`, since the leader's read binds. False for
    /// every other request.
    decided: std::cell::Cell<bool>,
    /// A write (a push, a REST call other than `GET`, a GraphQL mutation): a read its members shed is asked again,
    /// backing off, within SHED_RETRY_FOR, before the write fails 503 (`spread_read`).
    write: std::cell::Cell<bool>,
    /// A REST merge (`api::rest_merge`): its owner's `Authz` reads, the name lookup and the fold, go to the `Authz`
    /// group's leader by `call_opt`, as its rows go to the repository's (`merge-p99`). False for every other request.
    leader_reads: std::cell::Cell<bool>,
    /// This request's stages, for `server-timing` and the sampled log (`api::Timing`).
    timing: api::Timing,
    /// A push the edge may class Agent (`bounded`): when AGENT_PUSH_BOUND from its arrival ends, in ms since the epoch.
    /// Its shed reads are retried only until then, and an Agent push past it before its commit is sent answers `ng`.
    bound: std::cell::Cell<Option<f64>>,
    /// edge-refusal-bound: this request went as its chain's probe on a lapsed full advert (`advert_refusal`), so its
    /// later checks, a push's at its sends, let it through as the same probe.
    probe: std::cell::Cell<bool>,
    /// A push's repository rows from the read gate's one read: the access rows, and the agent rows when the member
    /// answered them too (`api::push_rows`), which `agents::push_context` reuses instead of reading them again.
    push_rows: std::cell::RefCell<Option<(gate::Rows, bool)>>,
    /// The install's `Authz` (`install_authz`), read once per request: a push starts it with its first reads, and
    /// `push_context` and `admission` both take it from here.
    install: std::cell::RefCell<Option<Option<graftyard_identity::AuthzFold>>>,
    /// The request came from a site's `SshOutbound` over its `EDGE` binding, whose `props` are `{"ssh": true}`
    /// (`ssh`): its caller is the SSH key in `x-graftyard-ssh-key`, and no other credential counts. A public request
    /// has no props, so no client can set this.
    ssh_front: bool,
}

impl<'a> Ctx<'a> {
    /// A context for work after a request's reply (`reread_ahead`): the request's bindings, config, sites, anchor,
    /// colo, and token reach, and none of its per-request state or test switches. Not read-class, so its owner's
    /// `Authz` is read fresh and the decision is stamped now: on a cached fold the re-read would carry the fold's old
    /// stamp and refresh nothing.
    #[allow(clippy::too_many_arguments)]
    fn background(
        env: &'a Env,
        ctx: &'a Context,
        cfg: &'a Config,
        sites: &'a SiteTable,
        anchor: Option<Anchor>,
        colo: u32,
        token_fields: Option<graftyard_identity::TokenFields>,
        ssh_front: bool,
    ) -> Ctx<'a> {
        Ctx {
            env,
            ctx,
            cfg,
            rid: RequestId(graftyard_types::Ulid(u128::from_be_bytes({
                let mut b = [0u8; 16];
                b[6..].copy_from_slice(&rand10());
                b
            }))),
            sites,
            anchor,
            colo,
            #[cfg(feature = "test-faults")]
            kill: None,
            #[cfg(feature = "test-faults")]
            down_ms: 0,
            #[cfg(feature = "test-faults")]
            shard: None,
            #[cfg(feature = "test-faults")]
            place_fail: false,
            #[cfg(feature = "test-faults")]
            ls_refs_edge: true,
            #[cfg(feature = "test-faults")]
            lean: false,
            #[cfg(feature = "test-faults")]
            client_push: None,
            #[cfg(feature = "test-faults")]
            push_end: std::cell::RefCell::new(None),
            #[cfg(feature = "test-faults")]
            colo_off: false,
            #[cfg(feature = "test-faults")]
            link_guess: None,
            #[cfg(feature = "test-faults")]
            advert_force: None,
            token_app: std::cell::Cell::new(None),
            token_fields: std::cell::RefCell::new(token_fields),
            agent_session: false,
            read_class: std::cell::Cell::new(false),
            decided: std::cell::Cell::new(false),
            write: std::cell::Cell::new(false),
            leader_reads: std::cell::Cell::new(false),
            timing: api::Timing::default(),
            bound: std::cell::Cell::new(None),
            probe: std::cell::Cell::new(false),
            push_rows: std::cell::RefCell::new(None),
            install: std::cell::RefCell::new(None),
            ssh_front,
        }
    }
}

impl Ctx<'_> {
    fn bindings(&self) -> graftyard_lookup::Bindings<'_> {
        graftyard_lookup::Bindings { env: self.env, sites: self.sites }
    }

    /// A call to `graft-ops` over the `OPS` `fetch()` binding, with a version override while the cached view's
    /// `OpsPointer` pins it (`design/DESIGN.md`, Shape, `Pointers`; journey 7). `OPS_SCRIPT` names the script the
    /// override header keys on. The edge reads the view only here, in the release endpoint, and in `ssh::meta`, so only
    /// those requests refresh it (`graftyard_lookup::view::ensure`).
    ///
    /// Each `/mint` answer counts in this isolate's `MintWatch` by the version that answered (`OPS_VERSION_HEADER`):
    /// once a running `OpsRoll`'s N+1 fails past `MINT_ERROR_RATIO`, the isolate submits the roll's pre-signed
    /// `OpsPointer` to N on the `Release` group, after the reply, with no API call (DECISIONS "`mint-ratio`").
    async fn ops(&self, url: &str, mut init: RequestInit) -> Result<Response> {
        if let Some(a) = &self.anchor {
            graftyard_lookup::view::ensure(&self.bindings(), a, Some(self.ctx)).await;
        }
        if let (Some(v), Ok(name)) =
            (graftyard_lookup::view::pin(graftyard_release::Script::Ops), self.env.var("OPS_SCRIPT"))
        {
            let h = Headers::new();
            h.set(
                "cloudflare-workers-version-overrides",
                &graftyard_release::view::version_override(&name.to_string(), v),
            )?;
            init.with_headers(h);
        }
        let res = self.env.service("OPS")?.fetch(url, Some(init)).await;
        if url.ends_with("/mint") {
            let ran = res.as_ref().ok().and_then(|r| r.headers().get(OPS_VERSION_HEADER).ok().flatten());
            let status = res.as_ref().ok().map(Response::status_code);
            let undo = graftyard_lookup::view::with(|v| {
                MINTS.with(|m| m.borrow_mut().record(&v.fold, ran.as_deref(), status, now().0))
            });
            if let Some(e) = undo {
                self.submit_mint_undo(e);
            }
        }
        res
    }

    /// Commits the pre-signed `OpsPointer` an isolate's mint errors called for, after the reply. A refusal or no
    /// outcome is logged; the isolate submits it again after `MINT_RESEND_MS` while the errors last.
    fn submit_mint_undo(&self, e: graftyard_release::SignedEntry) {
        let Some(record) = graftyard_lookup::view::release_record() else {
            console_error!("PAGE mint errors past MINT_ERROR_RATIO, but this isolate has no Release record");
            return;
        };
        console_error!("graft-ops mint errors past MINT_ERROR_RATIO: submitting the pre-signed OpsPointer");
        let (env, sites, colo) = (self.env.clone(), self.sites.clone(), self.colo);
        self.ctx.wait_until(async move {
            let b = graftyard_lookup::Bindings { env: &env, sites: &sites };
            let body =
                json!({ "rid": graftyard_lookup::request_id().to_string(), "cmd": graftyard_lookup::hex(&e.encode()) })
                    .to_string();
            let hooks = graftyard_lookup::Hooks::default();
            match graftyard_lookup::commit_hedged(&b, ChainId::Release, &record, body.as_bytes(), colo, 30_000, hooks)
                .await
            {
                Ok(d) if d.get("ok").is_some() => console_log!("mint undo: OpsPointer committed"),
                Ok(d) => console_error!("mint undo: {d}"),
                Err(Ok(d)) => console_error!("mint undo refused: {d}"),
                Err(Err(e)) => console_error!("mint undo: {e}"),
            }
        });
    }

    /// A system chain's record: from the `Release` view, accepted against this script's anchor. With `test-faults`
    /// only, `GRAFTYARD_GENESIS` stands in for the view on a test install with no `Release` group.
    async fn system_record(&self, chain: ChainId) -> std::result::Result<PlacementRecord, String> {
        #[cfg(feature = "test-faults")]
        if let Ok(v) = self.env.var("GRAFTYARD_GENESIS") {
            let g: serde_json::Value = serde_json::from_str(&v.to_string()).map_err(|e| e.to_string())?;
            let k = match chain {
                ChainId::Names(_) => "names",
                ChainId::Tokens(_) => "tokens",
                ChainId::Ids => "ids",
                _ => "release",
            };
            return serde_json::from_value(g[k].clone()).map_err(|e| e.to_string());
        }
        let anchor = self.anchor.as_ref().ok_or("this edge has no trust anchor")?;
        graftyard_lookup::system_record(&self.bindings(), anchor, chain).await
    }

    /// After a system chain call found no group or failed, and before creating a system group: whether the isolate's
    /// genesis came from the `GRAFTYARD_GENESIS_SIGNED` binding and the `Release` view now holds another, so the
    /// caller asks `system_record` again and retries under the view's record (`graftyard_lookup::check_bound`). The
    /// view is read at most once per isolate when it matches.
    async fn rebind(&self) -> bool {
        match &self.anchor {
            Some(a) => graftyard_lookup::check_bound(&self.bindings(), a).await,
            None => false,
        }
    }
}

/// `who` is the caller the read gate authenticated.
async fn receive_pack(
    c: &Ctx<'_>,
    mut req: Request,
    repo: &Repo,
    path: &str,
    who: Option<Principal>,
    early: Option<Refusal>,
) -> Result<Response> {
    let Some(actor) = who else { return unauthorized(c.rid, "No anonymous write access.") };
    let gz = req.headers().get("content-encoding")?.is_some_and(|e| e == "gzip");
    // The body is held while it may be one the edge buffers: until its commands are read and more than INLINE_MAX of
    // pack follows them (`smart::BodyReader`). The rest of a longer body streams (`tail`). A gzip body, which git
    // sends only below `postBuffer`, is held whole, inflated once it ended.
    let mut s = req.stream()?;
    let mut head = if gz { BodyReader::whole(c.cfg.cap) } else { BodyReader::new(c.cfg.cap) };
    let streams = loop {
        let Some(chunk) = s.next().await else { break false };
        match head.push(&chunk?) {
            smart::Read::More => {}
            smart::Read::OverCap => return over_cap(c, gz, head.cap_head(), path),
            smart::Read::Stream => break true,
        }
    };
    let (rp, body, mut tail) = if streams {
        let (rp, st) = head.streaming();
        (rp, Vec::new(), Some(Tail::new(st, s, c.cfg.cap)))
    } else {
        let body = match head.end() {
            smart::Ended::Whole(b) => b,
            smart::Ended::Bad(e) => return text(c.rid, 400, &format!("{e}\n")),
        };
        let body = if gz { gunzip(&body, c.cfg.cap)? } else { body };
        // git's `probe_rpc` (remote-curl.c): before streaming a request over `http.postBuffer` it posts a bare flush,
        // after authentication, and streams the real body only on a 200. Nothing reaches the group.
        if body == b"0000" {
            return git_result(c.rid, Service::ReceivePack, Vec::new());
        }
        match parse_receive_pack(&body) {
            Ok(rp) => (rp, body, None),
            Err(e) => return text(c.rid, 400, &format!("{e}\n")),
        }
    };
    let atomicity = if rp.caps.atomic { Atomicity::AllOrNothing } else { Atomicity::PerRef };
    let refuse = |r: Refusal| git_result(c.rid, Service::ReceivePack, r.render(&rp));
    // A refusal decided before the body was read (`gate_bound_refusal`), rendered per ref once its commands are.
    if let Some(r) = early {
        return after_body(c, &mut tail, path, refuse(r)).await;
    }
    // edge-refusal: a bounded push whose leader advertised a wait past what is left of its bound is refused here, before
    // context, staging (`first`), validate, or commit; nothing reaches the group.
    if let Some(left) = budget_left(c)
        && let Some(s) = advert_refusal(c, repo, &actor, true, At::Check, left).await
    {
        return after_body(c, &mut tail, path, refuse(Refusal::agent_full(s))).await;
    }
    if atomicity == Atomicity::AllOrNothing && rp.updates.len() > ATOMIC_REFS_MAX {
        return after_body(c, &mut tail, path, refuse(Refusal::too_many_atomic(rp.updates.len()))).await;
    }
    let mut push = Push {
        updates: rp.updates.clone(),
        atomicity,
        pack: None,
        actor,
        options: rp.options.iter().map(|s| s.as_str().into()).collect(),
        request: c.rid,
        agent: None,
        provenance: Vec::new(),
        protected: Vec::new(),
        changed: Vec::new(),
        leases: false,
        test_format: false,
    };
    // A push under a `ghs_` token is format 32.7, sent only once `FormatsEnable` enabled it (M3-WP4); one whose token
    // carries a `task` claim is 32.12 (AM1-WP3), on the same terms; one over SSH is 32.26 (M6-WP2).
    let late = [
        graftyard_git::repo::REPO_FORMAT_STATELESS,
        graftyard_git::repo::REPO_FORMAT_TASKS,
        graftyard_git::repo::REPO_FORMAT_SSH,
    ];
    let f = graftyard_raft::Command::format(&push);
    if late.contains(&f) && !c.cfg.enabled.contains(&f) {
        return after_body(c, &mut tail, path, unsupported(c.rid, f.family, f.version)).await;
    }
    // Format 2 carries the client's agent. A forced unknown format is sent with format 2's fields and its own tag.
    let leader = graftyard_lookup::cached_leader(ChainId::Repo(repo.id)).map(|t| t.site);
    let format = c.cfg.push_format(leader);
    if format != REPO_FORMAT_V1 {
        push.agent = Some(rp.caps.agent.clone().unwrap_or_default().into());
    }
    // BM-5's test format is 32.2's layout under its own tag, which a `test-format` site decodes as itself.
    push.test_format = format == graftyard_git::repo::REPO_FORMAT_TEST;
    // `design/SHARED.md`, "Refs and pushes", Size: the updates alone over the cap refuse the whole push.
    let alone = push.encoded_len_without_pack();
    if alone > COMMAND_MAX {
        let r = refuse(Refusal::updates_too_large(alone, COMMAND_MAX, push.updates.len()));
        return after_body(c, &mut tail, path, r).await;
    }
    let pack = body.get(rp.pack_at..).unwrap_or_default();
    let mut index = None;
    let deletes_only = push.updates.iter().all(|u| u.new.is_none());
    // AM1: a push that may be an agent's asks the git cache for `validate`'s detail and carries what it gives
    // (`agents::agentish`). A default push asks for none, so its calls and bytes are unchanged.
    let mut agent = api::timed(c, "context", agents::push_context(c, repo, &push)).await;
    let detail = graftyard_git::agent::detail_lines(&push.updates, agent.is_some());
    // Only updates with a new tip get detail, so a push of deletes alone asks for none.
    let mut got: Vec<graftyard_git::agent::RefDetail> = Vec::new();
    if !deletes_only {
        if pack.is_empty() && tail.is_none() {
            return refuse(Refusal::unpack_failed("no pack"));
        }
        // Inline only when the pack fits INLINE_MAX and the push with it stays under COMMAND_MAX. A pack that streams
        // has more than INLINE_MAX.
        // A session log is always a stored pack (`design/AGENT-LAYER.md`, Shape, `SESSION_REFS`), however small.
        let session =
            push.updates.iter().any(|u| u.name.as_bytes().starts_with(graftyard_git::agent::SESSION_REFS.as_bytes()));
        let inline = tail.is_none() && !session && pack.len() <= INLINE_MAX && alone + pack.len() + 16 < COMMAND_MAX;
        // A group whose leader asked this isolate to stage within HOT_WINDOW, or an Agent pack past
        // AGENT_INLINE_ENTRY: stage now rather than after a refused commit (`hot`). Any other push keeps its inline
        // pack. A Human push classes without a read, and `admission` reads the install's `Authz` once per request.
        let first = inline
            && !placement_failing(c)
            && admission(c, &push.actor, &[], true, 0).await.is_some_and(|a| {
                hot::stage_first(repo.id, now(), pack.len(), a.class == graftyard_git::group::ClassWire::Agent)
            });
        let inline = inline && !first;
        if first {
            // B0-32's count of pushes that staged before their first commit (`stagefirst` in `server-timing`).
            c.timing.put("stagefirst", 0.0);
        }
        // While placement fails a pack that cannot go inline cannot be placed: an agent's is refused at once in the
        // admission shape (B0-31's placement-failing half), any other's is a 503 (Failure modes, R2 outage).
        if placement_failing(c) && !inline {
            let agentish = matches!(
                admission(c, &push.actor, &[], true, 0).await,
                Some(a) if a.class == graftyard_git::group::ClassWire::Agent
            );
            let r = if agentish {
                refuse(Refusal::agent_full(OUTAGE_RETRY_S))
            } else {
                text(c.rid, 503, "pack store: placement is failing; retry\n")
            };
            return after_body(c, &mut tail, path, r).await;
        }
        let tips: Vec<String> = push.updates.iter().filter_map(|u| u.new.map(|o| o.hex())).collect();
        let raw = buckets(c.env, c.sites)?;
        // The three site buckets of the network's placement.
        let refs: Vec<(SiteId, &R2)> =
            raw.iter().filter(|(s, _)| repo.record.sites.contains(s)).map(|(s, b)| (*s, b)).collect();
        let store = PackStore { sites: c.sites, buckets: &refs, copies: Copies::TwoRegions };
        let id = PackId(Ulid::new(now(), rand10()));
        let meta = ObjectMeta { chain: ChainId::Repo(repo.id), request: c.rid };
        let detail_out = std::cell::RefCell::new(Vec::new());
        let streamed = tail.is_some();
        let since = js_sys::Date::now();
        let placed = match tail.take() {
            None => {
                let stage = Stage { network: repo.network(), id, meta, bytes: pack, inline, part: c.cfg.part };
                let validate = async {
                    let (ix, d) = validate(c, repo, pack, &tips, &detail, agent.is_some()).await?;
                    *detail_out.borrow_mut() = d;
                    ix.ok_or_else(|| ShardRefusal::Invalid("validate gave no index".to_owned()))
                };
                store.place_teed(stage, validate).await
            }
            // `design/DESIGN.md`, journey 1, step 2: the pack streams to the buckets in parts and to the git cache,
            // and the edge holds two parts of it, whatever its size.
            Some(mut t) => {
                let s = Streamed { network: repo.network(), id, meta, part: c.cfg.part };
                let pack = futures_util::stream::once(std::future::ready(Ok(std::mem::take(&mut t.pack))));
                let pipe = graftyard_git::Pipe::default();
                let validate = async {
                    let r = validate_streamed(c, repo, pipe.reader(), &tips, &detail, agent.is_some()).await;
                    // The shard answered: whatever it did not read, the tee hands it no more.
                    pipe.stop();
                    let (ix, d) = r?;
                    *detail_out.borrow_mut() = d;
                    ix.ok_or_else(|| ShardRefusal::Invalid("validate gave no index".to_owned()))
                };
                match store.place_streamed(s, pack.chain(&mut t.rest), &pipe, validate).await {
                    Ok(r) => r,
                    Err(Cut::OverCap) => return over_cap(c, false, &t.cap_head, path),
                    Err(Cut::Broke(e)) => return Err(e),
                }
            }
        };
        c.timing.add("validate", since);
        match placed {
            Ok((d, ix)) => {
                push.pack = Some(d.into_pack());
                // Only an inline pack is staged again (`AdmitError::Stage`), from its bytes.
                index = (!streamed).then_some(ix);
                got = detail_out.into_inner();
            }
            Err(TeeError::Invalid(e)) => match shard_refusal(&e) {
                Some(r) => return refuse(r),
                // No shard could check the pack: a retryable error before any byte of a git result, so the client
                // sends the push again rather than reading a refusal (`unsupported` answers the same way).
                None => return shard_unavailable(c.rid, &e.to_string()),
            },
            Err(TeeError::Place(e)) => return text(c.rid, 503, &format!("pack store: {e}\n")),
        }
    }
    // A push of deletes alone takes no pack, but its body is still read to its end against the cap.
    if let Some(r) = drain(c, &mut tail, path).await? {
        return Ok(r);
    }
    if let Some(a) = agent.as_mut() {
        agents::fresh_if_unlinked(c, repo, a, &push, &got, path).await;
        agents::fill_push(c, a, &mut push, &got, path);
    }
    let f = graftyard_raft::Command::format(&push);
    if push.validated() && !c.cfg.enabled.contains(&f) {
        return unsupported(c.rid, f.family, f.version);
    }
    let len = push.encoded_len();
    if len > COMMAND_MAX {
        return refuse(Refusal::updates_too_large(len, COMMAND_MAX, push.updates.len()));
    }
    let mut body = serde_json::to_value(CommitReq { rid: c.rid.to_string(), cmd: push.clone() })?;
    // The admission class (AM1-WP2), sent only for a command classed other than Human.
    let inline = match &push.pack {
        Some(graftyard_git::pack::PackRef::Inline(b)) => b.len(),
        _ => 0,
    };
    let mut admit = api::timed(c, "admission", admission(c, &push.actor, &push.provenance, true, inline)).await;
    // AGENT_PUSH_BOUND: an Agent push whose bound passed before its commit is sent is refused in B0-32's shape, and
    // nothing reaches the group; one sent carries what is left, which the leader holds it to (`Admit::budget`).
    if let Some(a) = admit.as_mut() {
        match bound_at_send(a.class == graftyard_git::group::ClassWire::Agent, budget_left(c)) {
            Ok(b) => {
                // why-logs: what is left of the bound at the first send, so a refusal's stage reads its budget.
                if let Some(ms) = b {
                    c.timing.put("budget", ms as f64);
                }
                a.budget_ms = b
            }
            Err(r) => {
                c.timing.put("bound", 0.0);
                return refuse(r);
            }
        }
    }
    // edge-refusal-bound's send check: the advert again, against what is left of the bound now, after push context
    // and validate spent it (erBA: p50 2.1 s between `receive_pack`'s check and the deciding send).
    if let Some(ms) = admit.as_ref().and_then(|a| a.budget_ms)
        && let Some(s) =
            advert_refusal(c, repo, &push.actor, true, At::Send, std::time::Duration::from_millis(ms)).await
    {
        return refuse(Refusal::agent_full(s));
    }
    if let Some(a) = admit {
        body["admit"] = serde_json::to_value(a)?;
    }
    // BF-16's forced unknown variant: the same command, its format tag rewritten to one no release reads.
    if format != REPO_FORMAT_V1 && format != REPO_FORMAT_V2 && !push.test_format {
        let mut wire = push.to_wire();
        wire[..2].copy_from_slice(&format.family.to_be_bytes());
        wire[2..4].copy_from_slice(&format.version.to_be_bytes());
        body["cmd"] = json!(graftyard_git::pack::b64::encode(&wire));
    }
    // BF-12 on pushes: the leader that decides this push is killed between quorum and reply. The kill rides on the
    // first send only.
    #[allow(unused_mut)]
    let mut first: Option<serde_json::Value> = None;
    #[cfg(feature = "test-faults")]
    if let Some(k) = c.kill.as_deref() {
        let mut f = body.clone();
        f["kill"] = json!(k);
        f["down_ms"] = json!(c.down_ms);
        first = Some(f);
    }
    let lookup = lookup_body(c.rid, &push);
    let mut reply = api::timed(c, "commit", commit(c, repo, &body, first.as_ref(), None, Some(&lookup))).await?;
    if let Err(CommitFail::Wire(CallErrorWire::Stage(_))) = &reply {
        hot::staged(repo.id, now());
    }
    // `AdmitError::Stage`: the leader asks for the inline pack as a stored one, sent again under the same request id.
    if let (Err(CommitFail::Wire(CallErrorWire::Stage(_))), true) = (&reply, placement_failing(c)) {
        return refuse(Refusal::agent_full(OUTAGE_RETRY_S));
    }
    if let (Err(CommitFail::Wire(CallErrorWire::Stage(_))), Some(ix), Some(mut a)) = (&reply, index, admit) {
        let raw = buckets(c.env, c.sites)?;
        let refs: Vec<(SiteId, &R2)> =
            raw.iter().filter(|(s, _)| repo.record.sites.contains(s)).map(|(s, b)| (*s, b)).collect();
        let store = PackStore { sites: c.sites, buckets: &refs, copies: Copies::TwoRegions };
        let stage = Stage {
            network: repo.network(),
            id: PackId(Ulid::new(now(), rand10())),
            meta: ObjectMeta { chain: ChainId::Repo(repo.id), request: c.rid },
            bytes: pack,
            inline: false,
            part: c.cfg.part,
        };
        let since = js_sys::Date::now();
        match store.place_teed(stage, async { Ok::<_, String>(ix) }).await {
            Ok((d, _)) => push.pack = Some(d.into_pack()),
            Err(e) => return text(c.rid, 503, &format!("pack store: {e}\n")),
        }
        c.timing.add("stage", since);
        a.inline = 0;
        if a.budget_ms.is_some() {
            a.budget_ms = budget_left(c).map(|l| l.as_millis() as u64);
        }
        // edge-refusal-bound: the Stage round spent the bound too; the re-send meets the advert again first.
        if let Some(ms) = a.budget_ms
            && let Some(s) =
                advert_refusal(c, repo, &push.actor, true, At::Stage, std::time::Duration::from_millis(ms)).await
        {
            return refuse(Refusal::agent_full(s));
        }
        body = serde_json::to_value(CommitReq { rid: c.rid.to_string(), cmd: push.clone() })?;
        body["admit"] = serde_json::to_value(a)?;
        reply = api::timed(c, "commit", commit(c, repo, &body, None, None, Some(&lookup))).await?;
    }
    #[cfg(feature = "test-faults")]
    c.push_end.replace(Some(match &reply {
        Ok(PushOutcome::Ok(ok)) => json!({ "index": ok.index, "refs": ok.out.refs }),
        Ok(PushOutcome::Refused(r)) => json!({ "refused": r.status }),
        Ok(PushOutcome::Full(f)) => json!({ "full": f.retry_after_s }),
        Ok(PushOutcome::NotLeader(_)) => json!({ "notLeader": true }),
        Err(e) => json!({ "error": e }),
    }));
    match reply {
        Ok(PushOutcome::Ok(ok)) => {
            // The member rendered these bytes from its `&Acked`; the edge only frames them.
            let status = ok.out.status_bytes().ok_or("status")?;
            let r = git_result(c.rid, Service::ReceivePack, wrap(&rp.caps, &status, &[]))?;
            // BF-12's driver compares the outcome's index with the one the killed leader recorded.
            #[cfg(feature = "test-faults")]
            {
                r.headers().set("x-graft-outcome-index", &ok.index.to_string())?;
                r.headers().set("x-graft-rid", &c.rid.to_string())?;
            }
            Ok(r)
        }
        Ok(PushOutcome::Refused(r)) => match r.status {
            401 => unauthorized(c.rid, "Bad credentials"),
            403 => text(c.rid, 403, "Permission denied\n"),
            _ => refuse(Refusal::refused(&r.err)),
        },
        Ok(PushOutcome::Full(f)) => refuse(Refusal::agent_full(f.retry_after_s)),
        Ok(PushOutcome::NotLeader(_)) | Err(CommitFail::Wire(CallErrorWire::NotLeader(_))) => {
            text(c.rid, 503, "the repository's group has no leader; retry\n")
        }
        Err(CommitFail::Wire(CallErrorWire::Unsupported(u))) => unsupported(c.rid, u.family, u.version),
        Err(CommitFail::Wire(CallErrorWire::TooLarge(t))) => {
            refuse(Refusal::updates_too_large(t.len, t.max, rp.updates.len()))
        }
        Err(CommitFail::Wire(CallErrorWire::Stage(_))) => text(c.rid, 503, "the pack could not be staged; retry\n"),
        Err(CommitFail::NoOutcome(e)) => no_outcome(c.rid, &e),
    }
}

/// Why `validate` gave no index: the git cache refused the pack, (an agent's push only) its slots stayed full for
/// the agent's wait, which the push answers `ng <ref> agent admission full, retry after <n>s` (B0-32's shape), or no
/// shard could answer, which the push answers 503 `Retry-After` (`shard_unavailable`).
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ShardRefusal {
    Invalid(String),
    Busy(u32),
    /// No shard could check the pack (each failed without a 422 or 429): not a refusal, but a 503 the client retries.
    Unavailable(String),
}

impl std::fmt::Display for ShardRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ShardRefusal::Invalid(e) | ShardRefusal::Unavailable(e) => f.write_str(e),
            ShardRefusal::Busy(s) => write!(f, "the git cache is busy; retry after {s}s"),
        }
    }
}

/// A validate's URL on the network's first shard: the body's `tipsn` and `detailn` lengths, `class=agent` for an
/// agent's push, the pack's trailer (its last 20 bytes), from which the shard picks the fan-out shard, and the owner's
/// placement record, which saves the shard its two `Names` reads, as `/merge` and `/demand` do: on B0-32 m3fr8 the
/// network shard took p50 3,038 ms to forward a burst push, most of it in those reads' gaps, against the pool shard's
/// 370 ms (`tests/am1/results/lm1-rows-m3f.md`, "m3fr8 after the run").
///
/// `head` is the body's tips and detail, its first `tipsn` bytes and the rest: the URL names its SHA-256
/// (`headsum`), and a pack's when the edge holds it whole (`packsum`), so the container answers a body that came
/// through changed 502, which moves the push to the next shard, rather than refusing it `unpacker error`: on B0-32
/// m3fpc1 and m3fr13, 4 pushes' bodies reached a pool shard's container with pack bytes where their detail lines
/// had been framed (`decisions/OPEN-QUESTIONS.md`, G35).
fn validate_url(repo: &Repo, head: &[u8], tipsn: usize, agent: bool, pack: &[u8]) -> String {
    use sha2::{Digest as _, Sha256};
    let sum = |b: &[u8]| Digest(Sha256::digest(b).into()).hex();
    let record = serde_json::to_string(&repo.record).unwrap_or_default();
    let record: String = url::form_urlencoded::byte_serialize(record.as_bytes()).collect();
    let mut url = format!("https://shard/validate?repo={}&record={record}&tipsn={tipsn}", repo.id.0.0);
    let detailn = head.len() - tipsn;
    if detailn > 0 {
        url.push_str(&format!("&detailn={detailn}"));
    }
    url.push_str(&format!("&headsum={}", sum(head)));
    if !pack.is_empty() {
        url.push_str(&format!("&packsum={}", sum(pack)));
    }
    if agent {
        url.push_str("&class=agent");
    }
    if let Some(t) = pack.len().checked_sub(20).and_then(|at| <[u8; 20]>::try_from(&pack[at..]).ok()) {
        url.push_str(&format!("&trailer={}", graftyard_git::refs::ObjectId(t).hex()));
    }
    url
}

/// The push's refusal when `validate` gave no index: B0-32's `ng <ref> agent admission full, retry after <n>s` when
/// the git cache was busy, `unpacker error` with the cache's reason when a shard refused the pack, and none when no
/// shard could check it, which the push answers 503 instead (`shard_unavailable`).
fn shard_refusal(e: &ShardRefusal) -> Option<Refusal> {
    match e {
        ShardRefusal::Busy(s) => Some(Refusal::agent_full(*s)),
        ShardRefusal::Invalid(why) => Some(Refusal::unpack_failed(why)),
        ShardRefusal::Unavailable(_) => None,
    }
}

/// The sites whose shards a validate or a read tries, in order: the home site's, then the network's other warm site
/// (`graftyard_git::warm_sites`, which holds the chain, its k and its clone-cache entries), then the record's other
/// Full sites' in record order (a Witness site holds no content, so it has no git cache), or only the one a test
/// pinned (`x-graft-shard`). A home shard that rehydrated about 3 GB after a reset took more than 30 min, and every
/// push meanwhile waited on it until its deadline (`tests/am1/results/hot-shards.md`). In record order, B0-34 mt22's
/// clones after the kill of site a went to site b, which held no chain, not to the warm site c: miss catch-up p50
/// 7,260 ms on b against 897 on c (`warm-order`).
fn validate_sites(record: &PlacementRecord, network: NetworkId, pinned: Option<SiteId>) -> Vec<SiteId> {
    if let Some(s) = pinned {
        return vec![s];
    }
    let [_, warm] = graftyard_git::warm_sites(network, record.home, &record.sites);
    let mut others: Vec<_> = record
        .sites
        .iter()
        .zip(record.name.kinds())
        .filter(|(s, k)| **s != record.home && **k == graftyard_types::MemberKind::Full)
        .map(|(s, _)| *s)
        .collect();
    others.sort_by_key(|s| *s != warm);
    std::iter::once(record.home).chain(others).collect()
}

/// After a shard of a network answers anything but 200, 422 or 429, or a hedge passes it, this isolate tries it last
/// for this long, so pushes during a 30 min rehydrate do not each wait out the home shard first: j9d's pushes took
/// about 21 s each, 15 of them the home's `wait` (`tests/am1/results/hot-shards.md`). Past it the home is tried first
/// again.
const COLD_SHARD_MS: u64 = 30_000;

/// A shard this isolate saw cold, until when, and whether a try failed on it (else a hedge only passed it, slow but
/// live). Only a failed home makes a call elsewhere moved load (`moved-mark`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Cold {
    until: u64,
    failed: bool,
}

type ColdMap = std::collections::BTreeMap<(i64, u8), Cold>;

thread_local! {
    static COLD: std::cell::RefCell<ColdMap> = const { std::cell::RefCell::new(std::collections::BTreeMap::new()) };
}

/// What a try told this isolate about a shard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Seen {
    /// 200, or the pack's or the gate's answer (422, 429): warm.
    Answered,
    Failed,
    /// A hedge started the sites left beside it (`validate-hedge`).
    Passed,
}

/// Records `seen` for `key` at `now`. A pass keeps a live failure's mark, so it never hides one.
fn note(m: &mut ColdMap, key: (i64, u8), seen: Seen, now: u64) {
    let failed = match seen {
        Seen::Answered => {
            m.remove(&key);
            return;
        }
        Seen::Failed => true,
        Seen::Passed => m.get(&key).is_some_and(|c| c.until > now && c.failed),
    };
    m.insert(key, Cold { until: now + COLD_SHARD_MS, failed });
}

/// Whether a call to `site` is moved load (`graftyard_git::maint_hold`): `site` is not the record's home, and a try
/// failed on the home within `COLD_SHARD_MS`. A hedge that only passed a slow home moves nothing: on B0-34 mt21 the
/// first tries such a pass sent to site b held its maintenance 573 s with the home alive (G41).
fn moved(m: &ColdMap, net: i64, home: SiteId, site: SiteId, now: u64) -> bool {
    site != home && m.get(&(net, home.0)).is_some_and(|c| c.until > now && c.failed)
}

/// `moved` for a call to `site` of `repo`'s network now.
fn moved_to(repo: &Repo, site: SiteId) -> bool {
    COLD.with(|m| moved(&m.borrow(), repo.id.0.0, repo.record.home, site, now().0))
}

/// `sites` with the ones marked cold at `now` moved to the end, keeping the order within each part.
fn warm_first(sites: Vec<SiteId>, cold: impl Fn(SiteId) -> bool) -> Vec<SiteId> {
    let (warm, cold): (Vec<_>, Vec<_>) = sites.into_iter().partition(|s| !cold(*s));
    warm.into_iter().chain(cold).collect()
}

/// `validate_sites` for `repo`, with the shards this isolate saw cold in the last `COLD_SHARD_MS` last.
fn shard_order(repo: &Repo, pinned: Option<SiteId>) -> Vec<SiteId> {
    let t = now().0;
    let net = repo.id.0.0;
    COLD.with(|m| {
        let m = m.borrow();
        warm_first(validate_sites(&repo.record, repo.network(), pinned), |s| {
            m.get(&(net, s.0)).is_some_and(|c| c.until > t)
        })
    })
}

/// Records a try's answer on `site`: `failed` marks it cold, else it is warm again.
fn mark_shard(repo: &Repo, site: SiteId, failed: bool) {
    let seen = if failed { Seen::Failed } else { Seen::Answered };
    COLD.with(|m| note(&mut m.borrow_mut(), (repo.id.0.0, site.0), seen, now().0));
}

/// How long the first shard tried, usually the home's, may hold a validate in its catch-up before the edge tries the
/// next: past it the shard answers 503 `warming` and goes on catching up in the background. Each later shard gets
/// `VALIDATE_WAIT_OTHER_S`. With three Full sites they stay under the push's 180 s deadline.
/// The shard's `FIRST_TRY_WAIT_S` names the same wait (`maint-hold` reads `moved`, not this).
const VALIDATE_WAIT_HOME_S: u32 = graftyard_git::FIRST_TRY_WAIT_S;
const VALIDATE_WAIT_OTHER_S: u32 = 60;

/// Try `i` of `n` on a shard: the first waits `VALIDATE_WAIT_HOME_S` on its catch-up, each later one
/// `VALIDATE_WAIT_OTHER_S`, and every try but the last says another shard follows (`next=1`), so a shard that is
/// rehydrating answers it 503 `warming` at once rather than after the wait (`workers/site/src/shard.rs`). `moved`
/// (`moved_to`) adds `moved=1`, which holds that shard's maintenance (`graftyard_git::maint_hold`).
fn shard_url(url: &str, i: usize, n: usize, moved: bool) -> String {
    let wait = if i == 0 { VALIDATE_WAIT_HOME_S } else { VALIDATE_WAIT_OTHER_S };
    let next = if i + 1 < n { "&next=1" } else { "" };
    let moved = if moved { "&moved=1" } else { "" };
    format!("{url}&wait={wait}{next}{moved}")
}

/// `GitCacheShard::validate` on the network's home shard, or, while that one catches up past its wait or fails,
/// another placement site's shard of the network (`shard_order`): tips in the body ahead of the pack. With
/// `detail` (the `"<new> <old> <ref>"` lines of an agent push, `graftyard_git::agent::detail_lines`), the shard also
/// answers each update's new commits and changed paths; with an empty pack it answers only those, and no index.
/// Without `detail` the call is the one a default push has always made. The pack's trailer rides in the URL, so the
/// shard picks the fan-out shard without reading the body, which it streams on (LM-1 round 6). A 422 is the pack's
/// answer and ends the tries. A 429 is that container's full slots (an `agent` validate waits at most 2 s for one):
/// it moves to the next site, without marking the shard cold, and when no site answers 200 the push is
/// `ShardRefusal::Busy` (`agent-spill`: on B0-33 g4B 99% of agent validates stayed on the home site while its gate
/// refused 3,722 and site b's shard sat at 0.3 to 0.7 vCPU, G40). A 503 `warming`, another status, or a failed call
/// moves to the next site. When every site's shard failed that way, the error is `Unavailable`, which a push answers
/// 503 `Retry-After` so git and the client retry it, not `unpacker error`: 23 pushes in flight at a home-shard kill
/// were refused that way (`tests/am1/results/hot-shards-2.md`).
pub(crate) async fn validate(
    c: &Ctx<'_>,
    repo: &Repo,
    pack: &[u8],
    tips: &[String],
    detail: &str,
    agent: bool,
) -> std::result::Result<(Option<PackIndex>, Vec<graftyard_git::agent::RefDetail>), ShardRefusal> {
    let mut head = if tips.is_empty() { Vec::new() } else { tips.join("\n").into_bytes() };
    if !tips.is_empty() || detail.is_empty() {
        head.push(b'\n');
    }
    let tipsn = head.len();
    head.extend_from_slice(detail.as_bytes());
    let url = validate_url(repo, &head, tipsn, agent, pack);
    head.extend_from_slice(pack);
    #[cfg(feature = "test-faults")]
    let pinned = c.shard;
    #[cfg(not(feature = "test-faults"))]
    let pinned = None;
    let sites = shard_order(repo, pinned);
    let n = sites.len();
    let (head, url) = (&head, &url);
    // An agent's: each try's slot wait from what is left of its bound when the try starts (`slot-bound`).
    let slot_ms = || if agent { agent_slot_ms(budget_left(c)) } else { None };
    let start = |i: usize| {
        let site = sites[i];
        let url = with_slot(shard_url(url, i, n, moved_to(repo, site)), slot_ms());
        async move {
            let body = || Some(js_sys::Uint8Array::from(&head[..]).into());
            let r = validate_on(c, repo, site, &url, &body, detail).await;
            let r = r.map_err(|e| (0, e.to_string())).and_then(|r| r);
            mark_shard(repo, site, !matches!(r, Ok(_) | Err((422 | 429, _))));
            r
        }
    };
    let passed = |i: usize| COLD.with(|m| note(&mut m.borrow_mut(), (repo.id.0.0, sites[i].0), Seen::Passed, now().0));
    let t = hedged(n, start, validate_hedge(agent), passed, worker::Delay::from).await;
    // Which site answered, how many tries started, and whether a hedge started one (`validate-hedge`).
    c.timing.put("validate_site", t.answer.as_ref().map_or(-1.0, |(i, _)| f64::from(sites[*i].0)));
    c.timing.put("validate_tries", t.started as f64);
    c.timing.put("validate_hedge", if t.hedged { 1.0 } else { 0.0 });
    let mut tries: Vec<_> = t.failed.into_iter().map(|(i, (status, e))| (sites[i], status, e)).collect();
    match t.answer {
        None => {}
        Some((_, Ok(r))) => return Ok(r),
        // The pack's own answer, the same on every shard: no other site is tried.
        Some((i, Err((status, e)))) => tries.push((sites[i], status, e)),
    }
    Err(validate_failed(tries))
}

/// How long a validate's try of a shard runs unanswered before the next site's shard is asked beside it (a hedge), on
/// every try but the last. Human pushes' validates outside a kill took p50 295, p95 1,122, p99 1,725 ms (2,105
/// pushes, B0-34 mt13 to mt16), and 7.0% passed 1 s; the push that straddled mt16's kill waited 2,087 ms on the shard
/// being torn down (`decisions/DECISIONS.md`, "`validate-hedge`").
const VALIDATE_HEDGE: std::time::Duration = std::time::Duration::from_millis(1_000);

/// The hedge a validate gets: none for an agent's, which the container already bounds with its admission wait
/// (`agent_slot_ms`) and whose p50 is 1.5 s, so a hedge would double most of them. On B0-33 h1A an agent's slow tail
/// was that wait on a full pool shard, so a hedge would start more tries into the same full gates (`slot-bound`).
fn validate_hedge(agent: bool) -> Option<std::time::Duration> {
    (!agent).then_some(VALIDATE_HEDGE)
}

/// What an agent validate's try must leave of the push's AGENT_PUSH_BOUND once its container slot is granted: the
/// validate's own work and the commit after it. On B0-33 h1A the home's quick validates spent p90 about 230 ms in
/// `v_walk` and `v_index`, the commit took p50 271 ms, and ok pushes had p5 809 ms of the bound left at their first
/// send [measured, `tests/am1/results/g40-slot.md`]; 1 s [guess] covers the three.
const AGENT_SLOT_RESERVE: std::time::Duration = std::time::Duration::from_millis(1_000);

/// The longest an agent validate's try may wait for a container slot (`slot_ms`, which the shard passes to its
/// container and keeps in a forward to a pool shard): what is left of the push's bound now less
/// `AGENT_SLOT_RESERVE`, never above the container's own 2 s, and none for a push with no bound. On h1A 1,010 agent
/// validates waited a pool shard's whole 2 s for its 429 and reached the next site with too little bound left: 29 of
/// them committed (`slot-bound`).
fn agent_slot_ms(left: Option<std::time::Duration>) -> Option<u64> {
    left.map(|l| l.saturating_sub(AGENT_SLOT_RESERVE).as_millis().min(2_000) as u64)
}

/// `url` with an agent try's slot wait (`agent_slot_ms`).
fn with_slot(url: String, slot_ms: Option<u64>) -> String {
    match slot_ms {
        Some(ms) => format!("{url}&slot_ms={ms}"),
        None => url,
    }
}

/// One try's answer: the shard's value, or `(status, body)` with status 0 for a failed call.
type TryAnswer<A> = std::result::Result<A, (u16, String)>;

/// What `hedged` ended with: the deciding try's index and answer (`None` when every try failed), the tries that
/// failed before it, in the order they answered, how many tries started, and whether a hedge started one.
struct Hedged<A> {
    answer: Option<(usize, TryAnswer<A>)>,
    failed: Vec<(usize, (u16, String))>,
    started: usize,
    hedged: bool,
}

/// `n` tries, `start(i)` starting try `i`: an answer, or `(status, body)` with status 0 for a failed call. A 200 or
/// 422 from any try decides and ends the rest; before the hedge any other answer starts the next try at once, as a
/// failed shard always has, a 429 included: that shard's gate is full, which another site's may not be
/// (`agent-spill`). With `hedge`, a try that has run that long unanswered while another try is left is
/// passed: `passed(i)` is told (the edge marks its shard cold) and every try left starts at once beside it, since the
/// next shard is often slow too, taking the load the passed one dropped (B0-34 mt17's straddler: home unanswered at
/// 2.8 s, site b past 1 s, site c answered 0.3 s after its start). Every try started keeps running, so a slow shard
/// that answers first still decides, and the last try keeps its whole wait, so a push never runs out of shards that
/// would wait (`chain-wait`, G30).
async fn hedged<A, F, Fut, P, S, D>(
    n: usize,
    mut start: F,
    hedge: Option<std::time::Duration>,
    mut passed: P,
    sleep: S,
) -> Hedged<A>
where
    F: FnMut(usize) -> Fut,
    Fut: std::future::Future<Output = TryAnswer<A>>,
    P: FnMut(usize),
    S: Fn(std::time::Duration) -> D,
    D: std::future::Future<Output = ()>,
{
    use futures_util::future::{Either, select};
    use futures_util::stream::FuturesUnordered;
    let tagged = |i: usize, f: Fut| async move { (i, f.await) };
    let mut running = FuturesUnordered::new();
    let mut out = Hedged { answer: None, failed: Vec::new(), started: 0, hedged: false };
    // The try started last and its hedge timer, while a later try is left.
    let mut timer: Option<(usize, std::pin::Pin<Box<D>>)> = None;
    // How many tries to start now.
    let mut due = usize::from(n > 0);
    loop {
        for _ in 0..std::mem::take(&mut due) {
            let i = out.started;
            running.push(tagged(i, start(i)));
            out.started += 1;
            timer = hedge.filter(|_| out.started < n).map(|d| (i, Box::pin(sleep(d))));
        }
        if running.is_empty() {
            return out;
        }
        let tick = match timer.as_mut() {
            Some((_, t)) => Either::Left(t.as_mut()),
            None => Either::Right(std::future::pending::<()>()),
        };
        let event = match select(running.next(), tick).await {
            Either::Left((r, _)) => r,
            Either::Right(_) => None,
        };
        match event {
            Some((i, r @ (Ok(_) | Err((422, _))))) => {
                out.answer = Some((i, r));
                return out;
            }
            // Before the hedge a failed try, or a full gate's 429, starts the next at once; after it every try has
            // started.
            Some((i, Err(e))) => {
                out.failed.push((i, e));
                due = usize::from(!out.hedged && out.started < n);
            }
            None => {
                let (i, _) = timer.take().expect("only a timer or an empty set ends the select without a try");
                passed(i);
                out.hedged = true;
                due = n - out.started;
            }
        }
    }
}

/// `validate` for a pack that streams (`PackStore::place_streamed`): the body is the tips and detail, then the pack as
/// the tee hands it on. The body is read once, so it fails over only before its first byte: `streamed_site` asks each
/// shard of `shard_order` but the last whether it is caught up, with a bodiless `/catchup` at that try's wait, and the
/// pack goes to the first that is, or to the last, with the last try's wait and no trailer: it is known only at the
/// pack's end, so the shard validates the push itself rather than forwarding it to a fan-out shard. A shard that
/// fails once the pack streams, without a 422 or 429, is `Unavailable`, which the push answers 503 `Retry-After`, as
/// when every shard failed. Sent to the first shard only, a big push during B0-34's home-shard stall would answer
/// 503 with the other sites' shards never asked (`tests/am1/results/lm1-memory.md`, round 18c).
async fn validate_streamed(
    c: &Ctx<'_>,
    repo: &Repo,
    pack: graftyard_git::PipeReader,
    tips: &[String],
    detail: &str,
    agent: bool,
) -> std::result::Result<(Option<PackIndex>, Vec<graftyard_git::agent::RefDetail>), ShardRefusal> {
    let mut head = tips.join("\n").into_bytes();
    if !tips.is_empty() || detail.is_empty() {
        head.push(b'\n');
    }
    let tipsn = head.len();
    head.extend_from_slice(detail.as_bytes());
    let url = validate_url(repo, &head, tipsn, agent, &[]);
    let chunks = futures_util::stream::once(std::future::ready(Ok(head)))
        .chain(pack)
        .map(|c| c.map(|b| js_sys::Uint8Array::from(&b[..]).into()).map_err(|e| wasm_bindgen::JsValue::from_str(&e)));
    let body = std::cell::RefCell::new(Some(wasm_streams::ReadableStream::from_stream(chunks).into_raw()));
    let body = || body.borrow_mut().take().map(Into::into);
    #[cfg(feature = "test-faults")]
    let pinned = c.shard;
    #[cfg(not(feature = "test-faults"))]
    let pinned = None;
    let sites = shard_order(repo, pinned);
    let record = serde_json::to_string(&repo.record).map(|r| String::from(js_sys::encode_uri_component(&r)));
    let record = record.unwrap_or_default();
    let probe = |i: usize, site: SiteId| {
        let url = format!("https://shard/catchup?repo={}&record={record}", repo.id.0.0);
        let url = shard_url(&url, i, sites.len(), moved_to(repo, site));
        async move {
            let mut init = RequestInit::new();
            init.with_method(Method::Post);
            let r = match shard_at(c, repo, Some(site)).and_then(|s| Ok((s, Request::new_with_init(&url, &init)?))) {
                Ok((s, req)) => s.fetch_with_request(req).await,
                Err(e) => Err(e),
            };
            let r = match r {
                Ok(mut r) => (r.status_code(), r.text().await.unwrap_or_default()),
                Err(e) => (0, e.to_string()),
            };
            mark_shard(repo, site, r.0 != 200);
            r
        }
    };
    let (site, mut tries) = streamed_site(&sites, probe).await;
    let slot = if agent { agent_slot_ms(budget_left(c)) } else { None };
    let r =
        validate_on(c, repo, site, &with_slot(shard_url(&url, 1, 2, moved_to(repo, site)), slot), &body, detail).await;
    let r = r.map_err(|e| (0, e.to_string())).and_then(|r| r);
    mark_shard(repo, site, !matches!(r, Ok(_) | Err((422 | 429, _))));
    match r {
        Ok(r) => Ok(r),
        Err((429, s)) => Err(ShardRefusal::Busy(s.parse().unwrap_or(1))),
        Err((status, e)) => {
            tries.push((site, status, e));
            Err(validate_failed(tries))
        }
    }
}

/// The shard a streamed pack goes to: the first of `sites` but the last whose `probe(i, site)` (a bodiless
/// catch-up at try `i`'s wait) answers 200, else the last, which is never probed: the pack goes there whatever it
/// would answer. Also the probes that failed, each `(site, status, body)`, for `validate_failed`.
async fn streamed_site<F, Fut>(sites: &[SiteId], mut probe: F) -> (SiteId, Vec<(SiteId, u16, String)>)
where
    F: FnMut(usize, SiteId) -> Fut,
    Fut: std::future::Future<Output = (u16, String)>,
{
    let mut tries = Vec::new();
    let (last, rest) = sites.split_last().expect("validate_sites always holds the home site");
    for (i, site) in rest.iter().copied().enumerate() {
        match probe(i, site).await {
            (200, _) => return (site, tries),
            (status, e) => tries.push((site, status, e)),
        }
    }
    (*last, tries)
}

/// The tries `validate` made, each `(site, status, body)` with status 0 for a failed call (a 429's body is its
/// `retry-after`), in order, and none answered 200: `Invalid` with the 422's body when a shard answered the pack 422,
/// else `Busy` with the least `retry-after` when a shard's gate was full (an agent's push keeps its class's refusal
/// shape, `agent admission full`, when the sites it moved on to failed too), else `Unavailable` naming the last.
fn validate_failed(tries: Vec<(SiteId, u16, String)>) -> ShardRefusal {
    if let Some((_, _, e)) = tries.iter().find(|(_, s, _)| *s == 422) {
        return ShardRefusal::Invalid(e.clone());
    }
    if let Some(s) = tries.iter().filter(|(_, s, _)| *s == 429).map(|(_, _, e)| e.parse().unwrap_or(1)).min() {
        return ShardRefusal::Busy(s);
    }
    let last = tries.last().map(|(site, status, e)| match status {
        0 => format!("site {}'s shard: {e}", site.0),
        s => format!("site {}'s shard answered {s}: {e}", site.0),
    });
    ShardRefusal::Unavailable(last.unwrap_or_else(|| "no shard to try".into()))
}

/// The log line for an upload-pack the shard did not answer 200, which the client gets as 503: the shard's status and
/// the first `SHARD_WHY_MAX` bytes of its body, so Workers Logs say why. B0-33 m53's 3,441 ls-refs 503s from the
/// shard carried no reason in any log (`tests/am1/results/b033-m53.md`, "The `ls-refs` split").
fn shard_fail_line(repo: i64, status: u16, why: &str, ls_refs: bool) -> String {
    const SHARD_WHY_MAX: usize = 200;
    let cut = why.char_indices().nth(SHARD_WHY_MAX).map_or(why.len(), |(i, _)| i);
    json!({ "shardFail": repo, "status": status, "why": why[..cut].trim_end(), "lsRefs": ls_refs }).to_string()
}

/// A read on the network's shards (`upload-pack`, `info-refs`) in `shard_order`: the first with
/// `wait=VALIDATE_WAIT_HOME_S`, then each other with `wait=VALIDATE_WAIT_OTHER_S`, until one answers 200.
/// The last answer otherwise. A fetch, like a push, waited on a home shard's 30 min rehydrate until its deadline.
pub(crate) async fn shard_read(
    c: &Ctx<'_>,
    repo: &Repo,
    url: &str,
    method: Method,
    proto: &str,
    body: Option<&[u8]>,
) -> Result<Response> {
    #[cfg(feature = "test-faults")]
    let pinned = c.shard;
    #[cfg(not(feature = "test-faults"))]
    let pinned = None;
    let sites = shard_order(repo, pinned);
    // The owner's record, as `/validate` and `/merge` carry it, so the shard reads no `Names` rows first: without it
    // every upload-pack on an isolate that had not cached the owner read `RepoOwner` and `Placement`, and on B0-29 run
    // 5 1,869 of them failed after LOOKUP_FOR on `names:708`, the owner's `Placement` shard
    // (`tests/m0/results/merge-tail.md`).
    let record = serde_json::to_string(&repo.record).map(|r| String::from(js_sys::encode_uri_component(&r)))?;
    let url = format!("{url}&record={record}");
    for (i, site) in sites.iter().enumerate() {
        let h = Headers::new();
        h.set("git-protocol", proto)?;
        let mut init = RequestInit::new();
        init.with_method(method.clone()).with_headers(h);
        if let Some(b) = body {
            init.with_body(Some(js_sys::Uint8Array::from(b).into()));
        }
        let req = Request::new_with_init(&shard_url(&url, i, sites.len(), moved_to(repo, *site)), &init)?;
        let r = shard_at(c, repo, Some(*site))?.fetch_with_request(req).await;
        mark_shard(repo, *site, !r.as_ref().is_ok_and(|r| r.status_code() == 200));
        if i + 1 == sites.len() || r.as_ref().is_ok_and(|r| r.status_code() == 200) {
            return r;
        }
    }
    unreachable!("validate_sites always holds the home site")
}

/// One validate on site `site`'s shard of the network: `Err((status, body))` for any status but 200, except a 429's
/// second part, which is its `retry-after`.
async fn validate_on(
    c: &Ctx<'_>,
    repo: &Repo,
    site: SiteId,
    url: &str,
    body: &dyn Fn() -> Option<wasm_bindgen::JsValue>,
    detail: &str,
) -> Result<std::result::Result<(Option<PackIndex>, Vec<graftyard_git::agent::RefDetail>), (u16, String)>> {
    let send = |url: String| async move {
        let mut init = RequestInit::new();
        init.with_method(Method::Post).with_body(body());
        shard_at(c, repo, Some(site))?.fetch_with_request(Request::new_with_init(&url, &init)?).await
    };
    let mut r = send(url.to_owned()).await?;
    // A pool shard that did not answer: the first shard validates it itself.
    if r.status_code() == 503 && r.headers().get("x-graft-fanout-failed")?.is_some() {
        r = send(format!("{url}&via=0")).await?;
    }
    // Full slots: the body is the `retry-after` seconds, which `validate` turns into `ShardRefusal::Busy`.
    if r.status_code() == 429 {
        return Ok(Err((429, r.headers().get("retry-after")?.unwrap_or_default().trim().to_owned())));
    }
    if r.status_code() != 200 {
        return Ok(Err((r.status_code(), r.text().await.unwrap_or_default())));
    }
    // The container's split of the validate, as `v_*` stages of this request (`api::Timing`).
    for (name, ms) in validate_timing(&r.headers().get("x-graft-validate-timing")?.unwrap_or_default()) {
        c.timing.put(name, ms as f64);
    }
    if detail.is_empty() {
        return Ok(Ok((Some(r.json::<PackIndex>().await?), Vec::new())));
    }
    #[derive(Deserialize)]
    struct Detailed {
        index: Option<PackIndex>,
        #[serde(default)]
        detail: Vec<graftyard_git::agent::RefDetail>,
    }
    let d: Detailed = r.json().await?;
    Ok(Ok((d.index, d.detail)))
}

/// The shard's `x-graft-validate-timing` (`name;dur=N, ...`) as this request's stages: `lock_wait`, `index`, and
/// `walk` become `v_lock_wait`, `v_index`, and `v_walk`; any other name is ignored.
fn validate_timing(h: &str) -> Vec<(&'static str, u64)> {
    h.split(',')
        .filter_map(|part| {
            let (name, dur) = part.trim().split_once(";dur=")?;
            let name = match name {
                "lock_wait" => "v_lock_wait",
                "index" => "v_index",
                "walk" => "v_walk",
                _ => return None,
            };
            Some((name, dur.trim().parse().ok()?))
        })
        .collect()
}

/// `auto_init`'s first commit on a new repository (`crate::repos`): the README commit's pack, validated by the git
/// cache and placed as a push's is, then committed as an ordinary `Push` of `refs/heads/main` under the caller's
/// credential. `Ok(false)` when the leader refused it; the repository then has no commit, as a create without
/// `auto_init` has none.
pub(crate) async fn push_init(
    c: &Ctx<'_>,
    repo: &Repo,
    who: &Principal,
    init: &graftyard_git::init::InitCommit,
) -> std::result::Result<bool, String> {
    // Its own request id: under the create's, the leader would answer the later `InitRepo` as a re-send of this push
    // (`/outcome_by_request`), and the settings would never commit.
    let rid = RequestId(Ulid::new(now(), rand10()));
    let main = RefName::parse(b"refs/heads/main").map_err(|e| format!("{e:?}"))?;
    let tips = vec![init.commit.hex()];
    let raw = buckets(c.env, c.sites).map_err(|e| e.to_string())?;
    let refs: Vec<(SiteId, &R2)> =
        raw.iter().filter(|(s, _)| repo.record.sites.contains(s)).map(|(s, b)| (*s, b)).collect();
    let store = PackStore { sites: c.sites, buckets: &refs, copies: Copies::TwoRegions };
    let stage = Stage {
        network: repo.network(),
        id: PackId(Ulid::new(now(), rand10())),
        meta: ObjectMeta { chain: ChainId::Repo(repo.id), request: rid },
        bytes: &init.pack,
        inline: init.pack.len() <= INLINE_MAX,
        part: c.cfg.part,
    };
    ensure_group(c, repo).await?;
    // A default push's validate: no agent detail, so the shard answers only the pack's index.
    let check = async {
        let (ix, _) = validate(c, repo, &init.pack, &tips, "", false).await.map_err(|e| e.to_string())?;
        ix.ok_or_else(|| "validate gave no index".to_owned())
    };
    let (d, _) = store.place_teed(stage, check).await.map_err(|e| e.to_string())?;
    let push = Push {
        updates: vec![graftyard_git::refs::RefUpdate { name: main, expect: None, new: Some(init.commit) }],
        atomicity: Atomicity::AllOrNothing,
        pack: Some(d.into_pack()),
        actor: who.clone(),
        options: Vec::new(),
        request: rid,
        agent: None,
        provenance: Vec::new(),
        protected: Vec::new(),
        changed: Vec::new(),
        // A new repository has no lease, so the README push keeps its earlier format.
        leases: false,
        test_format: false,
    };
    let body = serde_json::to_value(CommitReq { rid: rid.to_string(), cmd: push }).map_err(|e| e.to_string())?;
    match commit(c, repo, &body, None, None, None).await.map_err(|e| e.to_string())? {
        Ok(PushOutcome::Ok(ok)) => Ok(ok.out.refs.iter().all(|r| *r == graftyard_git::repo::RefOutcome::Ok)),
        Ok(PushOutcome::Refused(_)) => Ok(false),
        Ok(other) => Err(format!("{other:?}")),
        Err(e) => Err(format!("{e:?}")),
    }
}

// ---- upload-pack ----

/// An upload-pack's request, read and keyed before its read gate decides, and its clone-cache answer if one was found
/// (`upload_front`).
struct UploadFront {
    proto: String,
    body: Vec<u8>,
    v2: Option<graftyard_git::smart::V2Request>,
    key: Option<CloneKey>,
    answer: Option<Hit>,
}

/// What `upload_front` found before the gate decided. Neither starts a stream: a `Composed` hit's answer is built only
/// once the gate allows, so a refused request leaves no writer waiting on a reader.
enum Hit {
    /// A manifest entry whose chain packs were checked, and the hit's headers.
    Composed(graftyard_git::host::ChainAnswer, Headers),
    /// A whole entry's R2 body, `chain_late`, or the 503 of a tag read with no leader.
    Ready(Response),
}

/// The part of an upload-pack that runs beside its read gate (`graftyard_git::compose::beside_gate`, `hit-gate`): the
/// body, its `CloneKey`, and the clone-cache lookup, a manifest checked in the chain buckets or a whole entry. Nothing
/// it finds is sent before the gate allows. On B0-34 mt11 the lookup ran after the gate, and a manifest hit's gate read
/// (front p99 1,078 ms in j9), manifest read and chain checks summed into hits over 1 s.
async fn upload_front(c: &Ctx<'_>, req: &mut Request, repo: &Repo) -> Result<UploadFront> {
    let proto = req.headers().get("git-protocol")?.unwrap_or_default();
    let enc = req.headers().get("content-encoding")?.unwrap_or_default();
    let raw = req.bytes().await?;
    let body = if enc == "gzip" { gunzip(&raw, c.cfg.cap)? } else { raw };
    // The clone cache: a v2 fetch with no haves, read from R2 by its `CloneKey`, else upload-pack on the shard, which
    // fills the cache.
    // BF-18 clones from a chosen shard (`x-graft-shard`) bypass the clone cache, so the shard serves every one.
    // The lifeboat has no clone cache: it binds no bucket, and `graft-git` answers every fetch (`lifeboat::git`).
    #[cfg(feature = "test-faults")]
    let cache_ok = c.shard.is_none() && !graftyard_lookup::lifeboat();
    #[cfg(not(feature = "test-faults"))]
    let cache_ok = !graftyard_lookup::lifeboat();
    let v2 = if wants_v2(Some(&proto)) { parse_v2(&body).ok() } else { None };
    let mut answer = None;
    let key = match v2.as_ref() {
        Some(v) if cache_ok => {
            // The tags are read only for a key that holds them (`CloneKey::needs_tags`): an ls-refs read them from the
            // leader and threw them away.
            if CloneKey::needs_tags(repo.network(), v) {
                match read_refs(c, repo, b"refs/tags/").await {
                    Ok(Ok((_, refs))) => {
                        CloneKey::of(repo.network(), v, tags_digest(refs.iter().map(|(n, o)| (n.as_str(), o.as_str()))))
                    }
                    _ => {
                        answer = Some(Hit::Ready(text(c.rid, 503, "the repository's group has no leader; retry\n")?));
                        None
                    }
                }
            } else {
                CloneKey::of(repo.network(), v, graftyard_types::Digest([0; 32]))
            }
        }
        _ => None,
    };
    if let Some(k) = &key {
        answer = clone_hit(c, repo, k).await?;
    }
    Ok(UploadFront { proto, body, v2, key, answer })
}

/// The network's chain buckets as R2 handles, home first (`chain-warm`): after a fold the home bucket can lack a chain
/// the other warm bucket still holds whole, and its shards still name it.
fn chain_r2(c: &Ctx<'_>, repo: &Repo) -> Result<Vec<R2>> {
    graftyard_git::chain_buckets(repo.network(), &repo.record)
        .into_iter()
        .map(|s| Ok(R2(c.env.bucket(&format!("BUCKET_{}", s.0))?)))
        .collect()
}

/// The chain buckets' header sets (`colo-gate`), in `chain_r2`'s order: in the isolate and, unless this is the
/// share-off arm, the data center's Cache API.
fn chain_sets<'a>(c: &'a Ctx<'a>, repo: &Repo) -> colo::ChainSets<'a> {
    let network = repo.network();
    colo::ChainSets { c, network, sites: graftyard_git::chain_buckets(network, &repo.record), share: !colo_off(c) }
}

/// A clone-cache hit for `k`, from the home site's bucket: a composed miss's entry is a manifest (`edge-compose`),
/// checked in the chain buckets; one whose chain has moved on in every one (a fold deleted its files) is not a hit,
/// and neither is no entry. `chain_open` in `server-timing`: the manifest's chain checks, recorded whether the manifest
/// served or was declined (`chain-open`). The manifest's tail stays in its entry's body, never held whole (`hit-gate`).
async fn clone_hit(c: &Ctx<'_>, repo: &Repo, k: &CloneKey) -> Result<Option<Hit>> {
    let cache = R2(c.env.bucket(&format!("BUCKET_{}", repo.record.home.0))?);
    let chain = chain_r2(c, repo)?;
    let chain: Vec<&R2> = chain.iter().collect();
    let h = git_headers(c.rid, &Service::UploadPack.result_type())?;
    h.set("x-graft-clone-cache", "hit")?;
    if let Some(o) = cache.0.get(k.manifest_key().to_string()).execute().await.map_err(|e| e.to_string())?
        && o.size() <= graftyard_git::compose::TAIL_ANSWER_MAX
        && let Some(b) = o.body()
        && let worker::ResponseBody::Stream(body) = b.response_body()?
    {
        let net = repo.network();
        let sets = chain_sets(c, repo);
        match api::timed(c, "chain_open", graftyard_git::host::manifest_check(&chain, net, body, o.size(), &sets))
            .await
            .map_err(|e| e.0)?
        {
            ChainCheck::Checked(a) => return Ok(Some(Hit::Composed(a, h))),
            ChainCheck::Late => return Ok(Some(Hit::Ready(chain_late(c.rid)?))),
            ChainCheck::Declined => {}
        }
    }
    Ok(graftyard_git::host::cached_response(&cache, &k.r2_key(0).to_string(), h)
        .await
        .map_err(|e| e.0)?
        .map(Hit::Ready))
}

/// The front's clone key on `network`, and whether the front's lookup under it still stands: `false` when the front
/// built it on another network, and the caller looks it up again.
fn front_key(key: Option<CloneKey>, network: NetworkId) -> (Option<CloneKey>, bool) {
    match key {
        Some(k) if k.network != network => (Some(CloneKey { network, ..k }), false),
        k => (k, true),
    }
}

async fn upload_pack(c: &Ctx<'_>, repo: &Repo, front: UploadFront) -> Result<Response> {
    let UploadFront { proto, body, v2, key, answer } = front;
    // The front ran beside the gate on the isolate's guess at the network; the gate's own read fixed it since. A key
    // built on another network is built again, and what the front found under it is not served: it is another
    // network's entry, stale for this repository (`fork-network-read`).
    let (key, kept) = front_key(key, repo.network());
    let answer = match &key {
        Some(k) if !kept => clone_hit(c, repo, k).await?,
        _ => answer,
    };
    // On the lifeboat, `ls-refs` too comes from `graft-git`, so the refs it names are always ones it can send
    // (`lifeboat::git`); the edge's normal path is unchanged.
    if graftyard_lookup::lifeboat() {
        return lifeboat::git(c, repo.network(), &proto, Some(body), &Service::UploadPack.result_type()).await;
    }
    let is_ls = v2.as_ref().is_some_and(|v| v.command == "ls-refs");
    #[cfg(feature = "test-faults")]
    let edge_ls = c.ls_refs_edge;
    #[cfg(not(feature = "test-faults"))]
    let edge_ls = true;
    // A v2 ls-refs from the group's refs, with no shard: none of its rehydrate or catch-up stands in front of a clone's
    // first round trip or a fetch's tip read (B0-34, `graftyard_git::ls_refs`). Any request it does not serve, and
    // any read that fails or comes back partial, goes on to the shard below.
    if edge_ls
        && let Some(ls) = v2.as_ref().and_then(graftyard_git::ls_refs::LsRefs::parse)
        && let Some(answer) = ls_refs(c, repo, &ls).await
    {
        let h = git_headers(c.rid, &Service::UploadPack.result_type())?;
        #[cfg(feature = "test-faults")]
        h.set("x-graft-ls-refs", "edge")?;
        return Ok(Response::from_bytes(answer)?.with_headers(h));
    }
    match answer {
        Some(Hit::Composed(a, h)) => return graftyard_git::host::composed_answer(a, h).map_err(|e| e.0.into()),
        Some(Hit::Ready(r)) => return Ok(r),
        None => {}
    }
    let net = repo.network();
    let chain = chain_r2(c, repo)?;
    let chain: Vec<&R2> = chain.iter().collect();
    // A miss names its key (`fill=`): the shard spools the whole answer and puts it to the cache itself, so a client
    // that stops reading still leaves an entry. The edge's own tee stored only answers a client read to the end, and
    // B0-34's first-byte probe never did (`decisions/DECISIONS.md`, "The clone cache fills from the shard"). With
    // `tail=1` (`edge-compose`) a composed miss comes back as the chain's manifest and the want's own objects, and the
    // edge streams the chain's packs from the first chain bucket that holds them all, so only the tail leaves the
    // container. A chain file that no chain bucket holds sends the miss back to the shard for the composed pack.
    let fill = key.as_ref().map_or(String::new(), |k| format!("&fill={}", k.digest().hex()));
    let url = format!("https://shard/upload-pack?repo={}{fill}", repo.id.0.0);
    let tail_url = format!("{url}{}", if key.is_some() { "&tail=1" } else { "" });
    let mut r = shard_read(c, repo, &tail_url, Method::Post, &proto, Some(&body)).await?;
    if r.status_code() == 200 && r.headers().get("x-graft-compose")?.as_deref() == Some(graftyard_git::compose::TAIL) {
        let len = r.headers().get("content-length")?.and_then(|v| v.parse::<u64>().ok());
        let h = git_headers(c.rid, &Service::UploadPack.result_type())?;
        h.set("x-graft-clone-cache", "miss")?;
        h.set("x-graft-compose", "composed: r2")?;
        if let Some(v) = r.headers().get("x-graft-cursor")? {
            h.set("x-graft-cursor", &v)?;
        }
        if let Some(n) = len.filter(|n| *n <= graftyard_git::compose::TAIL_ANSWER_MAX)
            && let worker::ResponseBody::Stream(b) = r.body().clone()
        {
            let sets = chain_sets(c, repo);
            match api::timed(c, "chain_open", graftyard_git::host::manifest_answer(&chain, net, b, n, h, &sets))
                .await
                .map_err(|e| e.0)?
            {
                ChainCheck::Checked(a) => return Ok(a),
                ChainCheck::Late => return chain_late(c.rid),
                ChainCheck::Declined => {}
            }
        }
        r = shard_read(c, repo, &url, Method::Post, &proto, Some(&body)).await?;
    }
    let hh = git_headers(c.rid, &Service::UploadPack.result_type())?;
    if r.status_code() != 200 {
        let why = r.text().await.unwrap_or_default();
        worker::console_log!("{}", shard_fail_line(repo.id.0.0, r.status_code(), &why, is_ls));
        return shard_read_failed(c.rid, r.status_code(), &why);
    }
    for k in ["x-graft-cursor", "x-graft-compose"] {
        if let Some(v) = r.headers().get(k)? {
            hh.set(k, &v)?;
        }
    }
    if key.is_some() {
        hh.set("x-graft-clone-cache", "miss")?;
    }
    Ok(Response::from_body(r.body().clone())?.with_headers(hh))
}

/// The bytes of a v2 ls-refs from the group: HEAD and each of the request's reads as one page, all in one read
/// (`LsRefs::query`) that the group's learners and followers serve by read-index, never the leader
/// (`graftyard_lookup::read_off_leader`), so every line is from one fold at or above the read's floor and a push
/// acknowledged before the request shows. On run 6 the leader's 36 refs pages a second beside its commit path took
/// its commit p99 to 7,720 ms, and to 817 ms with them off (F91, `tests/am1/results/b029-m2b.md`). `None` on a shed,
/// declined, or failed read, a partial page, or a default branch that does not exist yet, so the shard answers. HEAD
/// is `refs/heads/main` for a repository with no `settings` row, as the shard's is (`graftyard_git::tiers::default_branch`).
async fn ls_refs(c: &Ctx<'_>, repo: &Repo, ls: &graftyard_git::ls_refs::LsRefs) -> Option<Vec<u8>> {
    let b = c.bindings();
    let (v, _, _) =
        graftyard_lookup::read_off_leader(&b, ChainId::Repo(repo.id), &repo.record, &ls.query()).await.ok()??;
    let r: graftyard_git::ls_refs::LsRefsRead = serde_json::from_value(v.get("value")?.clone()).ok()?;
    ls.answer_read(&r)
}

// ---- advertisements ----

/// `signed_in` is whether the read gate found a caller.
async fn info_refs(c: &Ctx<'_>, req: &Request, repo: &Repo, service: Service, signed_in: bool) -> Result<Response> {
    let proto = req.headers().get("git-protocol")?;
    match service {
        Service::UploadPack if wants_v2(proto.as_deref()) => {
            Ok(Response::from_bytes(upload_pack_v2_advertisement(AGENT))?
                .with_headers(git_headers(c.rid, &service.advertisement_type())?))
        }
        Service::UploadPack if graftyard_lookup::lifeboat() => {
            lifeboat::git(c, repo.network(), proto.as_deref().unwrap_or(""), None, &service.advertisement_type()).await
        }
        Service::UploadPack => {
            // v0: the shard's own advertisement, after it caught up to the leader's refs.
            let url = format!("https://shard/info-refs?repo={}", repo.id.0.0);
            let mut r = shard_read(c, repo, &url, Method::Get, proto.as_deref().unwrap_or(""), None).await?;
            let b = r.bytes().await?;
            Ok(Response::from_bytes(b)?.with_headers(git_headers(c.rid, &service.advertisement_type())?))
        }
        Service::ReceivePack => {
            if !signed_in {
                return unauthorized(c.rid, "No anonymous write access.");
            }
            let caps = receive_pack_caps(AGENT, &graftyard_wire::request_id_header(c.rid));
            match receive_pack_stream(c, repo, caps).await? {
                Ok(r) => Ok(r.with_headers(git_headers(c.rid, &service.advertisement_type())?)),
                Err(r) => Ok(r),
            }
        }
    }
}

// ---- the test driver's routes (test installs only) ----

/// A repository by id, as an LFS transfer path and the test driver name it: its owner from `RepoOwner` on `Names`,
/// a fork's network beside it, and the owner's record.
pub(crate) async fn repo_by_id(c: &Ctx<'_>, id: RepoId) -> std::result::Result<Repo, String> {
    // `repoNetwork`: a fork's network from its `RepoOwner` row (Names 0x10.4); a member before it answers no key.
    let q = json!({ "repoOwner": id.0.0, "repoNetwork": id.0.0 });
    let v = system_read(c, ChainId::Names(name_shard(&NameKey::Repo(id))), q).await?;
    let owner: AccountId = serde_json::from_value::<Option<AccountId>>(v["repoOwner"].clone())
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("{id:?} has no RepoOwner row"))?;
    let record = placement(c, owner).await?.ok_or_else(|| format!("{owner:?} has no placement record"))?;
    let repo = Repo::new(id, owner, record);
    if let Ok(Some(n)) = serde_json::from_value::<Option<NetworkId>>(v["repoNetwork"].clone()) {
        repo.fix(Some(n));
    }
    Ok(repo)
}

/// Sends `path` to every member the caller knows for the repository's chain: the config's voters, else the record's
/// bootstrap names. Each answer is the member's site, its object name, and JSON.
#[cfg(feature = "test-faults")]
async fn each_member(c: &Ctx<'_>, repo: &Repo, path: &str) -> Vec<(u8, String, serde_json::Value)> {
    let chain = ChainId::Repo(repo.id);
    let mut vs = graftyard_lookup::known_voters(chain);
    if vs.is_empty() {
        vs = repo
            .record
            .bootstrap_members(chain)
            .into_iter()
            .map(|m| graftyard_lookup::Target { site: m.site, do_name: m.do_name.into() })
            .collect();
    }
    let mut out = Vec::new();
    for v in vs {
        let Ok(stub) = c.bindings().member(v.site, &v.do_name) else { continue };
        if let Ok(mut r) = stub.fetch_with_str(&format!("https://member{path}")).await
            && let Ok(j) = r.json::<serde_json::Value>().await
        {
            out.push((v.site.0, v.do_name.to_string(), j));
        }
    }
    out
}

/// BF-18's archive of `from..=to` on the repository's group, as the design orders it (`design/DESIGN.md`, Shape,
/// "Packs"): the home shard packs the objects of the range's inline packs into one complete pack, which is validated
/// on the shard and placed as a stored pack; the range's stored packs' markers are re-put; the leader records
/// `Promoted` and the promoted pack's marker is put; the range's entries go into an `Archive` object, put create-only
/// in two buckets; and the leader records `Archived`, after which each member truncates its stored log through `to`.
/// A test-installs route: the design has the leader run this, with no trigger yet.
#[cfg(feature = "test-faults")]
async fn bed_archive(c: &Ctx<'_>, repo: &Repo, from: u64, to: u64) -> Result<serde_json::Value> {
    use sha2::Digest as _;
    let chain = ChainId::Repo(repo.id);
    let net = repo.network();
    let fail = |e: String| Error::RustError(e);
    // The range's entries from the leader, every page.
    let mut entries: Vec<serde_json::Value> = Vec::new();
    let mut at = from;
    while at <= to {
        let (v, _) = graftyard_lookup::call(
            &c.bindings(),
            chain,
            &repo.record,
            &format!("/entries?from={at}&to={to}"),
            &json!({}),
        )
        .await
        .map_err(fail)?;
        let page: Vec<serde_json::Value> = serde_json::from_value(v["entries"].clone())?;
        let Some(last) = page.last().and_then(|e| e["index"].as_u64()) else { break };
        entries.extend(page);
        at = last + 1;
    }
    if entries.len() as u64 != to + 1 - from {
        return Err(fail(format!("entries {from}..={to}: the leader answered {} (commit below {to}?)", entries.len())));
    }
    let logs: Vec<graftyard_git::LogPacks> = entries
        .iter()
        .map(|e| {
            Ok(graftyard_git::LogPacks {
                index: graftyard_types::Index(e["index"].as_u64().unwrap_or(0)),
                packs: serde_json::from_value(e["packs"].clone())?,
                promoted: None,
                compacted: None,
                consumed: Vec::new(),
                refs: None,
            })
        })
        .collect::<Result<_>>()?;
    let (inline, stored) =
        graftyard_git::promotion_inputs(&logs, graftyard_types::Index(from), graftyard_types::Index(to));
    // The range's stored packs' objects go into the promotion too. Pushed packs are thin: a stored pack in the range
    // can have its delta bases only in the range's inline packs, while a later inline pack's trees name its objects,
    // so a promotion of the inline packs alone and that stored pack each need the other, and a rehydrate's
    // `index-pack --strict` refuses both (BF-18 on the hosted install, `tests/m1/results/m1-wp5.md`). The home shard
    // ingested each stored pack under its checksum, so it can name them.
    let sums: Vec<String> = inline
        .iter()
        .map(|(_, b)| Digest(sha2::Sha256::digest(b).into()).hex())
        .chain(stored.iter().filter_map(|p| match p {
            graftyard_git::PackRef::Stored { checksum, .. } => Some(checksum.hex()),
            graftyard_git::PackRef::Inline(_) => None,
        }))
        .collect();
    // The home shard's promotion, then its validation of the promoted pack.
    let call_shard = |path: String, body: Vec<u8>| async move {
        let mut init = RequestInit::new();
        init.with_method(Method::Post).with_body(Some(js_sys::Uint8Array::from(body.as_slice()).into()));
        let mut r =
            shard(c, repo)?.fetch_with_request(Request::new_with_init(&format!("https://shard{path}"), &init)?).await?;
        if r.status_code() != 200 {
            return Err(fail(format!("shard {path}: {} {}", r.status_code(), r.text().await.unwrap_or_default())));
        }
        r.bytes().await
    };
    let promoted = call_shard(format!("/promote?repo={}", repo.id.0.0), serde_json::to_vec(&sums)?).await?;
    let ix: PackIndex = serde_json::from_slice(
        &call_shard(format!("/validate?repo={}&tipsn=0", repo.id.0.0), promoted.clone()).await?,
    )?;
    let raw = buckets(c.env, c.sites)?;
    let refs: Vec<(SiteId, &R2)> =
        raw.iter().filter(|(s, _)| repo.record.sites.contains(s)).map(|(s, b)| (*s, b)).collect();
    let store = PackStore { sites: c.sites, buckets: &refs, copies: Copies::TwoRegions };
    let place = graftyard_git::Place {
        network: net,
        id: PackId(Ulid::new(now(), rand10())),
        meta: ObjectMeta { chain, request: c.rid },
        bytes: &promoted,
        index: &ix,
        idx: None,
        idx_range: None,
        inline_max: 0,
    };
    let pack = store.place(place).await.map_err(|e| fail(e.to_string()))?.into_pack();
    // Markers complete below the promotion: the range's stored packs' markers again.
    let mut remarked = 0;
    for e in &logs {
        for p in e.packs.iter().filter(|p| p.stored_id().is_some()) {
            graftyard_git::mark_committed(&refs, net, p, chain, e.index).await;
            remarked += 1;
        }
    }
    let record = |events: serde_json::Value| async move {
        let rid = graftyard_lookup::request_id();
        let body = json!({ "rid": rid.to_string(), "events": events });
        let (v, _) =
            graftyard_lookup::call(&c.bindings(), chain, &repo.record, "/record", &body).await.map_err(fail)?;
        v["recorded"].as_u64().ok_or_else(|| fail(format!("record: {v}")))
    };
    let p = graftyard_git::Promoted {
        from: graftyard_types::Index(from),
        to: graftyard_types::Index(to),
        pack: pack.clone(),
    };
    let pi = record(json!([{ "Promoted": p }])).await?;
    let marker_failures = graftyard_git::mark_committed(&refs, net, &pack, chain, graftyard_types::Index(pi)).await;
    // The archive object: the leader puts the range's stored log rows, in the raft codec, in two buckets, keyed by
    // range and digest (`workers/site/src/load.rs`, `archive_put`), so a member that installs a snapshot applies the
    // entries as they were committed.
    let (v, _) = graftyard_lookup::call(
        &c.bindings(),
        chain,
        &repo.record,
        "/bench_seed",
        &json!({ "archive": { "from": from, "to": to } }),
    )
    .await
    .map_err(fail)?;
    let key: graftyard_git::R2Key = v["key"]
        .as_str()
        .ok_or_else(|| fail(format!("archive put: {v}")))?
        .parse()
        .map_err(|e| fail(format!("{e}")))?;
    let graftyard_git::R2Key::Archive { digest, .. } = key else { return Err(fail(format!("archive put: {v}"))) };
    let put = v["sites"].clone();
    let a = graftyard_git::Archived {
        key: key.clone(),
        from: graftyard_types::Index(from),
        to: graftyard_types::Index(to),
        from_at: Timestamp(v["fromAt"].as_u64().unwrap_or(0)),
        to_at: Timestamp(v["toAt"].as_u64().unwrap_or(0)),
        digest,
    };
    let ai = record(json!([{ "Archived": a }])).await?;
    Ok(json!({
        "inline_packs": inline.len(),
        "stored_in_range": stored.len(),
        "remarked": remarked,
        "promoted": { "id": pack.stored_id().map(|i| i.to_string()), "bytes": promoted.len(), "objects": ix.objects, "index": pi, "marker_failures": marker_failures },
        "archived_index": ai,
        "archive_key": key.to_string(),
        "archive_buckets": put,
    }))
}

/// BF-19's placement: `n` random bytes as a stored pack of the repository's network, placed through
/// `PackStore::place_teed` with the shard's validation stood in by the bytes' own index (they are no git pack, and
/// nothing reads them as one). With `fail`, only the record's home bucket takes the put and the others refuse it, so
/// the two-region rule fails, as a placement whose other puts failed; the one copy stays behind for the sweeper.
#[cfg(feature = "test-faults")]
async fn bed_place(
    c: &Ctx<'_>,
    repo: &Repo,
    n: usize,
    fail: bool,
) -> Result<std::result::Result<graftyard_git::PackRef, serde_json::Value>> {
    let mut bytes = vec![0u8; n];
    let crypto: web_sys::Crypto =
        js_sys::Reflect::get(&js_sys::global(), &"crypto".into()).map_err(|_| "crypto")?.into();
    for chunk in bytes.chunks_mut(65_536) {
        crypto.get_random_values_with_u8_array(chunk).map_err(|_| "random")?;
    }
    bed_place_bytes(c, repo, bytes, fail).await
}

/// `bytes` placed as a stored pack of the repository's network, as `bed_place` places its random ones: lifeboatbed's
/// real git packs (check 8), which the lifeboat's `graft-git` indexes.
#[cfg(feature = "test-faults")]
async fn bed_place_bytes(
    c: &Ctx<'_>,
    repo: &Repo,
    bytes: Vec<u8>,
    fail: bool,
) -> Result<std::result::Result<graftyard_git::PackRef, serde_json::Value>> {
    use sha2::Digest as _;
    let n = bytes.len();
    let raw = buckets(c.env, c.sites)?;
    let refs: Vec<(SiteId, &R2)> = raw
        .iter()
        .filter(|(s, _)| repo.record.sites.contains(s) && (!fail || *s == repo.record.home))
        .map(|(s, b)| (*s, b))
        .collect();
    let store = PackStore { sites: c.sites, buckets: &refs, copies: Copies::TwoRegions };
    let checksum = Digest(sha2::Sha256::digest(&bytes).into());
    let ix = PackIndex { checksum, bytes: n as u64, objects: 0, trailer: "".into() };
    let stage = Stage {
        network: repo.network(),
        id: PackId(Ulid::new(now(), rand10())),
        meta: ObjectMeta { chain: ChainId::Repo(repo.id), request: c.rid },
        bytes: &bytes,
        inline: false,
        part: c.cfg.part,
    };
    let id = stage.id;
    Ok(match store.place_teed(stage, async { Ok::<_, String>(ix) }).await {
        Ok((d, _)) => Ok(d.into_pack()),
        Err(e) => Err(json!({ "id": id.to_string(), "error": e.to_string() })),
    })
}

/// `/_bed/...` behind the bed token, the routes `tests/m1/wp6/bf16.sh` and `bf17.sh` read, now answered from the
/// repository's real group and its home site's shard: `log` (the leader's stored log with each entry's packs and ref
/// updates), `status` (the members' receive counters, summed), `refs`, `markers` (each stored pack's object and
/// `Committed` marker in every bucket, for the entries from `index`), and the shard's `fsck`, `kill`, `teardown`,
/// and `catchup`.
#[cfg(feature = "test-faults")]
async fn bed(c: &Ctx<'_>, req: &mut Request, path: &str) -> Result<Response> {
    let token = c.env.secret("BED_TOKEN")?.to_string();
    if req.headers().get("x-bed-token")?.as_deref() != Some(token.as_str()) {
        return text(c.rid, 404, "not found\n");
    }
    let url = req.url()?;
    let qp = |k: &str| url.query_pairs().find(|(n, _)| n == k).map(|(_, v)| v.into_owned());
    // BM-1's site fault: `?site=N&after_ms=A&for_ms=F[&rate=R]` makes site N's member class fail every call (or, with
    // `rate`, that share of calls) from A ms from now for F ms (default: until cleared); `?off=1` clears it. Written to every site's `Registry` object `faults`, which
    // each member isolate rereads at most once a second (`workers/site/src/faults.rs`, `SiteFault`).
    // actions-artifacts: stored gzip bytes leave the edge as stored (tests/edgeboot).
    if path == "/_bed/gzip" || path == "/_bed/artifacts" {
        return artifacts::bed(c, req).await;
    }
    if path == "/_bed/fault" {
        let now_ms = now().0;
        let plan = if qp("off").is_some() {
            json!({ "site": null, "from": 0, "until": 0 })
        } else {
            let site: u8 = qp("site").and_then(|v| v.parse().ok()).ok_or("site")?;
            let from = now_ms + qp("after_ms").and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
            // "Until cleared" is 30 days, so the bound stays an exact JSON number.
            let for_ms = qp("for_ms").and_then(|v| v.parse::<u64>().ok()).unwrap_or(30 * 86_400_000);
            let until = from + for_ms;
            let rate = qp("rate").and_then(|v| v.parse::<f64>().ok());
            // M5-WP2: `chains=release,repo:7` fails only those chains' members; `group_only=1` only their
            // member-to-member Raft calls, so the group cannot commit and members still answer `/read` and `/view`.
            let chains: Vec<String> =
                qp("chains").map(|c| c.split(',').map(str::to_owned).collect()).unwrap_or_default();
            let group_only = qp("group_only").is_some();
            // `hang_ms=H`: a hit call hangs H ms before it fails, a blackhole rather than a refusal (B0-07's gray site).
            let hang_ms = qp("hang_ms").and_then(|v| v.parse::<u64>().ok());
            json!({ "site": site, "from": from, "until": until, "rate": rate, "chains": chains,
                    "group_only": group_only, "hang_ms": hang_ms })
        };
        let mut set = Vec::new();
        for s in &c.sites.0 {
            let mut init = RequestInit::new();
            init.with_method(Method::Put).with_body(Some(plan.to_string().into()));
            let r = async {
                let stub = c.bindings().registry(s.id, "faults")?;
                let mut res =
                    stub.fetch_with_request(Request::new_with_init("https://registry/site_fault", &init)?).await?;
                res.json::<serde_json::Value>().await
            }
            .await;
            set.push(json!({ "site": s.id.0, "plan": r.map_err(|e| e.to_string()) .unwrap_or_else(|e| json!({ "error": e })) }));
        }
        return Response::from_json(&json!({ "now": now_ms, "plan": plan, "sites": set }));
    }
    // The change feed's hold (`workers/site/src/export_feed.rs`, `HOLD_SCHEMA`): `?hold=1` stops every round on the
    // feed site (the install's first), `?hold=0` lets them go; with neither, the state. BF-53 holds it while a
    // repository archives, so the lifeboat first copies that chain from the archive (`decisions/OPEN-QUESTIONS.md`,
    // R59).
    if path == "/_bed/feed" {
        let feed = c.sites.0.iter().map(|s| s.id).min().ok_or("no site")?;
        let q = qp("hold").map(|h| format!("?hold={h}")).unwrap_or_default();
        let mut init = RequestInit::new();
        init.with_method(Method::Post);
        let stub = c.bindings().registry(feed, "export")?;
        return stub
            .fetch_with_request(Request::new_with_init(&format!("https://registry/export_hold{q}"), &init)?)
            .await;
    }
    // The sweeper cron's cursor and last report on site `?site=N` (`workers/site/src/sweep.rs`); with `&now=MS`, one
    // run of the site's sweeper with its clock at MS (BF-19), in that site's `Registry` object `sweep`.
    if path == "/_bed/sweep" {
        let site: u8 = qp("site").and_then(|v| v.parse().ok()).ok_or("site")?;
        let stub = c.bindings().registry(SiteId(site), "sweep")?;
        return match qp("now") {
            Some(n) => {
                let mut init = RequestInit::new();
                init.with_method(Method::Post);
                stub.fetch_with_request(Request::new_with_init(&format!("https://registry/sweep_run?now={n}"), &init)?)
                    .await
            }
            None => stub.fetch_with_str("https://registry/sweep").await,
        };
    }
    // BF-14 on identity commands: `{"chain": "names:N", "cmd": {...}, "receipt"?}` committed through `commit_hedged`
    // on a system chain, the receipt fault on the first send, with the edge's trace; and every member's arrivals.
    if path == "/_bed/identity" || path == "/_bed/identity_arrivals" {
        let b: serde_json::Value = if path == "/_bed/identity" { req.json().await? } else { json!({}) };
        let chain_s = b["chain"].as_str().map(str::to_owned).or_else(|| qp("chain")).ok_or("chain")?;
        let chain: ChainId = chain_s.parse().map_err(|_| "chain")?;
        // An owner chain (`authz:`, `identity:`) under its owner's `Placement` record, created on first use.
        let record = match chain {
            ChainId::Authz(o) | ChainId::Identity(o) => {
                let r = placement(c, o).await.map_err(Error::RustError)?.ok_or("the owner has no Placement row")?;
                graftyard_lookup::create(&c.bindings(), chain, &r, &json!({})).await.map_err(Error::RustError)?;
                r
            }
            // A repository chain, read only (BF-30's `{"settings": true}`), under its `RepoOwner`'s record, or with
            // `"record_of": owner` under that owner's; `"cold": true` drops the isolate's cached configs first, so the
            // read finds the group from the record alone, as a new edge isolate would.
            ChainId::Repo(id) if b.get("read").is_some() => {
                if b["cold"].as_bool() == Some(true) {
                    graftyard_lookup::forget_all();
                }
                match serde_json::from_value::<Option<AccountId>>(b["record_of"].clone()).ok().flatten() {
                    Some(o) => {
                        placement(c, o).await.map_err(Error::RustError)?.ok_or("that owner has no Placement row")?
                    }
                    None => repo_by_id(c, id).await.map_err(Error::RustError)?.record,
                }
            }
            // A system chain; `"create": true` creates its group first, as `commit_system` does (an install with a real
            // genesis creates only its own shards: `tests/lifeboatbed/seed.mjs`).
            _ => {
                let r = c.system_record(chain).await.map_err(Error::RustError)?;
                if b["create"].as_bool() == Some(true) {
                    graftyard_lookup::create(&c.bindings(), chain, &r, &json!({})).await.map_err(Error::RustError)?;
                }
                r
            }
        };
        if path == "/_bed/identity_arrivals" {
            let since = qp("since").unwrap_or_else(|| "0".into());
            let mut out = Vec::new();
            // The config's voters this isolate learned, and the record's bootstrap names, once each.
            let mut names: Vec<(SiteId, String)> =
                graftyard_lookup::known_voters(chain).into_iter().map(|t| (t.site, t.do_name)).collect();
            for m in record.bootstrap_members(chain) {
                if !names.iter().any(|(_, n)| **n == *m.do_name) {
                    names.push((m.site, m.do_name.to_string()));
                }
            }
            for (site, name) in names {
                let stub = c.bindings().member(site, &name)?;
                let mut r = stub.fetch_with_str(&format!("https://member/arrivals?since={since}")).await?;
                out.push(json!({ "site": site.0, "name": name, "rows": r.json::<serde_json::Value>().await? }));
            }
            return Response::from_json(&out);
        }
        // `{"chain", "read": q}`: the chain's fresh read, so a gate can see an `Identity` chain's credential list. With
        // `"config": true`, also the answering leader's committed config (BF-30: where a moving group's voters are).
        if let Some(q) = b.get("read") {
            let body = json!({ "q": q }).to_string();
            let v = graftyard_lookup::call_opt(&c.bindings(), chain, &record, "/read", body.as_bytes())
                .await
                .map_err(Error::RustError)?;
            let mut config = serde_json::Value::Null;
            if let (Some((_, _, t)), true) = (&v, b["config"].as_bool() == Some(true)) {
                let mut r = c.bindings().member(t.site, &t.do_name)?.fetch_with_str("https://member/status").await?;
                config = r.json::<serde_json::Value>().await?.pointer("/config").cloned().unwrap_or_default();
            }
            return Response::from_json(&json!({ "value": v.map(|(v, _, _)| v["value"].clone()), "config": config }));
        }
        let body = json!({ "rid": c.rid.to_string(), "cmd": b["cmd"] });
        // The first send may carry `receipt` (BF-14) or `kill` with `down_ms` (BF-33 on a code shard's leader).
        let mut first: Option<serde_json::Value> = None;
        for k in ["kill", "receipt"] {
            if let Some(v) = b.get(k).and_then(serde_json::Value::as_str) {
                let f = first.get_or_insert_with(|| body.clone());
                f[k] = json!(v);
                f["down_ms"] = b.get("down_ms").cloned().unwrap_or(json!(0));
            }
        }
        let (bytes, first) = (body.to_string().into_bytes(), first.map(|f| f.to_string().into_bytes()));
        let trace = std::cell::RefCell::new(Vec::new());
        let t0 = now().0;
        let r = graftyard_lookup::commit_hedged(
            &c.bindings(),
            chain,
            &record,
            &bytes,
            c.colo,
            60_000,
            graftyard_lookup::Hooks { first: first.as_deref(), trace: Some(&trace), lookup: None, lean: false },
        )
        .await;
        return Response::from_json(&json!({
            "rid": c.rid.to_string(),
            "ms": now().0 - t0,
            "outcome": r.as_ref().ok(),
            "error": r.err().map(|e| format!("{e:?}")),
            "trace": trace.into_inner(),
        }));
    }
    // A site's pool ledger as stored (`GET /pool` on `git#<site>#pool:1`, `graftyard_git::PoolLedger`). The read
    // runs nothing on the ledger, so a lease missing from it was dropped by a call or by the ledger's own heartbeat.
    if path == "/_bed/pool" {
        let site = qp("site").and_then(|v| v.parse::<u8>().ok()).map(SiteId).ok_or("site")?;
        let region = c.sites.get(site).ok_or("unknown site")?.region;
        return c
            .env
            .durable_object(&format!("SHARD_{}", site.0))?
            .get_by_name_with_location_hint(&format!("git#{}#pool:1", site.0), region.hint())?
            .fetch_with_str("https://shard/pool")
            .await;
    }
    // A hot-pool shard, `git#<site>#pool:<n>`, belongs to no one network, so `?site=S&pool=N` names it directly; it
    // is torn down like a network's shard (`tests/fanout/teardown.py --pools`).
    if path == "/_bed/kill_pool" {
        let site = qp("site").and_then(|v| v.parse::<u8>().ok()).map(SiteId).ok_or("site")?;
        let pool = qp("pool").and_then(|v| v.parse::<u32>().ok()).ok_or("pool")?;
        let region = c.sites.get(site).ok_or("unknown site")?.region;
        let mut init = RequestInit::new();
        init.with_method(Method::Post);
        return c
            .env
            .durable_object(&format!("SHARD_{}", site.0))?
            .get_by_name_with_location_hint(&format!("git#{}#pool:{pool}", site.0), region.hint())?
            .fetch_with_request(Request::new_with_init("https://shard/teardown", &init)?)
            .await;
    }
    // A cold catch-up of `repo` on a hot-pool shard (`/catchup?warm=pool`, which starts its container), so two
    // containers on one site rehydrate the same packs at once: their spool rates tell a per-container R2 ceiling from
    // a per-site or per-bucket one (LM-1 round 27b). Torn down with `/_bed/kill_pool`.
    if path == "/_bed/catchup_pool" {
        let site = qp("site").and_then(|v| v.parse::<u8>().ok()).map(SiteId).ok_or("site")?;
        let pool = qp("pool").and_then(|v| v.parse::<u32>().ok()).ok_or("pool")?;
        let repo = qp("repo").and_then(|v| v.parse::<i64>().ok()).ok_or("repo")?;
        let wait = qp("wait").and_then(|v| v.parse::<u64>().ok()).unwrap_or(60);
        let region = c.sites.get(site).ok_or("unknown site")?.region;
        let mut init = RequestInit::new();
        init.with_method(Method::Post);
        return c
            .env
            .durable_object(&format!("SHARD_{}", site.0))?
            .get_by_name_with_location_hint(&format!("git#{}#pool:{pool}", site.0), region.hint())?
            .fetch_with_request(Request::new_with_init(
                &format!("https://shard/catchup?warm=pool&repo={repo}&wait={wait}"),
                &init,
            )?)
            .await;
    }
    let id = qp("repo").and_then(|v| v.parse::<i64>().ok()).map(|r| RepoId(DbId(r))).ok_or("repo")?;
    let repo = repo_by_id(c, id).await.map_err(Error::RustError)?;
    let chain = ChainId::Repo(repo.id);
    let record = repo.record.clone();
    let leader = |path: String| {
        let record = record.clone();
        async move {
            let b = c.bindings();
            graftyard_lookup::call(&b, chain, &record, &path, &json!({}))
                .await
                .map(|(v, _)| v)
                .map_err(Error::RustError)
        }
    };
    match path {
        "/_bed/log" => {
            let from = qp("from").unwrap_or_else(|| "1".into());
            let stub = {
                refs_page(c, &repo, b"", None).await?.map_err(|e| Error::RustError(format!("{e:?}")))?;
                let lead = each_member(c, &repo, "/status").await.into_iter().find(|(_, _, s)| s["role"] == "leader");
                let (site, name, _) = lead.ok_or("no leader")?;
                c.bindings().member(SiteId(site), &name)?
            };
            stub.fetch_with_str(&format!("https://member/log?from={from}&n=20000")).await
        }
        "/_bed/status" => {
            let mut received: std::collections::BTreeMap<String, u64> = std::collections::BTreeMap::new();
            for (_, _, s) in each_member(c, &repo, "/stats").await {
                if let Some(m) = s["received"].as_object() {
                    for (k, v) in m {
                        *received.entry(k.clone()).or_insert(0) += v.as_u64().unwrap_or(0);
                    }
                }
            }
            Response::from_json(&json!({ "received": received }))
        }
        "/_bed/refs" => {
            let refs = read_refs(c, &repo, b"").await?.map_err(|e| Error::RustError(format!("{e:?}")))?.1;
            Response::from_json(&refs)
        }
        "/_bed/markers" => {
            let from = qp("index").and_then(|v| v.parse::<u64>().ok()).unwrap_or(1);
            let v = leader(format!("/entries?from={from}")).await?;
            let entries: Vec<serde_json::Value> = serde_json::from_value(v["entries"].clone())?;
            let raw = buckets(c.env, c.sites)?;
            let mut out = Vec::new();
            for e in entries {
                let packs: Vec<graftyard_git::PackRef> = serde_json::from_value(e["packs"].clone())?;
                for id in packs.iter().filter_map(graftyard_git::PackRef::stored_id) {
                    let pack = graftyard_git::R2Key::Pack { network: repo.network(), id };
                    let marker = graftyard_git::R2Key::committed(pack.clone()).ok_or("marker")?.to_string();
                    let mut per = Vec::new();
                    for (site, b) in &raw {
                        let se = |e: graftyard_git::StoreError| Error::RustError(e.0);
                        let has_pack = graftyard_git::Bucket::head(b, &pack.to_string()).await.map_err(se)?.is_some();
                        let has_marker = graftyard_git::Bucket::head(b, &marker).await.map_err(se)?.is_some();
                        per.push(json!({ "site": site.0, "pack": has_pack, "marker": has_marker }));
                    }
                    out.push(json!({ "id": id.to_string(), "index": e["index"], "stored": true, "buckets": per }));
                }
            }
            Response::from_json(&out)
        }
        // Every member's `fault` rows for a request id, and every member's BF-12 audit (request ids at two indexes,
        // `dup` rows written by apply).
        "/_bed/faults" | "/_bed/audit" => {
            let inner = match path {
                "/_bed/faults" => format!("/faults?rid={}", qp("rid").unwrap_or_default()),
                _ => "/audit?prefix=".into(),
            };
            let rows: Vec<serde_json::Value> =
                each_member(c, &repo, &inner).await.into_iter().map(|(s, _, v)| json!({ "site": s, "r": v })).collect();
            Response::from_json(&rows)
        }
        // BF-12, BF-14, and BF-19 on pushes: one `Push` of a ref update (`{ref, expect, new}`, object ids as hex), under
        // the caller's token, through the edge's commit path (`commit_hedged`). The first send carries `kill` or
        // `receipt` (BF-14's missed receipt); `no_marker` makes the deciding leader skip its marker puts; `stored`
        // (bytes) places that many random bytes as a stored pack first, through `PackStore::place_teed` with no
        // validation, refused by every bucket but the home's with `fail_place`, which then stops before the commit.
        // The answer carries the edge's trace of sends, misses, `suspect` answers, and `NotLeader` redirects.
        "/_bed/push" => {
            let Some(actor) = authenticate(c, req).await? else { return unauthorized(c.rid, "Bad credentials") };
            let b: serde_json::Value = req.json().await?;
            let oid = |k: &str| b[k].as_str().and_then(|h| ObjectId::parse_hex(h.as_bytes()));
            let name = RefName::parse(b["ref"].as_str().unwrap_or_default().as_bytes()).map_err(|e| e.to_string())?;
            let mut push = Push {
                updates: vec![graftyard_git::refs::RefUpdate { name, expect: oid("expect"), new: oid("new") }],
                atomicity: Atomicity::PerRef,
                pack: None,
                actor,
                options: Vec::new(),
                request: c.rid,
                agent: None,
                provenance: Vec::new(),
                protected: Vec::new(),
                changed: Vec::new(),
                leases: false,
                test_format: false,
            };
            let t0 = now().0;
            // lifeboatbed's check 8: a real git pack (`pack`, base64), inline or placed as a stored pack.
            if let Some(p) = b.get("pack").and_then(serde_json::Value::as_str) {
                let bytes = graftyard_git::pack::b64::decode(p).ok_or("pack is not base64")?;
                if b.get("inline").and_then(serde_json::Value::as_bool) == Some(true) {
                    push.pack = Some(graftyard_git::PackRef::Inline(bytes));
                } else {
                    match bed_place_bytes(c, &repo, bytes, false).await? {
                        Ok(p) => push.pack = Some(p),
                        Err(e) => return Response::from_json(&json!({ "placed": e })),
                    }
                }
            }
            if let Some(n) = b.get("stored").and_then(serde_json::Value::as_u64) {
                let fail = b.get("fail_place").and_then(serde_json::Value::as_bool).unwrap_or(false);
                match bed_place(c, &repo, n as usize, fail).await? {
                    Ok(p) => push.pack = Some(p),
                    Err(e) => {
                        return Response::from_json(
                            &json!({ "rid": c.rid.to_string(), "ms": now().0 - t0, "placed": e }),
                        );
                    }
                }
            }
            let mut body = serde_json::to_value(CommitReq { rid: c.rid.to_string(), cmd: push.clone() })?;
            if b.get("no_marker").and_then(serde_json::Value::as_bool) == Some(true) {
                body["no_marker"] = json!(true);
            }
            let mut first: Option<serde_json::Value> = None;
            for k in ["kill", "receipt"] {
                if let Some(v) = b.get(k).and_then(serde_json::Value::as_str) {
                    let f = first.get_or_insert_with(|| body.clone());
                    f[k] = json!(v);
                    f["down_ms"] = b.get("down_ms").cloned().unwrap_or(json!(0));
                }
            }
            let trace = std::cell::RefCell::new(Vec::new());
            let r = commit(c, &repo, &body, first.as_ref(), Some(&trace), None).await;
            Response::from_json(&json!({
                "rid": c.rid.to_string(),
                "ms": now().0 - t0,
                "pack": push.pack,
                "outcome": r.as_ref().ok().map(|o| serde_json::to_value(o).unwrap_or_default()),
                "error": r.err().map(|e| e.to_string()),
                "trace": trace.into_inner(),
            }))
        }
        // BF-19: for each pack id of `{"ids": [...]}` (at most 500 a call), whether each bucket holds the pack and its
        // `Committed` marker.
        "/_bed/heads" => {
            let b: serde_json::Value = req.json().await?;
            let ids: Vec<String> = serde_json::from_value(b["ids"].clone())?;
            if ids.len() > 500 {
                return text(c.rid, 400, "at most 500 ids\n");
            }
            let raw = buckets(c.env, c.sites)?;
            let net = repo.network();
            // Every id's six HEADs at once, 25 ids at a time (the HEADs of one id run serially in 1.1 s).
            let one = |id: String| {
                let raw = &raw;
                async move {
                    let Ok(pid) = id.parse::<Ulid>().map(PackId) else { return Ok(None) };
                    let pack = graftyard_git::R2Key::Pack { network: net, id: pid };
                    let marker = graftyard_git::R2Key::committed(pack.clone()).ok_or("marker")?.to_string();
                    let pack = pack.to_string();
                    let se = |e: graftyard_git::StoreError| Error::RustError(e.0);
                    let per = futures_util::future::join_all(raw.iter().map(|(site, bk)| {
                        let (pack, marker) = (&pack, &marker);
                        async move {
                            let (p, m) = futures_util::future::join(
                                graftyard_git::Bucket::head(bk, pack),
                                graftyard_git::Bucket::head(bk, marker),
                            )
                            .await;
                            Ok::<_, Error>(json!({
                                "site": site.0, "pack": p.map_err(se)?.is_some(), "marker": m.map_err(se)?.is_some()
                            }))
                        }
                    }))
                    .await
                    .into_iter()
                    .collect::<Result<Vec<_>>>()?;
                    Ok::<_, Error>(Some(json!({ "id": id, "buckets": per })))
                }
            };
            let mut out = Vec::new();
            let mut ids = ids.into_iter();
            loop {
                let batch: Vec<String> = ids.by_ref().take(25).collect();
                if batch.is_empty() {
                    break;
                }
                for r in futures_util::future::join_all(batch.into_iter().map(one)).await {
                    out.extend(r?);
                }
            }
            Response::from_json(&out)
        }
        // Every member's BF-14 arrivals since `?since=` ms.
        "/_bed/arrivals" => {
            let since = qp("since").unwrap_or_else(|| "0".into());
            let rows: Vec<serde_json::Value> = each_member(c, &repo, &format!("/arrivals?since={since}"))
                .await
                .into_iter()
                .map(|(s, n, v)| json!({ "site": s, "name": n, "rows": v }))
                .collect();
            Response::from_json(&rows)
        }
        // lifeboatbed's check 8, a roll on a real pack history: `place` puts `{"pack": base64}` as a stored pack and
        // answers its `PackRef`; `record` has the leader record `{"events", "format"}` (a `Promoted` or `Compacted` at
        // 32.25) and marks each stored pack it names at the entry, as `bed_archive` marks a promotion; `drop_pack`
        // deletes `?id=`'s pack and `.idx` from every bucket, as the sweeper deletes a roll's consumed packs.
        "/_bed/place" => {
            let b: serde_json::Value = req.json().await?;
            let bytes = graftyard_git::pack::b64::decode(b["pack"].as_str().unwrap_or_default()).ok_or("pack")?;
            Response::from_json(&match bed_place_bytes(c, &repo, bytes, false).await? {
                Ok(p) => json!({ "pack": p }),
                Err(e) => json!({ "placed": e }),
            })
        }
        "/_bed/record" => {
            let b: serde_json::Value = req.json().await?;
            let body = json!({ "rid": graftyard_lookup::request_id().to_string(), "events": b["events"],
                "format": b["format"] });
            let (v, _) = graftyard_lookup::call(&c.bindings(), chain, &repo.record, "/record", &body)
                .await
                .map_err(Error::RustError)?;
            if let Some(at) = v["recorded"].as_u64() {
                let raw = buckets(c.env, c.sites)?;
                let refs: Vec<(SiteId, &R2)> =
                    raw.iter().filter(|(s, _)| repo.record.sites.contains(s)).map(|(s, b)| (*s, b)).collect();
                let events: Vec<graftyard_git::repo::RepoEvent> = serde_json::from_value(b["events"].clone())?;
                for p in graftyard_git::repo::stored_packs(&events) {
                    let pack = graftyard_git::PackRef::Stored {
                        id: p.0,
                        checksum: p.1,
                        bytes: p.2,
                        sites: graftyard_types::SiteSet(refs.iter().fold(0, |m, (s, _)| m | 1 << s.0)),
                    };
                    graftyard_git::mark_committed(&refs, repo.network(), &pack, chain, graftyard_types::Index(at))
                        .await;
                }
            }
            Response::from_json(&v)
        }
        "/_bed/drop_pack" => {
            let id = qp("id").and_then(|v| v.parse::<Ulid>().ok()).map(PackId).ok_or("id")?;
            let net = repo.network();
            let keys = vec![
                graftyard_git::R2Key::Pack { network: net, id }.to_string(),
                graftyard_git::R2Key::Idx { network: net, id }.to_string(),
            ];
            for (_, b) in &buckets(c.env, c.sites)? {
                graftyard_git::Bucket::delete(b, &keys).await.map_err(|e| Error::RustError(e.0))?;
            }
            Response::from_json(&json!({ "dropped": keys }))
        }
        // BF-18: archive `?from=F&to=T` on the repository's real group (`bed_archive`).
        "/_bed/archive" => {
            let num = |k: &str| qp(k).and_then(|v| v.parse::<u64>().ok()).ok_or_else(|| Error::RustError(k.into()));
            let (from, to) = (num("from")?, num("to")?);
            Response::from_json(&bed_archive(c, &repo, from, to).await?)
        }
        // The home shard's routes, or with `?shard=N` site N's shard of the same network (BF-18's second shard,
        // behind its own `IngestCursor`).
        "/_bed/fsck" | "/_bed/kill" | "/_bed/teardown" | "/_bed/catchup" | "/_bed/shard_status"
        | "/_bed/shard_abort" => {
            let inner = match path {
                "/_bed/kill" => "/teardown",
                // The shard object resets; its container and disk stay (`workers/site/src/shard.rs`, `/abort`).
                "/_bed/shard_abort" => "/abort",
                "/_bed/shard_status" => "/status",
                p => &p[5..],
            };
            let at = qp("shard").and_then(|v| v.parse::<u8>().ok()).map(SiteId);
            let mut init = RequestInit::new();
            init.with_method(if inner == "/status" { Method::Get } else { Method::Post });
            shard_at(c, &repo, at)?
                .fetch_with_request(Request::new_with_init(
                    &format!("https://shard{inner}?repo={}", repo.id.0.0),
                    &init,
                )?)
                .await
        }
        _ => text(c.rid, 404, "not found\n"),
    }
}

/// The colo's IATA code from the request's `cf` object, or "" when it has none. A request over a service binding (a
/// site's SSH front, `ssh`) has a `cf` object without `colo`, and worker-rs's `Cf::colo` panics on that.
fn colo(req: &Request) -> String {
    let cf = js_sys::Reflect::get(req.inner().as_ref(), &"cf".into()).unwrap_or(wasm_bindgen::JsValue::UNDEFINED);
    if cf.is_undefined() || cf.is_null() {
        return String::new();
    }
    js_sys::Reflect::get(&cf, &"colo".into()).ok().and_then(|c| c.as_string()).unwrap_or_default()
}

/// Packs the colo's IATA code into the integer `Colo` a `suspect` report carries.
fn colo_id(req: &Request) -> u32 {
    let colo = colo(req);
    colo.bytes().take(4).fold(0u32, |n, b| n.wrapping_mul(256).wrapping_add(u32::from(b)))
}

#[event(fetch)]
async fn fetch(req: Request, env: Env, ctx: Context) -> Result<Response> {
    let rid = RequestId(Ulid::new(now(), rand10()));
    let cfg = Config::read(&env)?;
    let url = req.url()?;
    let path = url.path().to_owned();
    let sites: SiteTable = serde_json::from_str(&env.var("GRAFTYARD_SITES")?.to_string())?;
    let anchor = match (env.var("GRAFTYARD_INSTALL"), env.var("GRAFTYARD_CUSTOMER_KEY")) {
        (Ok(i), Ok(k)) => Anchor::from_vars(&i.to_string(), &k.to_string()).ok(),
        _ => None,
    };
    // The signed genesis the deploy tool wrote, accepted once per isolate against the anchor; a refused one is logged
    // and the isolate reads the `Release` view ("Install operations", Genesis, step 6).
    if let Some(a) = &anchor
        && let Err(e) =
            graftyard_lookup::bind_genesis(a, || env.var("GRAFTYARD_GENESIS_SIGNED").ok().map(|v| v.to_string()))
    {
        console_error!("{e}");
    }
    let boat = lifeboat::mode(&env);
    graftyard_lookup::set_lifeboat(boat);
    graftyard_lookup::set_colo(colo_id(&req));
    graftyard_lookup::set_client();
    // A spread read's learner refresh runs after the reply, not inside the read (`cold-write`, B).
    graftyard_lookup::defer_refreshes();
    // The version this request ran, from the version metadata binding, for `graft-front`'s mismatch check. Only a
    // request that asks for it (`graft-front` sends `FRONT_HEADER`) gets the header, and `graft-front` takes it off
    // the client's response, so no client response changes.
    let report = req.headers().has(FRONT_HEADER)?;
    let method = req.method();
    let r = serve(req, &env, &ctx, rid, &cfg, url, path.clone(), &sites, anchor).await;
    // The lifeboat's front: every response carries the copy's lag (`LifeboatStatus`), and `/api/v3/meta` its status.
    let r = match r {
        Ok(res) if boat => Ok(lifeboat::stamp(&env, res, &path).await?),
        r => r,
    };
    if graftyard_lookup::refreshes_pending() {
        let (env, sites) = (env.clone(), sites.clone());
        ctx.wait_until(async move {
            graftyard_lookup::run_refreshes(&graftyard_lookup::Bindings { env: &env, sites: &sites }).await;
        });
    }
    // why-logs: one line per 5xx, whatever route answered it, with the reply's message, so a row counts its 5xx by
    // reason (`tests/fanout/fivexxwhy.py`). The body is read from a clone after the reply is sent.
    let r = match r {
        Ok(mut res) if res.status_code() >= 500 => {
            let (status, kind) = (res.status_code(), route_kind(&method, &path));
            if let Ok(mut copy) = res.cloned() {
                ctx.wait_until(async move {
                    let body = copy.text().await.unwrap_or_default();
                    console_log!("{}", fivexx_line(status, kind, &body));
                });
            }
            Ok(res)
        }
        Err(e) => {
            console_log!("{}", fivexx_line(500, route_kind(&method, &path), &e.to_string()));
            Err(e)
        }
        r => r,
    };
    match (r, report.then(|| env.get_binding::<WorkerVersionMetadata>("VERSION"))) {
        (Ok(res), Some(Ok(v))) => {
            // A response whose headers are immutable (an R2 or subrequest body passed through) is copied once.
            if res.headers().set(EDGE_VERSION_HEADER, &v.id()).is_ok() {
                return Ok(res);
            }
            let headers = res.headers().clone();
            headers.set(EDGE_VERSION_HEADER, &v.id())?;
            let status = res.status_code();
            Ok(Response::from_body(res.body().clone())?.with_status(status).with_headers(headers))
        }
        (r, _) => r,
    }
}

/// A git request's class, (read-class, write). A fetch, upload-pack's advertisement and its POSTs (a v2 `ls-refs`
/// among them), is read-class (`Ctx::read_class`): its token, login, `Placement`, and `Authz` may come from `rcache`
/// within `READ_CACHE`, as an API read's do (`design/DESIGN.md`, "Follower read"). Receive-pack is a write, which reads
/// them fresh. Dumb HTTP is neither: it is refused after the name resolves.
fn git_class(route: &Route) -> (bool, bool) {
    match route {
        Route::InfoRefs { service: Some(Service::UploadPack), .. }
        | Route::Rpc { service: Service::UploadPack, .. } => (true, false),
        Route::InfoRefs { service: Some(Service::ReceivePack), .. }
        | Route::Rpc { service: Service::ReceivePack, .. } => (false, true),
        Route::InfoRefs { service: None, .. } | Route::Dumb { .. } => (false, false),
    }
}

/// Whether a git request may take its token row from `rcache` though it is a write (`Ctx::decided`,
/// `write-identity`): receive-pack's POST, whose `Push::decide` reads the token or `jti` row and the actor's `Account`
/// row once the entry's commands are frozen and answers 401 (`push_reply`). A pack sent under a token revoked within
/// `READ_CACHE` is placed and validated before that refusal, and the sweeper deletes it. The advertisement commits
/// nothing, so nothing but the front would refuse a revoked token there: it reads fresh.
fn git_decided(route: &Route) -> bool {
    matches!(route, Route::Rpc { service: Service::ReceivePack, .. })
}

/// A request's route class for `fivexx_line`, from its method and path.
fn route_kind(method: &Method, path: &str) -> &'static str {
    let agents = path.starts_with(graftyard_api::agents::PREFIX);
    match () {
        _ if path.ends_with("/git-upload-pack") => "upload-pack",
        _ if path.ends_with("/git-receive-pack") => "receive-pack",
        _ if path.ends_with("/info/refs") => "info-refs",
        _ if path.ends_with("/graphql") => "graphql",
        _ if agents && path.contains("/renew") => "renew",
        _ if agents && path.contains("/leases") => "lease",
        _ if agents && path.contains("/tasks") => "task",
        _ if agents => "agents",
        _ if path.contains("/merge") => "merge",
        _ if path.contains("/repos/") && *method == Method::Get => "rest-repo-read",
        _ if path.contains("/repos/") => "rest-repo-write",
        _ => "other",
    }
}

/// The log line of a 5xx reply: `{"fivexx": status, "kind", "why"}`, where `why` is a JSON body's `message`, else the
/// body itself, cut to 200 characters on a char boundary.
fn fivexx_line(status: u16, kind: &str, body: &str) -> String {
    const WHY_MAX: usize = 200;
    let msg =
        serde_json::from_str::<serde_json::Value>(body).ok().and_then(|v| v["message"].as_str().map(str::to_owned));
    let why = msg.as_deref().unwrap_or(body).trim();
    let cut = why.char_indices().nth(WHY_MAX).map_or(why.len(), |(i, _)| i);
    json!({ "fivexx": status, "kind": kind, "why": why[..cut].trim_end() }).to_string()
}

/// The version of `graft-ops` that answered a mint, which every `/mint` answer carries (`workers/ops`).
pub const OPS_VERSION_HEADER: &str = "x-graftyard-ops-version";

thread_local! {
    /// This isolate's mint counts for `MINT_ERROR_RATIO` (`Ctx::ops`).
    static MINTS: std::cell::RefCell<graftyard_release::gate::MintWatch> = std::cell::RefCell::default();
}

/// The request header `graft-front` sends, asking for `EDGE_VERSION_HEADER`.
pub const FRONT_HEADER: &str = "x-graftyard-front";
/// The response header that carries the edge version that served a request (`graft-front`'s mismatch check).
pub const EDGE_VERSION_HEADER: &str = "x-graftyard-edge-version";

#[allow(clippy::too_many_arguments)]
async fn serve(
    mut req: Request,
    env: &Env,
    ctx: &Context,
    rid: RequestId,
    cfg: &Config,
    url: Url,
    path: String,
    sites: &SiteTable,
    anchor: Option<Anchor>,
) -> Result<Response> {
    let c = Ctx {
        env,
        ctx,
        cfg,
        rid,
        sites,
        anchor,
        colo: colo_id(&req),
        token_app: std::cell::Cell::new(None),
        token_fields: std::cell::RefCell::new(None),
        agent_session: req.headers().has("x-graftyard-agent-session")?,
        read_class: std::cell::Cell::new(false),
        write: std::cell::Cell::new(false),
        leader_reads: std::cell::Cell::new(false),
        timing: api::Timing::default(),
        bound: std::cell::Cell::new(None),
        probe: std::cell::Cell::new(false),
        decided: std::cell::Cell::new(false),
        push_rows: std::cell::RefCell::new(None),
        install: std::cell::RefCell::new(None),
        ssh_front: ssh::from_front(ctx),
        #[cfg(feature = "test-faults")]
        kill: req.headers().get("x-graft-kill")?,
        #[cfg(feature = "test-faults")]
        down_ms: req.headers().get("x-graft-down-ms")?.and_then(|d| d.parse().ok()).unwrap_or(0),
        #[cfg(feature = "test-faults")]
        shard: match (req.headers().get("x-graft-shard")?, req.headers().get("x-bed-token")?, env.secret("BED_TOKEN")) {
            (Some(n), Some(t), Ok(s)) if t == s.to_string() => n.parse().ok().map(SiteId),
            _ => None,
        },
        #[cfg(feature = "test-faults")]
        place_fail: req.headers().has("x-graft-place-fail")?
            && matches!((req.headers().get("x-bed-token")?, env.secret("BED_TOKEN")), (Some(t), Ok(s)) if t == s.to_string()),
        #[cfg(feature = "test-faults")]
        ls_refs_edge: ls_refs_at_edge(
            req.headers().get("x-graft-ls-refs")?.as_deref(),
            req.headers().get("x-bed-token")?.as_deref(),
            env.secret("BED_TOKEN").ok().map(|s| s.to_string()).as_deref(),
        ),
        #[cfg(feature = "test-faults")]
        lean: lean_asked(
            req.headers().has("x-graft-lean")?,
            req.headers().get("x-bed-token")?.as_deref(),
            env.secret("BED_TOKEN").ok().map(|s| s.to_string()).as_deref(),
        ),
        #[cfg(feature = "test-faults")]
        client_push: client_push_id(req.headers().get("x-graft-client-push")?.as_deref()),
        #[cfg(feature = "test-faults")]
        push_end: std::cell::RefCell::new(None),
        #[cfg(feature = "test-faults")]
        colo_off: lean_asked(
            req.headers().get("x-graft-colo")?.as_deref() == Some("off"),
            req.headers().get("x-bed-token")?.as_deref(),
            env.secret("BED_TOKEN").ok().map(|s| s.to_string()).as_deref(),
        ),
        #[cfg(feature = "test-faults")]
        link_guess: req
            .headers()
            .get("x-graft-link")?
            .filter(|_| {
                lean_asked(
                    true,
                    req.headers().get("x-bed-token").ok().flatten().as_deref(),
                    env.secret("BED_TOKEN").ok().map(|s| s.to_string()).as_deref(),
                )
            })
            .and_then(|v| {
                let (r, o) = v.split_once(',')?;
                Some((r.trim().parse().ok()?, o.trim().parse().ok()?))
            }),
        #[cfg(feature = "test-faults")]
        advert_force: req
            .headers()
            .get("x-graft-advert")?
            .filter(|_| {
                lean_asked(
                    true,
                    req.headers().get("x-bed-token").ok().flatten().as_deref(),
                    env.secret("BED_TOKEN").ok().map(|s| s.to_string()).as_deref(),
                )
            })
            .and_then(|v| v.trim().parse().ok()),
    };
    // The lifeboat is read-only: a write, the export and release endpoints, a mint, or a test route is 503.
    if graftyard_lookup::lifeboat() && !lifeboat::serves(req.method().as_ref(), &path, url.query().unwrap_or("")) {
        return lifeboat::read_only(&c, &path);
    }
    // A site's SSH front reaches the key lookup and git, and nothing else (`ssh`).
    if c.ssh_front && !ssh::front_path(&path) {
        return text(rid, 404, "Not Found\n");
    }
    if path == ssh::KEY_PATH {
        return ssh::key(&c, &req).await;
    }
    if path == ssh::META_PATH && req.method() == Method::Get {
        return ssh::meta(&c).await;
    }
    if path == release::ENTRIES_PATH {
        return release::entries(&c, &mut req).await;
    }
    if path == release::ERRORS_PATH {
        return release::errors(&c, &mut req).await;
    }
    if path == precreate::TOKENS_PATH {
        return precreate::tokens(&c, &mut req).await;
    }
    #[cfg(feature = "test-faults")]
    if path == precreate::BED_PATH {
        return precreate::bed(&c, &req).await;
    }
    if path == oidc::INSTANCE_AUTH_PATH {
        return oidc::instance_auth(&c, &mut req).await;
    }
    if let Some(r) = Box::pin(export::route(&c, &mut req, &path)).await? {
        return Ok(r);
    }
    if let Some(r) = purge::route(&c, &mut req, &path).await? {
        return agents::respond(&c, r);
    }
    if path == usertok::TOKENS_PATH {
        let r = usertok::create_fine_grained(&c, &mut req).await?;
        return agents::respond(&c, r);
    }
    if path == lifeboat::TOKENS_PATH {
        let r = lifeboat::mint(&c, &mut req).await?;
        return agents::respond(&c, r);
    }
    if let Some(r) = oidc::route(&c, &mut req, &path).await? {
        return Ok(r);
    }
    #[cfg(feature = "test-faults")]
    if path.starts_with(bed_rows::PREFIX) {
        return bed_rows::serve(&c, &mut req, &path).await;
    }
    // actions-artifacts v4: `@actions/artifact` 2.x posts its Twirp calls at the results URL's origin (`twirp`).
    if let Some(method) = path.strip_prefix(graftyard_actions::twirp::PREFIX) {
        return twirp::serve(&c, req, method).await;
    }
    // The Actions run service, its launch service, and the live-log viewer (`actions`).
    if actions::launch_job(&c, &req, &path).is_some() {
        return actions::launch(&c, req).await;
    }
    if let Some(rest) = path.strip_prefix(actions::SERVICES) {
        return actions::serve(&c, req, rest).await;
    }
    if let Some(rest) = path.strip_prefix(actions::VIEWER) {
        return actions::viewer(&c, req, rest).await;
    }
    if let Some(rest) = path.strip_prefix(graftyard_api::agents::PREFIX) {
        let segs: Vec<&str> = rest.trim_start_matches('/').split('/').collect();
        if let Some(r) = hosted::route(&c, &mut req, &segs).await? {
            return agents::respond(&c, r.with("content-type", "application/json; charset=utf-8"));
        }
    }
    if let Some(route) = path.strip_prefix(graftyard_api::agents::PREFIX).and_then(graftyard_api::mcp::route) {
        let r = mcp::grants(&c, &mut req, route).await?;
        return agents::respond(&c, r.with("cache-control", "no-store"));
    }
    if let Some(rest) = path.strip_prefix(graftyard_api::agents::PREFIX)
        && let Some(r) = actions::graftyard_route(&c, &mut req, rest).await?
    {
        return Ok(r);
    }
    if let Some(rest) = path.strip_prefix(graftyard_api::agents::PREFIX)
        && let Some(r) = secrets::graftyard_route(&c, &mut req, rest).await?
    {
        return agents::respond(&c, r);
    }
    if let Some(rest) = path.strip_prefix(graftyard_api::agents::PREFIX)
        && let Some(r) = actions::pinmap::graftyard_route(&c, &mut req, rest).await?
    {
        return agents::respond(&c, r.with("content-type", "application/json; charset=utf-8"));
    }
    if let Some(rest) = path.strip_prefix(graftyard_api::agents::PREFIX).filter(|r| r.starts_with('/'))
        && graftyard_api::agents::route(rest).is_some()
    {
        return agents::serve(&c, &mut req, rest).await;
    }
    #[cfg(feature = "test-faults")]
    if path.starts_with("/_bed/") {
        return bed(&c, &mut req, &path).await;
    }
    // REST, GraphQL, LFS, uploads, and codeload (`api`); git smart HTTP below.
    let authority = url.host_str().unwrap_or_default().to_owned();
    let host = graftyard_wire::Host::new(&cfg.host, false).ok();
    let endpoint = host.as_ref().and_then(|h| h.endpoint(&authority, &path));
    // A session page on the base (`api::session_page`), before the web app's shell: no GHES repository route starts
    // with `_`.
    if let Some(graftyard_wire::Endpoint::Repo { owner, repo, rest }) = endpoint
        && let Some(id) = rest.strip_prefix("_graftyard/sessions/").filter(|i| !i.is_empty() && !i.contains('/'))
        && req.method() == Method::Get
    {
        let r = api::session_page(&c, &req, owner, repo, id).await?;
        return agents::respond(&c, r);
    }
    if let (Some(host), Some(e)) = (&host, endpoint)
        && api::serves(&e)
    {
        return api::serve(&c, &mut req, host, e).await;
    }
    // The web app (`ui`): its private API, its assets, and the HTML shell for a browser's `GET` of a page path, which
    // is a `Web` path or a repository path that is not git's (no `info/refs`, no RPC).
    if let Some(host) = &host {
        if let Some(rest) = path.strip_prefix(ui::PREFIX) {
            return ui::serve(&c, &mut req, host, rest).await;
        }
        if (path.starts_with("/assets/") || path == "/favicon.ico") && req.method() == Method::Get {
            return match ui::asset(&c, req.clone()?).await? {
                Some(r) => Ok(r),
                None => text(c.rid, 404, "Not Found\n"),
            };
        }
        let page = match endpoint {
            Some(graftyard_wire::Endpoint::Web(_)) => true,
            Some(graftyard_wire::Endpoint::Repo { .. }) => {
                matches!(smart::route(&path, url.query().unwrap_or("")), Some(Route::Dumb { .. }) | None)
            }
            _ => false,
        };
        if page && !path.starts_with("/assets/") && ui::wants_shell(&req)? {
            return ui::shell(&c, &req, host, &path).await;
        }
    }
    let ua = req.headers().get("user-agent")?;
    let incoming = graftyard_wire::Incoming { user_agent: ua.as_deref(), ..Default::default() };
    if let Err(refused) = graftyard_wire::serve(&incoming, graftyard_wire::Surface::Git) {
        let body = match refused.body {
            graftyard_wire::Body::Bytes(b) => String::from_utf8(b).unwrap_or_default(),
            _ => String::new(),
        };
        return text(rid, refused.status, &body);
    }
    let Some(route) = smart::route(&path, url.query().unwrap_or("")) else { return text(rid, 404, "Not Found\n") };
    let (owner, name) = match &route {
        Route::InfoRefs { owner, repo, .. } | Route::Rpc { owner, repo, .. } | Route::Dumb { owner, repo } => {
            (owner.clone(), repo.clone())
        }
    };
    let (read_class, write) = git_class(&route);
    c.read_class.set(read_class);
    c.write.set(write);
    c.decided.set(git_decided(&route));
    let rpc_push = matches!(route, Route::Rpc { service: Service::ReceivePack, .. });
    if rpc_push || matches!(route, Route::Rpc { service: Service::UploadPack, .. }) {
        c.timing.start();
    }
    // The token, the name, and a push's install `Authz` start together (`design/DESIGN.md`, journey 1, step 1): none
    // needs another's answer. The install read starts only for a push that reads it anyway (`needs_install`).
    let dumb = matches!(route, Route::InfoRefs { service: None, .. } | Route::Dumb { .. });
    let auth = async {
        if dumb {
            return Ok(None);
        }
        // Under 32.8 every push reads the install's `Authz` whoever sent it (`needs_install`), so it starts beside
        // the token's read rather than after it.
        if rpc_push && c.cfg.enabled.contains(&graftyard_git::repo::REPO_FORMAT_AGENT) {
            let (who, _) = Box::pin(futures_util::future::join(
                api::timed(&c, "auth", authenticate(&c, &req)),
                api::timed(&c, "install", install_authz(&c)),
            ))
            .await;
            return who;
        }
        let who = api::timed(&c, "auth", authenticate(&c, &req)).await?;
        if rpc_push && who.as_ref().is_some_and(|w| needs_install(&c, w)) {
            api::timed(&c, "install", install_authz(&c)).await;
        }
        Ok::<_, Error>(who)
    };
    // `beside`: a fetch on an isolate that holds no guess of its own for the owner guesses the repository from the
    // data center's link entry, and starts the read gate's access rows on it beside the front, unless a shared gate
    // entry will answer the gate; the gate keeps those rows only if the resolve names the same repository.
    let own = held_guess(&owner).is_some();
    // A receive-pack takes the guess too, for its `resolve` alone: it reads no rows beside (`cold-write`, E).
    let link = ((read_class || write) && !dumb && !own && !colo_off(&c))
        .then(|| Some(colo::link_url(&Login::parse(&owner).ok()?.key(), &RepoName::parse(&name).ok()?.key())))
        .flatten();
    let linked = match &link {
        Some(u) => colo::match_link(u).await,
        None => None,
    };
    #[cfg(feature = "test-faults")]
    let linked = match (linked, c.link_guess.filter(|_| link.is_some())) {
        (None, Some((r, o))) => c
            .system_record(ChainId::Ids)
            .await
            .ok()
            .map(|record| Repo::new(RepoId(DbId(r)), AccountId(DbId(o)), record)),
        (l, _) => l,
    };
    c.timing.put(
        "resolve_guess",
        if own {
            1.0
        } else if linked.is_some() {
            2.0
        } else {
            0.0
        },
    );
    let mut side = match &linked {
        Some(g) if read_class && !gate_shared(&c, &req, g).await? => Some(beside_start(&c, g.clone())),
        _ => None,
    };
    let both =
        futures_util::future::join(api::timed(&c, "resolve", resolve_on(&c, &owner, &name, linked.as_ref())), auth);
    let (repo, who) = api::timed(&c, "front", driving(both, side.as_mut().map(|s| s.rows.as_mut()))).await;
    if let (Some(u), Ok(Ok(Some(r)))) = (&link, &repo)
        && linked.as_ref() != Some(r)
    {
        c.ctx.wait_until(colo::put_link(u.clone(), r.clone()));
    }
    let repo = match repo? {
        Ok(Some(r)) => r,
        Ok(None) => return text(rid, 404, "Repository not found.\n"),
        Err(e) => {
            let r = api::read_failed("the repository lookup failed; retry", &e);
            let res = text(rid, 503, &format!("the repository lookup failed; retry ({e})\n"))?;
            res.headers().set("retry-after", r.header("retry-after").unwrap_or("5"))?;
            // A test install names the stages a failed lookup ran, so a bed shows whether it took a guess
            // (`resolve_guess`) and read the gate's rows beside it (`beside_rows`), `tests/edgeboot/run.sh`.
            #[cfg(feature = "test-faults")]
            res.headers().set("server-timing", &c.timing.header())?;
            return Ok(res);
        }
    };
    let repo = &repo;
    // The token's answer counts only once the repository resolved, as when it was read after it.
    let who = who?;
    if rpc_push && who.as_ref().is_some_and(|w| bounded(&c, w)) {
        c.timing.bound_from_start(&c.bound);
    }
    let mut early = None;
    let upload = matches!(route, Route::Rpc { service: Service::UploadPack, .. }) && req.method() == Method::Post;
    let mut front = None;
    // The read gate, before any byte of the repository: an anonymous request for a repository that is not visibly
    // public gets 401 at once, so git asks for credentials, and a caller who may not read it gets the 404 an unknown
    // repository gets (`design/SHARED.md`, "Identity and tokens", `authorize`). A push needs read too.
    if !dumb {
        let push = matches!(
            route,
            Route::InfoRefs { service: Some(Service::ReceivePack), .. }
                | Route::Rpc { service: Service::ReceivePack, .. }
        );
        // An upload-pack's body, key and clone-cache lookup run beside the gate, and are kept only if it allows
        // (`hit-gate`): no byte of the repository leaves before the gate decides.
        let share = match route {
            _ if read_class => colo::READ,
            Route::InfoRefs { service: Some(Service::ReceivePack), .. } => colo::ADVERT,
            Route::Rpc { service: Service::ReceivePack, .. } => colo::POST,
            _ => colo::OFF,
        };
        let gate = api::timed(&c, "gate", read_gate(&c, repo, who.as_ref(), side.take(), share));
        let gate = if upload {
            let allows = |g: &Result<_>| matches!(g, Ok(Ok(graftyard_identity::Decision::Allow)));
            let (g, f) = graftyard_git::compose::beside_gate(gate, upload_front(&c, &mut req, repo), allows).await;
            front = f;
            g
        } else {
            gate.await
        }?;
        // AGENT_PUSH_BOUND at the read gate: an authenticated push the edge classes Agent, whose gate read its members
        // shed until the bound, answers B0-32's `ng` once git has sent its commands, not 503. The repository resolved,
        // so the `ng` says no more than the 503 did: an unknown repository is 404 before the gate.
        let agent = match (&who, &gate) {
            (Some(w), Err(_)) if c.bound.get().is_some() && rpc_push => {
                matches!(admission(&c, w, &[], true, 0).await,
                    Some(a) if a.class == graftyard_git::group::ClassWire::Agent)
            }
            _ => false,
        };
        match gate {
            Err(e) if gate_ng(agent, &e).is_some() => {
                c.timing.put("bound", 0.0);
                early = gate_ng(agent, &e);
            }
            Ok(graftyard_identity::Decision::Allow) => {}
            Ok(_) if who.is_none() && push => return unauthorized(rid, "No anonymous write access."),
            Ok(_) if who.is_none() => return unauthorized(rid, "Authentication required\n"),
            Ok(_) => return text(rid, 404, "Repository not found.\n"),
            Err(e) => {
                let r = api::read_failed("the repository's access rows did not answer; retry", &e);
                let res = text(rid, 503, &format!("the repository's access rows did not answer; retry ({e})\n"))?;
                res.headers().set("retry-after", r.header("retry-after").unwrap_or("5"))?;
                return Ok(res);
            }
        }
    }
    let r = match route {
        // Dumb HTTP is refused: info/refs without a service is 403 and object paths 404, as on github.com.
        Route::InfoRefs { service: None, .. } => text(rid, 403, "Please upgrade your git client.\n"),
        Route::Dumb { .. } => text(rid, 404, "Not Found\n"),
        Route::InfoRefs { service: Some(s), .. } => info_refs(&c, &req, repo, s, who.is_some()).await,
        Route::Rpc { service: Service::UploadPack, .. } => {
            if req.method() != Method::Post {
                return text(rid, 405, "Method Not Allowed\n");
            }
            let r = async {
                let front = match front.take() {
                    Some(f) => f?,
                    None => upload_front(&c, &mut req, repo).await?,
                };
                upload_pack(&c, repo, front).await
            }
            .await;
            // A test install answers each fetch with its stages: `total` is the edge's time to the shard's headers,
            // its time to first byte.
            #[cfg(feature = "test-faults")]
            if let Ok(r) = &r {
                r.headers().set("server-timing", &c.timing.header())?;
            }
            r
        }
        Route::Rpc { service: Service::ReceivePack, .. } => {
            if req.method() != Method::Post {
                return text(rid, 405, "Method Not Allowed\n");
            }
            #[cfg(feature = "test-faults")]
            let open = push_log::opened();
            let r = receive_pack(&c, req, repo, &format!("{owner}/{name}"), who, early).await;
            // A test install answers each push with its stages (`api::Timing`), as REST answers each request.
            #[cfg(feature = "test-faults")]
            {
                push_log::closed();
                if let Ok(r) = &r {
                    r.headers().set("server-timing", &c.timing.header())?;
                    r.headers().set("x-graft-spread", &c.timing.spread_header())?;
                }
                let status = r.as_ref().map_or(500, |r| r.status_code());
                let end = c.push_end.borrow().clone();
                worker::console_log!(
                    "{}",
                    push_log::line(rid, open, status, &c.timing.header(), c.client_push.as_deref(), end)
                );
            }
            r
        }
    };
    match r {
        Ok(r) => Ok(r),
        Err(e) => text(rid, 500, &format!("{e}\n")),
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use graftyard_types::{AccountId, DbId, NetworkId, PlacementName, PlacementRecord, RepoId, SiteId, Timestamp};

    use super::{
        CommitFail, Cut, Repo, ShardRefusal, beside_rows, bound_at_send, capped, chained, client_push_id,
        commit_result, fivexx_line, gate_ng, git_class, gunzip, lean_asked, ls_refs_at_edge, reopened, route_kind,
        shard_fail_line, shard_refusal, shard_url, streamed_site, validate_failed, validate_sites, validate_timing,
        validate_url, warm_first,
    };

    /// A commit that gave no outcome in time is a `CommitFail::NoOutcome` the push answers 503 with `Retry-After`,
    /// not an edge error, which the runtime answers 500 (B0-33 m59r33: four receive-pack 500s, each `no outcome: …:
    /// missed`). A member's error and an unreadable reply stay `Wire`, and an outcome stays an outcome.
    #[test]
    fn a_commit_with_no_outcome_is_a_retryable_failure_not_an_edge_error() {
        use graftyard_git::group::{CallErrorWire, PushOutcome};
        let missed = "repo:131373: no outcome: repo:131373#1#f1: missed".to_owned();
        match commit_result(Err(Err(missed.clone()))) {
            Ok(Err(CommitFail::NoOutcome(e))) => assert_eq!(e, missed),
            other => panic!("{other:?}"),
        }
        let nl = serde_json::json!({ "notLeader": { "term": 4, "leader": null } });
        assert!(matches!(commit_result(Err(Ok(nl))), Ok(Err(CommitFail::Wire(CallErrorWire::NotLeader(_))))));
        assert!(matches!(
            commit_result(Err(Ok(serde_json::json!({ "nonsense": 1 })))),
            Ok(Err(CommitFail::Wire(CallErrorWire::Unsupported(_))))
        ));
        // An answer that is not a `PushOutcome` stays `Unsupported` (503), never a lost reply a re-send could replay.
        assert!(matches!(
            commit_result(Ok(serde_json::json!({ "nonsense": 1 }))),
            Ok(Err(CommitFail::Wire(CallErrorWire::Unsupported(_))))
        ));
        let full = serde_json::json!({ "full": { "retry_after_s": 2 } });
        assert!(matches!(commit_result(Ok(full)), Ok(Ok(PushOutcome::Full(_)))));
        // A test install's `/_bed/push` and BF-12's driver read a member error as `{"Err": <CallErrorWire>}`.
        let wire = CommitFail::Wire(CallErrorWire::Stage(true));
        assert_eq!(serde_json::to_value(&wire).unwrap(), serde_json::json!({ "stage": true }));
    }

    /// The shard's validate timing becomes the three `v_*` stages, and an unknown name is dropped.
    #[test]
    fn validate_timing_keeps_the_three_known_stages() {
        assert_eq!(
            validate_timing("lock_wait;dur=12, index;dur=3, walk;dur=40, other;dur=9"),
            [("v_lock_wait", 12), ("v_index", 3), ("v_walk", 40)]
        );
    }

    /// Every 5xx reaches one log line with its route class and the reply's reason: a JSON reply's `message`, not its
    /// whole body, and a text reply's body, both cut to 200 characters on a char boundary.
    #[test]
    fn a_5xx_logs_its_kind_and_the_reply_message() {
        let v: serde_json::Value = serde_json::from_str(&fivexx_line(
            503,
            "rest-repo-read",
            r#"{"message":"Service Unavailable: rows (overloaded)","documentation_url":"https://docs.github.com/rest"}"#,
        ))
        .unwrap();
        assert_eq!(
            v,
            serde_json::json!({"fivexx": 503, "kind": "rest-repo-read", "why": "Service Unavailable: rows (overloaded)"})
        );
        let v: serde_json::Value = serde_json::from_str(&fivexx_line(
            503,
            "receive-pack",
            "git cache: no shard could check the pack (x); retry\n",
        ))
        .unwrap();
        assert_eq!(v["why"], "git cache: no shard could check the pack (x); retry");
        let long = format!("{}é{}", "x".repeat(199), "y".repeat(500));
        let v: serde_json::Value = serde_json::from_str(&fivexx_line(500, "other", &long)).unwrap();
        assert_eq!(v["why"].as_str().unwrap(), format!("{}é", "x".repeat(199)));
        use worker::Method::{Get, Post};
        assert_eq!(route_kind(&Get, "/api/v3/repos/o/r/pulls"), "rest-repo-read");
        assert_eq!(route_kind(&Post, "/api/v3/repos/o/r/labels"), "rest-repo-write");
        assert_eq!(route_kind(&Post, "/o/r.git/git-receive-pack"), "receive-pack");
        assert_eq!(route_kind(&Post, "/api/graftyard/v1/repos/o/r/leases/7/renew"), "renew");
        assert_eq!(route_kind(&Post, "/api/graphql"), "graphql");
    }

    /// A shard's non-200 answer reaches the edge's log line with its reason, cut to 200 characters on a char boundary.
    #[test]
    fn a_shard_503_logs_its_reason_cut_on_a_char_boundary() {
        let v: serde_json::Value =
            serde_json::from_str(&shard_fail_line(131364, 503, "{\"warming\":true}\n", true)).unwrap();
        assert_eq!((v["shardFail"].as_i64(), v["status"].as_u64()), (Some(131364), Some(503)));
        assert_eq!(v["why"], "{\"warming\":true}", "the shard's body is the reason");
        assert_eq!(v["lsRefs"], true);
        // 199 ASCII bytes then a 2-byte char: the cut keeps 200 chars and never splits one.
        let long = format!("{}é{}", "x".repeat(199), "y".repeat(500));
        let v: serde_json::Value = serde_json::from_str(&shard_fail_line(1, 500, &long, false)).unwrap();
        assert_eq!(v["why"].as_str().unwrap(), format!("{}é", "x".repeat(199)));
    }

    /// F91's A/B switch: the edge answers `ls-refs` unless a request names the shard with the bed token; a wrong or
    /// missing token, another value, or an install with no bed token leaves the edge's answer on. And the push id a
    /// driver sends reaches the log line only as a short plain token.
    #[test]
    fn ls_refs_go_to_the_shard_only_when_a_bed_request_asks() {
        assert!(ls_refs_at_edge(None, None, Some("bed")), "on by default");
        assert!(ls_refs_at_edge(None, Some("bed"), Some("bed")), "the token alone switches nothing");
        assert!(!ls_refs_at_edge(Some("shard"), Some("bed"), Some("bed")), "the A/B's off arm");
        assert!(ls_refs_at_edge(Some("shard"), Some("guess"), Some("bed")), "a wrong token");
        assert!(ls_refs_at_edge(Some("shard"), None, Some("bed")), "no token");
        assert!(ls_refs_at_edge(Some("shard"), Some("bed"), None), "an install with no bed token");
        assert!(ls_refs_at_edge(Some("edge"), Some("bed"), Some("bed")), "only `shard` turns it off");
        assert!(lean_asked(true, Some("bed"), Some("bed")), "the A/B's arm B");
        assert!(!lean_asked(false, Some("bed"), Some("bed")), "off by default, the token alone switches nothing");
        assert!(!lean_asked(true, Some("guess"), Some("bed")), "a wrong token");
        assert!(!lean_asked(true, None, Some("bed")), "no token");
        assert!(!lean_asked(true, Some("bed"), None), "an install with no bed token");
        assert_eq!(client_push_id(Some("mwm4d2ho-a17-3")).as_deref(), Some("mwm4d2ho-a17-3"));
        assert_eq!(client_push_id(Some("x\", \"end\": 1")), None);
        assert_eq!(client_push_id(Some(&"a".repeat(65))), None);
        assert_eq!(client_push_id(Some("")), None);
    }

    /// B0-33 m51: a write whose read the members kept shedding held 19 to 24 s, LOOKUP_FOR, before its 503. A write
    /// now asks again for SHED_RETRY_FOR, well under LOOKUP_FOR, a bounded push no longer than its bound, and a
    /// read-class request not at all.
    #[test]
    fn a_write_asks_a_shed_read_again_for_seconds_not_lookup_for() {
        use std::time::Duration;
        let w = super::shed_retry_for(true, None);
        assert_eq!(w, graftyard_lookup::SHED_RETRY_FOR);
        assert!(w * 4 <= graftyard_lookup::LOOKUP_FOR, "{w:?}");
        // `shed-retry-2s`: at 4 s a write that met a shed made about five member reads before its 503 (B0-33 m56).
        assert!(w <= Duration::from_secs(2), "{w:?}");
        assert_eq!(super::shed_retry_for(true, Some(Duration::from_secs(60))), w);
        assert_eq!(super::shed_retry_for(true, Some(Duration::from_millis(900))), Duration::from_millis(900));
        assert_eq!(super::shed_retry_for(false, None), Duration::ZERO);
    }

    /// edge-refusal: only a fresh advert whose wait passes the budget refuses, and only an Agent-classed caller whose
    /// token reaches the repository; Human, Bot, and MergeQueue send whatever the advert says. edge-refusal-bound: a
    /// lapsed advert that said full holds an Agent write up to ADVERT_STALE (10 s) old and not a millisecond past it,
    /// and never holds any other class (flip R1a).
    #[test]
    fn the_edge_refuses_only_an_agent_write_whose_fresh_advert_passes_its_budget() {
        use std::time::Duration;

        use graftyard_git::group::ClassWire;
        use graftyard_lookup::{Advert, Heard};

        use super::{AdvertSays, advert_says, advert_verdict};
        let budget = Duration::from_millis(5_000);
        let a = |wait_ms| Advert { wait_ms, retry_after_s: 9 };
        assert_eq!(advert_says(Heard::Fresh(a(5_001)), budget), AdvertSays::Refuse(a(5_001)));
        assert_eq!(advert_says(Heard::Fresh(a(5_000)), budget), AdvertSays::Send, "a wait at the budget fits it");
        assert_eq!(advert_says(Heard::Fresh(a(100)), Duration::ZERO), AdvertSays::Refuse(a(100)));
        assert_eq!(advert_says(Heard::Lapsed(a(9_000), 2_001), budget), AdvertSays::Hold(a(9_000), 2_001));
        assert_eq!(advert_says(Heard::Lapsed(a(9_000), 10_000), budget), AdvertSays::Hold(a(9_000), 10_000));
        assert_eq!(advert_says(Heard::Lapsed(a(9_000), 10_001), budget), AdvertSays::Stale(10_001), "ADVERT_STALE");
        assert_eq!(advert_says(Heard::Lapsed(a(10), 2_001), budget), AdvertSays::Send, "a lapsed advert under budget");
        assert_eq!(advert_says(Heard::Never, budget), AdvertSays::Send);
        let refuse = AdvertSays::Refuse(a(9_000));
        let hold = AdvertSays::Hold(a(9_000), 3_000);
        for s in [refuse, hold, AdvertSays::Stale(11_000)] {
            assert_eq!(advert_verdict(s, Some(ClassWire::Agent), true), s);
            for class in [None, Some(ClassWire::Human), Some(ClassWire::Bot), Some(ClassWire::MergeQueue)] {
                assert_eq!(advert_verdict(s, class, true), AdvertSays::Send, "{class:?} is never refused or held");
            }
            assert_eq!(
                advert_verdict(s, Some(ClassWire::Agent), false),
                AdvertSays::Send,
                "a token that cannot reach it"
            );
        }
    }

    /// edge-refusal-bound's probe: on a lapsed full advert one Agent write a chain an isolate per ADVERT_TTL goes, and a
    /// push that went as the probe at `receive_pack` still goes at its send checks, though the chain's slot is taken.
    #[test]
    fn a_probe_push_is_not_held_by_its_own_probe_at_its_send() {
        use super::{ChainId, probe_goes};
        let chain = ChainId::Repo(RepoId(DbId(9_402)));
        assert!(probe_goes(false, chain, 50_000), "the chain's first in ADVERT_TTL is the probe");
        assert!(!probe_goes(false, chain, 50_100), "another write within ADVERT_TTL is held");
        assert!(probe_goes(true, chain, 50_100), "the probe itself, at its send");
    }

    /// edge-refusal-bound's send check sits where the push's budget is spent: in `receive_pack`, after
    /// `bound_at_send` sets what is left at the first commit send, on that `budget_ms`; and in the Stage re-send, after
    /// `budget_ms` is set again from the clock. Neither runs natively (`receive_pack` needs workerd and members), so this
    /// reads the source, as `graftyard_lookup`'s isolate-state test does; plant 99 mutants 4 and 5 fail it.
    #[test]
    fn the_send_check_reads_the_advert_at_each_commit_send_on_the_budget_left_there() {
        let src = include_str!("lib.rs");
        let start = src.find("\nasync fn receive_pack(").expect("receive_pack");
        let body = &src[start..start + src[start..].find("\n}\n").expect("its end")];
        let at = |pat: &str| body.find(pat).unwrap_or_else(|| panic!("receive_pack has no `{pat}`"));
        let send = at("At::Send,");
        assert!(at("bound_at_send(") < send, "the send check follows the first send's budget");
        let before_send = &body[body[..send].rfind("if let").expect("its if")..send];
        assert!(before_send.contains("a.budget_ms)"), "on the first send's budget_ms: {before_send}");
        assert!(body[send..send + 80].contains("from_millis(ms)"), "the budget there, not another");
        let stage = at("At::Stage,");
        assert!(at("place_teed(stage, async") < stage, "after the Stage round");
        let reset = at("a.budget_ms = budget_left(c)");
        assert!(reset < stage && body[reset..stage].contains("if let Some(ms) = a.budget_ms"), "on the budget reset");
        assert!(body[stage..stage + 80].contains("from_millis(ms)"));
    }

    /// edge-refusal's disclosure rule: an installation token selected for other repositories, or a user token scoped
    /// to another owner or other repositories, does not reach this one, so the advert never answers it before the read
    /// gate does; an unscoped token, or one that lists it, does.
    #[test]
    fn a_token_that_cannot_reach_the_repository_is_never_refused_by_the_advert() {
        use graftyard_identity::stateless::InstallationClaims;
        use graftyard_identity::{
            AppId, Credential, InstallationId, Jti, Kid, PermissionSet, Principal, RepoScope, ScopeSet, TokenFields,
            UserId,
        };
        use graftyard_types::Ulid;

        use super::token_reaches;
        let (repo, other, owner) = (RepoId(DbId(5)), RepoId(DbId(6)), AccountId(DbId(9)));
        let app = |repos| Principal {
            actor: graftyard_identity::Actor::App { app: AppId(DbId(1)), installation: InstallationId(DbId(2)) },
            scopes: ScopeSet(0),
            credential: Credential::Stateless(Box::new(InstallationClaims {
                app: AppId(DbId(1)),
                installation: InstallationId(DbId(2)),
                repos,
                permissions: PermissionSet(Default::default()),
                jti: Jti(Ulid(1)),
                iat: Timestamp(0),
                exp: Timestamp(1),
                kid: Kid(0),
                task: None,
            })),
            sso_orgs: Default::default(),
        };
        assert!(token_reaches(&app(RepoScope::All), None, repo, owner));
        assert!(token_reaches(&app(RepoScope::Selected([repo].into_iter().collect())), None, repo, owner));
        assert!(!token_reaches(&app(RepoScope::Selected([other].into_iter().collect())), None, repo, owner));
        let user = Principal {
            actor: graftyard_identity::Actor::User(UserId(AccountId(DbId(3)))),
            scopes: ScopeSet(1),
            credential: Credential::Stored([0; 32]),
            sso_orgs: Default::default(),
        };
        let scoped = |o, r| TokenFields { resource_owner: o, repos: r, ..TokenFields::default() };
        assert!(token_reaches(&user, None, repo, owner));
        assert!(token_reaches(&user, Some(&scoped(Some(owner), None)), repo, owner));
        assert!(!token_reaches(&user, Some(&scoped(Some(AccountId(DbId(10))), None)), repo, owner));
        assert!(!token_reaches(
            &user,
            Some(&scoped(None, Some(RepoScope::Selected([other].into_iter().collect())))),
            repo,
            owner
        ));
    }

    /// `b032-shed` B: an Agent push past its AGENT_PUSH_BOUND at the commit send is refused in B0-32's `ng` shape and
    /// sends nothing; one within it sends what is left; a Human push, or one with no bound, is never refused for it.
    #[test]
    fn an_agent_push_past_its_bound_is_refused_in_the_ng_shape_before_its_commit() {
        use std::time::Duration;
        let r = bound_at_send(true, Some(Duration::ZERO)).expect_err("past the bound");
        assert_eq!((r.unpack.as_str(), r.reason.as_str()), ("ok", "agent admission full, retry after 1s"));
        assert_eq!(bound_at_send(true, Some(Duration::from_millis(1_200))).ok(), Some(Some(1_200)));
        assert_eq!(bound_at_send(false, Some(Duration::ZERO)).ok(), Some(None), "a Human push is not bounded");
        assert_eq!(bound_at_send(true, None).ok(), Some(None), "a push with no bound sends none");
        // At the read gate: a shed gate read at the bound is `ng` for an Agent push, and stays 503 otherwise.
        let shed = format!("repo:1: every member asked shed the read: {}", graftyard_lookup::OVERLOADED);
        let r = gate_ng(true, &shed).expect("shed at the bound");
        assert_eq!(r.reason, "agent admission full, retry after 1s");
        assert!(gate_ng(false, &shed).is_none(), "a Human push keeps the 503");
        assert!(gate_ng(true, "the repository's group has no leader").is_none(), "only a shed read becomes ng");
    }

    /// A streamed body's cap counts the bytes the edge read ahead of the stream, so a body one byte over the cap is
    /// refused at the chunk that passes it, wherever the head ended, and one at the cap is read to its end.
    #[test]
    fn a_streamed_body_counts_its_head_against_the_cap() {
        use futures_util::{FutureExt, StreamExt};
        let run = |read: u64, cap: u64| {
            let body = futures_util::stream::iter((0..4).map(|_| Ok(vec![0u8; 100])));
            let out: Vec<_> = capped(body, read, cap).collect().now_or_never().expect("ready");
            out.iter().map(|c| matches!(c, Err(Cut::OverCap))).collect::<Vec<bool>>()
        };
        assert_eq!(run(1000, 1400), [false; 4]);
        assert_eq!(run(1000, 1399), [false, false, false, true]);
        assert_eq!(run(1050, 1199), [false, true, true, true]);
    }
    /// The shard picks the fan-out shard from the URL's trailer without reading the body, so the trailer must be
    /// the pack's last 20 bytes, not its first; an agent's validate is marked so the container sheds it first; and the
    /// owner's record rides in the URL, which the shard reads instead of its two `Names` reads (`shard.rs`, `record`).
    #[test]
    fn a_validate_url_names_the_packs_trailer_its_class_and_the_record() {
        let rec = record(PlacementName::Tri, &[0, 1, 2], 1);
        let repo = super::Repo::new(RepoId(DbId(7)), graftyard_types::AccountId(DbId(3)), rec.clone());
        let mut pack = vec![0xaa; 40];
        pack[20..].copy_from_slice(&[0x01; 20]);
        let q = |u: &str| {
            let u = url::Url::parse(u).unwrap();
            u.query_pairs().map(|(k, v)| (k.into_owned(), v.into_owned())).collect::<Vec<_>>()
        };
        let tip = [b'a'; 41];
        let pairs = q(&validate_url(&repo, &tip, 41, true, &pack));
        let get = |k: &str| pairs.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
        assert_eq!(get("trailer"), Some("01".repeat(20)));
        assert_eq!(
            (get("repo"), get("tipsn"), get("class")),
            (Some("7".into()), Some("41".into()), Some("agent".into()))
        );
        let sent: PlacementRecord =
            serde_json::from_str(&get("record").expect("the record rides on validate")).unwrap();
        assert_eq!(sent, rec);
        // The head's and the pack's SHA-256 ride too, so the container tells a body changed in transit from a bad
        // push (G35): the digests of these exact bytes, the head being the tips and detail, never the pack.
        let hex = |b: &[u8]| graftyard_types::Digest(<sha2::Sha256 as sha2::Digest>::digest(b).into()).hex();
        assert_eq!((get("headsum"), get("packsum")), (Some(hex(&tip)), Some(hex(&pack))));
        let head = b"\nabcdefghi";
        let pairs = q(&validate_url(&repo, head, 1, false, &[]));
        assert!(pairs.iter().any(|(k, v)| k == "detailn" && v == "9"));
        assert!(pairs.iter().any(|(k, v)| k == "headsum" && *v == hex(head)));
        assert!(!pairs.iter().any(|(k, _)| k == "class" || k == "trailer" || k == "packsum"));
    }

    /// The body a push's commit asks `/outcome_by_request` with while its send is open names the same request id and
    /// the same push, ref updates, actor and format included, without its pack: a 256 KiB inline pack is not sent again
    /// to a leader that is busy.
    #[test]
    fn a_push_asks_for_its_outcome_without_its_pack() {
        use graftyard_git::group::CommitReq;
        use graftyard_git::pack::PackRef;
        use graftyard_git::refs::{Atomicity, ObjectId, RefName, RefUpdate};
        let rid = graftyard_types::RequestId(graftyard_types::Ulid::new(Timestamp(5), [7; 10]));
        let push = graftyard_git::repo::Push {
            updates: vec![RefUpdate {
                name: RefName::parse(b"refs/heads/main").unwrap(),
                expect: None,
                new: Some(ObjectId([1; 20])),
            }],
            atomicity: Atomicity::PerRef,
            pack: Some(PackRef::Inline(vec![0x50; 256 << 10])),
            actor: graftyard_identity::Principal {
                actor: graftyard_identity::Actor::User(graftyard_identity::UserId(graftyard_types::AccountId(DbId(4)))),
                scopes: graftyard_identity::ScopeSet(0),
                credential: graftyard_identity::Credential::Stored([9; 32]),
                sso_orgs: Default::default(),
            },
            options: Vec::new(),
            request: rid,
            agent: None,
            provenance: Vec::new(),
            protected: Vec::new(),
            changed: Vec::new(),
            leases: false,
            test_format: false,
        };
        let body = super::lookup_body(rid, &push);
        assert!(body.len() < 4 << 10, "{} bytes: the pack went with it", body.len());
        let got: CommitReq = serde_json::from_slice(&body).unwrap();
        assert_eq!(got.rid, rid.to_string());
        assert_eq!(got.cmd, graftyard_git::repo::Push { pack: None, ..push });
    }

    /// A busy git cache refuses an agent's push in B0-32's allowed shape, `agent admission full`, never as an
    /// unpacker error; a pack the cache rejected stays an unpacker error.
    #[test]
    fn a_busy_git_cache_refuses_in_the_admission_shape() {
        let busy = shard_refusal(&ShardRefusal::Busy(3)).unwrap();
        assert_eq!((busy.unpack.as_str(), busy.reason.as_str()), ("ok", "agent admission full, retry after 3s"));
        let bad = shard_refusal(&ShardRefusal::Invalid("connectivity".into())).unwrap();
        assert_eq!(bad.reason, "unpacker error");
        // No shard could check it: no refusal at all, so the push is answered 503 and sent again.
        assert!(shard_refusal(&ShardRefusal::Unavailable("site 2's shard answered 503: warming".into())).is_none());
    }

    fn record(name: PlacementName, sites: &[u8], home: u8) -> PlacementRecord {
        PlacementRecord {
            name,
            sites: sites.iter().map(|&s| SiteId(s)).collect(),
            home: SiteId(home),
            generation: 1,
            moving_from: None,
            at: Timestamp(1),
        }
    }

    /// A validate or a read tries the home site's shard first even when the record lists it last, then the network's
    /// other warm site, which holds its chain, then the other Full sites in record order. A Witness site has no git
    /// cache, so it is never tried, and a test-pinned site is tried alone. B0-34 mt21's network 1048616 is warm on
    /// sites 0 and 1 and mt22's 1048617 on 0 and 2 (sha256 rendezvous, checked by hand), so in record order mt22's
    /// reads after the kill of site 0 went to site 1, which held no chain.
    #[test]
    fn a_validate_falls_back_from_the_home_shard_to_the_warm_site_then_the_other_full_sites() {
        let ids = |v: Vec<SiteId>| v.into_iter().map(|s| s.0).collect::<Vec<_>>();
        let (mt21, mt22) = (NetworkId(DbId(1048616)), NetworkId(DbId(1048617)));
        let tri = |home| record(PlacementName::Tri, &[0, 1, 2], home);
        assert_eq!(ids(validate_sites(&tri(0), mt22, None)), [0, 2, 1]);
        assert_eq!(ids(validate_sites(&tri(0), mt21, None)), [0, 1, 2]);
        assert_eq!(ids(validate_sites(&tri(2), mt22, None)), [2, 0, 1]);
        assert_eq!(ids(validate_sites(&tri(2), mt21, None)), [2, 1, 0]);
        // The rendezvous is among the sites but the home: taken from site 0 as home, 1048608 would pick site 2.
        assert_eq!(ids(validate_sites(&tri(2), NetworkId(DbId(1048608)), None)), [2, 1, 0]);
        // Us2w: the third slot is the Witness, site 1 here.
        assert_eq!(ids(validate_sites(&record(PlacementName::Us2w, &[3, 4, 1], 4), mt22, None)), [4, 3]);
        assert_eq!(ids(validate_sites(&record(PlacementName::Solo, &[5], 5), mt22, None)), [5]);
        assert_eq!(ids(validate_sites(&tri(0), mt22, Some(SiteId(1)))), [1]);
    }

    /// Only a shard's 422 is the pack's answer and refuses the push. Every shard warming, broken, or unreachable is
    /// `Unavailable`, which a push answers 503 so it is sent again: at a home-shard kill the in-flight pushes' shard
    /// answers were 5xx and failed calls, and each was refused `unpacker error`.
    #[test]
    fn only_a_shards_422_refuses_a_push_and_shards_that_all_fail_are_unavailable() {
        let t = |s: u8, st: u16, e: &str| (SiteId(s), st, e.to_owned());
        assert_eq!(
            validate_failed(vec![t(0, 422, "index-pack: bad")]),
            ShardRefusal::Invalid("index-pack: bad".into())
        );
        // A 422 after earlier sites failed is still the pack's answer.
        assert_eq!(
            validate_failed(vec![t(0, 503, "warming"), t(1, 0, "reset"), t(2, 422, "connectivity")]),
            ShardRefusal::Invalid("connectivity".into())
        );
        let u =
            validate_failed(vec![t(0, 0, "The container has not been started"), t(1, 503, "warming"), t(2, 500, "x")]);
        assert_eq!(u, ShardRefusal::Unavailable("site 2's shard answered 500: x".into()));
        let u = validate_failed(vec![t(0, 503, "warming"), t(1, 0, "Network connection lost")]);
        assert_eq!(u, ShardRefusal::Unavailable("site 1's shard: Network connection lost".into()));
        assert!(matches!(validate_failed(vec![]), ShardRefusal::Unavailable(_)));
        // A full gate keeps the agent's refusal shape when the sites it moved on to failed too, with the shortest
        // `retry-after` any gate gave; a 422 anywhere is still the pack's answer.
        let b = validate_failed(vec![t(0, 429, "7"), t(1, 503, "warming"), t(2, 429, "3")]);
        assert_eq!(b, ShardRefusal::Busy(3));
        assert_eq!(validate_failed(vec![t(0, 0, "reset"), t(1, 429, "x")]), ShardRefusal::Busy(1));
        assert!(matches!(validate_failed(vec![t(0, 429, "2"), t(1, 422, "bad")]), ShardRefusal::Invalid(_)));
    }

    /// A human validate is never held on one shard past `VALIDATE_HEDGE`: every site left is asked beside it and the
    /// first deciding answer wins, while the last try keeps its whole wait. mt16's straddling push waited 2,087 ms
    /// on the shard being torn down (`decisions/DECISIONS.md`, "`validate-hedge`"). Each script entry is when try `i`
    /// answers after it starts and with what, or `None` for a shard that never answers.
    #[test]
    fn a_human_validate_asks_every_shard_left_beside_a_slow_one_and_the_last_try_keeps_its_wait() {
        use std::cell::Cell;
        use std::rc::Rc;
        use std::time::Duration;

        use futures_util::FutureExt;

        use super::{VALIDATE_HEDGE, hedged, validate_hedge};
        // Wakes itself while pending, so `FuturesUnordered` polls it again on the next tick of the clock.
        struct At(Rc<Cell<u64>>, u64);
        impl std::future::Future for At {
            type Output = ();
            fn poll(self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<()> {
                if self.0.get() >= self.1 {
                    return std::task::Poll::Ready(());
                }
                cx.waker().wake_by_ref();
                std::task::Poll::Pending
            }
        }
        type Out = (Option<(usize, std::result::Result<(), u16>)>, Vec<(usize, u16)>, usize, Vec<(usize, u64)>, u64);
        // (deciding try and status, failed tries, tries started, (passed try, ms), ms at the end)
        let run = |script: &[Option<(u64, u16)>], hedge: Option<Duration>| -> Out {
            let clock = Rc::new(Cell::new(0u64));
            let passed = std::cell::RefCell::new(Vec::new());
            // Each try's answer time is fixed when it starts, not at its first poll.
            let start = |i: usize| {
                let step = script[i].map(|(after, status)| (At(clock.clone(), clock.get() + after), status));
                async move {
                    let Some((at, status)) = step else { return std::future::pending().await };
                    at.await;
                    if status == 200 { Ok(()) } else { Err((status, String::new())) }
                }
            };
            let sleep = |d: Duration| At(clock.clone(), clock.get() + d.as_millis() as u64);
            let mut f =
                Box::pin(hedged(script.len(), start, hedge, |i| passed.borrow_mut().push((i, clock.get())), sleep));
            // Polled a few times per ms: `FuturesUnordered` yields after two of its futures wake themselves, so one
            // poll may not reach a third that is ready.
            for _ in 0..80_000 {
                if let Some(t) = (0..4).find_map(|_| f.as_mut().now_or_never()) {
                    let answer = t.answer.map(|(i, r)| (i, r.map_err(|(s, _)| s)));
                    let failed = t.failed.into_iter().map(|(i, (s, _))| (i, s)).collect();
                    assert_eq!(t.hedged, !passed.borrow().is_empty());
                    return (answer, failed, t.started, passed.borrow().clone(), clock.get());
                }
                clock.set(clock.get() + 1);
            }
            panic!("the validate never answered");
        };
        let h = validate_hedge(false);
        assert_eq!(h, Some(VALIDATE_HEDGE));
        assert!(VALIDATE_HEDGE < Duration::from_millis(1_380), "the straddling push must land under 3 s");
        // An agent's validate is never hedged: the container bounds it, and its p50 is 1.5 s.
        assert_eq!(validate_hedge(true), None);
        // mt16's straddler: the home answers at 2,087 ms, site b in 250. Hedged at 1,000, b answers at 1,250, and the
        // home is passed (marked cold) at the hedge, which starts every site left.
        let kill = [Some((2_087, 200)), Some((250, 200)), Some((300, 200))];
        assert_eq!(run(&kill, h), (Some((1, Ok(()))), vec![], 3, vec![(0, 1_000)], 1_250));
        // Unhedged (an agent's), the same push waits the home out.
        assert_eq!(run(&kill, None), (Some((0, Ok(()))), vec![], 1, vec![], 2_087));
        // mt17's straddler: the home never answers and site b, taking its load, is slow too. The hedge starts b and c
        // together, so c decides at 1,300 ms, not after a second hedge at 2,300.
        let mt17 = [None, Some((1_500, 200)), Some((300, 200))];
        assert_eq!(run(&mt17, h), (Some((2, Ok(()))), vec![], 3, vec![(0, 1_000)], 1_300));
        // A slow home that still answers first decides: the hedge keeps it running rather than dropping it.
        let slow = [Some((1_100, 200)), Some((500, 200)), Some((500, 200))];
        assert_eq!(run(&slow, h), (Some((0, Ok(()))), vec![], 3, vec![(0, 1_000)], 1_100));
        // A failed shard moves on at once, as before, with no hedge spent, and the next try gets its own hedge.
        let failed = [Some((200, 503)), Some((300, 200)), Some((50, 200))];
        assert_eq!(run(&failed, h), (Some((1, Ok(()))), vec![(0, 503)], 2, vec![], 500));
        // After the hedge every try has started: failures start nothing more, and the last try decides at its time.
        let late = [Some((1_500, 503)), Some((400, 503)), Some((2_000, 200))];
        assert_eq!(run(&late, h), (Some((2, Ok(()))), vec![(1, 503), (0, 503)], 3, vec![(0, 1_000)], 3_000));
        // The pack's 422 from the hedged shard decides; the home is never waited out.
        let bad = [None, Some((100, 422)), Some((150, 200))];
        assert_eq!(run(&bad, h), (Some((1, Err(422))), vec![], 3, vec![(0, 1_000)], 1_100));
        // The last try is never passed: one site (pinned or Solo), or the last of two, waits all of its time (G30).
        assert_eq!(run(&[Some((5_000, 200))], h), (Some((0, Ok(()))), vec![], 1, vec![], 5_000));
        let two = [None, Some((9_000, 200))];
        assert_eq!(run(&two, h), (Some((1, Ok(()))), vec![], 2, vec![(0, 1_000)], 10_000));
        // Every shard fails: each failure is kept, in the order they answered.
        let none = [Some((100, 503)), Some((100, 500)), Some((100, 0))];
        assert_eq!(run(&none, h), (None, vec![(0, 503), (1, 500), (2, 0)], 3, vec![], 300));
        // An agent's validate (no hedge) whose home gate is full goes to the next site rather than ending the push:
        // g4B's home refused 3,722 while site b sat idle (`agent-spill`, G40). The gate's 2 s wait is the home try's.
        let full = [Some((2_000, 429)), Some((600, 200)), Some((300, 200))];
        assert_eq!(run(&full, None), (Some((1, Ok(()))), vec![(0, 429)], 2, vec![], 2_600));
        // A 429 at once (the agent queue past 8 a slot) moves on at once; the next site's 422 is the pack's answer.
        let bad = [Some((0, 429)), Some((200, 422)), Some((300, 200))];
        assert_eq!(run(&bad, None), (Some((1, Err(422))), vec![(0, 429)], 2, vec![], 200));
        // Every gate full: each site was asked once, in order, and none decides; `validate_failed` makes it `Busy`.
        let all = [Some((2_000, 429)), Some((2_000, 429)), Some((2_000, 429))];
        assert_eq!(run(&all, None), (None, vec![(0, 429), (1, 429), (2, 429)], 3, vec![], 6_000));
        // A human's 429 (none is sent today: a human waits for a slot) would move on the same way, hedge or not.
        let human = [Some((100, 429)), Some((300, 200)), Some((50, 200))];
        assert_eq!(run(&human, h), (Some((1, Ok(()))), vec![(0, 429)], 2, vec![], 400));
    }

    /// `slot-bound`, B0-33 h1A: an agent validate's slot wait is what is left of its push's bound less what the
    /// validate and its commit need after the slot, at most the container's 2 s. On main every agent try waited 2 s,
    /// so a push that moved with 2.4 s left (h1A's p50 at the first send) spent all of it on the next pool's 429.
    #[test]
    fn an_agent_try_waits_for_a_slot_only_what_its_bound_leaves() {
        use std::time::Duration;

        use super::{agent_slot_ms, with_slot};
        let ms = |m: u64| Some(Duration::from_millis(m));
        // A fresh push: the container's own 2 s cap.
        assert_eq!(agent_slot_ms(ms(4_500)), Some(2_000));
        assert_eq!(agent_slot_ms(ms(3_000)), Some(2_000));
        // 2,433 ms left (h1A's ok pushes' p50 at their first send) leaves 1,433 ms to wait, not 2,000.
        assert_eq!(agent_slot_ms(ms(2_433)), Some(1_433));
        // A try with less than the reserve left waits for no slot: a free one or a 429 at once.
        assert_eq!(agent_slot_ms(ms(800)), Some(0));
        assert_eq!(agent_slot_ms(ms(0)), Some(0));
        // A push with no bound (a human's) sends none, and the container keeps its default.
        assert_eq!(agent_slot_ms(None), None);
        assert_eq!(with_slot("u?a=1".into(), Some(1_433)), "u?a=1&slot_ms=1433");
        assert_eq!(with_slot("u?a=1".into(), None), "u?a=1");
    }

    /// A streamed pack can be sent once, so it fails over before its first byte: it goes to the first site whose
    /// bodiless probe answers 200, in `shard_order`, and the last site is taken without a probe. Sent to the home only,
    /// a big push through B0-34's home stall would answer 503 with the other sites never asked (round 18c).
    #[test]
    fn a_streamed_pack_goes_to_the_first_shard_that_is_caught_up() {
        use futures_util::FutureExt;
        let s = |v: &[u8]| v.iter().map(|&i| SiteId(i)).collect::<Vec<_>>();
        let pick = |sites: &[SiteId], ok: &[u8]| {
            let mut asked = Vec::new();
            let (site, tries) = streamed_site(sites, |i, site| {
                asked.push((i, site.0));
                let ok = ok.contains(&site.0);
                async move { if ok { (200, String::new()) } else { (503, "{\"warming\":true}".to_owned()) } }
            })
            .now_or_never()
            .expect("ready");
            (site.0, asked, tries.iter().map(|t| (t.0.0, t.1)).collect::<Vec<_>>())
        };
        // The home is caught up: it takes the pack after one probe.
        assert_eq!(pick(&s(&[2, 0, 1]), &[2, 0, 1]), (2, vec![(0, 2)], vec![]));
        // The home warms: the next site takes it, at the second try's index (its wait).
        assert_eq!(pick(&s(&[2, 0, 1]), &[0, 1]), (0, vec![(0, 2), (1, 0)], vec![(2, 503)]));
        // Both probed sites warm: the last takes it unprobed, and both refusals are kept.
        assert_eq!(pick(&s(&[2, 0, 1]), &[]), (1, vec![(0, 2), (1, 0)], vec![(2, 503), (0, 503)]));
        // One site (a test-pinned one, or Solo): no probe at all.
        assert_eq!(pick(&s(&[5]), &[]), (5, vec![], vec![]));
    }

    /// AM2's review of head 1: the upload front builds its clone key and looks it up on the isolate's guess before the
    /// gate fixes a fork's network, so a key on the fork's own id is built again on the fixed network and its lookup
    /// runs again; a key already on it, or none, is kept with what the front found.
    #[test]
    fn a_clone_key_built_before_the_gate_fixed_a_forks_network_is_built_again() {
        use graftyard_git::clone::CloneKey;
        use graftyard_types::NetworkId;
        let mut b = Vec::new();
        graftyard_git::pkt::line(&mut b, "command=fetch");
        graftyard_git::pkt::delim(&mut b);
        graftyard_git::pkt::line(&mut b, &format!("want {}", "ab".repeat(20)));
        graftyard_git::pkt::line(&mut b, "done");
        graftyard_git::pkt::flush(&mut b);
        let v = graftyard_git::smart::parse_v2(&b).unwrap();
        let (own, base) = (NetworkId(DbId(131_902)), NetworkId(DbId(131_900)));
        let guessed = CloneKey::of(own, &v, graftyard_types::Digest([0; 32])).unwrap();
        let (rebuilt, kept) = super::front_key(Some(guessed.clone()), base);
        let rebuilt = rebuilt.unwrap();
        assert!(!kept, "what the front found under the fork's own id is looked up again");
        assert_eq!(rebuilt, CloneKey::of(base, &v, graftyard_types::Digest([0; 32])).unwrap());
        assert_ne!(rebuilt.digest(), guessed.digest(), "the fork's entry is under its network's key");
        assert_eq!(super::front_key(Some(rebuilt.clone()), base), (Some(rebuilt), true), "a key on the network stays");
        assert_eq!(super::front_key(None, base), (None, true));
    }

    /// Every try but the last says another shard follows, so a rehydrating shard answers it at once; the last try
    /// waits, so a call never runs out of shards that would serve it. The home try waits less than the others.
    #[test]
    fn every_shard_try_but_the_last_says_another_follows() {
        assert_eq!(shard_url("u?repo=1", 0, 3, false), "u?repo=1&wait=15&next=1");
        assert_eq!(shard_url("u?repo=1", 1, 3, false), "u?repo=1&wait=60&next=1");
        assert_eq!(shard_url("u?repo=1", 2, 3, false), "u?repo=1&wait=60");
        assert_eq!(shard_url("u?repo=1", 0, 1, false), "u?repo=1&wait=15");
        assert_eq!(shard_url("u?repo=1", 1, 3, true), "u?repo=1&wait=60&next=1&moved=1");
    }

    /// A call elsewhere is moved load only while the home is cold because a try failed on it: a hedge that passed a
    /// slow, live home still sends this isolate's next pushes to the other sites first, but unmarked, so their shards'
    /// maintenance runs. On B0-34 mt21 a hedge pass at 03:10:59Z held site b's maintenance 573 s before any failover
    /// (G41). A pass keeps a live failure's mark, the mark lapses with `COLD_SHARD_MS`, and an answer clears it.
    #[test]
    fn only_a_failed_home_makes_a_call_elsewhere_moved_load() {
        use super::{COLD_SHARD_MS, ColdMap, Seen, moved, note};
        let (net, home, b) = (7, SiteId(0), SiteId(1));
        let key = (net, home.0);
        let cold = |m: &ColdMap, t: u64| m.get(&key).is_some_and(|c| c.until > t);
        // mt21: a hedge passes the home. It goes last for COLD_SHARD_MS, and nothing sent to site b is moved.
        let mut m = ColdMap::new();
        note(&mut m, key, Seen::Passed, 1_000);
        assert!(cold(&m, 1_001), "a passed home is still tried last");
        assert!(!moved(&m, net, home, b, 1_001), "a hedge pass is not a failed home");
        // A try fails on the home: calls elsewhere are moved, and the home's own never is.
        note(&mut m, key, Seen::Failed, 2_000);
        assert!(moved(&m, net, home, b, 2_001));
        assert!(!moved(&m, net, home, home, 2_001), "the home's own call");
        assert!(!moved(&m, net + 1, home, b, 2_001), "another network");
        // A later pass does not hide the live failure, and renews the cold mark.
        note(&mut m, key, Seen::Passed, 3_000);
        assert!(moved(&m, net, home, b, 3_000 + COLD_SHARD_MS - 1));
        assert!(!moved(&m, net, home, b, 3_000 + COLD_SHARD_MS), "the mark lapses");
        // A pass after a failure that lapsed is a pass.
        note(&mut m, key, Seen::Passed, 3_000 + COLD_SHARD_MS);
        assert!(!moved(&m, net, home, b, 3_001 + COLD_SHARD_MS));
        // The home answers: warm, and nothing is moved.
        note(&mut m, key, Seen::Failed, 100_000);
        note(&mut m, key, Seen::Answered, 100_001);
        assert!(!cold(&m, 100_002) && !moved(&m, net, home, b, 100_002));
        // Only the home's failure counts: site b failing makes a call to site c no moved load.
        let mut m = ColdMap::new();
        note(&mut m, (net, b.0), Seen::Failed, 1_000);
        assert!(!moved(&m, net, home, SiteId(2), 1_001));
    }

    /// A shard seen cold goes last and the rest keep their order: the home first among the warm ones, and the cold
    /// ones in their own order. With none cold the order is unchanged.
    #[test]
    fn a_cold_shard_is_tried_last_and_the_rest_keep_their_order() {
        let s = |v: &[u8]| v.iter().map(|&i| SiteId(i)).collect::<Vec<_>>();
        let ids = |v: Vec<SiteId>| v.into_iter().map(|s| s.0).collect::<Vec<_>>();
        assert_eq!(ids(warm_first(s(&[2, 0, 1]), |x| x.0 == 2)), [0, 1, 2]);
        assert_eq!(ids(warm_first(s(&[2, 0, 1]), |x| x.0 == 0)), [2, 1, 0]);
        assert_eq!(ids(warm_first(s(&[2, 0, 1]), |x| x.0 != 1)), [1, 2, 0]);
        assert_eq!(ids(warm_first(s(&[2, 0, 1]), |_| false)), [2, 0, 1]);
        assert_eq!(ids(warm_first(s(&[2, 0, 1]), |_| true)), [2, 0, 1]);
    }

    fn gzip(b: &[u8]) -> Vec<u8> {
        let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        e.write_all(b).expect("writes to a Vec");
        e.finish().expect("finishes into a Vec")
    }

    /// A body that inflates past the cap fails as a corrupt gzip does, without inflating the rest; one at the cap
    /// inflates whole.
    #[test]
    fn a_body_that_inflates_past_the_cap_is_refused_as_a_bad_gzip() {
        let body = vec![0u8; 1 << 20];
        let z = gzip(&body);
        assert_eq!(gunzip(&z, 1 << 20).expect("at the cap"), body);
        let e = gunzip(&z, (1 << 20) - 1).expect_err("past the cap").to_string();
        assert!(e.starts_with("gzip: "), "{e}");
        assert!(gunzip(b"not gzip", 1 << 20).expect_err("corrupt").to_string().starts_with("gzip: "));
    }

    /// A fetch is read-class and never a write: upload-pack's advertisement and POSTs, a v2 `ls-refs` among them,
    /// may take `rcache` entries. Receive-pack's advertisement and POST stay write-class, so a push reads its token and
    /// grants fresh, and dumb HTTP is neither. On main every git request was write-class or neither.
    #[test]
    fn a_fetch_is_read_class_and_a_push_stays_a_write() {
        let class =
            |path: &str, query: &str| git_class(&graftyard_git::smart::route(path, query).expect("a git route"));
        assert_eq!(class("/o/r.git/info/refs", "service=git-upload-pack"), (true, false));
        assert_eq!(class("/o/r.git/git-upload-pack", ""), (true, false));
        assert_eq!(class("/o/r/git-upload-pack", ""), (true, false));
        assert_eq!(class("/o/r.git/info/refs", "service=git-receive-pack"), (false, true));
        assert_eq!(class("/o/r.git/git-receive-pack", ""), (false, true));
        assert_eq!(class("/o/r.git/info/refs", ""), (false, false));
        assert_eq!(class("/o/r.git/HEAD", ""), (false, false));
    }

    /// `write-identity`: of git's requests only receive-pack's POST takes its token row from `rcache`, since
    /// `Push::decide` reads the credential again; its advertisement commits nothing and a fetch is read-class.
    #[test]
    fn only_a_push_s_post_is_decided_among_git_requests() {
        let decided = |path: &str, query: &str| {
            super::git_decided(&graftyard_git::smart::route(path, query).expect("a git route"))
        };
        assert!(decided("/o/r.git/git-receive-pack", ""));
        assert!(!decided("/o/r.git/info/refs", "service=git-receive-pack"));
        assert!(!decided("/o/r.git/git-upload-pack", ""));
        assert!(!decided("/o/r.git/info/refs", "service=git-upload-pack"));
        assert!(!decided("/o/r.git/HEAD", ""));
    }

    /// One read of `chained`'s harness: its name and arguments, and the ms it started.
    type Started = Vec<(String, u64)>;

    /// Runs `resolve`'s chain on a clock, each read taking 100 ms, over a store where login `o` is account `a`, `a`'s
    /// record is `r`, and the name read on (account, record) answers `names`. Returns the answer, the ms it came, and
    /// each read with its start.
    fn run_chain(
        guess: Option<(u8, u8)>,
        a: Option<u8>,
        r: Option<u8>,
        names: fn(u8, u8) -> Result<Option<u32>, String>,
    ) -> (Result<Option<u32>, String>, u64, Started) {
        use std::cell::{Cell, RefCell};
        use std::rc::Rc;

        use futures_util::FutureExt;

        struct At(Rc<Cell<u64>>, u64);
        impl std::future::Future for At {
            type Output = ();
            fn poll(self: std::pin::Pin<&mut Self>, _: &mut std::task::Context<'_>) -> std::task::Poll<()> {
                if self.0.get() >= self.1 { std::task::Poll::Ready(()) } else { std::task::Poll::Pending }
            }
        }
        let clock = Rc::new(Cell::new(0u64));
        let log = Rc::new(RefCell::new(Started::new()));
        let read = |what: String| {
            let (clock, log) = (clock.clone(), log.clone());
            async move {
                let at = clock.get();
                log.borrow_mut().push((what, at));
                At(clock, at + 100).await;
            }
        };
        let first = {
            let r = read("login".into());
            async move {
                r.await;
                Ok::<_, String>(a)
            }
        };
        let second = |acct: u8| {
            let rd = read(format!("placement {acct}"));
            async move {
                rd.await;
                Ok(if Some(acct) == a { r } else { None })
            }
        };
        let third = |acct: u8, rec: u8| {
            let rd = read(format!("name {acct} {rec}"));
            async move {
                rd.await;
                names(acct, rec)
            }
        };
        let mut f = Box::pin(chained(guess, first, second, third));
        for _ in 0..1_000 {
            if let Some(out) = f.as_mut().now_or_never() {
                return (out, clock.get(), log.borrow().clone());
            }
            clock.set(clock.get() + 1);
        }
        panic!("the chain never answered");
    }

    /// `resolve`'s three reads: with no guess they run in turn (300 ms); with a right guess all three start at once
    /// and the answer comes in one read's time (100 ms), which main's serial resolve never did. A wrong guess never
    /// changes the answer: a moved account reads its record and name again, a moved record its name, a guessed read
    /// that failed or answered for the old account counts for nothing, and a login that no longer resolves answers
    /// `None` even when the guessed name read found a repository.
    #[test]
    fn resolve_starts_its_three_reads_at_once_on_a_held_guess_and_keeps_only_what_the_chain_confirms() {
        let names = |a: u8, r: u8| Ok(Some(u32::from(a) * 100 + u32::from(r)));
        let s = |v: &[(&str, u64)]| v.iter().map(|(n, t)| (n.to_string(), *t)).collect::<Started>();
        assert_eq!(
            run_chain(None, Some(1), Some(7), names),
            (Ok(Some(107)), 300, s(&[("login", 0), ("placement 1", 100), ("name 1 7", 200)]))
        );
        assert_eq!(
            run_chain(Some((1, 7)), Some(1), Some(7), names),
            (Ok(Some(107)), 100, s(&[("login", 0), ("placement 1", 0), ("name 1 7", 0)]))
        );
        // The login now names account 2: its record and name are read again, on the fresh answers.
        assert_eq!(
            run_chain(Some((1, 7)), Some(2), Some(9), names),
            (
                Ok(Some(209)),
                300,
                s(&[("login", 0), ("placement 1", 0), ("name 1 7", 0), ("placement 2", 100), ("name 2 9", 200)])
            )
        );
        // Same account, moved record: only the name is read again.
        assert_eq!(
            run_chain(Some((1, 7)), Some(1), Some(8), names),
            (Ok(Some(108)), 200, s(&[("login", 0), ("placement 1", 0), ("name 1 7", 0), ("name 1 8", 100)]))
        );
        // The guessed name read failed on the old record: not kept, so not an error.
        let stale =
            |a: u8, r: u8| if r == 7 { Err("old members".into()) } else { Ok(Some(u32::from(a) * 100 + u32::from(r))) };
        assert_eq!(run_chain(Some((1, 7)), Some(1), Some(8), stale).0, Ok(Some(108)));
        // A guessed read that answers is still dropped when the login no longer resolves.
        assert_eq!(run_chain(Some((1, 7)), None, Some(7), names).0, Ok(None));
        // A fresh read that fails fails the chain, guess or not.
        let down = |_: u8, _: u8| Err::<Option<u32>, _>("down".to_string());
        assert_eq!(run_chain(Some((1, 7)), Some(1), Some(7), down).0, Err("down".into()));
    }

    /// `merge-front`: a merge guesses its repository from this isolate's login, `Placement` and `Authz` entries at
    /// any age, which `resolve_cached` refused past READ_CACHE, so a merge an isolate meets more than 10 s after its
    /// last read of the owner still reads ahead. The guess is the entries' repository and record, and a name or login
    /// the entries do not hold gives none.
    #[test]
    fn a_merge_guesses_from_its_isolates_entries_at_any_age() {
        use crate::rcache;
        use graftyard_identity::{AuthzFold, Login, NameEpoch, RepoEntry, RepoName};
        let (owner, rec) = (AccountId(DbId(4_100)), record(PlacementName::Tri, &[0, 1, 2], 1));
        let mut fold = AuthzFold::new(owner);
        let repo = RepoId(DbId(65_586));
        fold.names.insert(RepoName::parse("fleet").unwrap().key(), RepoEntry::Live { repo, epoch: NameEpoch(1) });
        let old = Timestamp(1);
        rcache::put(&rcache::LOGINS, Login::parse("m9fleet").unwrap().key(), old, owner);
        rcache::put(&rcache::PLACEMENTS, owner, old, rec.clone());
        rcache::put(&rcache::AUTHZ, owner, old, fold);
        assert!(rcache::get(&rcache::AUTHZ, &owner, Timestamp(60_000), true).is_none(), "past READ_CACHE");
        assert_eq!(super::held_repo("m9fleet", "fleet"), Some(Repo::new(repo, owner, rec)));
        assert_eq!(super::held_repo("m9fleet", "other"), None, "a name the fold does not hold");
        assert_eq!(super::held_repo("m9other", "fleet"), None, "a login the isolate never read");
    }

    /// cw3: the install's guessed fold read is kept when the real `Placement` row differs from the guess only in `at`,
    /// and read again on the real row when any field a read uses differs: name, sites, home, generation, or
    /// `moving_from`, whose own `at` again does not count.
    #[test]
    fn the_install_guess_is_kept_across_the_stamp_and_dropped_on_any_other_field() {
        use futures_util::FutureExt;
        let genesis = record(PlacementName::Tri, &[0, 1, 2], 0);
        let run = |guess: PlacementRecord, real: PlacementRecord| {
            let reads = std::cell::RefCell::new(Vec::new());
            let read = |r: PlacementRecord| {
                reads.borrow_mut().push(r.clone());
                std::future::ready(Ok::<_, String>(Some(r)))
            };
            let fresh = std::future::ready(Ok::<_, String>(Some(real)));
            let got = super::install_link(Some(guess), fresh, read).now_or_never().expect("ready");
            (got.unwrap().unwrap(), reads.into_inner())
        };
        let stamped = PlacementRecord { at: Timestamp(5), ..genesis.clone() };
        let (answer, reads) = run(genesis.clone(), stamped);
        assert_eq!((answer, reads.len()), (genesis.clone(), 1), "a guess that differs only in `at` is kept");
        let others = [
            ("generation", PlacementRecord { generation: 2, ..genesis.clone() }),
            ("moving_from", PlacementRecord { moving_from: Some(Box::new(genesis.clone())), ..genesis.clone() }),
            ("home", PlacementRecord { home: SiteId(1), ..genesis.clone() }),
            ("sites", record(PlacementName::Tri, &[0, 1, 3], 0)),
        ];
        for (field, real) in others {
            let (answer, reads) = run(genesis.clone(), PlacementRecord { at: Timestamp(5), ..real.clone() });
            assert_eq!(answer.generation, real.generation, "{field}");
            assert_eq!(reads.len(), 2, "a guess that differs in {field} is read again");
            assert_eq!(reads[1], PlacementRecord { at: Timestamp(5), ..real }, "{field}: on the real row");
        }
        // `moving_from` compares as a record: its own `at` alone does not count, its members do.
        let moved = |from: PlacementRecord| PlacementRecord {
            generation: 2,
            moving_from: Some(Box::new(from)),
            ..genesis.clone()
        };
        let guess = moved(genesis.clone());
        assert_eq!(run(guess.clone(), moved(PlacementRecord { at: Timestamp(9), ..genesis.clone() })).1.len(), 1);
        assert_eq!(run(guess, moved(PlacementRecord { generation: 3, ..genesis.clone() })).1.len(), 2);
    }

    /// `beside`: rows read beside the front on a guessed repository are kept only when the resolve answered that same
    /// repository, owner and placement record. Any other answer reads them again on the resolved repository, and a
    /// guessed read that failed counts for nothing then. A kept read is never waited on twice.
    #[test]
    fn rows_read_beside_the_front_are_kept_only_for_the_repository_the_resolve_named() {
        use futures_util::FutureExt;
        let rec = |g| PlacementRecord { generation: g, ..record(PlacementName::Tri, &[0, 1, 2], 0) };
        let repo = |id: i64, owner: i64, g| Repo::new(RepoId(DbId(id)), AccountId(DbId(owner)), rec(g));
        let run = |guessed: Repo, started: Result<Option<i64>, String>, resolved: Repo| {
            let reads = std::cell::RefCell::new(Vec::new());
            let read = |r: Repo| {
                reads.borrow_mut().push(r.id.0.0);
                std::future::ready(Ok::<_, String>(Some(r.id.0.0 * 10)))
            };
            let out = beside_rows(guessed, std::future::ready(started), &resolved, read).now_or_never().expect("ready");
            (out, reads.into_inner())
        };
        let g = repo(7, 1, 1);
        assert_eq!(run(g.clone(), Ok(Some(70)), g.clone()), ((Ok(Some(70)), true), vec![]));
        // Another repository, another owner, another record: read again on the resolved one, never kept.
        for other in [repo(8, 1, 1), repo(7, 2, 1), repo(7, 1, 2)] {
            let id = other.id.0.0;
            assert_eq!(run(g.clone(), Ok(Some(70)), other), ((Ok(Some(id * 10)), false), vec![id]));
        }
        // A guessed read that failed is not the gate's error when it was not kept; it is when it was.
        assert_eq!(run(g.clone(), Err("shed".into()), repo(8, 1, 1)), ((Ok(Some(80)), false), vec![8]));
        assert_eq!(run(g.clone(), Err("shed".into()), g.clone()), ((Err("shed".into()), true), vec![]));
        // A guess, at any age, is never the answer: a fresh read that fails fails the link, and one that finds nothing
        // answers nothing, whatever the guessed read found.
        let next = |a: i64| std::future::ready(Ok::<_, String>(Some(a * 10)));
        let link = |prev: Result<Option<i64>, String>| {
            super::link(Some((1, next(1))), std::future::ready(prev), next).now_or_never().expect("ready")
        };
        assert_eq!(link(Err("down".into())), Err("down".into()));
        assert_eq!(link(Ok(None)), Ok(None));
        assert_eq!(link(Ok(Some(2))), Ok(Some((2, 20))));
        // A guessed read still running when `prev` names another answer is dropped before the read again starts, so
        // it holds nothing the read again waits on (a write-class read's batch, `rcache::batched`). `prev` answers on
        // its second poll, so the guessed read has started by then.
        struct Held<'a>(&'a std::cell::Cell<bool>);
        impl Drop for Held<'_> {
            fn drop(&mut self) {
                self.0.set(false);
            }
        }
        let running = std::cell::Cell::new(false);
        let guessed = async {
            running.set(true);
            let _held = Held(&running);
            std::future::pending::<Result<Option<i64>, String>>().await
        };
        let mut polled = false;
        let prev = std::future::poll_fn(move |cx| {
            if std::mem::replace(&mut polled, true) {
                std::task::Poll::Ready(Ok::<_, String>(Some(2)))
            } else {
                cx.waker().wake_by_ref();
                std::task::Poll::Pending
            }
        });
        let again = |a: i64| {
            let held = running.get();
            futures_util::future::Either::Right(async move { Ok::<_, String>((!held).then_some(a * 10)) })
        };
        let mut got = std::pin::pin!(super::link(Some((1, futures_util::future::Either::Left(guessed))), prev, again));
        let got = loop {
            if let Some(v) = got.as_mut().now_or_never() {
                break v;
            }
        };
        assert_eq!(got, Ok(Some((2, 20))), "the read again ran while the guessed read was still held");
    }

    /// The git read gate on a cached `Authz`: a decision that does not allow is made again on a fresh read (a grant
    /// opens at once), and one that allows is kept from the fold's read, not from now, so a revocation binds within one
    /// `READ_CACHE`. A fresh first read is never read again, and a failure is not retried.
    #[test]
    fn a_gate_closed_on_a_cached_authz_reads_it_fresh_and_an_open_one_ages_from_the_folds_read() {
        use futures_util::FutureExt;
        use graftyard_identity::Decision::{Allow, Hidden};

        let at = Timestamp(20_000);
        let run = |held: Option<Timestamp>, first: Result<graftyard_identity::Decision, String>, fresh: bool| {
            let read = std::cell::Cell::new(0);
            let out = reopened(
                held,
                at,
                first,
                || {
                    read.set(read.get() + 1);
                    std::future::ready(fresh)
                },
                |open: bool| Ok(if open { Allow } else { Hidden }),
            )
            .now_or_never()
            .expect("ready");
            (out, read.get())
        };
        assert_eq!(run(Some(Timestamp(12_000)), Ok(Hidden), true), (Ok((Allow, at)), 1));
        assert_eq!(run(Some(Timestamp(12_000)), Ok(Hidden), false), (Ok((Hidden, at)), 1));
        assert_eq!(run(Some(Timestamp(12_000)), Ok(Allow), false), (Ok((Allow, Timestamp(12_000))), 0));
        assert_eq!(run(None, Ok(Hidden), true), (Ok((Hidden, at)), 0));
        assert_eq!(run(None, Ok(Allow), true), (Ok((Allow, at)), 0));
        assert_eq!(run(Some(Timestamp(12_000)), Err("down".into()), true), (Err("down".into()), 0));
    }

    /// `cold-write` C: an advertisement puts its `Allow` for a POST only when the caller may push, on the same terms
    /// as `Push::decide`'s `may_write`: a `Write` grant passes; a `Read` grant, a token whose own reach is
    /// `contents: read` or another owner, and anyone on a deleted repository do not.
    #[test]
    fn only_a_caller_who_may_push_has_an_allow_a_post_can_take() {
        use graftyard_identity::{
            AccountKind, Actor, AuthzEvent, AuthzFold, AuthzRead, Credential, PermissionSet, Principal, RepoPermission,
            ScopeSet, TokenFields, UserId,
        };
        use graftyard_raft::Fold;
        let (owner, repo) = (AccountId(DbId(70)), RepoId(DbId(9)));
        let user = |id: i64| Principal {
            actor: Actor::User(UserId(AccountId(DbId(id)))),
            scopes: ScopeSet(1),
            credential: Credential::Stored([id as u8; 32]),
            sso_orgs: smallvec::SmallVec::new(),
        };
        let mut fold = AuthzFold::new(owner);
        fold.apply(&AuthzEvent::Opened { kind: AccountKind::User, enterprise: None });
        for (who, p) in [(71, RepoPermission::Write), (72, RepoPermission::Read)] {
            fold.apply(&AuthzEvent::Grant { repo, grantee: UserId(AccountId(DbId(who))), permission: Some(p) });
        }
        let folds: [&dyn AuthzRead; 1] = [&fold];
        let core = graftyard_fold::MemStore::default();
        let access = graftyard_git::issue_ops::Access { repo, owner, core: &core };
        let fields = |level, resource_owner| TokenFields {
            permissions: Some(PermissionSet([(Box::from("contents"), level)].into_iter().collect())),
            resource_owner,
            ..TokenFields::default()
        };
        let may = |p: &Principal, f: Option<&TokenFields>| super::may_push(p, f, &folds, &access, &core);
        assert!(may(&user(71), None), "a Write grant may push");
        assert!(may(&user(70), None), "the owner may push");
        assert!(!may(&user(72), None), "a Read grant's Allow is a fetch's, never a push's");
        assert!(!may(&user(73), None), "no grant");
        let w = graftyard_identity::Access::Write;
        assert!(may(&user(71), Some(&fields(w, None))));
        assert!(!may(&user(71), Some(&fields(graftyard_identity::Access::Read, None))), "a contents: read token");
        assert!(!may(&user(71), Some(&fields(w, Some(AccountId(DbId(99)))))), "a token for another owner");
        let mut gone = graftyard_fold::MemStore::default();
        let tomb = postcard::to_allocvec(&(Timestamp(5), None::<AccountId>)).expect("encodes");
        let row =
            graftyard_fold::Stored { written: Default::default(), cols: vec![graftyard_fold::Val::Blob(tomb.into())] };
        graftyard_fold::Store::put(&mut gone, "deleted", Vec::new(), row);
        let access = graftyard_git::issue_ops::Access { repo, owner, core: &gone };
        assert!(!super::may_push(&user(71), None, &folds, &access, &gone), "a deleted repository");
    }

    /// An owner's `Authz` read its members shed keeps the shed reason through the read gate, so the git and LFS gates
    /// answer 503 with `Retry-After: 1`; a group with no answer is 503 with 5.
    #[test]
    fn a_shed_authz_read_is_a_short_503_at_the_read_gate() {
        let shed = format!("authz:7: every member asked shed the read: {}", graftyard_lookup::OVERLOADED);
        let e = super::gate_owner(Err(shed)).expect_err("shed");
        let r = crate::api::read_failed("the repository's access rows did not answer; retry", &e);
        assert_eq!((r.status, r.header("retry-after")), (503, Some("1")), "{e}");
        let e = super::gate_owner(Ok(None)).expect_err("no group");
        let r = crate::api::read_failed("the repository's access rows did not answer; retry", &e);
        assert_eq!((r.status, r.header("retry-after")), (503, Some("5")), "{e}");
        assert!(super::gate_owner(Ok(Some(graftyard_identity::AuthzFold::default()))).is_ok());
    }

    /// BM-5: `FORCE_FORMAT_SITE` forces the test format only on a push whose chain's leader sits on that site; any other
    /// leader, or one this isolate has not seen, gets the newest enabled format.
    #[test]
    fn bm5_a_forced_format_aimed_at_a_site_reaches_only_that_site_s_leaders() {
        let test = graftyard_git::repo::REPO_FORMAT_TEST;
        let c = |force_site| super::Config {
            plan: super::Plan::Pro,
            ssh_option: false,
            cap: 1,
            host: String::new(),
            enabled: vec![graftyard_git::repo::REPO_FORMAT_V2],
            force_format: Some(test),
            force_site,
            part: 1,
        };
        let aimed = c(Some(SiteId(1)));
        assert_eq!(aimed.push_format(Some(SiteId(1))), test);
        assert_eq!(aimed.push_format(Some(SiteId(2))), graftyard_git::repo::REPO_FORMAT_V2);
        assert_eq!(aimed.push_format(None), graftyard_git::repo::REPO_FORMAT_V2);
        assert_eq!(c(None).push_format(Some(SiteId(2))), test, "unaimed, BF-16 forces every push");
    }
}
