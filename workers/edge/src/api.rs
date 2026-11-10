//! REST, GraphQL, LFS, uploads, and codeload on the edge (`build/BUILD-PLAN.md`, M2-WP2 and M2-WP4): `crates/api`'s
//! handlers over a repository's tiers, read through `fresh` on its group.
//!
//! - A read is one `{"q": {"rows": true}}` read of the repository's leader: both tiers at one index, loaded into
//!   `MemStore`s and read through `graftyard_api::view::Repo`. A repository over `hosted::ROWS_MAX` rows answers 503.
//! - The `Directory` is `hosted::HostedDir`: the owner's `Authz` fold, read as a `dep` through `fresh`, the owner's
//!   login from the path, every other account's login from its `AccountLogin` row on its `account_shard` (F50), and
//!   the signed-in account from its token.
//! - A write is an `IssueCall` committed through `commit_hedged`. Its leader renders the reply from the `&Acked` over
//!   its own tiers (`hosted::render_ok`), and the edge adds only the cross-cutting headers (`graftyard_wire::finish`).
//!   The edge sends one only once `FormatsEnable` has enabled `REPO_FORMAT_ISSUES`; until then a write answers 503.
//! - Upload and LFS transfer bytes go to the placement's buckets through `BlobStore::place`, and are read back through
//!   `BlobStore::read`. LFS `verify` and the asset upload commit an `IssueCall` naming the `BlobRef`; the leader
//!   rebuilds the `DurableBlob` with `BlobStore::confirm` before it decides (BF-25).
//! - `DELETE /git/refs/{ref}` is a `Push`, M1's write path.
//! - A merge (`mergePullRequest`, `PUT /pulls/{n}/merge`) reads `merge_request` from the fresh core tier, asks the
//!   leader site's shard of the network for the `MergePlan` (`GitCacheShard::merge`, the container's `/merge` with
//!   `method`), and
//!   commits `Merge` as a `MergeCall`; `enqueuePullRequest`, `enablePullRequestAutoMerge` on a queued base, and
//!   `dequeuePullRequest` commit a `QueueOp`. All are format 32.4, sent only once `FormatsEnable` enabled it.

use graftyard_api::exec::Request as Gql;
use graftyard_api::hosted::{HostedDir, Render, TierRows, What, WireReply};
use graftyard_api::view::{Repo as View, RepoRef};
use graftyard_api::{graphql, lfs, release, rest};
use graftyard_fold::MemStore;
use graftyard_git::blob::{ArchiveFormat, ArchiveRequest, Archiver, BlobRef, BlobStore};
use graftyard_git::group::{CommitReq, PushOutcome};
use graftyard_git::host::R2;
use graftyard_git::issue_ops::IssueOp;
use graftyard_git::issue_ops::call::{IssueCall, REPO_FORMAT_ISSUES};
use graftyard_git::merge::{MergeCmd, MergeError, MergePlan, MergeRequest, REPO_FORMAT_V4};
use graftyard_git::merge_cache::{CacheError, ChainAnswer};
use graftyard_git::merge_call::MergeCall;
use graftyard_git::queue::{QueueAction, QueueOp};
use graftyard_git::refs::Atomicity;
use graftyard_git::repo::Push;
use graftyard_git::{Copies, ObjectMeta};
use graftyard_identity::{Actor, AuthzFold, Principal};
use graftyard_types::{ChainId, NetworkId, SiteId};
use graftyard_wire::{Body, Endpoint, Host, Incoming, Meta, RateLimits, Reply, Surface};
use serde::Deserialize as _;
use serde_json::{Value, json};
use worker::*;

use crate::{Ctx, Repo, authenticate, ensure_group, resolve};

/// Whether `endpoint` is one this module serves; `Repo` and `Web` are git's or nobody's.
pub fn serves(e: &Endpoint<'_>) -> bool {
    !matches!(e, Endpoint::Repo { .. } | Endpoint::Web(_))
}

fn surface(e: &Endpoint<'_>) -> Surface {
    match e {
        Endpoint::Rest(_) | Endpoint::Uploads(_) => Surface::Rest,
        Endpoint::Graphql => Surface::Graphql,
        Endpoint::Lfs { .. } => Surface::Lfs,
        _ => Surface::Content,
    }
}

/// Frames a reply: the cross-cutting headers, then the Worker response.
fn frame(i: &Incoming<'_>, r: Reply, s: Surface, m: &Meta) -> Result<Response> {
    let r = graftyard_wire::finish(i, r, s, m);
    let h = Headers::new();
    for (k, v) in &r.headers {
        h.set(k, v)?;
    }
    let resp = match r.body {
        Body::Json(v) => Response::from_bytes(serde_json::to_vec(&v)?)?,
        Body::Bytes(b) => Response::from_bytes(b)?,
        Body::Empty => Response::empty()?,
    };
    Ok(resp.with_status(r.status).with_headers(h))
}

/// Test installs: each phase of one request and its time in ms, sent back as `server-timing`, so a latency gate can
/// say where the time went. A worker isolate runs one request's future at a time per `serve` call chain, and `serve`
/// clears the list first.
#[cfg(feature = "test-faults")]
mod timing {
    use std::cell::RefCell;
    thread_local! { static PHASES: RefCell<(f64, Vec<(&'static str, f64)>)> = RefCell::default(); }
    pub fn start() {
        PHASES.with(|p| *p.borrow_mut() = (js_sys::Date::now(), Vec::new()));
    }
    /// Closes the phase that ends now.
    pub fn mark(name: &'static str) {
        PHASES.with(|p| {
            let mut p = p.borrow_mut();
            let now = js_sys::Date::now();
            let last = p.0;
            p.1.push((name, now - last));
            p.0 = now;
        });
    }
    thread_local! { static TRACE: RefCell<Vec<serde_json::Value>> = const { RefCell::new(Vec::new()) }; }
    /// The commit's hedge steps, sent back as `x-graft-trace`.
    pub fn trace(t: Vec<serde_json::Value>) {
        TRACE.with(|x| *x.borrow_mut() = t);
    }
    pub fn header() -> String {
        PHASES.with(|p| p.borrow().1.iter().map(|(n, d)| format!("{n};dur={d:.0}")).collect::<Vec<_>>().join(", "))
    }
    thread_local! { static NOTE: RefCell<String> = const { RefCell::new(String::new()) }; }
    /// The shard's own timing of its merge plan, sent back as `x-graft-shard-timing`.
    pub fn note(t: String) {
        NOTE.with(|n| *n.borrow_mut() = t);
    }
    pub fn take_note() -> String {
        NOTE.with(|n| std::mem::take(&mut *n.borrow_mut()))
    }
    pub fn take_trace() -> String {
        TRACE.with(|x| serde_json::Value::from(std::mem::take(&mut *x.borrow_mut())).to_string())
    }
}
#[cfg(not(feature = "test-faults"))]
mod timing {
    pub fn start() {}
    pub fn mark(_: &'static str) {}
    pub fn trace(_: Vec<serde_json::Value>) {}
}

/// A commit's hedge steps for this request's `x-graft-trace` (test installs only).
#[cfg(feature = "test-faults")]
pub(crate) fn trace(t: Vec<Value>) {
    timing::trace(t);
}

pub(crate) fn unavailable(why: &str) -> Reply {
    Reply::error(503, why, "rest").with("retry-after", "5")
}

/// One request to a surface this module serves.
pub async fn serve(c: &Ctx<'_>, req: &mut Request, host: &Host, e: Endpoint<'_>) -> Result<Response> {
    let s = surface(&e);
    let (ua, ver, accept, inm) = (
        req.headers().get("user-agent")?,
        req.headers().get("x-github-api-version")?,
        req.headers().get("accept")?,
        req.headers().get("if-none-match")?,
    );
    let i = Incoming {
        user_agent: ua.as_deref(),
        api_version: ver.as_deref(),
        accept: accept.as_deref(),
        if_none_match: inm.as_deref(),
    };
    let version = match graftyard_wire::serve(&i, s) {
        Ok(v) => v,
        Err(r) => {
            return frame(
                &i,
                r,
                s,
                &Meta { rid: c.rid, version: graftyard_wire::ApiVersion::V2022_11_28, rate: RateLimits::Off },
            );
        }
    };
    let m = Meta { rid: c.rid, version, rate: RateLimits::Off };
    // The colo this edge ran in, so a latency gate can show where the client entered (BM-2's home-site clause).
    #[cfg(feature = "test-faults")]
    let colo = req.cf().map(|c| c.colo()).unwrap_or_default();
    timing::start();
    let r = match route(c, req, host, e, &m).await {
        Ok(r) => r,
        Err(err) => unavailable(&format!("Service Unavailable: {err}")),
    };
    #[cfg(feature = "test-faults")]
    let r = r
        .with("server-timing", timing::header())
        .with("x-graft-trace", timing::take_trace())
        .with("x-graft-shard-timing", timing::take_note())
        .with("x-graft-colo", colo);
    frame(&i, r, s, &m)
}

/// What a handler reads: the repository, its tiers at one index, and the directory.
pub(crate) struct Read {
    pub(crate) repo: Repo,
    pub(crate) at: RepoRef,
    pub(crate) core: MemStore,
    meta: MemStore,
    pub(crate) dir: HostedDir,
    /// The leader's applied index the tiers were read at, which `GET /repos/{o}/{r}`'s ETag names.
    pub(crate) index: u64,
}

impl Read {
    pub(crate) fn view<'a>(&'a self, host: &'a Host) -> View<'a> {
        View { host, at: self.at.clone(), core: &self.core, meta: &self.meta, dir: &self.dir }
    }

    /// The same read under another name, as a rename's `Settings` renders under the new one.
    pub(crate) fn clone_at(&self, at: RepoRef) -> Read {
        Read {
            repo: self.repo.clone(),
            at,
            core: self.core.clone(),
            meta: self.meta.clone(),
            dir: self.dir.clone(),
            index: self.index,
        }
    }
}

/// `read_repo` of a repository `crate::repos` just created, under the login and name it was created with, behind the
/// same read gate as every repository route.
pub(crate) async fn read_new(
    c: &Ctx<'_>,
    repo: Repo,
    owner: &str,
    name: &str,
    viewer: Option<&Principal>,
) -> Result<std::result::Result<Read, Reply>> {
    read_repo(c, repo, owner, name, viewer, None, Rows::All, Gate::Readers).await
}

/// `o/r` resolved, then its tiers and the owner's `Authz` read through `fresh`. `Ok(Err(reply))` when the repository
/// is unknown, cannot be read now, or is hidden from `viewer` (404).
pub(crate) async fn read(
    c: &Ctx<'_>,
    owner: &str,
    name: &str,
    viewer: Option<&Principal>,
) -> Result<std::result::Result<Read, Reply>> {
    let repo = match resolve(c, owner, name).await? {
        Ok(Some(r)) => r,
        Ok(None) => return Ok(Err(Reply::not_found())),
        Err(e) => return Ok(Err(unavailable(&format!("the repository lookup failed; retry ({e})")))),
    };
    read_repo(c, repo, owner, name, viewer, None, Rows::All, Gate::Readers).await
}

/// `read` for a command that decides on the core tier alone (a lease command, L0-16): served while the metadata
/// tier loads, with an empty metadata view, behind the same read gate as `read`.
pub(crate) async fn read_core(
    c: &Ctx<'_>,
    owner: &str,
    name: &str,
    viewer: Option<&Principal>,
) -> Result<std::result::Result<Read, Reply>> {
    let repo = match resolve(c, owner, name).await? {
        Ok(Some(r)) => r,
        Ok(None) => return Ok(Err(Reply::not_found())),
        Err(e) => return Ok(Err(unavailable(&format!("the repository lookup failed; retry ({e})")))),
    };
    read_repo(c, repo, owner, name, viewer, None, Rows::Core, Gate::Readers).await
}

/// `read` for the invitee's side of a repository invitation (`crate::collab::user_invitation`), with no read gate: an
/// invitee is not yet a collaborator. The caller answers 404 unless the viewer holds an invitation or `readable`.
pub(crate) async fn read_for_invitee(
    c: &Ctx<'_>,
    owner: &str,
    name: &str,
    viewer: &Principal,
) -> Result<std::result::Result<Read, Reply>> {
    let repo = match resolve(c, owner, name).await? {
        Ok(Some(r)) => r,
        Ok(None) => return Ok(Err(Reply::not_found())),
        Err(e) => return Ok(Err(unavailable(&format!("the repository lookup failed; retry ({e})")))),
    };
    read_repo(c, repo, owner, name, Some(viewer), None, Rows::All, Gate::Invitees).await
}

/// Which callers `read_repo` serves: those `authorize_read` allows, or anyone, for a route that checks the caller
/// against the repository's invitations itself.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Gate {
    Readers,
    Invitees,
}

/// `graftyard_identity::authorize_read` over the core rows a read holds and the owner's `Authz` it read.
fn may_read(
    c: &Ctx<'_>,
    repo: &Repo,
    core: &MemStore,
    authz: Option<&AuthzFold>,
    viewer: Option<&Principal>,
) -> graftyard_identity::Decision {
    let access = graftyard_git::issue_ops::Access { repo: repo.id, owner: repo.owner, core };
    let folds: Vec<&dyn graftyard_identity::AuthzRead> = authz.iter().map(|a| *a as _).collect();
    graftyard_identity::authorize_read(viewer, c.token_fields.borrow().as_ref(), &folds, &access)
}

/// Whether `viewer` may read the repository `rd` holds, as `read` checks it.
pub(crate) fn readable(c: &Ctx<'_>, rd: &Read, viewer: &Principal) -> bool {
    may_read(c, &rd.repo, &rd.core, rd.dir.authz.as_ref(), Some(viewer)) == graftyard_identity::Decision::Allow
}

/// `read` of a repository already resolved: its tiers and the owner's `Authz`, both read through `fresh` at once.
/// `known` is the owner's `Authz` this request already read through `fresh` to resolve the repository, if it did.
/// With `Rows::Merge`, only the rows a `mergePullRequest` plans from are read (`TierRows::merge_rows`), or taken as
/// the caller already read them, and `dir.logins` is left for the caller to read alongside the merge plan.
#[expect(clippy::too_many_arguments, reason = "each caller supplies a different subset of what it already read")]
async fn read_repo(
    c: &Ctx<'_>,
    repo: Repo,
    owner: &str,
    name: &str,
    viewer: Option<&Principal>,
    known: Option<AuthzFold>,
    rows: Rows,
    gate: Gate,
) -> Result<std::result::Result<Read, Reply>> {
    let merge = match &rows {
        Rows::All | Rows::Core => None,
        Rows::Merge(id, _) => Some(*id),
    };
    let core_only = matches!(rows, Rows::Core);
    timing::mark("resolve");
    let read_authz = async {
        match known {
            Some(a) => Some(a),
            None => authz(c, &repo).await,
        }
    };
    let read_rows = async {
        match rows {
            Rows::Merge(_, Some(v)) => v,
            _ => tier_rows(c, &repo, merge).await,
        }
    };
    let (v, authz) = futures_util::future::join(read_rows, read_authz).await;
    let v = match v? {
        Ok(v) => v,
        Err(r) => return Ok(Err(r)),
    };
    // A member built before the `rows` read answered `ranges`: its tiers cannot be served.
    let Ok(rows) = TierRows::from_value(&v["value"]) else {
        return Ok(Err(unavailable("the repository's members do not serve tier rows yet (site release)")));
    };
    if !rows.complete {
        return Ok(Err(unavailable("the repository is too large for this edge's tier read")));
    }
    let no_meta = Vec::new();
    let Some(meta) = served_meta(&rows, core_only, &no_meta) else {
        return Ok(Err(unavailable("the repository's metadata tier is loading; retry")));
    };
    timing::mark("rows_authz");
    // The read gate (`crate::read_gate`), on the rows this read already holds: every read and merge row set carries
    // `settings` and the `collaborator` rows. A private repository whose owner's `Authz` did not answer is 503, not a
    // 404 the caller would take as "no such repository".
    let core = TierRows::store(&rows.core);
    if gate == Gate::Readers {
        match may_read(c, &repo, &core, authz.as_ref(), viewer) {
            graftyard_identity::Decision::Allow => {}
            _ if authz.is_none() => return Ok(Err(unavailable("the owner's Authz chain did not answer; retry"))),
            _ => return Ok(Err(Reply::not_found())),
        }
    }
    let at = RepoRef { id: repo.id, owner: owner.to_owned(), owner_id: repo.owner, name: name.to_owned() };
    let mut dir = HostedDir { home: Some(at.clone()), authz, viewer: None, ..HostedDir::default() };
    dir.viewer = viewer.and_then(|p| match p.actor {
        Actor::User(u) => Some(u.0),
        Actor::App { .. } | Actor::Anonymous => None,
    });
    let meta = TierRows::store(meta);
    let mut rd = Read { repo, at, core, meta, dir, index: rows.index };
    if merge.is_none() {
        rd.dir.logins = read_logins(c, &rd).await;
    }
    Ok(Ok(rd))
}

/// The metadata rows a read serves: the member's, or, while that tier loads, `empty` for a core-only read and none
/// otherwise.
pub(crate) fn served_meta<'a>(
    rows: &'a TierRows,
    core_only: bool,
    empty: &'a Vec<(String, graftyard_fold::Key, Vec<graftyard_fold::Val>)>,
) -> Option<&'a Vec<(String, graftyard_fold::Key, Vec<graftyard_fold::Val>)>> {
    match rows.meta.as_ref() {
        Some(m) => Some(m),
        None => core_only.then_some(empty),
    }
}

/// Which tier rows `read_repo` reads: both whole tiers; both, served with an empty metadata view while that tier
/// loads (`Core`); or a merge's rows (`TierRows::merge_rows`) for the pull request with this database id, with the
/// leader's answer if the caller already read it.
enum Rows {
    All,
    Core,
    Merge(i64, Option<Result<std::result::Result<Value, Reply>>>),
}

/// The logins of every account `rd`'s tiers name, the viewer's, and the owner's from the request path (F50).
async fn read_logins(c: &Ctx<'_>, rd: &Read) -> std::collections::BTreeMap<i64, String> {
    let mut named = graftyard_api::hosted::accounts(&rd.core, &rd.meta);
    named.extend(rd.dir.viewer);
    named.remove(&rd.repo.owner);
    let mut out = account_logins(c, &named).await;
    out.insert(rd.repo.owner.0.0, rd.at.owner.clone());
    for (id, l) in logins(c) {
        out.entry(id).or_insert(l);
    }
    out
}

/// The repository's `{"rows": true}` read on its leader, creating its group on first use; `{"rows": {"merge": id}}`
/// for a merge's rows only, which a member built before it answers with every row.
pub(crate) async fn tier_rows(
    c: &Ctx<'_>,
    repo: &Repo,
    merge: Option<i64>,
) -> Result<std::result::Result<Value, Reply>> {
    let chain = ChainId::Repo(repo.id);
    let b = c.bindings();
    let q = tier_rows_query(merge);
    let mut v = graftyard_lookup::call_opt(&b, chain, &repo.record, "/read", q.as_bytes()).await;
    if let Ok(None) = v {
        ensure_group(c, repo).await.map_err(Error::RustError)?;
        v = graftyard_lookup::call_opt(&b, chain, &repo.record, "/read", q.as_bytes()).await;
    }
    Ok(match v {
        Ok(Some((v, _, _))) => Ok(v),
        Ok(None) => Err(unavailable("the repository's group has no leader; retry")),
        Err(e) => Err(unavailable(&e)),
    })
}

/// `tier_rows`'s query. `ranges` too: a member built before the `rows` read answers its small ranges reply instead of
/// every ref. `packed`: the whole tiers as one JSON string (`TierRows::packed`), which `TierRows::from_value` reads.
fn tier_rows_query(merge: Option<i64>) -> String {
    let rows = merge.map_or(json!(true), |id| json!({ "merge": id }));
    json!({ "q": { "rows": rows, "ranges": true, "packed": true } }).to_string()
}

/// The repository's narrow `{"rows": {which: true}}` core read on its leader: `access` for `crate::read_gate`, or
/// `agent` for an agent push. `Ok(Err(why))` when the leader cannot answer it now.
pub(crate) async fn core_rows(c: &Ctx<'_>, repo: &Repo, which: &str) -> Result<std::result::Result<MemStore, String>> {
    let chain = ChainId::Repo(repo.id);
    let b = c.bindings();
    let q = json!({ "q": { "rows": { which: true }, "ranges": true } }).to_string();
    let mut v = graftyard_lookup::call_opt(&b, chain, &repo.record, "/read", q.as_bytes()).await;
    if let Ok(None) = v {
        ensure_group(c, repo).await.map_err(Error::RustError)?;
        v = graftyard_lookup::call_opt(&b, chain, &repo.record, "/read", q.as_bytes()).await;
    }
    Ok(match v {
        Ok(Some((v, _, _))) => match TierRows::from_value(&v["value"]) {
            Ok(rows) => Ok(TierRows::store(&rows.core)),
            Err(_) => Err("the repository's members do not serve tier rows yet (site release)".into()),
        },
        Ok(None) => Err("the repository's group has no leader".into()),
        Err(e) => Err(e),
    })
}

/// The owner's `Authz` fold, read as a `dep` through `fresh`, for permissions and `author_association`.
async fn authz(c: &Ctx<'_>, repo: &Repo) -> Option<AuthzFold> {
    let chain = ChainId::Authz(repo.owner);
    let q = json!({ "q": { "dep": true } }).to_string();
    let (v, _, _) =
        graftyard_lookup::call_opt(&c.bindings(), chain, &repo.record, "/read", q.as_bytes()).await.ok()??;
    let bytes = hex(v["value"].as_str()?)?;
    graftyard_raft::codec::from_bytes(&bytes).ok()
}

fn hex(s: &str) -> Option<Vec<u8>> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

/// The placement's buckets, home site first.
fn buckets(c: &Ctx<'_>, repo: &Repo) -> Result<Vec<(SiteId, R2)>> {
    let mut out: Vec<(SiteId, R2)> =
        crate::buckets(c.env, c.sites)?.into_iter().filter(|(s, _)| repo.record.sites.contains(s)).collect();
    out.sort_by_key(|(s, _)| *s != repo.record.home);
    Ok(out)
}

fn can_write(r: &View<'_>, who: Option<&Principal>) -> bool {
    match who.map(|p| p.actor) {
        Some(Actor::User(u)) => r.dir.permission(u.0) >= Some(graftyard_identity::RepoPermission::Write),
        _ => false,
    }
}

async fn route(c: &Ctx<'_>, req: &mut Request, host: &Host, e: Endpoint<'_>, m: &Meta) -> Result<Reply> {
    let method = req.method();
    let url = req.url()?;
    let query = url.query().unwrap_or("").to_owned();
    match e {
        Endpoint::Status => Ok(rest::status()),
        Endpoint::Rest("" | "/") => Ok(rest::root(host)),
        Endpoint::Rest("/meta") => Ok(rest::meta()),
        Endpoint::Rest("/status") => Ok(rest::status()),
        Endpoint::Rest("/rate_limit") => Ok(rest::rate_limit(&m.rate, m.version)),
        Endpoint::Rest(p) => match (&method, p.trim_end_matches('/').split('/').skip(1).collect::<Vec<_>>().as_slice())
        {
            (Method::Post, ["app", "installations", id, "access_tokens"]) => {
                crate::ident::access_tokens(c, req, id).await
            }
            (Method::Delete, ["installation", "token"]) => crate::ident::delete_installation_token(c, req).await,
            (Method::Delete, ["applications", client, "token"]) => crate::ident::revoke_app_token(c, req, client).await,
            (Method::Get, ["user"]) => crate::ident::get_user(c, req).await,
            (Method::Get, ["users", login]) => crate::ident::get_named_user(c, login).await,
            (m @ (Method::Get | Method::Put | Method::Delete), ["orgs", org, "memberships", user]) => {
                crate::ident::membership(c, req, m.clone(), org, user).await
            }
            (Method::Post, ["repos", owner, repo, "transfer"]) => crate::transfer::transfer(c, req, owner, repo).await,
            (Method::Delete, ["orgs", org, "members", user]) => {
                crate::ident::membership(c, req, Method::Delete, org, user).await
            }
            (Method::Get, ["user", "keys"]) => crate::keys::list(c, req).await,
            (Method::Post, ["user", "keys"]) => crate::keys::add(c, req).await,
            (_, ["orgs", _, "teams", ..] | ["user", "memberships", "orgs", _]) => {
                crate::collab::org_route(c, req, p).await
            }
            (Method::Post, ["orgs", org, "repos"]) => {
                let body = req.json().await.unwrap_or(Value::Null);
                crate::repos::create(c, req, crate::repos::Via::Org((*org).to_owned()), body).await
            }
            (Method::Post, ["user", "repos"]) => {
                let body = req.json().await.unwrap_or(Value::Null);
                crate::repos::create(c, req, crate::repos::Via::User, body).await
            }
            (Method::Get, ["user", "repository_invitations"]) => crate::collab::user_invitation(c, req, None).await,
            (Method::Patch | Method::Delete, ["user", "repository_invitations", id]) => {
                crate::collab::user_invitation(c, req, Some(id)).await
            }
            _ => rest_repo(c, req, host, p, &query).await,
        },
        Endpoint::Graphql => graphql_post(c, req, host).await,
        Endpoint::Uploads(p) => upload(c, req, host, p, &query).await,
        Endpoint::Lfs { owner, repo, rest } => lfs_route(c, req, host, owner, repo, rest, &query, method).await,
        Endpoint::Objects(p) => transfer(c, req, host, p, method).await,
        Endpoint::Codeload { owner, repo, rest } => codeload(c, req, host, owner, repo, rest).await,
        Endpoint::Raw { .. } => Ok(Reply::not_found()),
        Endpoint::OAuth(p) => match (method, p) {
            (Method::Post, "/login/device/code") => crate::ident::device_code(c, req).await,
            (Method::Post, "/login/oauth/access_token") => crate::ident::access_token(c, req).await,
            (Method::Get | Method::Post, "/login/oauth/authorize") => crate::ident::authorize(c, req).await,
            (Method::Post, "/login/oauth/device/approve") => crate::ident::device_approve(c, req).await,
            (Method::Post, "/login/oauth/link") => crate::ident::sign_in_link(c, req).await,
            _ => Ok(Reply::not_found()),
        },
        Endpoint::Repo { .. } | Endpoint::Web(_) => Ok(Reply::not_found()),
    }
}

// ---- writes ----

/// Commits an issue command and returns the reply its leader rendered.
pub(crate) async fn commit_issue(
    c: &Ctx<'_>,
    rd: &Read,
    who: &Principal,
    op: IssueOp<BlobRef>,
    what: What,
) -> Result<Reply> {
    commit_issue_in(c, rd, who, op, what, None).await
}

/// `commit_issue` with the session a pull request's creator named in `X-Graftyard-Agent-Session` (AM1-WP3), which
/// `decide` records in `AgentMeta` only for a task of the creating App. Carried only on `OpenPr` by an App's token or
/// a user-to-server token, and only once 32.12 is enabled; otherwise the header changes nothing.
async fn commit_issue_in(
    c: &Ctx<'_>,
    rd: &Read,
    who: &Principal,
    op: IssueOp<BlobRef>,
    what: What,
    session: Option<graftyard_identity::SessionId>,
) -> Result<Reply> {
    if !c.cfg.enabled.contains(&REPO_FORMAT_ISSUES) {
        return Ok(unavailable(&format!(
            "issue writes need record format {}.{}, which FormatsEnable has not enabled on this install",
            REPO_FORMAT_ISSUES.family, REPO_FORMAT_ISSUES.version
        )));
    }
    let render = Render { host: c.cfg.host.clone(), isolation: false, dir: rd.dir.clone(), what };
    let mut call = IssueCall::new(op, who.clone(), c.rid, serde_json::to_value(&render)?);
    // Format 32.8, once `FormatsEnable` enabled it, only for a command that can write an agent event: a pull request
    // opened by an App or with a user-to-server token (`AgentMeta` when its App is an `AgentApp`), and a review of a
    // pull request that has `AgentMeta` (`AgentGate`). Every other command keeps its format and its bytes; under a
    // `ghs_` token that is 32.9.
    let agent_event = match &call.op {
        IssueOp::OpenPr { .. } => c.token_app.get().is_some() || matches!(who.actor, Actor::App { .. }),
        IssueOp::Review { n, .. } => graftyard_git::tiers::agent_meta(&rd.core, *n).is_some(),
        _ => false,
    };
    call.agent = agent_event && c.cfg.enabled.contains(&graftyard_git::repo::REPO_FORMAT_AGENT);
    if call.agent && matches!(call.op, IssueOp::OpenPr { .. }) {
        call.session = session.filter(|_| c.cfg.enabled.contains(&graftyard_git::repo::REPO_FORMAT_TASKS));
    }
    // 32.14 (AM1-WP4) only for a command whose entry may write lease events (`graftyard_git::lease::marks`).
    call.leases = !call.op.is_lease()
        && c.cfg.enabled.contains(&graftyard_git::repo::REPO_FORMAT_LEASES)
        && graftyard_git::lease::marks(&call.op, &rd.core);
    if !c.cfg.enabled.contains(&call.format()) {
        let f = call.format();
        return Ok(unavailable(&format!(
            "this write needs record format {}.{}, which FormatsEnable has not enabled on this install",
            f.family, f.version
        )));
    }
    let mut body = json!({ "rid": c.rid.to_string(), "cmd": { "issue": call } });
    if let Some(a) = crate::admission(c, who, &[], false, 0).await {
        body["admit"] = serde_json::to_value(a)?;
    }
    // `rd` came from the group's leader, so the group exists: no create probe before the commit.
    let r = graftyard_lookup::commit_hedged(
        &c.bindings(),
        ChainId::Repo(rd.repo.id),
        &rd.repo.record,
        body.to_string().as_bytes(),
        c.colo,
        60_000,
        graftyard_lookup::Hooks::default(),
    )
    .await;
    Ok(match r {
        Ok(done) => {
            // The refusal's cause (`IssueErr`) is not in the client's reply, so it is logged.
            if let Some(e) = done.pointer("/refused/err") {
                worker::console_log!("issue command {} refused: {e}", c.rid);
            }
            let wire = done
                .pointer("/ok/out/issue/reply")
                .or_else(|| done.pointer("/refused/issue"))
                .and_then(|v| serde_json::from_value::<WireReply>(v.clone()).ok());
            match wire {
                Some(w) => w.reply(),
                None => unavailable(&format!("the member answered {done}")),
            }
        }
        Err(Ok(e)) => match e.pointer("/unsupported") {
            Some(u) => unavailable(&format!("the repository's members do not read record format {u}")),
            None => unavailable(&format!("{e}")),
        },
        Err(Err(e)) => unavailable(&e),
    })
}

/// `GET` and `PUT /api/graftyard/v1/repos/{o}/{r}/agents/policy` (`crate::agents`): the repository's `AgentPolicy`,
/// for a repository admin. The write is `SetAgentPolicy`, format 32.8, whose `decide` checks the admin right.
pub(crate) async fn agent_policy(
    c: &Ctx<'_>,
    _req: &mut Request,
    owner: &str,
    name: &str,
    who: Option<Principal>,
    method: Method,
    body: &Value,
) -> Result<Reply> {
    let Some(who) = who else { return Ok(Reply::error(401, "Requires authentication", "rest")) };
    if !c.cfg.enabled.contains(&graftyard_git::repo::REPO_FORMAT_AGENT) {
        return Ok(unavailable("agent settings need record format 32.8, which FormatsEnable has not enabled"));
    }
    let rd = match read(c, owner, name, Some(&who)).await? {
        Ok(r) => r,
        Err(r) => return Ok(r),
    };
    let admin = match who.actor {
        Actor::User(u) => {
            graftyard_api::view::Directory::permission(&rd.dir, u.0) >= Some(graftyard_identity::RepoPermission::Admin)
        }
        _ => false,
    };
    match method {
        Method::Get if admin => {
            Ok(Reply::json(200, graftyard_api::agents::policy_json(&graftyard_git::tiers::agent_policy(&rd.core))))
        }
        Method::Get => Ok(graftyard_api::agents::refused(403, "")),
        Method::Put => match graftyard_api::agents::parse_policy(body) {
            Ok(p) => commit_issue(c, &rd, &who, IssueOp::SetAgentPolicy(p), What::AgentPolicy).await,
            Err(r) => Ok(r),
        },
        _ => Ok(Reply::error(405, "Method Not Allowed", "graftyard/agents")),
    }
}

/// The task API under `/api/graftyard/v1/repos/{o}/{r}/tasks` (AM1-WP3, `graftyard_api::tasks`), served only once
/// `FormatsEnable` enabled 32.12; until then 503. Reads render the repository's tiers and the owner's `Authz` from one
/// `read`, so an uninstalled App's tasks read cancelled with no sweep; writes commit an `IssueCall` whose leader renders
/// the reply from the `&Acked`.
pub(crate) async fn tasks(
    c: &Ctx<'_>,
    route: graftyard_api::tasks::Route<'_>,
    who: Option<Principal>,
    method: Method,
    body: &Value,
    query: &str,
) -> Result<Reply> {
    use graftyard_api::tasks::{self as t, Route};
    if !c.cfg.enabled.contains(&graftyard_git::repo::REPO_FORMAT_TASKS) {
        return Ok(unavailable("the task API needs record format 32.12, which FormatsEnable has not enabled"));
    }
    let (Route::Tasks { owner, repo }
    | Route::Task { owner, repo, .. }
    | Route::Cancel { owner, repo, .. }
    | Route::Log { owner, repo, .. }) = route;
    let rd = match read(c, owner, repo, who.as_ref()).await? {
        Ok(r) => r,
        Err(r) => return Ok(r),
    };
    let Some(authz) = rd.dir.authz.as_ref() else {
        return Ok(unavailable("the owner's Authz chain did not answer; retry"));
    };
    let access = graftyard_git::issue_ops::Access { repo: rd.repo.id, owner: rd.repo.owner, core: &rd.core };
    let reader = t::Reader::of(who.as_ref(), authz, &access);
    let Ok(host) = Host::new(&c.cfg.host, false) else { return Ok(unavailable("the install's host is not valid")) };
    let view = rd.view(&host);
    let caller = who.as_ref().and_then(|p| match p.actor {
        Actor::App { app, .. } => Some(app),
        _ => None,
    });
    let op = match (route, method) {
        (Route::Tasks { .. }, Method::Get) => {
            let f = match t::parse_filter(query, &|l| graftyard_api::view::Directory::by_login(&rd.dir, l)) {
                Ok(f) => f,
                Err(r) => return Ok(r),
            };
            return Ok(t::list(&view, authz, crate::now(), &f, reader, &format!("/repos/{owner}/{repo}/tasks")));
        }
        (Route::Task { id, .. }, Method::Get) => return Ok(t::get(&view, authz, crate::now(), id, reader)),
        (Route::Log { id, .. }, Method::Get) => return Ok(t::log(&view, id, reader)),
        (Route::Tasks { .. }, Method::Post) => {
            let requester = match body.get("requester").and_then(Value::as_str) {
                Some(login) => match crate::account_of(c, login).await {
                    Ok(a) => a.map(graftyard_identity::UserId),
                    Err(e) => return Ok(unavailable(&e)),
                },
                None => None,
            };
            t::parse_create(body, requester).map(IssueOp::CreateTask)
        }
        (Route::Task { id, .. }, Method::Patch) => match t::path_id(id, caller) {
            Some(id) => t::parse_patch(id, body),
            None => return Ok(Reply::not_found()),
        },
        (Route::Cancel { id, .. }, Method::Post) => match t::path_id(id, caller) {
            Some(id) => Ok(IssueOp::CancelTask { id }),
            None => return Ok(Reply::not_found()),
        },
        _ => return Ok(Reply::error(405, "Method Not Allowed", "graftyard/tasks")),
    };
    let op = match op {
        Ok(op) => op,
        Err(r) => return Ok(r),
    };
    let Some(who) = who else { return Ok(Reply::error(401, "Requires authentication", "rest")) };
    let created = matches!(op, IssueOp::CreateTask(_));
    commit_issue(c, &rd, &who, op, What::Task { created, contents: reader.contents }).await
}

/// The `graftyard_task` field of an installation token request (AM1-WP3, `design/SHARED.md`, "Identity and tokens"):
/// the task's repository is the one the grant names, its tiers and its owner's `Authz` are read fresh, and
/// `graftyard_git::task::token_task` decides. `Ok(Err(reply))` is GitHub's 422, or 503 while 32.12 is not enabled.
pub(crate) async fn task_claim(
    c: &Ctx<'_>,
    owner_authz: &AuthzFold,
    grant: &graftyard_identity::stateless::Grant,
    owner: graftyard_types::AccountId,
    field: &Value,
) -> Result<std::result::Result<graftyard_identity::SessionId, Reply>> {
    let doc = "rest/apps/apps#create-an-installation-access-token-for-an-app";
    if !c.cfg.enabled.contains(&graftyard_git::repo::REPO_FORMAT_TASKS) {
        return Ok(Err(unavailable("graftyard_task needs record format 32.12, which FormatsEnable has not enabled")));
    }
    let Some(session) = field.as_str().and_then(|s| graftyard_identity::SessionId::parse(s, None)) else {
        return Ok(Err(Reply::error(422, "graftyard_task is not a task id", doc)));
    };
    let repo = match &grant.repos {
        graftyard_identity::RepoScope::Selected(ids) if !ids.is_empty() => ids[0],
        _ => {
            return Ok(Err(Reply::error(
                422,
                &graftyard_git::task::TaskTokenError::NeedsRepositories.to_string(),
                doc,
            )));
        }
    };
    let Ok(Some(record)) = crate::placement(c, owner).await else {
        return Ok(Err(Reply::error(422, "no such repository", doc)));
    };
    let repo = crate::Repo { id: repo, owner, record };
    let (rows, install) = futures_util::future::join(tier_rows(c, &repo, None), crate::install_authz(c)).await;
    let rows = match rows? {
        Ok(v) => match TierRows::from_value(&v["value"]) {
            Ok(r) if r.complete => r,
            _ => return Ok(Err(unavailable("the task's repository could not be read; retry"))),
        },
        Err(r) => return Ok(Err(r)),
    };
    let core = TierRows::store(&rows.core);
    let checked = graftyard_git::task::token_task(
        &core,
        owner_authz,
        install.as_ref().map(|a| a as &dyn graftyard_identity::AuthzRead),
        crate::now(),
        grant.app,
        repo.id,
        &grant.repos,
        &session,
    );
    Ok(checked.map_err(|e| Reply::error(422, &e.to_string(), doc)))
}

/// A merge's conflict, as `mergePullRequest` reports it [unverified: GitHub's text for a conflicting merge].
fn conflict() -> graftyard_git::merge::RuleViolation {
    graftyard_git::merge::RuleViolation::NotUpToDate
}

/// `GitCacheShard::merge` on the leader site's shard of the network: the `MergePlan` of `req` (`POST /merge` with
/// `method` on the shard).
async fn merge_plan(
    c: &Ctx<'_>,
    repo: &Repo,
    req: &MergeRequest,
) -> std::result::Result<std::result::Result<MergePlan, CacheError>, String> {
    let q = graftyard_git::merge_cache::chain_query(req.base_old, &[(req.n, req.head)], req.method);
    let record = js_sys::encode_uri_component(&serde_json::to_string(&repo.record).map_err(|e| e.to_string())?);
    let url = format!("https://shard/merge?repo={}&{q}&record={record}", repo.id.0.0);
    // The shard on the leader's site, which the read just before named, as `PrSync` and the `MergeQueue` consumer
    // merge "where the leader is" (`workers/site/src/prsync.rs`): it is the shard `PrSync`'s test merge caught up, and
    // its catch-up reads are local to the leader. The home site's shard, when the leader is elsewhere, caught up
    // across regions first, 280 to 610 ms (`tests/m2/results/m2-wp6.md`, "Merge latency").
    let leader = graftyard_lookup::cached_leader(ChainId::Repo(repo.id)).map(|t| t.site);
    #[cfg(feature = "test-faults")]
    let leader = c.shard.or(leader);
    let stub = crate::shard_at(c, repo, leader).map_err(|e| e.to_string())?;
    let mut init = RequestInit::new();
    init.with_method(Method::Post);
    let mut r = stub
        .fetch_with_request(Request::new_with_init(&url, &init).map_err(|e| e.to_string())?)
        .await
        .map_err(|e| e.to_string())?;
    let text = r.text().await.map_err(|e| e.to_string())?;
    if r.status_code() != 200 {
        return Err(format!("{} {text}", r.status_code()));
    }
    let a: ChainAnswer = serde_json::from_str(&text).map_err(|e| format!("{e}: {text}"))?;
    timing::mark("merge_plan");
    #[cfg(feature = "test-faults")]
    if let Ok(Some(t)) = r.headers().get("x-graft-timing") {
        timing::note(t);
    }
    Ok(a.merge_plan(req))
}

/// Commits a merge or merge-queue command and returns the reply its leader rendered from the `&Acked`, once
/// `FormatsEnable` has enabled 32.4; until then 503.
async fn commit_merge(c: &Ctx<'_>, rd: &Read, cmd: MergeCmd, what: What) -> Result<Reply> {
    // `Merged` and the queue row name the merging user (`UserId`), so a merge has no form for an App yet: an
    // installation token gets GitHub's 403 for a permission it lacks, not a 401 that says the token is bad.
    let app = |p: &Principal| matches!(p.actor, Actor::App { .. });
    let by_app = match &cmd {
        MergeCmd::Merge(m) => app(&m.actor),
        MergeCmd::Queue(q) => q.actor.as_ref().is_some_and(app),
        MergeCmd::Land(_) | MergeCmd::Cursor(_) => false,
    };
    if by_app {
        return Ok(Reply::error(
            403,
            "Resource not accessible by integration",
            "rest/pulls/pulls#merge-a-pull-request",
        ));
    }
    if !c.cfg.enabled.contains(&REPO_FORMAT_V4) {
        return Ok(unavailable(&format!(
            "merges need record format {}.{}, which FormatsEnable has not enabled on this install",
            REPO_FORMAT_V4.family, REPO_FORMAT_V4.version
        )));
    }
    let admit = match &cmd {
        MergeCmd::Merge(m) => crate::admission(c, &m.actor, &[], false, 0).await,
        MergeCmd::Queue(graftyard_git::queue::QueueOp { actor: Some(p), .. }) => {
            crate::admission(c, p, &[], false, 0).await
        }
        _ => None,
    };
    let render = Render { host: c.cfg.host.clone(), isolation: false, dir: rd.dir.clone(), what };
    let call = MergeCall { cmd, render: serde_json::to_value(&render)? };
    let mut body = json!({ "rid": c.rid.to_string(), "cmd": { "merge": call } });
    if let Some(a) = admit {
        body["admit"] = serde_json::to_value(a)?;
    }
    timing::mark("plan");
    // `rd` came from the group's leader, so the group exists: no create probe before the commit.
    let trace = std::cell::RefCell::new(Vec::new());
    let hooks = graftyard_lookup::Hooks { first: None, trace: cfg!(feature = "test-faults").then_some(&trace) };
    let r = graftyard_lookup::commit_hedged(
        &c.bindings(),
        ChainId::Repo(rd.repo.id),
        &rd.repo.record,
        body.to_string().as_bytes(),
        c.colo,
        60_000,
        hooks,
    )
    .await;
    timing::mark("commit");
    timing::trace(trace.into_inner());
    Ok(match r {
        Ok(done) => {
            if let Some(e) = done.pointer("/refused/err") {
                worker::console_log!("merge command {} refused: {e}", c.rid);
            }
            let wire = done
                .pointer("/ok/out/merge/reply")
                .or_else(|| done.pointer("/refused/merge/reply"))
                .and_then(|v| serde_json::from_value::<WireReply>(v.clone()).ok());
            match wire {
                Some(w) => w.reply(),
                None => unavailable(&format!("the member answered {done}")),
            }
        }
        Err(Ok(e)) => match e.pointer("/unsupported") {
            Some(u) => unavailable(&format!("the repository's members do not read record format {u}")),
            None => unavailable(&format!("{e}")),
        },
        Err(Err(e)) => unavailable(&e),
    })
}

/// A `Push` with one ref update, M1's write path: `DELETE /git/refs/{ref}`.
async fn delete_ref(c: &Ctx<'_>, rd: &Read, who: &Principal, u: graftyard_git::refs::RefUpdate) -> Result<Reply> {
    let push = Push {
        updates: vec![u],
        atomicity: Atomicity::PerRef,
        pack: None,
        actor: who.clone(),
        options: Vec::new(),
        request: c.rid,
        agent: None,
        provenance: Vec::new(),
        protected: Vec::new(),
        changed: Vec::new(),
        leases: false,
    };
    let admit = crate::admission(c, who, &[], true, 0).await;
    let mut body = serde_json::to_value(CommitReq { rid: c.rid.to_string(), cmd: push })?;
    if let Some(a) = admit {
        body["admit"] = serde_json::to_value(a)?;
    }
    Ok(match crate::commit(c, &rd.repo, &body, None, None).await? {
        Ok(PushOutcome::Ok(ok)) if ok.out.refs.iter().all(|o| *o == graftyard_git::repo::RefOutcome::Ok) => {
            Reply::empty(204)
        }
        Ok(PushOutcome::Ok(_)) => Reply::error(422, "Reference cannot be updated", "rest/git/refs"),
        Ok(PushOutcome::Refused(r)) if r.status == 403 => {
            Reply::error(403, "Resource not accessible by integration", "rest/git/refs")
        }
        Ok(PushOutcome::Refused(r)) => Reply::error(r.status, &r.err, "rest/git/refs"),
        Ok(PushOutcome::Full(f)) => Reply::secondary_rate_limit(f.retry_after_s),
        Ok(PushOutcome::NotLeader(_)) | Err(_) => unavailable("the repository's group has no leader; retry"),
    })
}

// ---- GraphQL ----

async fn graphql_post(c: &Ctx<'_>, req: &mut Request, host: &Host) -> Result<Reply> {
    let session = req.headers().get("x-graftyard-agent-session")?;
    let body: Value = req.json().await.unwrap_or(Value::Null);
    let q = body["query"].as_str().unwrap_or("").to_owned();
    let vars = body["variables"].as_object().cloned();
    let gq = match Gql::parse(&q, vars.clone(), body["operationName"].as_str().map(str::to_owned)) {
        Ok(r) => r,
        Err(e) => return Ok(Reply::json(200, json!({ "errors": [e.json()] }))),
    };
    // gh's `repo create` without a README: `createRepository(input:)` names no repository yet (`crate::repos`).
    if let Ok(roots) = gq.roots()
        && let [(field, args)] = roots.as_slice()
        && field == "createRepository"
    {
        let input = args.get("input").cloned().unwrap_or(Value::Null);
        let owner = input["ownerId"].as_str().and_then(graftyard_wire::NodeId::parse).and_then(|n| {
            matches!(n.ty, graftyard_wire::NodeType::User | graftyard_wire::NodeType::Organization)
                .then(|| graftyard_types::AccountId(graftyard_types::DbId(n.id())))
        });
        let via = crate::repos::Via::Graphql {
            owner,
            query: q.clone(),
            variables: vars.clone().map(Value::Object).unwrap_or(Value::Null),
        };
        let r = crate::repos::create(c, req, via, graftyard_api::repos::graphql_input(&input)).await?;
        // A refusal before the leader rendered is GraphQL's error shape, at 200.
        return Ok(if r.status == 200 {
            r
        } else {
            let m = r.value()["message"].as_str().unwrap_or("Repository creation failed.").to_owned();
            let m = r.value()["errors"][0]["message"].as_str().map(str::to_owned).unwrap_or(m);
            Reply::json(
                200,
                json!({ "data": { "createRepository": null },
                "errors": [{ "type": "UNPROCESSABLE", "path": ["createRepository"], "message": m }] }),
            )
        });
    }
    let target = match graphql::route(&gq) {
        Ok(t) => t,
        Err(e) => return Ok(Reply::json(200, json!({ "errors": [e.json()] }))),
    };
    // A merge reads only the rows it plans from, and its logins while the git cache plans it: neither needs the
    // other, and only the leader's render reads them (the `Plan::Merge` arm). Every other request reads both tiers
    // and its logins first.
    let merging = merge_pull_id(&gq);
    // The caller's token and the repository are read at once: neither read needs the other.
    let find = async {
        match &target {
            None => Ok::<_, Error>(None),
            Some(graphql::Target::Name { owner, name }) => Ok(Some(match resolve(c, owner, name).await? {
                Ok(Some(repo)) => Ok((owner.clone(), name.clone(), repo, None, None)),
                Ok(None) => Err(None),
                Err(e) => Err(Some(e)),
            })),
            // A node id names the repository by id: it is resolved once, by `repo_named`, not again by its name. A
            // merge's tier read needs only the repository, so it runs alongside the name's reads.
            Some(graphql::Target::Id(id)) => {
                let Ok(repo) = crate::repo_by_id(c, graftyard_types::RepoId(graftyard_types::DbId(*id))).await else {
                    return Ok(Some(Err(None)));
                };
                let rows = async {
                    match merging {
                        Some(_) => Some(tier_rows(c, &repo, merging).await),
                        None => None,
                    }
                };
                let (named, rows) = futures_util::future::join(repo_named(c, repo.clone()), rows).await;
                Ok(Some(match named {
                    Some((owner, name, repo, a)) => Ok((owner, name, repo, Some(a), rows)),
                    None => Err(None),
                }))
            }
        }
    };
    let (who, found) = futures_util::future::join(authenticate(c, req), find).await;
    timing::mark("auth_resolve");
    let Some(who) = who? else { return Ok(Reply::json(401, json!({ "message": "Bad credentials", "status": "401" }))) };
    let (owner, name, rd) = match found? {
        None => {
            let viewer = match who.actor {
                Actor::User(u) => crate::ident::login(c, u.0).await.map(|login| graphql::Viewer { id: u.0, login }),
                _ => None,
            };
            return Ok(Reply::json(200, graphql::query_viewer(&gq, viewer.as_ref())));
        }
        Some(Ok((owner, name, repo, a, rows))) => {
            let rows = match merging {
                Some(id) => Rows::Merge(id, rows),
                None => Rows::All,
            };
            let rd = read_repo(c, repo, &owner, &name, Some(&who), a, rows, Gate::Readers).await?;
            (owner, name, rd)
        }
        Some(Err(Some(e))) => return Ok(unavailable(&format!("the repository lookup failed; retry ({e})"))),
        Some(Err(None)) => match &target {
            Some(graphql::Target::Name { owner, name }) => (owner.clone(), name.clone(), Err(Reply::not_found())),
            _ => {
                return Ok(Reply::json(
                    200,
                    json!({ "errors": [{ "type": "NOT_FOUND", "message": "Could not resolve to a node with that id." }] }),
                ));
            }
        },
    };
    let mut rd = match rd {
        Ok(r) => r,
        // A node id the caller may not read answers as an unknown node, so the reply never names the repository.
        Err(r) if r.status == 404 && matches!(target, Some(graphql::Target::Id(_))) => {
            return Ok(Reply::json(
                200,
                json!({ "errors": [{ "type": "NOT_FOUND", "message": "Could not resolve to a node with that id." }] }),
            ));
        }
        Err(r) if r.status == 404 => {
            let m = format!("Could not resolve to a Repository with the name '{owner}/{name}'.");
            return Ok(Reply::json(
                200,
                json!({ "data": { "repository": null },
                "errors": [{ "type": "NOT_FOUND", "path": ["repository"], "message": m }] }),
            ));
        }
        Err(r) => return Ok(r),
    };
    let r = rd.view(host);
    if !gq.is_mutation().unwrap_or(false) {
        return Ok(Reply::json(200, graphql::query(&gq, Some(&r))));
    }
    let p = match graphql::plan(&gq, &r) {
        Ok(p) => p,
        Err(e) => return Ok(Reply::json(200, e)),
    };
    let what = What::Graphql {
        query: q.clone(),
        variables: vars.map(Value::Object).unwrap_or(Value::Null),
        field: p.field.clone(),
        subject: p.subject.map(|n| n.0),
    };
    let queue = |op| MergeCmd::Queue(QueueOp { op, actor: Some(who.clone()) });
    match p.plan.clone() {
        graphql::Plan::Issue(op) => {
            let op = op.map_blob(|b| Ok::<_, ()>(b.blob())).expect("no blob");
            // A bare name is scoped to the calling App, as in the task API's paths.
            let app = match who.actor {
                Actor::App { app, .. } => Some(app),
                _ => c.token_app.get(),
            };
            let session = session.as_deref().and_then(|s| graftyard_identity::SessionId::parse(s, app));
            commit_issue_in(c, &rd, &who, op, what, session).await
        }
        graphql::Plan::Merge(m) => {
            let req = match graphql::merge_request(&p.field, &m, &r) {
                Ok(req) => req,
                Err(e) => return Ok(Reply::json(200, e)),
            };
            let (plan, logins) = futures_util::future::join(merge_plan(c, &rd.repo, &req), read_logins(c, &rd)).await;
            let plan = match plan {
                Ok(Ok(plan)) => plan,
                Ok(Err(CacheError::Conflict(_))) => {
                    return Ok(Reply::json(200, graphql::refused_merge(&p, &MergeError::Protection(conflict()))));
                }
                Ok(Err(CacheError::Bad(e))) | Err(e) => return Ok(unavailable(&format!("the git cache: {e}"))),
            };
            let cmd = MergeCmd::Merge(graphql::merge_command(&m, plan, who.clone()));
            rd.dir.logins = logins;
            commit_merge(c, &rd, cmd, what).await
        }
        graphql::Plan::Enqueue { n, head } => commit_merge(c, &rd, queue(QueueAction::Enqueue { n, head }), what).await,
        graphql::Plan::Dequeue { n } => commit_merge(c, &rd, queue(QueueAction::Dequeue { n }), what).await,
    }
}

/// The database id of the pull request a `mergePullRequest` names, when the request is that one mutation.
fn merge_pull_id(gq: &Gql<'_>) -> Option<i64> {
    let roots = gq.roots().ok()?;
    let [(f, a)] = roots.as_slice() else { return None };
    let id = a.get("input")?.get("pullRequestId")?.as_str()?;
    let n = graftyard_api::wire::NodeId::parse(id)?;
    (f == "mergePullRequest" && n.ty == graftyard_api::wire::NodeType::PullRequest && !n.legacy).then(|| n.id())
}

/// A repository's `o/r` from its id: its owner from `RepoOwner` on `Names`, its name from the owner's `Authz` names,
/// and the owner's login from its `AccountLogin` row (F50), else from `logins`.
async fn repo_name(c: &Ctx<'_>, id: i64) -> Option<(String, String, Repo, AuthzFold)> {
    let repo = crate::repo_by_id(c, graftyard_types::RepoId(graftyard_types::DbId(id))).await.ok()?;
    repo_named(c, repo).await
}

/// `repo_name` of a repository already found by id.
async fn repo_named(c: &Ctx<'_>, repo: Repo) -> Option<(String, String, Repo, AuthzFold)> {
    // The owner's login and `Authz` at once: each needs only the owner and its record.
    let owner = std::iter::once(repo.owner).collect();
    let (mut known, a) = futures_util::future::join(account_logins(c, &owner), authz(c, &repo)).await;
    let login = known.remove(&repo.owner.0.0).or_else(|| logins(c).remove(&repo.owner.0.0))?;
    let a = a?;
    let name = a.id_name(repo.id)?.as_str().to_owned();
    Some((login, name, repo, a))
}

/// The most `account_shard` groups one request reads for logins [guess]. An account past it renders as `ghost`, as an
/// account with no row does.
const LOGIN_SHARDS_MAX: usize = 24;

/// The logins of `accounts`, from each one's `AccountLogin` row (F50): one read-class `fresh` read per distinct
/// `account_shard`, all at once. A shard that fails to answer, or whose members predate the row, names none of its
/// accounts, which then render as `ghost`.
async fn account_logins(
    c: &Ctx<'_>,
    accounts: &std::collections::BTreeSet<graftyard_types::AccountId>,
) -> std::collections::BTreeMap<i64, String> {
    let mut by_shard: std::collections::BTreeMap<_, Vec<i64>> = Default::default();
    for a in accounts {
        by_shard.entry(graftyard_identity::account_shard(*a)).or_default().push(a.0.0);
    }
    let reads = by_shard.into_iter().take(LOGIN_SHARDS_MAX).map(|(shard, ids)| async move {
        crate::system_read(c, ChainId::Names(shard), json!({ "accountLogins": ids })).await
    });
    let mut out = std::collections::BTreeMap::new();
    for v in futures_util::future::join_all(reads).await.into_iter().flatten() {
        for (id, l) in v["accountLogins"].as_object().into_iter().flatten() {
            if let (Ok(id), Some(l)) = (id.parse(), l.as_str()) {
                out.insert(id, l.to_owned());
            }
        }
    }
    out
}

/// Test installs' logins by account id, from `LOGINS` (`{"<id>": "<login>"}`), for accounts whose `AccountLogin` row
/// was never written: every account an install seeded before F50's row existed. A product edge has none.
fn logins(c: &Ctx<'_>) -> std::collections::BTreeMap<i64, String> {
    #[cfg(feature = "test-faults")]
    let v: Value = c.env.var("LOGINS").ok().and_then(|v| serde_json::from_str(&v.to_string()).ok()).unwrap_or_default();
    #[cfg(not(feature = "test-faults"))]
    let v = {
        let _ = c;
        Value::Null
    };
    v.as_object().into_iter().flatten().filter_map(|(k, l)| Some((k.parse().ok()?, l.as_str()?.to_owned()))).collect()
}

// ---- REST ----

async fn rest_repo(c: &Ctx<'_>, req: &mut Request, host: &Host, p: &str, query: &str) -> Result<Reply> {
    let Some((at, segs)) = graftyard_wire::repo_path(p) else { return Ok(Reply::not_found()) };
    let tail = segs.as_slice();
    let who = authenticate(c, req).await?;
    // A credential the edge refuses (revoked, lapsed, unknown) is 401, as on GitHub, not an anonymous caller whom the
    // read gate would answer 404 on a private repository.
    if who.is_none() && crate::token_of(req)?.is_some() {
        return Ok(Reply::error(401, "Bad credentials", "rest"));
    }
    timing::mark("auth");
    let method = req.method();
    let rd = match at {
        graftyard_wire::RepoPath::Name { owner, name } => read(c, owner, name, who.as_ref()).await?,
        graftyard_wire::RepoPath::Id(id) => match repo_name(c, id).await {
            Some((owner, name, repo, a)) => {
                read_repo(c, repo, &owner, &name, who.as_ref(), Some(a), Rows::All, Gate::Readers).await?
            }
            None => return Ok(Reply::not_found()),
        },
    };
    let mut rd = match rd {
        Ok(r) => r,
        Err(r) => return Ok(r),
    };
    // A `github_pat_` reads only inside its scope (BF-31); every write's leader checks it again.
    let access = graftyard_git::issue_ops::Access { repo: rd.repo.id, owner: rd.repo.owner, core: &rd.core };
    if let Some(r) = crate::usertok::reaches(c, &access) {
        return Ok(r);
    }
    if let Some(r) = crate::collab::repo_route(c, req, &rd, who.as_ref(), tail).await? {
        return Ok(r);
    }
    if let Some(r) = crate::repos::repo_route(c, req, &rd, who.as_ref(), tail, rd.index, host).await? {
        return Ok(r);
    }
    if let Some(r) = crate::hooks::repo_route(c, req, &rd, who.as_ref(), tail, host).await? {
        return Ok(r);
    }
    // The trust view (AM1-WP5): a reply changes only under `AGENT_MEDIA_TYPE`, and only then does the edge read the
    // install's `AgentApp` rows for `is_agent`.
    let agent_accept = req.headers().get("accept")?;
    if graftyard_api::trust::opted_in(agent_accept.as_deref()) {
        rd.dir.install = crate::install_authz(c).await;
    }
    let r = rd.view(host);
    let need = |w: &Option<Principal>| -> std::result::Result<Principal, Reply> {
        w.clone().ok_or_else(|| Reply::error(401, "Requires authentication", "rest"))
    };
    let body = |b: Vec<u8>| serde_json::from_slice::<Value>(&b).unwrap_or(Value::Null);
    let id = |s: &str| s.parse::<i64>().ok();
    let reply = match (method, tail) {
        (Method::Get, ["commits", reference, "statuses"]) => {
            rest::list_commit_statuses(&r, reference, graftyard_wire::Page::parse(query))
        }
        (Method::Get, ["issues"]) => rest::list_issues(&r, query),
        (Method::Post, ["labels"]) => {
            let who = match need(&who) {
                Ok(w) => w,
                Err(x) => return Ok(x),
            };
            match rest::plan_create_label(&body(req.bytes().await?)) {
                Ok(op) => commit_issue(c, &rd, &who, to_wire(op), What::LabelCreated).await?,
                Err(x) => x,
            }
        }
        // B0-36's labels write: `issues/{n}/labels`, add, replace, remove all, and remove one.
        (m @ (Method::Post | Method::Put | Method::Delete), ["issues", n, "labels", rest @ ..]) if rest.len() <= 1 => {
            let who = match need(&who) {
                Ok(w) => w,
                Err(x) => return Ok(x),
            };
            let m = match m {
                Method::Post => "POST",
                Method::Put => "PUT",
                _ => "DELETE",
            };
            let name = rest.first().copied();
            let b = if m == "DELETE" { Value::Null } else { body(req.bytes().await?) };
            match rest::plan_issue_labels(&r, m, n, name, &b) {
                Ok(op) => {
                    let what = What::IssueLabels { n: n.parse().unwrap_or(0), empty: m == "DELETE" && name.is_none() };
                    commit_issue(c, &rd, &who, to_wire(op), what).await?
                }
                Err(x) => x,
            }
        }
        (Method::Get | Method::Head, ["releases", "tags", tag]) => release::get_by_tag(&r, tag),
        (Method::Get, ["releases", n]) if id(n).is_some() => release::get(&r, id(n).unwrap_or(0)),
        (Method::Get, ["releases", "assets", n]) => {
            let accept = req.headers().get("accept")?;
            match release::get_asset(&r, id(n).unwrap_or(0), accept.as_deref()) {
                release::AssetGet::Reply(x) => x,
                release::AssetGet::Bytes(row) => {
                    let raw = buckets(c, &rd.repo)?;
                    let refs: Vec<(SiteId, &R2)> = raw.iter().map(|(s, b)| (*s, b)).collect();
                    let store = BlobStore { sites: c.sites, buckets: &refs, copies: Copies::TwoRegions };
                    match store.read(rd.repo.id, &row.rec.blob).await {
                        Ok(Some(f)) => release::asset_bytes(&row, f.bytes),
                        Ok(None) => Reply::not_found(),
                        Err(e) => unavailable(&e.0),
                    }
                }
            }
        }
        (Method::Get, [kind @ ("tarball" | "zipball"), reference]) => {
            let f = if *kind == "tarball" { ArchiveFormat::TarGz } else { ArchiveFormat::Zip };
            release::archive_redirect(&r, f, reference)
        }
        (Method::Post, ["releases"]) => {
            let who = match need(&who) {
                Ok(w) => w,
                Err(x) => return Ok(x),
            };
            match release::plan_create(&r, &body(req.bytes().await?)) {
                Ok(op) => commit_issue(c, &rd, &who, to_wire(op), What::ReleaseCreated).await?,
                Err(x) => x,
            }
        }
        (Method::Patch, ["releases", n]) => {
            let who = match need(&who) {
                Ok(w) => w,
                Err(x) => return Ok(x),
            };
            match release::plan_update(&r, id(n).unwrap_or(0), &body(req.bytes().await?)) {
                Ok(op) => commit_issue(c, &rd, &who, to_wire(op), What::ReleaseUpdated).await?,
                Err(x) => x,
            }
        }
        (Method::Delete, ["releases", "assets", n]) => {
            let who = match need(&who) {
                Ok(w) => w,
                Err(x) => return Ok(x),
            };
            match release::plan_delete_asset(&r, id(n).unwrap_or(0)) {
                Ok(op) => commit_issue(c, &rd, &who, to_wire(op), What::Deleted).await?,
                Err(x) => x,
            }
        }
        (Method::Delete, ["releases", n]) => {
            let who = match need(&who) {
                Ok(w) => w,
                Err(x) => return Ok(x),
            };
            match release::plan_delete(&r, id(n).unwrap_or(0)) {
                Ok(op) => commit_issue(c, &rd, &who, to_wire(op), What::Deleted).await?,
                Err(x) => x,
            }
        }
        (Method::Post, ["statuses", sha]) => {
            let who = match need(&who) {
                Ok(w) => w,
                Err(x) => return Ok(x),
            };
            let Actor::User(me) = who.actor else { return Ok(Reply::error(401, "Requires authentication", "rest")) };
            let id = graftyard_types::DbId(js_sys::Date::now() as i64);
            match rest::plan_create_status(sha, &body(req.bytes().await?), me.0, id) {
                Ok(op) => {
                    let IssueOp::RecordStatus(st) = &op else { unreachable!("a status") };
                    let what = What::StatusCreated { sha: st.sha, id: id.0 };
                    commit_issue(c, &rd, &who, to_wire(op), what).await?
                }
                Err(x) => x,
            }
        }
        (Method::Put, ["pulls", n, "merge"]) => {
            let who = match need(&who) {
                Ok(w) => w,
                Err(x) => return Ok(x),
            };
            let (n, req, sha) = match rest::plan_rest_merge(&r, n, &body(req.bytes().await?)) {
                Ok(x) => x,
                Err(x) => return Ok(x),
            };
            match merge_plan(c, &rd.repo, &req).await {
                Ok(Ok(plan)) => {
                    let m = graftyard_git::merge::Merge { n, plan, expected_head: sha, actor: who.clone() };
                    commit_merge(c, &rd, MergeCmd::Merge(m), What::RestMerge).await?
                }
                Ok(Err(CacheError::Conflict(_))) => {
                    Reply::error(405, "Pull Request is not mergeable", "rest/pulls/pulls#merge-a-pull-request")
                }
                Ok(Err(CacheError::Bad(e))) | Err(e) => unavailable(&format!("the git cache: {e}")),
            }
        }
        // AM1: `POST /git/refs` and `PATCH /git/refs/{ref}`, served in `crate::agents` so an agent's write carries
        // `validate`'s outputs on an empty pack and a lease check sees its `changed` paths.
        (Method::Post, ["git", "refs"]) | (Method::Patch, ["git", "refs", ..]) => {
            let who = match need(&who) {
                Ok(w) => w,
                Err(x) => return Ok(x),
            };
            let reference = match tail {
                [_, _, rest @ ..] if !rest.is_empty() => Some(rest.join("/")),
                _ => None,
            };
            match rest::plan_write_ref(&r, reference.as_deref(), &body(req.bytes().await?)) {
                Ok((u, force)) => crate::agents::write_ref(c, &rd, &r, &who, u, force).await?,
                Err(x) => x,
            }
        }
        (Method::Delete, ["git", "refs", reference @ ..]) => {
            let who = match need(&who) {
                Ok(w) => w,
                Err(x) => return Ok(x),
            };
            match rest::plan_delete_ref(&r, &reference.join("/")) {
                Ok(u) => delete_ref(c, &rd, &who, u).await?,
                Err(x) => x,
            }
        }
        _ => Reply::not_found(),
    };
    Ok(graftyard_api::trust::layer(agent_accept.as_deref(), p, &r, reply))
}

/// A command planned over `DurableBlob`s, as it crosses the binding: its blob as the `BlobRef`.
fn to_wire(op: IssueOp) -> IssueOp<BlobRef> {
    op.map_blob(|d| Ok::<_, ()>(d.blob())).expect("infallible")
}

// ---- uploads ----

async fn upload(c: &Ctx<'_>, req: &mut Request, host: &Host, p: &str, query: &str) -> Result<Reply> {
    let segs: Vec<&str> = p.trim_start_matches('/').split('/').collect();
    let ["repos", owner, name, "releases", rid, "assets"] = segs.as_slice() else { return Ok(Reply::not_found()) };
    let Some(who) = authenticate(c, req).await? else {
        return Ok(Reply::error(401, "Requires authentication", "rest"));
    };
    let rd = match read(c, owner, name, Some(&who)).await? {
        Ok(r) => r,
        Err(r) => return Ok(r),
    };
    let r = rd.view(host);
    if !can_write(&r, Some(&who)) {
        return Ok(Reply::error(403, "Resource not accessible by integration", "rest"));
    }
    let ct = req.headers().get("content-type")?;
    let u = match release::check_upload(&r, rid.parse().unwrap_or(0), query, ct.as_deref()) {
        Ok(u) => u,
        Err(x) => return Ok(x),
    };
    let bytes = req.bytes().await?;
    let raw = buckets(c, &rd.repo)?;
    let refs: Vec<(SiteId, &R2)> = raw.iter().map(|(s, b)| (*s, b)).collect();
    let store = BlobStore { sites: c.sites, buckets: &refs, copies: Copies::TwoRegions };
    let meta = ObjectMeta { chain: ChainId::Repo(rd.repo.id), request: c.rid };
    let placed = match store.place(rd.repo.id, &bytes, None, meta).await {
        Ok(d) => d,
        Err(e) => return Ok(release::upload_failed(&e)),
    };
    let op = to_wire(release::plan_upload(u, placed));
    commit_issue(c, &rd, &who, op, What::ReleaseCreated).await
}

// ---- LFS ----

#[allow(clippy::too_many_arguments)]
async fn lfs_route(
    c: &Ctx<'_>,
    req: &mut Request,
    host: &Host,
    owner: &str,
    name: &str,
    rest: &str,
    query: &str,
    method: Method,
) -> Result<Reply> {
    let Some(who) = authenticate(c, req).await? else { return Ok(lfs::challenge()) };
    let rd = match read(c, owner, name, Some(&who)).await? {
        Ok(r) => r,
        Err(r) => return Ok(r),
    };
    let r = rd.view(host);
    let write = can_write(&r, Some(&who));
    let me = match who.actor {
        Actor::User(u) => Some(u.0),
        Actor::App { .. } | Actor::Anonymous => None,
    };
    let auth = req.headers().get("authorization")?.unwrap_or_default();
    let body = serde_json::from_slice::<Value>(&req.bytes().await.unwrap_or_default()).unwrap_or(Value::Null);
    Ok(match (method, rest) {
        (Method::Post, "objects/batch") => lfs::batch(&r, &body, write, &auth),
        (Method::Post, "objects/verify") => {
            let (oid, size) = match lfs::verify_request(&body) {
                Ok(x) => x,
                Err(x) => return Ok(x),
            };
            let blob = BlobRef { oid, bytes: size, sites: Default::default() };
            commit_issue(c, &rd, &who, IssueOp::LfsObject(blob), What::LfsVerified).await?
        }
        (Method::Get, "locks") => lfs::list_locks(&r, query),
        (Method::Post, "locks/verify") => lfs::verify_locks(&r, me, write),
        (Method::Post, "locks") => match lfs::plan_lock(&r, &body) {
            Ok(op) => commit_issue(c, &rd, &who, to_wire(op), What::LfsLocked).await?,
            Err(x) => x,
        },
        (Method::Post, p) if let Some(id) = p.strip_prefix("locks/").and_then(|r| r.strip_suffix("/unlock")) => {
            match lfs::plan_unlock(&r, id, &body) {
                Ok((op, lock)) => commit_issue(c, &rd, &who, to_wire(op), What::LfsUnlocked { lock }).await?,
                Err(x) => x,
            }
        }
        _ => Reply::not_found(),
    })
}

/// The transfer: `PUT` places the bytes in two buckets and answers 200 with nothing committed; `GET` reads them back.
async fn transfer(c: &Ctx<'_>, req: &mut Request, host: &Host, p: &str, method: Method) -> Result<Reply> {
    let Some((repo_id, oid)) = lfs::transfer_path(p) else { return Ok(Reply::not_found()) };
    let Some(who) = authenticate(c, req).await? else { return Ok(lfs::challenge()) };
    let Ok(repo) = crate::repo_by_id(c, graftyard_types::RepoId(graftyard_types::DbId(repo_id))).await else {
        return Ok(Reply::not_found());
    };
    match crate::read_gate(c, &repo, Some(&who)).await? {
        Ok(graftyard_identity::Decision::Allow) => {}
        Ok(_) => return Ok(Reply::not_found()),
        Err(e) => return Ok(unavailable(&format!("the repository's access rows did not answer; retry ({e})"))),
    }
    let raw = buckets(c, &repo)?;
    let refs: Vec<(SiteId, &R2)> = raw.iter().map(|(s, b)| (*s, b)).collect();
    let store = BlobStore { sites: c.sites, buckets: &refs, copies: Copies::TwoRegions };
    match method {
        Method::Put => {
            let _ = host;
            let bytes = req.bytes().await?;
            let meta = ObjectMeta { chain: ChainId::Repo(repo.id), request: c.rid };
            Ok(match store.place(repo.id, &bytes, Some((oid, bytes.len() as u64)), meta).await {
                Ok(d) => lfs::uploaded(&d),
                Err(e) => lfs::upload_failed(&e),
            })
        }
        Method::Get => {
            // The object's row names the sites that held it; reading every bucket of the placement finds it too.
            let blob = BlobRef { oid, bytes: 0, sites: Default::default() };
            let key = graftyard_git::R2Key::Blob { repo: repo.id, oid }.to_string();
            for (_, b) in &refs {
                if let Ok(Some(o)) = graftyard_git::Bucket::head(*b, &key).await {
                    let blob = BlobRef { bytes: o.size, ..blob };
                    if let Ok(Some(f)) = store.read(repo.id, &blob).await {
                        return Ok(lfs::download_reply(f.bytes));
                    }
                }
            }
            Ok(Reply::error(404, "Object does not exist on the server", "rest"))
        }
        _ => Ok(Reply::not_found()),
    }
}

// ---- codeload ----

/// `git archive` on the network's git cache shard (`/archive?repo=&commit=&prefix=&format=`).
struct ShardArchiver<'a> {
    c: &'a Ctx<'a>,
    repo: &'a Repo,
}

impl Archiver for ShardArchiver<'_> {
    async fn archive(
        &self,
        _: NetworkId,
        commit: &str,
        prefix: &str,
        format: ArchiveFormat,
    ) -> std::result::Result<Vec<u8>, String> {
        let url = format!(
            "https://shard/archive?repo={}&commit={commit}&prefix={}&format={}",
            self.repo.id.0.0,
            js_sys::encode_uri_component(prefix),
            format.ext()
        );
        let stub = crate::shard(self.c, self.repo).map_err(|e| e.to_string())?;
        let mut r = stub.fetch_with_str(&url).await.map_err(|e| e.to_string())?;
        if r.status_code() != 200 {
            return Err(format!("git cache archive: {} {}", r.status_code(), r.text().await.unwrap_or_default()));
        }
        r.bytes().await.map_err(|e| e.to_string())
    }
}

/// Read-gated like the API: the redirect from `/repos/{o}/{r}/tarball` lands on the same host, so the client sends
/// its credential again, and a private repository's archive is served only to a caller who may read it.
async fn codeload(c: &Ctx<'_>, req: &Request, host: &Host, owner: &str, name: &str, rest: &str) -> Result<Reply> {
    let who = authenticate(c, req).await?;
    let rd = match read(c, owner, name, who.as_ref()).await? {
        Ok(r) => r,
        Err(r) => return Ok(r),
    };
    let r = rd.view(host);
    let cl = match release::codeload(&r, rest) {
        Ok(cl) => cl,
        Err(x) => return Ok(x),
    };
    let warm = R2(c.env.bucket(&format!("BUCKET_{}", rd.repo.record.home.0))?);
    let ar =
        ArchiveRequest { network: NetworkId(rd.repo.id.0), commit: cl.commit, prefix: &cl.prefix, format: cl.format };
    let git = ShardArchiver { c, repo: &rd.repo };
    Ok(match graftyard_git::blob::archive(&warm, &git, &ar).await {
        Ok((bytes, _)) => release::archive_reply(&cl, bytes),
        Err(e) => unavailable(&e),
    })
}

#[cfg(test)]
mod tests {
    /// The whole-tier read asks for the packed answer, so the leader builds one string, not a `Value` tree of 18 times
    /// its size; a merge's narrow read asks for it too, which a member answers unpacked.
    #[test]
    fn the_tier_read_asks_for_packed_rows() {
        let q: serde_json::Value = serde_json::from_str(&super::tier_rows_query(None)).unwrap();
        assert_eq!(q, serde_json::json!({ "q": { "rows": true, "ranges": true, "packed": true } }));
        let q: serde_json::Value = serde_json::from_str(&super::tier_rows_query(Some(7))).unwrap();
        assert_eq!(q["q"]["rows"], serde_json::json!({ "merge": 7 }));
    }
}
