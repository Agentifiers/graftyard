//! REST, GraphQL, LFS, uploads, and codeload on the edge (`build/BUILD-PLAN.md`, M2-WP2 and M2-WP4): `crates/api`'s
//! handlers over a repository's tiers, read through `fresh` on its group.
//!
//! - A read is one `{"q": {"rows": true}}` read of the repository's leader: both tiers at one index, loaded into
//!   `MemStore`s and read through `graftyard_api::view::Repo`. A repository over `hosted::ROWS_MAX` rows answers 503.
//! - The `Directory` is `hosted::HostedDir`: the owner's `Authz` fold, read as a `dep` through `fresh`, the owner's
//!   login from the path, and the signed-in account from its token.
//! - A write is an `IssueCall` committed through `commit_hedged`. Its leader renders the reply from the `&Acked` over
//!   its own tiers (`hosted::render_ok`), and the edge adds only the cross-cutting headers (`graftyard_wire::finish`).
//!   The edge sends one only once `FormatsEnable` has enabled `REPO_FORMAT_ISSUES`; until then a write answers 503.
//! - Upload and LFS transfer bytes go to the placement's buckets through `BlobStore::place`, and are read back through
//!   `BlobStore::read`. LFS `verify` and the asset upload commit an `IssueCall` naming the `BlobRef`; the leader
//!   rebuilds the `DurableBlob` with `BlobStore::confirm` before it decides (BF-25).
//! - `DELETE /git/refs/{ref}` is a `Push`, M1's write path.

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
use graftyard_git::refs::Atomicity;
use graftyard_git::repo::Push;
use graftyard_git::{Copies, ObjectMeta};
use graftyard_identity::{Actor, AuthzFold, Principal};
use graftyard_types::{ChainId, NetworkId, SiteId};
use graftyard_wire::{Body, Endpoint, Host, Incoming, Meta, RateLimits, Reply, Surface};
use serde_json::{Value, json};
use worker::*;

use crate::{Ctx, Repo, authenticate, ensure_group, resolve};

/// Whether `endpoint` is one this module serves; `Repo` and `Web` are git's or nobody's.
pub fn serves(e: &Endpoint<'_>) -> bool {
    !matches!(e, Endpoint::Repo { .. } | Endpoint::Web(_) | Endpoint::OAuth(_))
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

fn unavailable(why: &str) -> Reply {
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
    let r = match route(c, req, host, e, &m).await {
        Ok(r) => r,
        Err(err) => unavailable(&format!("Service Unavailable: {err}")),
    };
    frame(&i, r, s, &m)
}

/// What a handler reads: the repository, its tiers at one index, and the directory.
struct Read {
    repo: Repo,
    at: RepoRef,
    core: MemStore,
    meta: MemStore,
    dir: HostedDir,
}

impl Read {
    fn view<'a>(&'a self, host: &'a Host) -> View<'a> {
        View { host, at: self.at.clone(), core: &self.core, meta: &self.meta, dir: &self.dir }
    }
}

/// `o/r` resolved, then its tiers and the owner's `Authz` read through `fresh`. `Ok(Err(reply))` when the repository
/// is unknown or cannot be read now.
async fn read(
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
    let chain = ChainId::Repo(repo.id);
    let b = c.bindings();
    // `ranges` too: a member built before the `rows` read answers its small ranges reply instead of every ref.
    let q = json!({ "q": { "rows": true, "ranges": true } }).to_string();
    let mut v = graftyard_lookup::call_opt(&b, chain, &repo.record, "/read", q.as_bytes()).await;
    if let Ok(None) = v {
        ensure_group(c, &repo).await.map_err(Error::RustError)?;
        v = graftyard_lookup::call_opt(&b, chain, &repo.record, "/read", q.as_bytes()).await;
    }
    let v = match v {
        Ok(Some((v, _, _))) => v,
        Ok(None) => return Ok(Err(unavailable("the repository's group has no leader; retry"))),
        Err(e) => return Ok(Err(unavailable(&e))),
    };
    // A member built before the `rows` read answered `ranges`: its tiers cannot be served.
    let Ok(rows) = serde_json::from_value::<TierRows>(v["value"].clone()) else {
        return Ok(Err(unavailable("the repository's members do not serve tier rows yet (site release)")));
    };
    if !rows.complete {
        return Ok(Err(unavailable("the repository is too large for this edge's tier read")));
    }
    let Some(meta) = rows.meta.as_ref() else {
        return Ok(Err(unavailable("the repository's metadata tier is loading; retry")));
    };
    let authz = authz(c, &repo).await;
    let at = RepoRef { id: repo.id, owner: owner.to_owned(), owner_id: repo.owner, name: name.to_owned() };
    let mut dir = HostedDir { home: Some(at.clone()), authz, viewer: None, ..HostedDir::default() };
    dir.logins.insert(repo.owner.0.0, owner.to_owned());
    dir.viewer = viewer.and_then(|p| match p.actor {
        Actor::User(u) => Some(u.0),
        Actor::Anonymous => None,
    });
    for (id, l) in logins(c) {
        dir.logins.entry(id).or_insert(l);
    }
    Ok(Ok(Read { repo, at, core: TierRows::store(&rows.core), meta: TierRows::store(meta), dir }))
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
        Endpoint::Rest(p) => rest_repo(c, req, host, p, &query).await,
        Endpoint::Graphql => graphql_post(c, req, host).await,
        Endpoint::Uploads(p) => upload(c, req, host, p, &query).await,
        Endpoint::Lfs { owner, repo, rest } => lfs_route(c, req, host, owner, repo, rest, &query, method).await,
        Endpoint::Objects(p) => transfer(c, req, host, p, method).await,
        Endpoint::Codeload { owner, repo, rest } => codeload(c, host, owner, repo, rest).await,
        Endpoint::Raw { .. } => Ok(Reply::not_found()),
        Endpoint::OAuth(_) | Endpoint::Repo { .. } | Endpoint::Web(_) => Ok(Reply::not_found()),
    }
}

// ---- writes ----

/// Commits an issue command and returns the reply its leader rendered.
async fn commit_issue(c: &Ctx<'_>, rd: &Read, who: &Principal, op: IssueOp<BlobRef>, what: What) -> Result<Reply> {
    if !c.cfg.enabled.contains(&REPO_FORMAT_ISSUES) {
        return Ok(unavailable(&format!(
            "issue writes need record format {}.{}, which FormatsEnable has not enabled on this install",
            REPO_FORMAT_ISSUES.family, REPO_FORMAT_ISSUES.version
        )));
    }
    let render = Render { host: c.cfg.host.clone(), isolation: false, dir: rd.dir.clone(), what };
    let call = IssueCall::new(op, who.clone(), c.rid, serde_json::to_value(&render)?);
    let body = json!({ "rid": c.rid.to_string(), "cmd": { "issue": call } });
    ensure_group(c, &rd.repo).await.map_err(Error::RustError)?;
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
    };
    let body = serde_json::to_value(CommitReq { rid: c.rid.to_string(), cmd: push })?;
    Ok(match crate::commit(c, &rd.repo, &body, None, None).await? {
        Ok(PushOutcome::Ok(ok)) if ok.out.refs.iter().all(|o| *o == graftyard_git::repo::RefOutcome::Ok) => {
            Reply::empty(204)
        }
        Ok(PushOutcome::Ok(_)) => Reply::error(422, "Reference cannot be updated", "rest/git/refs"),
        Ok(PushOutcome::Refused(r)) if r.status == 403 => {
            Reply::error(403, "Resource not accessible by integration", "rest/git/refs")
        }
        Ok(PushOutcome::Refused(r)) => Reply::error(r.status, &r.err, "rest/git/refs"),
        Ok(PushOutcome::NotLeader(_)) | Err(_) => unavailable("the repository's group has no leader; retry"),
    })
}

// ---- GraphQL ----

async fn graphql_post(c: &Ctx<'_>, req: &mut Request, host: &Host) -> Result<Reply> {
    let who = authenticate(c, req).await?;
    let Some(who) = who else { return Ok(Reply::json(401, json!({ "message": "Bad credentials", "status": "401" }))) };
    let body: Value = req.json().await.unwrap_or(Value::Null);
    let q = body["query"].as_str().unwrap_or("").to_owned();
    let vars = body["variables"].as_object().cloned();
    let gq = match Gql::parse(&q, vars.clone(), body["operationName"].as_str().map(str::to_owned)) {
        Ok(r) => r,
        Err(e) => return Ok(Reply::json(200, json!({ "errors": [e.json()] }))),
    };
    let target = match graphql::route(&gq) {
        Ok(t) => t,
        Err(e) => return Ok(Reply::json(200, json!({ "errors": [e.json()] }))),
    };
    let (owner, name) = match target {
        None => return Ok(Reply::json(200, graphql::query(&gq, None))),
        Some(graphql::Target::Name { owner, name }) => (owner, name),
        Some(graphql::Target::Id(id)) => match repo_name(c, id).await {
            Some(n) => n,
            None => {
                return Ok(Reply::json(
                    200,
                    json!({ "errors": [{ "type": "NOT_FOUND", "message": "Could not resolve to a node with that id." }] }),
                ));
            }
        },
    };
    let rd = match read(c, &owner, &name, Some(&who)).await? {
        Ok(r) => r,
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
    let op = match p.plan.clone() {
        graphql::Plan::Issue(op) => op.map_blob(|b| Ok::<_, ()>(b.blob())).expect("no blob"),
        graphql::Plan::Merge { .. } => {
            return Ok(Reply::json(
                200,
                json!({ "data": { "mergePullRequest": null }, "errors": [{
                "type": "UNPROCESSABLE", "path": ["mergePullRequest"],
                "message": "Merging is not served yet (M2-WP3)" }] }),
            ));
        }
    };
    let what = What::Graphql {
        query: q.clone(),
        variables: vars.map(Value::Object).unwrap_or(Value::Null),
        field: p.field.clone(),
        subject: p.subject.map(|n| n.0),
    };
    commit_issue(c, &rd, &who, op, what).await
}

/// A repository's `o/r` from its id: its owner from `RepoOwner` on `Names`, its name from the owner's `Authz` names,
/// and the owner's login from `logins`, since `Names` keys logins by name and holds no reverse row [inference].
async fn repo_name(c: &Ctx<'_>, id: i64) -> Option<(String, String)> {
    let repo = crate::repo_by_id(c, graftyard_types::RepoId(graftyard_types::DbId(id))).await.ok()?;
    let login = logins(c).get(&repo.owner.0.0)?.clone();
    let a = authz(c, &repo).await?;
    let name = a.names.iter().find_map(|(k, e)| match e {
        graftyard_identity::RepoEntry::Live { repo: r, .. } if *r == repo.id => Some(k.as_str().to_owned()),
        _ => None,
    })?;
    Some((login, name))
}

/// Logins by account id. No account-to-login read is built (`Names` keys logins by name), so a product edge knows
/// only the owner's login from the request path; test installs name more in `LOGINS` (`{"<id>": "<login>"}`).
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
    let Some(rest) = p.strip_prefix("/repos/") else { return Ok(Reply::not_found()) };
    let segs: Vec<&str> = rest.split('/').collect();
    let [owner, name, tail @ ..] = segs.as_slice() else { return Ok(Reply::not_found()) };
    let who = authenticate(c, req).await?;
    let method = req.method();
    let rd = match read(c, owner, name, who.as_ref()).await? {
        Ok(r) => r,
        Err(r) => return Ok(r),
    };
    let r = rd.view(host);
    let need = |w: &Option<Principal>| -> std::result::Result<Principal, Reply> {
        w.clone().ok_or_else(|| Reply::error(401, "Requires authentication", "rest"))
    };
    let body = |b: Vec<u8>| serde_json::from_slice::<Value>(&b).unwrap_or(Value::Null);
    let id = |s: &str| s.parse::<i64>().ok();
    // The trust view (AM1-WP5): a reply changes only under `AGENT_MEDIA_TYPE`.
    let agent_accept = req.headers().get("accept")?;
    let reply = match (method, tail) {
        (Method::Get, ["commits", reference, "statuses"]) => {
            rest::list_commit_statuses(&r, reference, graftyard_wire::Page::parse(query))
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
        Actor::Anonymous => None,
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
        (Method::Post, p) if p.starts_with("locks/") && p.ends_with("/unlock") => {
            let id = &p["locks/".len()..p.len() - "/unlock".len()];
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
    let raw = buckets(c, &repo)?;
    let refs: Vec<(SiteId, &R2)> = raw.iter().map(|(s, b)| (*s, b)).collect();
    let store = BlobStore { sites: c.sites, buckets: &refs, copies: Copies::TwoRegions };
    match method {
        Method::Put => {
            let _ = (host, &who);
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

async fn codeload(c: &Ctx<'_>, host: &Host, owner: &str, name: &str, rest: &str) -> Result<Reply> {
    let rd = match read(c, owner, name, None).await? {
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
