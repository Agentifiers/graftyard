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
use graftyard_api::hosted::{CommandSpec, HostedDir, MergeOf, Render, TierRows, What, WireReply};
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
use graftyard_types::{ChainId, NetworkId, SiteId, Timestamp};
use graftyard_wire::{Body, Endpoint, Host, Incoming, Meta, RateLimits, Reply, Surface};
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

/// Each stage of one request and its own time in ms, held in the request's `Ctx` (a Workers isolate serves many
/// requests at once, so no isolate-wide list can hold them). Reads that run at once each record their own wall
/// (`timed`), so stages may overlap and `total` is the request's. Test installs send it back as `server-timing`; every
/// install logs it for 1 request in `TIMING_SAMPLE` (Workers Observability), so a latency gate can say where the time
/// went (`design/DESIGN.md`, "Follower read").
#[derive(Default)]
pub(crate) struct Timing {
    start: std::cell::Cell<f64>,
    last: std::cell::Cell<f64>,
    phases: std::cell::RefCell<Vec<(&'static str, f64)>>,
    /// Test installs: the commit's hedge steps (`x-graft-trace`) and the shard's timing of its merge plan
    /// (`x-graft-shard-timing`). Per request, as the stages are: kept per isolate, a concurrent request took them, and
    /// 66 of 68 merges in LM-1's m19 smokes answered no shard timing (`merge-p99`).
    trace: std::cell::RefCell<Vec<Value>>,
    shard: std::cell::RefCell<String>,
    /// `edge-read-batch`: this request's write-class `rcache` reads it ran, shared, and the ms it waited in batches.
    rc: std::cell::Cell<(u32, u32, f64)>,
    /// Test installs: each spread read pass this request ran, with its ms and its member tries
    /// (`graftyard_lookup::read_spread_traced`), for `x-graft-spread` (`gate-trace`).
    pub(crate) spread: std::cell::RefCell<Vec<(u64, String)>>,
}

impl Timing {
    pub(crate) fn start(&self) {
        let now = js_sys::Date::now();
        self.start.set(now);
        self.last.set(now);
        self.phases.borrow_mut().clear();
        self.trace.borrow_mut().clear();
        self.shard.borrow_mut().clear();
        self.spread.borrow_mut().clear();
        self.rc.set((0, 0, 0.0));
    }
    /// One write-class `rcache` read: its own (`shared` false) or another call's, after `waited` ms in its batch.
    pub(crate) fn rcache(&self, shared: bool, waited: f64) {
        let (run, share, wait) = self.rc.get();
        self.rc.set(if shared { (run, share + 1, wait + waited) } else { (run + 1, share, wait + waited) });
    }
    /// Sets `bound` to AGENT_PUSH_BOUND after this request's start, its arrival at the edge.
    pub(crate) fn bound_from_start(&self, bound: &std::cell::Cell<Option<f64>>) {
        bound.set(Some(self.start.get() + graftyard_raft::admit::AGENT_PUSH_BOUND.as_millis() as f64));
    }
    /// Closes the phase that ends now.
    pub(crate) fn mark(&self, name: &'static str) {
        let now = js_sys::Date::now();
        self.phases.borrow_mut().push((name, now - self.last.get()));
        self.last.set(now);
    }
    /// A stage that ran from `since` to now, whatever ran beside it; the next `mark` measures from the later of the
    /// last mark and now.
    pub(crate) fn add(&self, name: &'static str, since: f64) {
        let now = js_sys::Date::now();
        self.phases.borrow_mut().push((name, now - since));
        self.last.set(self.last.get().max(now));
    }
    /// A stage another party measured, such as the member's own split of a tier read (`member_stages`).
    pub(crate) fn put(&self, name: &'static str, ms: f64) {
        self.phases.borrow_mut().push((name, ms));
    }
    /// Each stage and its ms, then `total`.
    fn phases(&self) -> Vec<(&'static str, f64)> {
        let mut out = self.phases.borrow().clone();
        let (run, share, wait) = self.rc.get();
        if run + share > 0 {
            out.extend([("rcread", f64::from(run)), ("rcshare", f64::from(share)), ("rcwait", wait)]);
        }
        out.push(("total", js_sys::Date::now() - self.start.get()));
        out
    }
    #[cfg(feature = "test-faults")]
    pub(crate) fn header(&self) -> String {
        self.phases().iter().map(|(n, d)| format!("{n};dur={d:.0}")).collect::<Vec<_>>().join(", ")
    }
    /// Test installs: the spread passes as `x-graft-spread`, `<ms> <chain> <tries> <end>` joined by `; `.
    #[cfg(feature = "test-faults")]
    pub(crate) fn spread_header(&self) -> String {
        self.spread.borrow().iter().map(|(ms, l)| format!("{ms} {l}")).collect::<Vec<_>>().join("; ")
    }
}

/// `f`, with its own wall time recorded as stage `name` of `c`'s request.
pub(crate) async fn timed<F: std::future::Future>(c: &Ctx<'_>, name: &'static str, f: F) -> F::Output {
    let since = js_sys::Date::now();
    let out = f.await;
    c.timing.add(name, since);
    out
}

/// One request in this many logs its stages (`timing`).
const TIMING_SAMPLE: f64 = 100.0;

/// A commit's hedge steps for this request's `x-graft-trace` (test installs only).
pub(crate) fn trace(c: &Ctx<'_>, t: Vec<Value>) {
    *c.timing.trace.borrow_mut() = t;
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
    let colo = crate::colo(req);
    c.timing.start();
    let kind = match &e {
        Endpoint::Graphql => "graphql",
        Endpoint::Rest(p) if p.starts_with("/repos/") => "rest-repo",
        _ => "other",
    };
    let r = match route(c, req, host, e, &m).await {
        Ok(r) => r,
        Err(err) => unavailable(&format!("Service Unavailable: {err}")),
    };
    if js_sys::Math::random() * TIMING_SAMPLE < 1.0 {
        let stages: serde_json::Map<String, Value> =
            c.timing.phases().into_iter().map(|(n, d)| (n.to_owned(), json!(d.round()))).collect();
        worker::console_log!("{}", json!({ "apiTiming": stages, "kind": kind, "status": r.status }));
    }
    #[cfg(feature = "test-faults")]
    let r = r
        .with("server-timing", c.timing.header())
        .with("x-graft-trace", Value::from(c.timing.trace.take()).to_string())
        .with("x-graft-shard-timing", c.timing.shard.take())
        .with("x-graft-colo", colo);
    frame(&i, r, s, &m)
}

/// What a handler reads: the repository, its tiers at one index, and the directory.
pub(crate) struct Read {
    pub(crate) repo: Repo,
    pub(crate) at: RepoRef,
    pub(crate) core: MemStore,
    pub(crate) meta: MemStore,
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
    read_as(c, owner, name, viewer, Rows::All).await
}

/// `read` with the rows `rows` names: a narrow API read (`Rows::Api`) or both whole tiers.
async fn read_as(
    c: &Ctx<'_>,
    owner: &str,
    name: &str,
    viewer: Option<&Principal>,
    rows: Rows,
) -> Result<std::result::Result<Read, Reply>> {
    let found = resolve(c, owner, name).await?;
    read_found(c, found, owner, name, viewer, rows).await
}

/// `read_as` of a name `resolve` already answered.
async fn read_found(
    c: &Ctx<'_>,
    found: std::result::Result<Option<Repo>, String>,
    owner: &str,
    name: &str,
    viewer: Option<&Principal>,
    rows: Rows,
) -> Result<std::result::Result<Read, Reply>> {
    let repo = match found {
        Ok(Some(r)) => r,
        Ok(None) => return Ok(Err(Reply::not_found())),
        Err(e) => return Ok(Err(read_failed("the repository lookup failed; retry", &e))),
    };
    read_repo(c, repo, owner, name, viewer, None, rows, Gate::Readers).await
}

/// One `uses:` action for the launch service (`crate::actions::launch`): its repository resolved by name on this
/// install (case-insensitive, renames followed, as every repository route), read as an anonymous caller, so only a
/// repository anyone may read resolves (`decisions/OPEN-QUESTIONS.md`, F90), and its version resolved in the refs the
/// narrow `tarball` read records; a full SHA must be a commit the network's git cache holds, as REST's
/// `commits/{sha}` reads it. Nothing is looked up outside the install.
pub(crate) async fn launch_resolve(
    c: &Ctx<'_>,
    host: &Host,
    a: &graftyard_actions::launch::ActionRef,
) -> graftyard_actions::launch::Found {
    use graftyard_actions::launch::{self, Found, Resolved};
    if a.owner.is_empty() {
        return Found::NoRepo;
    }
    let route = graftyard_api::narrow::Route::Rest {
        tail: vec!["tarball".into(), a.version.clone()],
        query: String::new(),
        path: format!("/repos/{}/{}/tarball/{}", a.owner, a.repo, a.version),
        agent: false,
    };
    let rd = match read_as(c, &a.owner, &a.repo, None, Rows::Api(route)).await {
        Ok(Ok(rd)) => rd,
        Ok(Err(r)) if r.status == 404 => return Found::NoRepo,
        Ok(Err(r)) => return Found::Failed(format!("{} {}: {}", a.name, r.status, wire_body(&r))),
        Err(e) => return Found::Failed(format!("{}: {e}", a.name)),
    };
    let view = rd.view(host);
    let sha = match launch::resolve(&a.version, |n| view.ref_value(n)) {
        Ok(Resolved::Ref(sha)) => sha,
        Ok(Resolved::Sha(sha)) => match commit_object(c, &rd.repo, sha).await {
            Ok(Some(_)) => sha,
            Ok(None) => return Found::NoVersion,
            Err(e) => return Found::Failed(format!("{}: the git cache: {e}", a.name)),
        },
        Err(f) => return f,
    };
    let (tar_url, zip_url) = launch::archive_urls(&host.codeload(&a.owner, &a.repo), sha);
    Found::Commit { sha, tar_url, zip_url }
}

fn wire_body(r: &Reply) -> String {
    match &r.body {
        Body::Json(v) => v.to_string(),
        _ => String::new(),
    }
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
        Err(e) => return Ok(Err(read_failed("the repository lookup failed; retry", &e))),
    };
    read_repo(c, repo, owner, name, viewer, None, Rows::Core, Gate::Readers).await
}

/// `read` for a command that reads only the read gate's rows and the one task it names (`Rows::Command`,
/// `decisions/DECISIONS.md`, "`command-rows`"): a lease, task, or row-free REST write.
pub(crate) async fn read_command(
    c: &Ctx<'_>,
    owner: &str,
    name: &str,
    viewer: Option<&Principal>,
    spec: CommandSpec,
) -> Result<std::result::Result<Read, Reply>> {
    let repo = match resolve(c, owner, name).await? {
        Ok(Some(r)) => r,
        Ok(None) => return Ok(Err(Reply::not_found())),
        Err(e) => return Ok(Err(read_failed("the repository lookup failed; retry", &e))),
    };
    read_repo(c, repo, owner, name, viewer, None, Rows::Command(spec), Gate::Readers).await
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
        Err(e) => return Ok(Err(read_failed("the repository lookup failed; retry", &e))),
    };
    read_repo(c, repo, owner, name, Some(viewer), None, Rows::All, Gate::Invitees).await
}

/// Which callers `read_repo` serves: those `authorize_read` allows, or anyone, for a route that checks the caller
/// against the repository's invitations itself.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Gate {
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
pub(crate) async fn read_repo(
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
        Rows::All | Rows::Core | Rows::Api(_) | Rows::Command(_) => None,
        Rows::Merge(of, _) => Some(*of),
    };
    let core_only = matches!(&rows, Rows::Core) || matches!(&rows, Rows::Command(s) if !s.loaded);
    let at = RepoRef { id: repo.id, owner: owner.to_owned(), owner_id: repo.owner, name: name.to_owned() };
    let read_authz = timed(c, "authz", async {
        match known {
            Some(a) => Some(a),
            None => authz(c, &repo).await,
        }
    });
    let read_rows = async {
        match rows {
            Rows::Merge(_, Some(v)) => v,
            Rows::Api(route) => {
                let spec = graftyard_api::narrow::Spec { at: at.clone(), route };
                let v = api_rows(c, &repo, &spec).await?;
                if let Ok(v) = &v {
                    member_stages(c, v);
                }
                match v {
                    // A route the member answers whole, or a member that cannot read the spec: both whole tiers.
                    Ok(v) if v["value"].get("whole").is_some() || v["value"].get("error").is_some() => {
                        tier_rows(c, &repo, None).await
                    }
                    v => Ok(v),
                }
            }
            Rows::Command(mut spec) => {
                spec.owner = repo.owner.0.0;
                command_tier_rows(c, &repo, &spec).await
            }
            _ => tier_rows(c, &repo, merge).await,
        }
    };
    let (v, mut authz) = futures_util::future::join(timed(c, "tier", read_rows), read_authz).await;
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
    // The read gate (`crate::read_gate`), on the rows this read already holds: every read and merge row set carries
    // `settings` and the `collaborator` rows. A private repository whose owner's `Authz` did not answer is 503, not a
    // 404 the caller would take as "no such repository".
    let core = TierRows::store(&rows.core);
    // A read-class request's owner `Authz` may be up to `READ_CACHE` old (`crate::rcache`): one that would hide the
    // repository is read again fresh first, so a grant shows at once and only a revocation waits out the cache.
    if gate == Gate::Readers
        && c.read_class.get()
        && may_read(c, &repo, &core, authz.as_ref(), viewer) != graftyard_identity::Decision::Allow
    {
        authz = timed(c, "authz_fresh", crate::authz_read_as(c, repo.owner, &repo.record, false)).await.ok().flatten();
    }
    if gate == Gate::Readers {
        match may_read(c, &repo, &core, authz.as_ref(), viewer) {
            graftyard_identity::Decision::Allow => {}
            _ if authz.is_none() => return Ok(Err(unavailable("the owner's Authz chain did not answer; retry"))),
            _ => return Ok(Err(Reply::not_found())),
        }
    }
    let picks = graftyard_api::narrow::Picks::of(rows.picks.clone());
    let mut dir = HostedDir { home: Some(at.clone()), authz, viewer: None, picks, ..HostedDir::default() };
    dir.viewer = viewer.and_then(|p| match p.actor {
        Actor::User(u) => Some(u.0),
        Actor::App { .. } | Actor::Anonymous => None,
    });
    let meta = TierRows::store(meta);
    let mut rd = Read { repo, at, core, meta, dir, index: rows.index };
    if merge.is_none() {
        rd.dir.logins = timed(c, "logins", read_logins(c, &rd)).await;
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
/// loads (`Core`); a merge's rows (`TierRows::merge_rows`) for the pull request it names, with the leader's answer
/// if the caller already read it; or only the rows one API read's reply renders
/// (`graftyard_api::narrow`), else both whole tiers when the member answers that the route needs them.
pub(crate) enum Rows {
    All,
    Core,
    Merge(MergeOf, Option<Result<std::result::Result<Value, Reply>>>),
    Api(graftyard_api::narrow::Route),
    /// A command's read (`CommandSpec`): the read gate's rows and the one task it names. `served_meta` serves it with
    /// an empty metadata view while that tier loads unless the spec needs it loaded.
    Command(CommandSpec),
}

/// The member's split of a read it answered (`stages`, `[grant, waited]` ms): `tier_grant`, the wait for the
/// leader's read index to be applied, and `tier_wait`, the wait for the answer's build (narrow-reads-3). A member built
/// before it sends none.
fn member_stages(c: &Ctx<'_>, v: &Value) {
    if let Some([g, w]) = v.get("stages").and_then(Value::as_array).map(Vec::as_slice) {
        c.timing.put("tier_grant", g.as_f64().unwrap_or(0.0));
        c.timing.put("tier_wait", w.as_f64().unwrap_or(0.0));
    }
}

/// A narrow API read (`{"rows": {"api": spec}}`, `graftyard_api::narrow::answer`) on the repository's learners and
/// followers by read-index, as `tier_rows` reads: only the rows one reply renders.
async fn api_rows(
    c: &Ctx<'_>,
    repo: &Repo,
    spec: &graftyard_api::narrow::Spec,
) -> Result<std::result::Result<Value, Reply>> {
    rows_read(c, repo, &json!({ "q": { "rows": { "api": spec } } })).await
}

/// A command read (`{"rows": {"access": true, "command": spec}}`, `hosted::TierRows::command_rows`), or both whole
/// tiers from a member that answers it without `"command": true`, which a member built before it does.
async fn command_tier_rows(c: &Ctx<'_>, repo: &Repo, spec: &CommandSpec) -> Result<std::result::Result<Value, Reply>> {
    let q = json!({ "q": { "rows": { "access": true, "command": spec }, "ranges": true } });
    match rows_read(c, repo, &q).await? {
        Ok(v) if v["value"]["command"] == json!(true) => Ok(Ok(v)),
        Ok(_) => tier_rows(c, repo, None).await,
        Err(r) => Ok(Err(r)),
    }
}

/// A rows read `q` on the repository's learners and followers by read-index, as `tier_rows` reads, creating its group
/// on first use.
async fn rows_read(c: &Ctx<'_>, repo: &Repo, q: &Value) -> Result<std::result::Result<Value, Reply>> {
    let chain = ChainId::Repo(repo.id);
    let b = c.bindings();
    let q = q.clone();
    let mut v = crate::spread_read(c, &b, chain, &repo.record, &q).await.map(|o| o.map(|(v, _, _)| v));
    if let Ok(None) = v {
        if let Err(r) = first_use(c, repo).await? {
            return Ok(Err(r));
        }
        v = crate::spread_read(c, &b, chain, &repo.record, &q).await.map(|o| o.map(|(v, _, _)| v));
    }
    Ok(match v {
        Ok(Some(v)) => Ok(v),
        Ok(None) => Err(unavailable("the repository's group has no leader; retry")),
        Err(e) => Err(read_failed("the repository's tier rows did not answer; retry", &e)),
    })
}

/// The logins of every account `rd`'s tiers name, the viewer's, and the owner's from the request path (F50).
/// For a narrow read, the member's picks name them, cut as a whole-tier read's were
/// (`graftyard_api::narrow::Picks::login_accounts`).
async fn read_logins(c: &Ctx<'_>, rd: &Read) -> std::collections::BTreeMap<i64, String> {
    let named = match rd.dir.picks.login_accounts(rd.dir.viewer, rd.repo.owner) {
        Some(n) => n.into_iter().collect(),
        None => {
            let mut named = graftyard_api::hosted::accounts(&rd.core, &rd.meta);
            named.extend(rd.dir.viewer);
            named.remove(&rd.repo.owner);
            named
        }
    };
    let mut out = account_logins(c, &named, LOGIN_SHARDS_MAX).await;
    out.insert(rd.repo.owner.0.0, rd.at.owner.clone());
    for (id, l) in logins(c) {
        out.entry(id).or_insert(l);
    }
    out
}

/// The repository's `{"rows": true}` read, on a follower by read-index first and then the leader
/// (`graftyard_lookup::read_spread`), creating its group on first use; `{"rows": {"merge": id}}` (`MergeOf::query`)
/// for a merge's rows only, which a member built before it answers with every row. A leader that kept shedding the
/// read (`graftyard_lookup::OVERLOADED`) answers 503 with `Retry-After: 1`. Identical reads are not shared across
/// requests: a Workers isolate refuses a promise resolved from another request's context, and every such read
/// answered 500 (`tests/am1/results/lm1-memory.md`); the member's dump cache serves them instead.
pub(crate) async fn tier_rows(
    c: &Ctx<'_>,
    repo: &Repo,
    merge: Option<MergeOf>,
) -> Result<std::result::Result<Value, Reply>> {
    let chain = ChainId::Repo(repo.id);
    let b = c.bindings();
    let q = tier_rows_query(merge);
    // A merge's few rows are read on the leader its commit goes to next, from its lease: through a learner or
    // follower they waited out a read-index round from a far region or a member behind, p99 1.0 to 1.4 s at 500
    // agents, for 10 merges a minute (`merge-p99`). Whole-tier reads stay off the leader.
    let read = || async {
        let r = match merge {
            Some(_) => graftyard_lookup::call_opt(&b, chain, &repo.record, "/read", q.to_string().as_bytes()).await,
            None => crate::spread_read(c, &b, chain, &repo.record, &q).await,
        };
        r.map(|o| o.map(|(v, _, _)| v))
    };
    let mut v = read().await;
    if let Ok(None) = v {
        if let Err(r) = first_use(c, repo).await? {
            return Ok(Err(r));
        }
        v = read().await;
    }
    Ok(match v {
        Ok(Some(v)) => Ok(v),
        Ok(None) => Err(unavailable("the repository's group has no leader; retry")),
        Err(e) => Err(read_failed("the repository's tier rows did not answer; retry", &e)),
    })
}

/// A repository read's group, created on first use (`crate::ensure_group`). A repository whose create has not
/// committed its `RepoOwner` row yet is 404, as GitHub answers a repository no one can see yet: its members refuse to
/// join for that reason alone (`graftyard_lookup::NOT_CREATED`). So is one whose group was retired, as a deleted one
/// reads (`graftyard_lookup::RETIRED`). Any other failure stays an error, a 503.
async fn first_use(c: &Ctx<'_>, repo: &Repo) -> Result<std::result::Result<(), Reply>> {
    match ensure_group(c, repo).await {
        Ok(()) => Ok(Ok(())),
        Err(e) if graftyard_lookup::is_gone(&e) => Ok(Err(Reply::not_found())),
        Err(e) => Err(Error::RustError(e)),
    }
}

/// A spread or leader read that failed (`graftyard_lookup::read_spread`): 503 with `Retry-After`, 1 s when the
/// members shed it (`graftyard_lookup::OVERLOADED`), which they answer within about one FOLLOWER_WAIT, else 5 s.
/// A read that `tier-off-leader` shed (`graftyard_lookup::TIER_OFF`: every member declined a whole-tier dump, which
/// the leader never serves) is also logged as `{"shed": "tierOff"}`, so a row counts the 503s this rule makes apart from
/// the members' own sheds (`decisions/DECISIONS.md`, "`tier-off-leader`", the flip).
pub(crate) fn read_failed(why: &str, e: &str) -> Reply {
    #[cfg(target_arch = "wasm32")]
    if let Some(line) = tier_off_line(why, e) {
        worker::console_log!("{line}");
    }
    let retry = if e.contains(graftyard_lookup::OVERLOADED) { "1" } else { "5" };
    Reply::error(503, &format!("{why} ({e})"), "rest").with("retry-after", retry)
}

/// `read_failed`'s log line for a `tier-off-leader` shed, `None` for any other failed read.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn tier_off_line(why: &str, e: &str) -> Option<String> {
    e.contains(graftyard_lookup::TIER_OFF)
        .then(|| json!({ "shed": graftyard_lookup::TIER_OFF, "why": why }).to_string())
}

/// `tier_rows`'s query. `ranges` too: a member built before the `rows` read answers its small ranges reply instead of
/// every ref. `packed`: the whole tiers as one JSON string (`TierRows::packed`), which `TierRows::from_value` reads.
fn tier_rows_query(merge: Option<MergeOf>) -> Value {
    let rows = merge.map_or(json!(true), MergeOf::query);
    json!({ "q": { "rows": rows, "ranges": true, "packed": true } })
}

/// The repository's narrow `{"rows": {which: true}}` core read, on a follower by read-index first and then the
/// leader (`graftyard_lookup::read_spread`): `access` for `crate::read_gate`, `agent` for an agent push, or `actions`
/// for `acquirejob`'s secrets and variables. `Ok(Err(why))` when the leader cannot answer it now.
pub(crate) async fn core_rows(c: &Ctx<'_>, repo: &Repo, which: &str) -> Result<std::result::Result<MemStore, String>> {
    Ok(core_rows_q(c, repo, json!({ which: true })).await?.map(|(store, _)| store))
}

/// A push's read gate rows and its agent rows in one read (`{"rows": {"access": true, "agent": true}}`, `design/DESIGN.md`,
/// journey 1, step 1): the store, and whether the member answered the agent rows too (`"agent": true` beside them; a
/// member built before it answers the access rows alone, and `agents::push_context` then reads them on their own).
pub(crate) async fn push_rows(c: &Ctx<'_>, repo: &Repo) -> Result<std::result::Result<(MemStore, bool), String>> {
    core_rows_q(c, repo, json!({ "access": true, "agent": true })).await
}

async fn core_rows_q(c: &Ctx<'_>, repo: &Repo, rows: Value) -> Result<std::result::Result<(MemStore, bool), String>> {
    core_rows_at(c, repo, rows, true).await
}

/// The read gate's access rows on a guessed repository (`beside`): `core_rows`, but a group that answers no leader is
/// not created on a guess; `None` then, and the gate reads again on the resolved repository.
pub(crate) async fn guessed_rows(c: &Ctx<'_>, repo: &Repo) -> Result<std::result::Result<Option<MemStore>, String>> {
    Ok(match core_rows_at(c, repo, json!({ "access": true }), false).await? {
        Ok((store, _)) => Ok(Some(store)),
        Err(e) if e == NO_LEADER => Ok(None),
        Err(e) => Err(e),
    })
}

const NO_LEADER: &str = "the repository's group has no leader";

async fn core_rows_at(
    c: &Ctx<'_>,
    repo: &Repo,
    rows: Value,
    create: bool,
) -> Result<std::result::Result<(MemStore, bool), String>> {
    let chain = ChainId::Repo(repo.id);
    let b = c.bindings();
    let q = json!({ "q": { "rows": rows, "ranges": true } });
    let mut v = crate::spread_read(c, &b, chain, &repo.record, &q).await;
    if create && let Ok(None) = v {
        if let Err(e) = ensure_group(c, repo).await {
            // `crate::read_gate` reads "not created yet" and a retired group as a repository the caller cannot see.
            return if graftyard_lookup::is_gone(&e) { Ok(Err(e)) } else { Err(Error::RustError(e)) };
        }
        v = crate::spread_read(c, &b, chain, &repo.record, &q).await;
    }
    Ok(match v {
        Ok(Some((v, _, _))) => match TierRows::from_value(&v["value"]) {
            Ok(rows) => Ok((TierRows::store(&rows.core), v["value"]["agent"] == json!(true))),
            Err(_) => Err("the repository's members do not serve tier rows yet (site release)".into()),
        },
        Ok(None) => Err(NO_LEADER.into()),
        Err(e) => Err(e),
    })
}

/// The owner's `Authz` fold by read-index (`crate::authz_read`), for permissions and `author_association`.
async fn authz(c: &Ctx<'_>, repo: &Repo) -> Option<AuthzFold> {
    crate::authz_read(c, repo.owner, &repo.record).await.ok().flatten()
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

/// Whether `who`'s single-PR read, GraphQL or REST, has its missing test merge computed (`ask_merges`): a writer's or
/// an App's (CI) does, anyone else's is counted as a list's.
fn merge_computes(r: &View<'_>, who: Option<&Principal>) -> bool {
    can_write(r, who) || who.is_some_and(|w| matches!(w.actor, Actor::App { .. }))
}

async fn route(c: &Ctx<'_>, req: &mut Request, host: &Host, e: Endpoint<'_>, m: &Meta) -> Result<Reply> {
    let method = req.method();
    let url = req.url()?;
    let query = url.query().unwrap_or("").to_owned();
    // M4-WP2 (`crate::activity`): only once 32.19 and 0x14.3 are enabled; before that every path falls through.
    if let Endpoint::Rest(p) = e {
        let segs: Vec<&str> = p.trim_end_matches('/').split('/').skip(1).collect();
        if let Some(r) = crate::activity::route(c, req, host, &segs, &query).await? {
            return Ok(r);
        }
    }
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
            (m @ (Method::Get | Method::Patch), ["app", "hook", "config"]) => {
                crate::ident::app_hook_config(c, req, m.clone()).await
            }
            (Method::Delete, ["installation", "token"]) => crate::ident::delete_installation_token(c, req).await,
            (Method::Delete, ["applications", client, "token"]) => crate::ident::revoke_app_token(c, req, client).await,
            (Method::Post, ["applications", client, "token", "scoped"]) => {
                crate::mcp::token_scoped(c, req, client).await
            }
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
            (Method::Delete, ["user", "keys", id]) => crate::ssh::delete_key(c, req, id).await,
            (_, ["orgs", _, "teams", ..] | ["user", "memberships", "orgs", _]) => {
                crate::collab::org_route(c, req, p).await
            }
            (_, ["orgs", _, "actions", "secrets" | "variables", ..]) => {
                let segs: Vec<&str> = p.trim_end_matches('/').split('/').skip(1).collect();
                match crate::secrets::org_route(c, req, &segs).await? {
                    Some(r) => Ok(r),
                    None => Ok(Reply::not_found()),
                }
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

/// `commit_issue` under its own request id, for an edge route that commits several commands in one request (the
/// Actions rotations): under the request's own id the leader would take the second as a replay of the first and
/// answer the first's outcome without committing it (BF-59 run 1, `tests/m4/results/m4-wp3.md`).
pub(crate) async fn commit_issue_as(
    c: &Ctx<'_>,
    rd: &Read,
    who: &Principal,
    op: IssueOp<BlobRef>,
    what: What,
    rid: graftyard_types::RequestId,
) -> Result<Reply> {
    commit_issue_rid(c, rd, who, op, what, None, rid).await
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
    commit_issue_rid(c, rd, who, op, what, session, c.rid).await
}

async fn commit_issue_rid(
    c: &Ctx<'_>,
    rd: &Read,
    who: &Principal,
    op: IssueOp<BlobRef>,
    what: What,
    session: Option<graftyard_identity::SessionId>,
    rid: graftyard_types::RequestId,
) -> Result<Reply> {
    if !c.cfg.enabled.contains(&REPO_FORMAT_ISSUES) {
        return Ok(unavailable(&format!(
            "issue writes need record format {}.{}, which FormatsEnable has not enabled on this install",
            REPO_FORMAT_ISSUES.family, REPO_FORMAT_ISSUES.version
        )));
    }
    let render = Render { host: c.cfg.host.clone(), isolation: false, dir: rd.dir.clone(), what };
    let mut call = IssueCall::new(op, who.clone(), rid, serde_json::to_value(&render)?);
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
    // 32.22 for a review comment outside a review, whose single-comment review then leaves an APPROVED or
    // CHANGES_REQUESTED verdict alone. A 32.8 or 32.14 mark wins, and the comment keeps that format's review.
    call.reviews = !call.agent
        && !call.leases
        && IssueCall::single_comment(&call.op)
        && c.cfg.enabled.contains(&graftyard_git::issue_ops::call::REPO_FORMAT_REVIEWS);
    if !c.cfg.enabled.contains(&call.format()) {
        let f = call.format();
        return Ok(unavailable(&format!(
            "this write needs record format {}.{}, which FormatsEnable has not enabled on this install",
            f.family, f.version
        )));
    }
    let mut body = json!({ "rid": rid.to_string(), "cmd": { "issue": call } });
    if let Some(a) = crate::admission(c, who, &[], false, 0).await {
        body["admit"] = serde_json::to_value(a)?;
    }
    // `leader-render`: the first send carries the render without the owner's `Authz`, which the leader fills from its
    // own dependency read; a later send and `outcome_by_request` carry the whole render, for a duplicate answered from
    // its `request` row, which has no dependency.
    let first = without_authz(&body).to_string();
    let whole = body.to_string();
    // `rd` came from the group's leader, so the group exists: no create probe before the commit.
    let hooks = graftyard_lookup::Hooks {
        first: Some(first.as_bytes()),
        trace: None,
        lookup: None,
        #[cfg(feature = "test-faults")]
        lean: c.lean,
    };
    let (b, chain) = (c.bindings(), ChainId::Repo(rd.repo.id));
    let mut r =
        graftyard_lookup::commit_hedged(&b, chain, &rd.repo.record, whole.as_bytes(), c.colo, 60_000, hooks).await;
    // Test installs: which `Authz` the committed reply rendered from (`authz_source`), one Workers Logs line per command.
    #[cfg(feature = "test-faults")]
    if let Some(source) = r.as_ref().ok().and_then(authz_source) {
        worker::console_log!("{}", json!({ "authzSource": source, "rid": rid.to_string() }));
    }
    // A member built before `leader-render` rendered the first send as it came, with no owner `Authz`: its answer
    // names no source. The command committed, so its outcome by request id renders it again with the whole render.
    if let Ok(done) = &r
        && graftyard_api::hosted::rendered_without_authz(done)
    {
        r = match graftyard_lookup::call_raw(&b, chain, &rd.repo.record, "/outcome_by_request", whole.as_bytes()).await
        {
            Ok((v, _, _)) => match v.get("done") {
                Some(d) => Ok(d.clone()),
                None => Err(Err(format!("the leader answered no outcome for the committed command: {v}"))),
            },
            Err(e) => Err(Err(e)),
        };
    }
    Ok(match r {
        Ok(done) => {
            // The refusal's cause (`IssueErr`) is not in the client's reply, so it is logged.
            if let Some(e) = done.pointer("/refused/err") {
                worker::console_log!("issue command {} refused: {e}", rid);
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

/// An issue command's `/commit` body with its render's owner `Authz` left out (`decisions/DECISIONS.md`,
/// "`leader-render`"): about 10 KB of each command, which the leader fills from its own dependency read.
/// Where a committed issue command's reply took the owner's `Authz` from (`leader-render`): `"dep"`, the leader's own
/// dependency read; `"render"`, the edge's render (a later send or an `outcome_by_request`); `"fallback"`, a member that
/// names no source, which the edge then asks `outcome_by_request` with the whole render. `None` for a refusal or any
/// answer that is not an issue command's.
#[cfg(any(test, feature = "test-faults"))]
fn authz_source(done: &Value) -> Option<&'static str> {
    if graftyard_api::hosted::rendered_without_authz(done) {
        return Some("fallback");
    }
    match done.pointer("/ok/out/issue/authz")?.as_str()? {
        "dep" => Some("dep"),
        "render" => Some("render"),
        _ => None,
    }
}

fn without_authz(body: &Value) -> Value {
    let mut b = body.clone();
    if let Some(dir) = b.pointer_mut("/cmd/issue/render/dir") {
        dir["authz"] = Value::Null;
    }
    b
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
    let caller = who.as_ref().and_then(|p| match p.actor {
        Actor::App { app, .. } => Some(app),
        _ => None,
    });
    // A create's requester, by login, before the read: its reply names the account (`CommandSpec::accounts`).
    let requester = match (&route, &method, body.get("requester").and_then(Value::as_str)) {
        (Route::Tasks { .. }, Method::Post, Some(login)) => match crate::account_of(c, login).await {
            Ok(a) => a,
            Err(e) => return Ok(unavailable(&e)),
        },
        _ => None,
    };
    let rd = match task_command(&route, &method, caller, requester) {
        None => read(c, owner, repo, who.as_ref()).await?,
        Some(spec) => read_command(c, owner, repo, who.as_ref(), spec).await?,
    };
    let rd = match rd {
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
            t::parse_create(body, requester.map(graftyard_identity::UserId)).map(IssueOp::CreateTask)
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

/// The command read a task request makes (`command-rows`): none for a `GET`, which renders the tiers here; a write
/// reads the read gate's rows and the task it names, with the metadata tier loaded and the logins its reply may name
/// (a create's requester, resolved before the read), and its reply renders on the leader.
fn task_command(
    route: &graftyard_api::tasks::Route<'_>,
    method: &Method,
    caller: Option<graftyard_git::issues::AppId>,
    requester: Option<graftyard_types::AccountId>,
) -> Option<CommandSpec> {
    use graftyard_api::tasks::Route;
    if *method == Method::Get {
        return None;
    }
    let session = match route {
        Route::Task { id, .. } | Route::Cancel { id, .. } => graftyard_api::tasks::path_id(id, caller),
        _ => None,
    };
    let accounts = requester.map(|a| a.0.0).into_iter().collect();
    Some(CommandSpec { session, loaded: true, accounts, owner: 0 })
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
    // The task's own rows (`command-rows`): `token_task` reads only its `agent_task` row.
    let spec = CommandSpec { session: Some(session.clone()), owner: owner.0.0, ..CommandSpec::default() };
    let (rows, install) = futures_util::future::join(command_tier_rows(c, &repo, &spec), crate::install_authz(c)).await;
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
    c.timing.mark("merge_plan");
    #[cfg(feature = "test-faults")]
    if let Ok(Some(t)) = r.headers().get("x-graft-timing") {
        *c.timing.shard.borrow_mut() = t;
    }
    Ok(a.merge_plan(req))
}

/// After a GraphQL read's reply, what its `mergeable` fields lacked (`graftyard_api::gql::MergeAsks`), to the leader
/// site's shard of the network, as `merge_plan` finds it: each single-PR read's merge as a GET's (`/demand?cause=get`),
/// which the shard coalesces per PR, budgets, computes, and commits, so a later poll reads it, and one count of the PRs
/// a list rendered (`/ask?cause=list`), which never computes. A GET computes only for a writer or an App token (CI);
/// another reader's is counted as a list's, a field that does not compute. The reply never waits on the shard.
fn ask_merges(c: &Ctx<'_>, repo: &Repo, asks: graftyard_api::gql::MergeAsks, computes: bool) {
    use graftyard_git::prsync::MergeCause;
    if asks.single.is_empty() && asks.listed.is_empty() {
        return;
    }
    let leader = graftyard_lookup::cached_leader(ChainId::Repo(repo.id)).map(|t| t.site);
    let Ok(stub) = crate::shard_at(c, repo, leader) else { return };
    let Ok(record) = serde_json::to_string(&repo.record) else { return };
    let record = String::from(js_sys::encode_uri_component(&record));
    let id = repo.id.0.0;
    let mut urls: Vec<String> = Vec::new();
    let mut listed = asks.listed.len();
    for (n, (base, head)) in &asks.single {
        if computes {
            let (b, h, cause) = (base.hex(), head.hex(), MergeCause::Get.as_str());
            urls.push(format!(
                "https://shard/demand?repo={id}&n={}&base={b}&head={h}&cause={cause}&record={record}",
                n.0
            ));
        } else {
            listed += 1;
        }
    }
    if listed > 0 {
        urls.push(format!("https://shard/ask?repo={id}&cause={}&count={listed}", MergeCause::List.as_str()));
    }
    c.ctx.wait_until(async move {
        for url in urls {
            let mut init = RequestInit::new();
            init.with_method(Method::Post);
            let Ok(req) = Request::new_with_init(&url, &init) else { continue };
            if let Err(e) = stub.fetch_with_request(req).await {
                console_error!("merge ask {}: {e}", url.split('&').next().unwrap_or(""));
            }
        }
    });
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
    c.timing.mark("plan");
    // `rd` came from the group's leader, so the group exists: no create probe before the commit.
    let trace_steps = std::cell::RefCell::new(Vec::new());
    let hooks = graftyard_lookup::Hooks {
        first: None,
        trace: cfg!(feature = "test-faults").then_some(&trace_steps),
        lookup: None,
        #[cfg(feature = "test-faults")]
        lean: c.lean,
    };
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
    c.timing.mark("commit");
    trace(c, trace_steps.into_inner());
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
    Ok(match crate::commit(c, &rd.repo, &body, None, None, None).await? {
        Ok(PushOutcome::Ok(ok)) if ok.out.refs.iter().all(|o| *o == graftyard_git::repo::RefOutcome::Ok) => {
            Reply::empty(204)
        }
        Ok(PushOutcome::Ok(_)) => Reply::error(422, "Reference cannot be updated", "rest/git/refs"),
        Ok(PushOutcome::Refused(r)) if r.status == 403 => {
            Reply::error(403, "Resource not accessible by integration", "rest/git/refs")
        }
        Ok(PushOutcome::Refused(r)) => crate::agents::ref_refused(r.status, &r.err, "rest/git/refs"),
        Ok(PushOutcome::Full(f)) => Reply::secondary_rate_limit(f.retry_after_s),
        Ok(PushOutcome::NotLeader(_)) | Err(_) => unavailable("the repository's group has no leader; retry"),
    })
}

// ---- GraphQL ----

async fn graphql_post(c: &Ctx<'_>, req: &mut Request, host: &Host) -> Result<Reply> {
    let session = req.headers().get("x-graftyard-agent-session")?;
    let body: Value = req.json().await.unwrap_or(Value::Null);
    // A persisted document by hash (the web app's); a request that carries its own `query` is unchanged.
    let q = match crate::ui::persisted(&body) {
        Ok(Some(text)) => text,
        Ok(None) => body["query"].as_str().unwrap_or("").to_owned(),
        Err(reply) => return Ok(reply),
    };
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
    // A query is read-class (`Ctx::read_class`); a mutation, including a merge, reads everything fresh.
    c.read_class.set(!gq.is_mutation().unwrap_or(true));
    c.write.set(!c.read_class.get());
    c.decided.set(graphql_decided(&gq));
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
                        Some(id) => Some(tier_rows(c, &repo, Some(MergeOf::Id(graftyard_types::DbId(id)))).await),
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
    let (who, found) =
        futures_util::future::join(timed(c, "auth", authenticate(c, req)), timed(c, "resolve", find)).await;
    let Some(who) = who? else { return Ok(Reply::json(401, json!({ "message": "Bad credentials", "status": "401" }))) };
    let (owner, name, rd, found) = match found? {
        None => {
            let viewer = match who.actor {
                Actor::User(u) => crate::ident::login(c, u.0).await.map(|login| graphql::Viewer { id: u.0, login }),
                _ => None,
            };
            return Ok(Reply::json(200, graphql::query_viewer(&gq, viewer.as_ref())));
        }
        Some(Ok((owner, name, repo, a, rows))) => {
            let rows = match merging {
                Some(id) => Rows::Merge(MergeOf::Id(graftyard_types::DbId(id)), rows),
                // A query reads only the rows its reply renders (`graftyard_api::narrow`); a mutation plans on both
                // whole tiers.
                None if !gq.is_mutation().unwrap_or(true) => Rows::Api(graftyard_api::narrow::Route::Graphql {
                    query: q.clone(),
                    variables: vars.clone(),
                    op: body["operationName"].as_str().map(str::to_owned),
                }),
                None => Rows::All,
            };
            let rd = read_repo(c, repo.clone(), &owner, &name, Some(&who), a.clone(), rows, Gate::Readers).await?;
            (owner, name, rd, Some((repo, a)))
        }
        Some(Err(Some(e))) => return Ok(read_failed("the repository lookup failed; retry", &e)),
        Some(Err(None)) => match &target {
            Some(graphql::Target::Name { owner, name }) => (owner.clone(), name.clone(), Err(Reply::not_found()), None),
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
        let rendered = js_sys::Date::now();
        let (mut v, mut asks) = graphql::query_asks(&gq, Some(&r));
        if crate::ui::read_compares(c, &rd).await {
            (v, asks) = graphql::query_asks(&gq, Some(&r));
        }
        if !rd.dir.picks.missed() {
            ask_merges(c, &rd.repo, asks, merge_computes(&r, Some(&who)));
            c.timing.add("render", rendered);
            return Ok(Reply::json(200, v));
        }
        // The render asked for a selection the member did not make (a member of another release): both whole tiers.
        drop(r);
        let Some((repo, a)) = found else { return Ok(unavailable("the repository could not be read; retry")) };
        let rd = match read_repo(c, repo, &owner, &name, Some(&who), a, Rows::All, Gate::Readers).await? {
            Ok(rd) => rd,
            Err(r) => return Ok(r),
        };
        let r = rd.view(host);
        let (mut v, mut asks) = graphql::query_asks(&gq, Some(&r));
        if crate::ui::read_compares(c, &rd).await {
            (v, asks) = graphql::query_asks(&gq, Some(&r));
        }
        ask_merges(c, &rd.repo, asks, merge_computes(&r, Some(&who)));
        return Ok(Reply::json(200, v));
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

/// Whether a repository REST write is one whose token row may come from `rcache` (`Ctx::decided`, `write-identity`):
/// it commits one command on the repository's chain, an issue command or a `Push` with no pack, whose `decide` reads
/// the caller's credential shard first and whose refusal renders 401 "Bad credentials", and it does nothing under the
/// caller's authority before that commit. A merge plans on the git cache shard first, a run's cancel commits on the
/// Actions shard, whose `decide` reads no credential, and the routes `collab`, `repos`, `hooks`, `secrets`, and
/// `activity` serve may answer without a command or render the refusal as another status, so they read fresh.
fn rest_decided(method: &Method, tail: &[&str]) -> bool {
    matches!(
        (method, tail),
        (Method::Post, ["labels"])
            | (Method::Post, ["pulls", _, "comments"])
            | (Method::Post, ["check-runs"])
            | (Method::Post | Method::Put | Method::Delete, ["issues", _, "labels"] | ["issues", _, "labels", _])
            | (Method::Post, ["releases"])
            | (Method::Patch, ["releases", _])
            | (Method::Delete, ["releases", _] | ["releases", "assets", _])
            | (Method::Post, ["statuses", _])
            | (Method::Put | Method::Delete, ["branches", .., "protection"])
            | (Method::Post, ["git", "refs"])
            | (Method::Patch, ["git", "refs", ..])
            | (Method::Delete, ["git", "refs", ..])
    )
}

/// Whether a GraphQL request is a mutation whose token row may come from `rcache` (`Ctx::decided`): every root field
/// plans one command on the repository's chain whose `decide` reads the caller's credential shard and whose refusal
/// renders 401 (`refused_field`, `refused_merge_field`), with nothing done first. `mergePullRequest` plans on the git
/// cache shard first, and `createRepository` commits on the owner's `Authz` and renders its refusal 200
/// `UNPROCESSABLE`, so both read fresh, as a query is read-class anyway.
fn graphql_decided(gq: &Gql<'_>) -> bool {
    gq.is_mutation().unwrap_or(false)
        && gq.roots().is_ok_and(|roots| {
            !roots.is_empty()
                && roots.iter().all(|(f, _)| !matches!(f.as_str(), "mergePullRequest" | "createRepository"))
        })
}

/// Whether a REST write plans from no row of the tiers, so it reads the read gate's rows alone (`Rows::Command`): a
/// check run, a commit status, and a label create. Each reply renders on the leader; it needs the metadata tier
/// loaded, as its whole-tier read did, and the login picks for the status's creator.
fn rest_command(method: &Method, tail: &[&str]) -> bool {
    matches!((method, tail), (Method::Post, ["check-runs"] | ["statuses", _] | ["labels"]))
}

/// The pull request a REST merge (`PUT pulls/{n}/merge`) names; `None` for every other request.
fn rest_merge(method: &Method, tail: &[&str]) -> Option<MergeOf> {
    match (method, tail) {
        (Method::Put, ["pulls", n, "merge"]) => {
            n.parse().ok().map(|n| MergeOf::Number(graftyard_git::issues::IssueNumber(n)))
        }
        _ => None,
    }
}

/// The reads a REST write made ahead for its `guess` (`resolve_cached`), kept only when its own fresh `resolve`
/// answered the same repository, owner, and placement record; a failed `Authz` read is dropped and read again.
fn kept_ahead<T, A, E>(
    found: &Option<std::result::Result<Option<Repo>, String>>,
    guess: Option<&Repo>,
    ahead: Option<(T, std::result::Result<Option<A>, E>)>,
) -> Option<(T, Option<A>)> {
    match (found, guess, ahead) {
        (Some(Ok(Some(r))), Some(g), Some((rows, authz))) if r == g => Some((rows, authz.ok().flatten())),
        _ => None,
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
pub(crate) async fn repo_named(c: &Ctx<'_>, repo: Repo) -> Option<(String, String, Repo, AuthzFold)> {
    repo_named_why(c, repo).await.ok()
}

/// `repo_named`, naming the read that failed: `acquirejob` logs it (F79).
pub(crate) async fn repo_named_why(
    c: &Ctx<'_>,
    repo: Repo,
) -> std::result::Result<(String, String, Repo, AuthzFold), String> {
    // The owner's login and `Authz` at once: each needs only the owner and its record.
    let owner = std::iter::once(repo.owner).collect();
    let (mut known, a) = futures_util::future::join(
        account_logins(c, &owner, LOGIN_SHARDS_MAX),
        crate::authz_read(c, repo.owner, &repo.record),
    )
    .await;
    let login = known
        .remove(&repo.owner.0.0)
        .or_else(|| logins(c).remove(&repo.owner.0.0))
        .ok_or("the owner's login did not answer")?;
    let a = a.map_err(|e| format!("the owner's Authz: {e}"))?.ok_or("the owner has no Authz")?;
    let name = a.id_name(repo.id).ok_or("the owner's Authz names no such repository")?.as_str().to_owned();
    Ok((login, name, repo, a))
}

/// The most `account_shard` groups one request reads for logins [guess]. An account past it renders as `ghost`, as an
/// account with no row does.
const LOGIN_SHARDS_MAX: usize = graftyard_api::narrow::LOGIN_SHARDS_MAX;

/// The logins of `accounts`, from each one's `AccountLogin` row (F50): one read-class `fresh` read per distinct
/// `account_shard`, at most `shards` of them, all at once. A shard that fails to answer, or whose members predate the
/// row, names none of its accounts, which then render as `ghost`.
pub(crate) async fn account_logins(
    c: &Ctx<'_>,
    accounts: &std::collections::BTreeSet<graftyard_types::AccountId>,
    shards: usize,
) -> std::collections::BTreeMap<i64, String> {
    // A read-class request may use a login up to `READ_CACHE` old (`crate::rcache`); an account with no login row
    // is read again.
    let at = crate::now();
    let mut out = std::collections::BTreeMap::new();
    let mut by_shard: std::collections::BTreeMap<_, Vec<i64>> = Default::default();
    for a in accounts {
        match crate::rcache::get(&crate::rcache::ACCOUNT_LOGINS, &a.0.0, at, c.read_class.get()) {
            Some(l) => {
                out.insert(a.0.0, l);
            }
            None => by_shard.entry(graftyard_identity::account_shard(*a)).or_default().push(a.0.0),
        }
    }
    let reads = by_shard.into_iter().take(shards).map(|(shard, ids)| async move {
        crate::system_read(c, ChainId::Names(shard), json!({ "accountLogins": ids })).await
    });
    for v in futures_util::future::join_all(reads).await.into_iter().flatten() {
        for (id, l) in v["accountLogins"].as_object().into_iter().flatten() {
            if let (Ok(id), Some(l)) = (id.parse(), l.as_str()) {
                crate::rcache::put(&crate::rcache::ACCOUNT_LOGINS, id, at, l.to_owned());
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
    let method = req.method();
    // A `GET` is read-class (`Ctx::read_class`). Any request reads its token and `o/r` at once, as `timed` stages:
    // neither needs the other. A write used to resolve after its token, untimed (`merge-p99`).
    c.read_class.set(method == Method::Get);
    c.write.set(method != Method::Get);
    c.decided.set(rest_decided(&method, tail));
    let merge_of = rest_merge(&method, tail);
    c.leader_reads.set(merge_of.is_some());
    let find = async {
        match &at {
            graftyard_wire::RepoPath::Name { owner, name } => Some(resolve(c, owner, name).await),
            graftyard_wire::RepoPath::Id(_) => None,
        }
    };
    // A REST merge reads its rows and the owner's `Authz`, both fresh, for the repository this isolate last resolved
    // the name to, while its own fresh `resolve` runs; it keeps them only if that `resolve` names the same repository
    // and record, and reads again otherwise. Serial, the resolve was a REST merge's third stage at 500 agents, p50 452
    // and p99 861 ms (`merge-p99`).
    let guess = match (&at, merge_of) {
        (graftyard_wire::RepoPath::Name { owner, name }, Some(of)) => {
            crate::resolve_cached(owner, name).map(|g| (g, of))
        }
        _ => None,
    };
    let ahead = async {
        let (g, of) = guess.as_ref()?;
        let rows = timed(c, "tier_ahead", tier_rows(c, g, Some(*of)));
        let authz = timed(c, "authz_ahead", crate::authz_read_as(c, g.owner, &g.record, false));
        Some(futures_util::future::join(rows, authz).await)
    };
    let (who, found, ahead) =
        futures_util::future::join3(timed(c, "auth", authenticate(c, req)), timed(c, "resolve", find), ahead).await;
    let who = who?;
    let found = found.transpose()?;
    // A credential the edge refuses (revoked, lapsed, unknown) is 401, as on GitHub, not an anonymous caller whom the
    // read gate would answer 404 on a private repository.
    if who.is_none() && crate::token_of(req)?.is_some() {
        return Ok(Reply::error(401, "Bad credentials", "rest"));
    }
    let ahead = kept_ahead(&found, guess.as_ref().map(|(g, _)| g), ahead)
        .map(|(rows, authz)| (guess.as_ref().expect("kept only with a guess").1, rows, authz));
    let agent_accept = req.headers().get("accept")?;
    // A `GET` that `graftyard_api::narrow::rest_get` serves reads only the rows its reply renders.
    if method == Method::Get
        && graftyard_api::narrow::rest_serves(tail)
        && let Some(r) =
            rest_narrow(c, &at, found.clone(), tail, p, query, who.as_ref(), agent_accept.as_deref(), host).await?
    {
        return Ok(r);
    }
    // A merge reads only the rows it plans from (`TierRows::merge_rows`), as `mergePullRequest` does: read whole,
    // each one parsed a 1 MB dump at journey 9, and REST merges ran 1.2 to 4.4 s at 500 agents (LM-1 round 7).
    // A write whose plan reads no row reads the read gate's rows alone (`command-rows`).
    let command = rest_command(&method, tail);
    let rows = || match merge_of {
        Some(of) => Rows::Merge(of, None),
        None if command => Rows::Command(CommandSpec { loaded: true, ..CommandSpec::default() }),
        None => Rows::All,
    };
    let rd = match (at, found) {
        (graftyard_wire::RepoPath::Name { owner, name }, Some(Ok(Some(repo)))) if ahead.is_some() => {
            let (of, v, authz) = ahead.expect("checked");
            read_repo(c, repo, owner, name, who.as_ref(), authz, Rows::Merge(of, Some(v)), Gate::Readers).await?
        }
        (graftyard_wire::RepoPath::Name { owner, name }, Some(found)) => {
            read_found(c, found, owner, name, who.as_ref(), rows()).await?
        }
        (graftyard_wire::RepoPath::Name { .. }, None) => unreachable!("a name is resolved beside the token"),
        (graftyard_wire::RepoPath::Id(id), _) => match repo_name(c, id).await {
            Some((owner, name, repo, a)) => {
                read_repo(c, repo, &owner, &name, who.as_ref(), Some(a), rows(), Gate::Readers).await?
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
    if let Some(r) = crate::secrets::repo_route(c, req, &rd, who.as_ref(), tail).await? {
        return Ok(r);
    }
    if let Some(r) = crate::activity::repo_route(c, req, &rd, who.as_ref(), tail, query, host).await? {
        return Ok(r);
    }
    // The trust view (AM1-WP5): a reply changes only under `AGENT_MEDIA_TYPE`, and only then does the edge read the
    // install's `AgentApp` rows for `is_agent`.
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
        (Method::Get, ["actions", "jobs", job, "logs"]) => crate::actions::job_log(c, &rd, job).await?,
        (Method::Get, ["actions", "runs", _] | ["actions", "runs", _, "jobs"] | ["actions", "jobs", _]) => {
            crate::actions::rest_read(c, &rd, &r, &tail[1..]).await?
        }
        // LF-4's trust resources over REST (F61): each read renders the pinned GHES 3.22 shape from the fold, and a
        // commit from the git cache shard.
        // A writer's or an App's poll asks the shard for the test merge its `mergeable` lacks, as GraphQL's single-PR
        // read does (`ask_merges`); anyone else's is counted as a list's. `mergeable` stays null until the merge is recorded.
        (Method::Get, ["pulls", n]) => {
            let (mut reply, mut asks) = rest::get_pull_asks(&r, n);
            if crate::ui::read_compares(c, &rd).await {
                (reply, asks) = rest::get_pull_asks(&r, n);
            }
            ask_merges(c, &rd.repo, asks, merge_computes(&r, who.as_ref()));
            reply
        }
        (Method::Get, ["issues", n, "comments"]) => rest::list_issue_comments(&r, n, query),
        (Method::Get, ["pulls", n, "reviews"]) => rest::list_reviews(&r, n, query),
        (Method::Get, ["pulls", n, "comments"]) => rest::list_review_comments(&r, n, query),
        (Method::Get, ["commits", reference, "check-runs"]) => rest::list_check_runs_for_ref(&r, reference, query),
        (Method::Get, ["check-runs", id]) => rest::get_check_run(&r, id),
        // The web app's chrome reads (`decisions/OPEN-QUESTIONS.md`, F61).
        (Method::Get, ["branches"]) => rest::list_branches(&r, query),
        (Method::Get, ["tags"]) => match peeled_tags(c, &rd, &r).await {
            Ok(peeled) => rest::list_tags(&r, query, &peeled),
            Err(e) => unavailable(&format!("the git cache: {e}")),
        },
        (Method::Get, ["labels"]) => rest::list_labels(&r, query),
        (Method::Get, ["commits", reference, "check-suites"]) => match rest::resolve(&r, reference) {
            Some(sha) => match commit_object(c, &rd.repo, sha).await {
                Ok(Some(raw)) => rest::list_check_suites_for_ref(&r, sha, &raw, query),
                Ok(None) => rest::commit_not_found(reference),
                Err(e) => unavailable(&format!("the git cache: {e}")),
            },
            None => rest::commit_not_found(reference),
        },
        (Method::Get, [on @ .., "reactions"]) => rest::list_reactions(&r, on, query),
        (Method::Get, ["commits", reference]) => match rest::resolve(&r, reference) {
            Some(sha) => match commit_object(c, &rd.repo, sha).await {
                Ok(Some(raw)) => rest::get_commit(&r, sha, &raw),
                Ok(None) => rest::commit_not_found(reference),
                Err(e) => unavailable(&format!("the git cache: {e}")),
            },
            None => rest::commit_not_found(reference),
        },
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
        (Method::Post, ["pulls", n, "comments"]) => {
            let who = match need(&who) {
                Ok(w) => w,
                Err(x) => return Ok(x),
            };
            match rest::plan_create_review_comment(&r, n, &body(req.bytes().await?)) {
                Ok(op) => {
                    commit_issue(c, &rd, &who, to_wire(op), What::ReviewCommentCreated { n: n.parse().unwrap_or(0) })
                        .await?
                }
                Err(x) => x,
            }
        }
        (Method::Post, ["check-runs"]) => {
            let who = match need(&who) {
                Ok(w) => w,
                Err(x) => return Ok(x),
            };
            let Actor::App { app, .. } = who.actor else {
                return Ok(Reply::error(
                    403,
                    "You must authenticate via a GitHub App.",
                    "rest/checks/runs#create-a-check-run",
                ));
            };
            // `decide` gives the run its id from the repository's `IdBlock` cursor (format 32.18).
            let now = Timestamp(js_sys::Date::now() as u64);
            match rest::plan_create_check_run(&body(req.bytes().await?), app, now) {
                Ok(op) => commit_issue(c, &rd, &who, to_wire(op), What::CheckRunCreated { id: 0 }).await?,
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
        (Method::Post, ["actions", "runs", run, "cancel"]) => {
            let who = match need(&who) {
                Ok(w) => w,
                Err(x) => return Ok(x),
            };
            crate::actions::cancel_run(c, &rd, &who, run).await?
        }
        // `rest:branches` (BF-56's rule): admin only, as on GHES; the leader checks again in `SetRule`'s `decide`.
        (method @ (Method::Get | Method::Put | Method::Delete), ["branches", branch @ .., "protection"])
            if !branch.is_empty() =>
        {
            let who = match need(&who) {
                Ok(w) => w,
                Err(x) => return Ok(x),
            };
            let access = graftyard_git::issue_ops::Access { repo: rd.repo.id, owner: rd.repo.owner, core: &rd.core };
            let folds: Vec<&dyn graftyard_identity::AuthzRead> =
                rd.dir.authz.iter().map(|a| a as &dyn graftyard_identity::AuthzRead).collect();
            if graftyard_identity::permission(&who, &folds, &access) != Some(graftyard_identity::RepoPermission::Admin)
            {
                return Ok(Reply::error(404, "Not Found", graftyard_api::protection::DOC));
            }
            let branch = branch.join("/");
            let what = What::Protection { branch: branch.clone() };
            let plan = match method {
                Method::Get => Err(graftyard_api::protection::get(&r, &branch)),
                Method::Put => graftyard_api::protection::plan_put(&r, &branch, &body(req.bytes().await?)),
                _ => graftyard_api::protection::plan_delete(&r, &branch),
            };
            match plan {
                Ok(op) => commit_issue(c, &rd, &who, to_wire(op), what).await?,
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

/// A repository `GET` that `graftyard_api::narrow::rest_get` serves, rendered from a narrow read: only the rows its
/// reply reads, and the member's selections. `None` when the render asked for a selection the member did not make
/// (a member and an edge of different releases): the caller then reads both whole tiers, as every route did before.
#[expect(clippy::too_many_arguments, reason = "the request's parts rest_repo already holds")]
async fn rest_narrow(
    c: &Ctx<'_>,
    at: &graftyard_wire::RepoPath<'_>,
    found: Option<std::result::Result<Option<Repo>, String>>,
    tail: &[&str],
    p: &str,
    query: &str,
    who: Option<&Principal>,
    accept: Option<&str>,
    host: &Host,
) -> Result<Option<Reply>> {
    let agent = graftyard_api::trust::opted_in(accept);
    let route = graftyard_api::narrow::Route::Rest {
        tail: tail.iter().map(|s| (*s).to_owned()).collect(),
        query: query.to_owned(),
        path: p.to_owned(),
        agent,
    };
    let rd = match at {
        graftyard_wire::RepoPath::Name { owner, name } => match found {
            Some(found) => read_found(c, found, owner, name, who, Rows::Api(route)).await?,
            None => read_as(c, owner, name, who, Rows::Api(route)).await?,
        },
        graftyard_wire::RepoPath::Id(id) => match repo_name(c, *id).await {
            Some((owner, name, repo, a)) => {
                read_repo(c, repo, &owner, &name, who, Some(a), Rows::Api(route), Gate::Readers).await?
            }
            None => return Ok(Some(Reply::not_found())),
        },
    };
    let mut rd = match rd {
        Ok(r) => r,
        Err(r) => return Ok(Some(r)),
    };
    let access = graftyard_git::issue_ops::Access { repo: rd.repo.id, owner: rd.repo.owner, core: &rd.core };
    if let Some(r) = crate::usertok::reaches(c, &access) {
        return Ok(Some(r));
    }
    if agent {
        rd.dir.install = crate::install_authz(c).await;
    }
    let r = rd.view(host);
    let rendered = js_sys::Date::now();
    let reply = match tail {
        ["commits", reference] => match rest::resolve(&r, reference) {
            Some(sha) => match commit_object(c, &rd.repo, sha).await {
                Ok(Some(raw)) => rest::get_commit(&r, sha, &raw),
                Ok(None) => rest::commit_not_found(reference),
                Err(e) => unavailable(&format!("the git cache: {e}")),
            },
            None => rest::commit_not_found(reference),
        },
        _ => match graftyard_api::narrow::rest_get(&r, tail, query, graftyard_types::Index(rd.index)) {
            // A pull request's counts are the git cache's (`crate::ui::read_compares`): read the pairs the render
            // asked for, then render again.
            Some(reply) if crate::ui::read_compares(c, &rd).await => {
                graftyard_api::narrow::rest_get(&r, tail, query, graftyard_types::Index(rd.index)).unwrap_or(reply)
            }
            Some(reply) => reply,
            None => return Ok(None),
        },
    };
    let reply = graftyard_api::trust::layer(accept, p, &r, reply);
    c.timing.add("render", rendered);
    Ok((!rd.dir.picks.missed()).then_some(reply))
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
    match crate::read_gate(c, &repo, Some(&who), None).await? {
        Ok(graftyard_identity::Decision::Allow) => {}
        Ok(_) => return Ok(Reply::not_found()),
        Err(e) => return Ok(read_failed("the repository's access rows did not answer; retry", &e)),
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

/// Commit `sha`'s raw object, fetched alone from the network's git cache shard over protocol v2
/// (`graftyard_git::commit_fetch`); `None` when the shard does not hold it.
async fn commit_object(
    c: &Ctx<'_>,
    repo: &Repo,
    sha: graftyard_git::refs::ObjectId,
) -> std::result::Result<Option<Vec<u8>>, String> {
    use graftyard_git::commit_fetch;
    let err = |e: worker::Error| e.to_string();
    let body = commit_fetch::request(sha);
    let url = format!("https://shard/upload-pack?repo={}", repo.id.0.0);
    let mut r = crate::shard_read(c, repo, &url, Method::Post, "version=2", Some(&body)).await.map_err(err)?;
    if r.status_code() != 200 {
        return Err(format!("{} {}", r.status_code(), r.text().await.unwrap_or_default()));
    }
    let bytes = r.bytes().await.map_err(err)?;
    match commit_fetch::commit_of(&bytes, sha) {
        Ok(raw) => Ok(Some(raw)),
        // upload-pack refuses a want it does not hold ("not our ref").
        Err(e) if e.contains("not our ref") || e.contains("no pack") => Ok(None),
        Err(e) => Err(e),
    }
}

/// The commit each annotated tag peels to, by short name, from the git cache's v2 `ls-refs` with `peel`
/// (`graftyard_git::ls_refs::peeled`). A repository with no tag asks nothing.
async fn peeled_tags(
    c: &Ctx<'_>,
    rd: &Read,
    r: &View<'_>,
) -> std::result::Result<std::collections::BTreeMap<String, graftyard_git::refs::ObjectId>, String> {
    use graftyard_git::ls_refs;
    if rest::refs_under(r, "refs/tags/").is_empty() {
        return Ok(Default::default());
    }
    let err = |e: worker::Error| e.to_string();
    let url = format!("https://shard/upload-pack?repo={}", rd.repo.id.0.0);
    let body = ls_refs::peel_request("refs/tags/");
    let mut r = crate::shard_read(c, &rd.repo, &url, Method::Post, "version=2", Some(&body)).await.map_err(err)?;
    if r.status_code() != 200 {
        return Err(format!("{} {}", r.status_code(), r.text().await.unwrap_or_default()));
    }
    ls_refs::peeled(&r.bytes().await.map_err(err)?, "refs/tags/")
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
    /// `write-identity`'s audit as the edge applies it: only the REST writes that commit one credential-checked
    /// command on the repository's chain, with nothing done first, take the token row from `rcache`; a merge (git
    /// cache plan first), a run's cancel (no credential in `decide`), a read, and every route `collab`, `repos`,
    /// `hooks`, `secrets`, and `activity` serve read fresh. A GraphQL mutation does, but not `mergePullRequest` or
    /// `createRepository`, alone or beside another field; a query is read-class, not decided.
    #[test]
    fn only_writes_whose_decide_binds_the_credential_take_the_cached_token() {
        use worker::Method::{Delete, Get, Patch, Post, Put};
        let rest = |m: worker::Method, path: &str| super::rest_decided(&m, &path.split('/').collect::<Vec<_>>());
        for (m, p) in [
            (Post, "labels"),
            (Post, "pulls/2/comments"),
            (Post, "check-runs"),
            (Post, "issues/1/labels"),
            (Put, "issues/1/labels"),
            (Delete, "issues/1/labels/bug"),
            (Post, "releases"),
            (Patch, "releases/4"),
            (Delete, "releases/4"),
            (Delete, "releases/assets/9"),
            (Post, "statuses/abc"),
            (Put, "branches/main/protection"),
            (Delete, "branches/rel/v1/protection"),
            (Post, "git/refs"),
            (Patch, "git/refs/heads/main"),
            (Delete, "git/refs/heads/main"),
        ] {
            assert!(rest(m.clone(), p), "{m:?} {p}");
        }
        for (m, p) in [
            (Put, "pulls/2/merge"),
            (Post, "actions/runs/5/cancel"),
            (Get, "labels"),
            (Get, "branches/main/protection"),
            (Put, "collaborators/u"),
            (Delete, "invitations/3"),
            (Patch, ""),
            (Delete, ""),
            (Post, "hooks"),
            (Put, "actions/secrets/S"),
            (Put, "subscription"),
            (Post, "issues/1/comments/extra/x"),
        ] {
            assert!(!rest(m.clone(), p), "{m:?} {p}");
        }
        let gql = |q: &str| super::graphql_decided(&super::Gql::parse(q, None, None).expect("parses"));
        assert!(gql("mutation { addComment(input: {subjectId: \"I\", body: \"x\"}) { clientMutationId } }"));
        assert!(gql("mutation { enqueuePullRequest(input: {pullRequestId: \"P\"}) { clientMutationId } }"));
        assert!(!gql("mutation { mergePullRequest(input: {pullRequestId: \"P\"}) { clientMutationId } }"));
        assert!(!gql("mutation { createRepository(input: {name: \"r\"}) { clientMutationId } }"));
        assert!(!gql("mutation { addComment(input: {subjectId: \"I\", body: \"x\"}) { clientMutationId } \
             mergePullRequest(input: {pullRequestId: \"P\"}) { clientMutationId } }"));
        assert!(!gql("query { viewer { login } }"));
    }

    /// The whole-tier read asks for the packed answer, so the leader builds one string, not a `Value` tree of 18 times
    /// its size; a merge's narrow read asks for it too, which a member answers unpacked.
    #[test]
    fn the_tier_read_asks_for_packed_rows() {
        let q = super::tier_rows_query(None);
        assert_eq!(q, serde_json::json!({ "q": { "rows": true, "ranges": true, "packed": true } }));
        use graftyard_api::hosted::MergeOf;
        let q = super::tier_rows_query(Some(MergeOf::Id(graftyard_types::DbId(7))));
        assert_eq!(q["q"]["rows"], serde_json::json!({ "merge": 7 }));
        // REST's merge names the pull request by number, which the member reads apart from an id.
        let q = super::tier_rows_query(Some(MergeOf::Number(graftyard_git::issues::IssueNumber(3))));
        assert_eq!(q["q"]["rows"], serde_json::json!({ "mergeN": 3 }));
        assert_eq!(MergeOf::of_query(&q["q"]["rows"]), Some(MergeOf::Number(graftyard_git::issues::IssueNumber(3))));
    }

    /// A REST merge keeps the rows and `Authz` it read ahead for its cached guess only when its fresh `resolve` names
    /// the same repository under the same placement record: a moved placement, another repository under the name, a
    /// failed or empty resolve, or no guess reads again (`merge-p99`).
    #[test]
    fn a_write_keeps_its_ahead_reads_only_for_the_repository_it_resolved() {
        use graftyard_types::{AccountId, DbId, PlacementName, PlacementRecord, RepoId, SiteId, Timestamp};
        let record = |home: u8, generation: u32| PlacementRecord {
            name: PlacementName::Tri,
            sites: [0, 1, 2].iter().map(|&s| SiteId(s)).collect(),
            home: SiteId(home),
            generation,
            moving_from: None,
            at: Timestamp(1),
        };
        let repo = |id: i64, rec| crate::Repo { id: RepoId(DbId(id)), owner: AccountId(DbId(9)), record: rec };
        let g = repo(7, record(0, 1));
        let ahead = || Some(("rows", Ok::<_, ()>(Some("authz"))));
        let found = |r: crate::Repo| Some(Ok(Some(r)));
        assert_eq!(super::kept_ahead(&found(g.clone()), Some(&g), ahead()), Some(("rows", Some("authz"))));
        let failed = Some(("rows", Err::<Option<&str>, _>(())));
        assert_eq!(super::kept_ahead(&found(g.clone()), Some(&g), failed), Some(("rows", None)), "Authz read again");
        for (why, f) in [
            ("another repository under the name", found(repo(8, record(0, 1)))),
            ("the placement moved", found(repo(7, record(1, 2)))),
            ("no such repository now", Some(Ok(None))),
            ("the resolve failed", Some(Err("x".into()))),
        ] {
            assert_eq!(super::kept_ahead(&f, Some(&g), ahead()), None, "{why}");
        }
        assert_eq!(super::kept_ahead(&found(g.clone()), None, ahead()), None, "no guess");
        assert_eq!(super::kept_ahead::<&str, &str, ()>(&found(g.clone()), Some(&g), None), None, "nothing read ahead");
    }

    /// Only `PUT pulls/{n}/merge` is a REST merge, whose `Authz` and rows are read on leaders: a `GET` of the same
    /// path, another pull request route, or a number that does not parse reads as any other request (`merge-p99`).
    #[test]
    fn only_a_rest_merge_reads_on_leaders() {
        use graftyard_api::hosted::MergeOf;
        use worker::Method;
        let n3 = Some(MergeOf::Number(graftyard_git::issues::IssueNumber(3)));
        assert_eq!(super::rest_merge(&Method::Put, &["pulls", "3", "merge"]), n3);
        for (m, tail) in [
            (Method::Get, &["pulls", "3", "merge"][..]),
            (Method::Post, &["pulls", "3", "merge"][..]),
            (Method::Put, &["pulls", "x", "merge"][..]),
            (Method::Put, &["pulls", "3"][..]),
            (Method::Put, &["issues", "3", "merge"][..]),
            (Method::Put, &["pulls", "3", "merge", "x"][..]),
        ] {
            assert_eq!(super::rest_merge(&m, tail), None, "{m:?} {tail:?}");
        }
    }

    /// A read every member shed is 503 with `Retry-After: 1`; any other failed read is 503 with `Retry-After: 5`;
    /// neither is a 5xx without one, which a client would not retry on its own.
    #[test]
    fn a_failed_read_is_503_with_retry_after() {
        let shed = format!("repo:9: every member asked shed the read: {}", graftyard_lookup::OVERLOADED);
        let r = super::read_failed("rows", &shed);
        assert_eq!((r.status, r.header("retry-after")), (503, Some("1")));
        // tier-off-leader's shed is a shed: 503 with `Retry-After: 1`, its reason in the body.
        let tier = format!(
            "repo:9: every member declined a read the leader never serves: {} ({})",
            graftyard_lookup::OVERLOADED,
            graftyard_lookup::TIER_OFF
        );
        let r = super::read_failed("rows", &tier);
        assert_eq!((r.status, r.header("retry-after")), (503, Some("1")));
        // ...and only it is logged apart, `{"shed": "tierOff"}`, which a row counts against its 503s.
        let line: serde_json::Value = serde_json::from_str(&super::tier_off_line("rows", &tier).unwrap()).unwrap();
        assert_eq!(line["shed"], "tierOff");
        assert_eq!(super::tier_off_line("rows", &shed), None, "a members' shed is not tierOff");
        let r = super::read_failed("rows", "repo:9: repo:9#1#f0: no answer");
        assert_eq!((r.status, r.header("retry-after")), (503, Some("5")));
    }

    /// `command-rows`: a task or lease write reads its one task and the read gate's rows, never the whole tiers, which
    /// main read for every one of them; a task GET and the list still read the tiers here. A create asks for its
    /// requester's login, a PATCH and a cancel name their task, and every task write needs the metadata tier loaded.
    #[test]
    fn task_and_lease_writes_read_their_command_rows() {
        use graftyard_api::tasks::Route;
        use graftyard_git::issues::AppId;
        use graftyard_types::{AccountId, DbId};
        use worker::Method::{Get, Patch, Post};
        let app = Some(AppId(DbId(3)));
        let tasks = Route::Tasks { owner: "o", repo: "r" };
        let task = Route::Task { owner: "o", repo: "r", id: "a" };
        let cancel = Route::Cancel { owner: "o", repo: "r", id: "a" };
        assert_eq!(super::task_command(&tasks, &Get, app, None), None);
        assert_eq!(super::task_command(&task, &Get, app, None), None);
        let created = super::task_command(&tasks, &Post, app, Some(AccountId(DbId(101)))).expect("a command read");
        assert_eq!((created.session, created.loaded, created.accounts), (None, true, vec![101]));
        let s = graftyard_api::tasks::path_id("a", app);
        assert!(s.is_some());
        for r in [&task, &cancel] {
            let spec = super::task_command(r, if matches!(r, Route::Task { .. }) { &Patch } else { &Post }, app, None);
            let spec = spec.expect("a command read");
            assert_eq!((spec.session.clone(), spec.loaded, spec.accounts.is_empty()), (s.clone(), true, true));
        }
        // The member answers the spec as JSON; a lease command's default asks for no session and no logins, and is
        // served while the metadata tier loads.
        let q = serde_json::json!({ "rows": { "access": true, "command": super::CommandSpec::default() } });
        assert_eq!(q["rows"]["command"], serde_json::json!({ "owner": 0 }));
    }

    /// Only a check run, a commit status, and a label create read command rows among REST writes: each plans from no
    /// row. An issue's labels, a review comment, a release, a merge, and every read keep their reads.
    #[test]
    fn only_row_free_rest_writes_read_command_rows() {
        use worker::Method::{Delete, Get, Patch, Post, Put};
        let rest = |m: worker::Method, p: &str| super::rest_command(&m, &p.split('/').collect::<Vec<_>>());
        for p in ["check-runs", "statuses/abc", "labels"] {
            assert!(rest(Post, p), "{p}");
        }
        for (m, p) in [
            (Get, "labels"),
            (Get, "check-runs"),
            (Post, "issues/1/labels"),
            (Put, "issues/1/labels"),
            (Delete, "issues/1/labels/bug"),
            (Post, "pulls/2/comments"),
            (Post, "releases"),
            (Patch, "releases/4"),
            (Put, "pulls/2/merge"),
            (Post, "check-runs/5"),
            (Patch, "check-runs/5"),
            (Post, "statuses"),
        ] {
            assert!(!rest(m.clone(), p), "{m:?} {p}");
        }
    }

    /// `leader-render`: the first send of an issue command leaves out only the render's owner `Authz`; the command,
    /// its request id, and the rest of the render cross as before. An answer that names no `Authz` source, from a
    /// member built before, is asked again; one that names either is not, and neither is a refusal or a merge.
    #[test]
    fn the_first_send_leaves_out_only_the_owner_authz() {
        use serde_json::json;
        let body = json!({ "rid": "01", "admit": { "class": 1 },
            "cmd": { "issue": { "op": { "Close": 1 }, "render": { "host": "h", "dir": { "home": 1, "authz": "abcd",
                "logins": { "5": "e" } }, "what": "Deleted" } } } });
        let first = super::without_authz(&body);
        assert_eq!(first["cmd"]["issue"]["render"]["dir"]["authz"], serde_json::Value::Null);
        let mut back = first.clone();
        back["cmd"]["issue"]["render"]["dir"]["authz"] = json!("abcd");
        assert_eq!(back, body, "nothing else changed");
        assert!(first.to_string().len() < body.to_string().len());
        let done = |issue: serde_json::Value| json!({ "ok": { "out": { "issue": issue } } });
        assert!(graftyard_api::hosted::rendered_without_authz(&done(json!({ "reply": {} }))));
        assert!(!graftyard_api::hosted::rendered_without_authz(&done(json!({ "reply": {}, "authz": "dep" }))));
        assert!(!graftyard_api::hosted::rendered_without_authz(&done(json!({ "reply": {}, "authz": "render" }))));
        assert!(!graftyard_api::hosted::rendered_without_authz(&json!({ "err": { "issue": {} } })));
        assert!(!graftyard_api::hosted::rendered_without_authz(&json!({ "ok": { "out": { "merge": {} } } })));
    }

    /// The authz-source counter's classes: a member's named source, a member that names none (the fallback the edge
    /// asks again), and nothing for a refusal or a merge, so those are not counted.
    #[test]
    fn a_committed_issue_reply_names_its_authz_source() {
        use serde_json::json;
        let done = |issue: serde_json::Value| json!({ "ok": { "out": { "issue": issue } } });
        assert_eq!(super::authz_source(&done(json!({ "reply": {}, "authz": "dep" }))), Some("dep"));
        assert_eq!(super::authz_source(&done(json!({ "reply": {}, "authz": "render" }))), Some("render"));
        assert_eq!(super::authz_source(&done(json!({ "reply": {} }))), Some("fallback"));
        assert_eq!(super::authz_source(&done(json!({ "reply": {}, "authz": "other" }))), None);
        assert_eq!(super::authz_source(&json!({ "refused": { "issue": {} } })), None);
        assert_eq!(super::authz_source(&json!({ "ok": { "out": { "merge": {} } } })), None);
    }
}
