//! The agent settings endpoints (AM1-WP2; `design/AGENT-LAYER.md`, "API reference"): the install's `AgentApp` rows,
//! an org's agent settings, and a repository's `AgentPolicy`, under `/api/graftyard/v1`. Each write commits one
//! command on its chain under the caller's credential, and the leader's `decide` checks who may write it. Served only
//! once `FormatsEnable` enabled the command's format (Authz 0x12.3, Repo 32.8); until then 503. The task API
//! (AM1-WP3) is routed here too, and served by `crate::api::tasks` once 32.12 is enabled. The lease API and the activity
//! feed (AM1-WP4) are served here (`leases`) once 32.14 is enabled.

use graftyard_api::agents::{self, Route};
use graftyard_identity::{AuthzCmd, AuthzFold, AuthzOp, AuthzRead, Principal};
use graftyard_types::ChainId;
use graftyard_wire::{Body, Reply};
use serde_json::{Value, json};
use worker::*;

use crate::Ctx;

pub(crate) fn respond(c: &Ctx<'_>, r: Reply) -> Result<Response> {
    let h = crate::headers(c.rid)?;
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
    Reply::error(503, why, "graftyard/agents").with("retry-after", "5")
}

fn off(f: graftyard_raft::Format) -> Reply {
    unavailable(&format!(
        "agent settings need record format {}.{}, which FormatsEnable has not enabled",
        f.family, f.version
    ))
}

pub async fn serve(c: &Ctx<'_>, req: &mut Request, rest: &str) -> Result<Response> {
    let Some(route) = agents::route(rest) else { return respond(c, Reply::not_found()) };
    let who = crate::authenticate(c, req).await?;
    let method = req.method();
    let body = match method {
        Method::Put | Method::Patch | Method::Post => {
            serde_json::from_slice::<Value>(&req.bytes().await?).unwrap_or(Value::Null)
        }
        _ => Value::Null,
    };
    let r = match route {
        Route::Policy { owner, repo } => crate::api::agent_policy(c, req, owner, repo, who, method, &body).await?,
        Route::Tasks(t) => {
            let query = req.url()?.query().unwrap_or("").to_owned();
            crate::api::tasks(c, t, who, method, &body, &query).await?
        }
        Route::Leases(l) => {
            let query = req.url()?.query().unwrap_or("").to_owned();
            let session = req.headers().get("x-graftyard-agent-session")?;
            let ip = req.headers().get("cf-connecting-ip")?.unwrap_or_default();
            leases(c, l, who, method, &body, &query, session.as_deref(), &ip).await?
        }
        _ => {
            let Some(who) = who else { return respond(c, Reply::error(401, "Requires authentication", "rest")) };
            authz_route(c, route, &who, method, &body).await?
        }
    };
    respond(c, r)
}

/// `/agents/apps[/{id}]` on the install's chain, and `/orgs/{org}/agents` on the org's.
async fn authz_route(c: &Ctx<'_>, route: Route<'_>, who: &Principal, method: Method, body: &Value) -> Result<Reply> {
    if !c.cfg.enabled.contains(&graftyard_identity::AUTHZ_FORMAT_V3) {
        return Ok(off(graftyard_identity::AUTHZ_FORMAT_V3));
    }
    let install = graftyard_identity::install_account();
    let (owner, org) = match route {
        Route::Org(login) => match crate::account_of(c, login).await {
            Ok(Some(a)) => (a, Some(login)),
            Ok(None) => return Ok(Reply::not_found()),
            Err(e) => return Ok(unavailable(&e)),
        },
        _ => (install, None),
    };
    let Some(record) = crate::placement(c, owner).await.map_err(Error::RustError)? else {
        return Ok(Reply::not_found());
    };
    let chain = ChainId::Authz(owner);
    let read = |f: Option<AuthzFold>| f.ok_or_else(|| unavailable("the owner's Authz chain did not answer; retry"));
    let fold = || async { crate::authz_fold(c, owner, &record).await };
    // Reads need the same right as writes: a site admin for the install's rows, an org admin for the org's.
    let admin = |f: &AuthzFold| match who.actor {
        graftyard_identity::Actor::User(u) => f.is_admin(u),
        _ => false,
    };
    let op = match (&route, method) {
        (Route::Apps, Method::Get) | (Route::Org(_), Method::Get) => {
            let f = match read(fold().await) {
                Ok(f) => f,
                Err(r) => return Ok(r),
            };
            if !admin(&f) {
                return Ok(agents::refused(403, ""));
            }
            return Ok(match org {
                Some(o) if f.opened() == Some(graftyard_identity::AccountKind::Org) => {
                    Reply::json(200, agents::org_json(o, &f.policy()))
                }
                Some(_) => Reply::not_found(),
                None => Reply::json(200, agents::apps_json(&f)),
            });
        }
        (Route::App(app), Method::Put) => match agents::parse_app(*app, body) {
            Ok(a) => AuthzOp::PutAgentApp(a),
            Err(r) => return Ok(r),
        },
        (Route::App(app), Method::Delete) => AuthzOp::DeleteAgentApp { app: *app },
        (Route::Org(_), Method::Patch) => match agents::parse_org(body) {
            Ok((enabled, workspace)) => AuthzOp::SetOrgAgents { enabled, workspace },
            Err(r) => return Ok(r),
        },
        _ => return Ok(Reply::error(405, "Method Not Allowed", "graftyard/agents")),
    };
    let put = match &op {
        AuthzOp::PutAgentApp(a) => Some(a.clone()),
        _ => None,
    };
    let cmd = AuthzCmd { cred: who.credential_shard(), op };
    let body = json!({ "rid": c.rid.to_string(), "cmd": cmd }).to_string();
    let r = graftyard_lookup::commit_hedged(
        &c.bindings(),
        chain,
        &record,
        body.as_bytes(),
        c.colo,
        60_000,
        graftyard_lookup::Hooks::default(),
    )
    .await;
    let done = match r {
        Ok(d) => d,
        Err(Ok(e)) => return Ok(unavailable(&format!("the owner's members refused the call: {e}"))),
        Err(Err(e)) => return Ok(unavailable(&e)),
    };
    if let Some(r) = done.get("refused") {
        let status = r["status"].as_u64().and_then(|s| u16::try_from(s).ok()).unwrap_or(422);
        return Ok(agents::refused(status, r["err"].as_str().unwrap_or("")));
    }
    if done.get("ok").is_none() {
        return Ok(unavailable(&format!("no outcome: {done}")));
    }
    // The reply is what the acknowledged command wrote, read back from the leader after its entry applied.
    let f = match read(fold().await) {
        Ok(f) => f,
        Err(r) => return Ok(r),
    };
    Ok(match (route, put) {
        (Route::App(_), Some(a)) => Reply::json(200, agents::app_json(&f, &a)),
        (Route::App(_), None) => Reply::empty(204),
        (Route::Org(o), _) => Reply::json(200, agents::org_json(o, &f.policy())),
        _ => Reply::empty(204),
    })
}

/// FEED_POLLS' keys: the token's credential shard and the client IP, each hashed so neither reaches a member.
fn poll_keys(c: &Ctx<'_>, who: &Principal, ip: &str) -> (String, String) {
    use sha2::Digest as _;
    let h = |s: &str| {
        let d = sha2::Sha256::digest(format!("{}:{s}", c.cfg.host).as_bytes());
        d.iter().take(12).map(|b| format!("{b:02x}")).collect::<String>()
    };
    (h(&format!("{:?}", who.credential_shard())), h(ip))
}

/// The lease API and the activity feed under `/api/graftyard/v1/repos/{o}/{r}` (AM1-WP4, `graftyard_api::leases`),
/// served only once `FormatsEnable` enabled 32.14; until then 503. Lease reads render the repository's core tier and
/// the owner's `Authz` from one read, so an expired or released lease, and one whose holder lost access, is absent
/// with no sweep; writes commit an `IssueCall` whose leader renders the reply from the `&Acked`. The feed's query is
/// checked here (422 for a bad `wait` or glob or over 32 `paths`), and a poll then goes to the group's learners and
/// followers (`activity`), never its leader.
#[allow(clippy::too_many_arguments)]
async fn leases(
    c: &Ctx<'_>,
    route: graftyard_api::leases::Route<'_>,
    who: Option<Principal>,
    method: Method,
    body: &Value,
    query: &str,
    session: Option<&str>,
    ip: &str,
) -> Result<Reply> {
    use graftyard_api::leases::{self as l, Route as R};
    use graftyard_git::issue_ops::IssueOp;
    if !c.cfg.enabled.contains(&graftyard_git::repo::REPO_FORMAT_LEASES) {
        return Ok(unavailable(
            "leases and the activity feed need record format 32.14, which FormatsEnable has not enabled",
        ));
    }
    let Some(who) = who else { return Ok(Reply::error(401, "Requires authentication", "rest")) };
    let (R::Leases { owner, repo }
    | R::Lease { owner, repo, .. }
    | R::Renew { owner, repo, .. }
    | R::Activity { owner, repo }) = route;
    if let R::Activity { .. } = route {
        if method != Method::Get {
            return Ok(Reply::error(405, "Method Not Allowed", "graftyard/activity"));
        }
        let q = match l::parse_feed(query) {
            Ok(q) => q,
            Err(r) => return Ok(r),
        };
        let rd = match crate::api::read_named(c, owner, repo, Some(&who)).await? {
            Ok(r) => r,
            Err(r) => return Ok(r),
        };
        let Some(authz) = rd.dir.authz.clone() else {
            return Ok(unavailable("the owner's Authz chain did not answer; retry"));
        };
        let access = graftyard_git::issue_ops::Access { repo: rd.repo.id, owner: rd.repo.owner, core: &rd.core };
        let reader = graftyard_api::tasks::Reader::of(Some(&who), &authz, &access);
        if !reader.metadata {
            return Ok(Reply::not_found());
        }
        let Ok(host) = graftyard_wire::Host::new(&c.cfg.host, false) else {
            return Ok(unavailable("the install's host is not valid"));
        };
        let view = rd.view(&host);
        let (token, ip) = poll_keys(c, &who, ip);
        return activity(c, &rd, &view, &q, reader.contents, &token, &ip).await;
    }
    let rd = match crate::api::read_named(c, owner, repo, Some(&who)).await? {
        Ok(r) => r,
        Err(r) => return Ok(r),
    };
    let Some(authz) = rd.dir.authz.clone() else {
        return Ok(unavailable("the owner's Authz chain did not answer; retry"));
    };
    let access = graftyard_git::issue_ops::Access { repo: rd.repo.id, owner: rd.repo.owner, core: &rd.core };
    let reader = graftyard_api::tasks::Reader::of(Some(&who), &authz, &access);
    let Ok(host) = graftyard_wire::Host::new(&c.cfg.host, false) else {
        return Ok(unavailable("the install's host is not valid"));
    };
    let view = rd.view(&host);
    let (op, created) = match (route, method) {
        (R::Leases { .. }, Method::Get) => {
            let path = format!("/repos/{owner}/{repo}/leases");
            return Ok(l::list(&view, &authz, crate::now(), query, reader.contents, &path));
        }
        (R::Leases { .. }, Method::Post) => match l::parse_acquire(body, session) {
            Ok(n) => (IssueOp::AcquireLease(n), true),
            Err(r) => return Ok(r),
        },
        (R::Renew { id, .. }, Method::Post) => match l::parse_renew(id, body) {
            Ok(op) => (op, false),
            Err(r) => return Ok(r),
        },
        (R::Lease { id, .. }, Method::Delete) => match graftyard_git::lease::LeaseId::parse(id) {
            Some(id) => (IssueOp::ReleaseLease { id }, false),
            None => return Ok(Reply::not_found()),
        },
        _ => return Ok(Reply::error(405, "Method Not Allowed", "graftyard/leases")),
    };
    crate::api::commit_issue(c, &rd, &who, op, graftyard_api::hosted::What::Lease { created }).await
}

// ---- agent pushes and REST ref writes (AM1's hosted half) ----

/// Whether a push or REST ref write by `who` may be an agent's, so the edge asks the git cache for `validate`'s detail
/// (`design/DESIGN.md`, Shape, `ProtectedHits`: "for each ref update by an `Agent` or under an agent prefix"): an
/// installation token whose App has an `AgentApp` row, or any App token once 32.8 is enabled (a classing trailer makes
/// it Agent for admission), or an update under a `confine` prefix or `refs/heads/graftyard/`. A user token's push
/// outside every agent prefix is never asked, so a default push's `validate` call and bytes are unchanged.
pub(crate) fn agentish(
    c: &Ctx<'_>,
    who: &Principal,
    install: Option<&AuthzFold>,
    updates: &[graftyard_git::refs::RefUpdate],
) -> bool {
    if !c.cfg.enabled.contains(&graftyard_git::repo::REPO_FORMAT_AGENT) {
        return false;
    }
    if matches!(who.actor, graftyard_identity::Actor::App { .. }) {
        return true;
    }
    let prefixes: Vec<&graftyard_identity::glob::RefPattern> = install
        .map(|a| a.more.more.agent_apps.values().filter_map(|r| r.confine.as_ref()).collect())
        .unwrap_or_default();
    updates.iter().any(|u| {
        u.name.starts_with(graftyard_git::task::RUN_PREFIX) || prefixes.iter().any(|p| p.matches(u.name.as_bytes()))
    })
}

/// The `validate` outputs of an agent push or REST ref write, linked: `graftyard_git::agent::validated_of` over the
/// git cache's detail with the repository's `AgentPolicy.extra_paths`, then `agent::link` against the pusher's App's
/// sessions on this repository and its token's claim, under `https://<host>/<owner>/<repo>`. `decide` keeps a
/// `Session` only when its own fresh read finds that task (`agent::unlinked`), so this read only saves a round.
pub(crate) fn linked(
    who: &Principal,
    core: &dyn graftyard_fold::Read,
    detail: &[graftyard_git::agent::RefDetail],
    origin: &str,
) -> graftyard_git::agent::Validated {
    let policy = graftyard_git::tiers::agent_policy(core);
    let mut v = graftyard_git::agent::validated_of(detail, &policy.extra_paths);
    let pusher = match who.actor {
        graftyard_identity::Actor::App { app, .. } => Some(app),
        _ => None,
    };
    let mut own = pusher.map(|a| graftyard_git::task::app_sessions(core, a)).unwrap_or_default();
    own.extend(graftyard_git::task::claim(who).cloned());
    v.provenance = graftyard_git::agent::link(&v.provenance, pusher, &own, Some(origin));
    v
}

/// What an agent push needs read before its commit: the install's `AgentApp` rows and the repository's core rows (its
/// `AgentPolicy` and tasks). `None` for a push `agentish` rules out, which reads nothing more.
pub(crate) struct AgentPush {
    install: Option<AuthzFold>,
    core: graftyard_fold::MemStore,
}

pub(crate) async fn push_context(
    c: &Ctx<'_>,
    repo: &crate::Repo,
    push: &graftyard_git::repo::Push,
) -> Option<AgentPush> {
    if !c.cfg.enabled.contains(&graftyard_git::repo::REPO_FORMAT_AGENT) {
        return None;
    }
    let install = crate::install_authz(c).await;
    if !agentish(c, &push.actor, install.as_ref(), &push.updates) {
        return None;
    }
    let rows = match crate::api::tier_rows(c, repo, None).await {
        Ok(Ok(v)) => serde_json::from_value::<graftyard_api::hosted::TierRows>(v["value"].clone()).ok(),
        _ => None,
    };
    let core = rows.map(|r| graftyard_api::hosted::TierRows::store(&r.core)).unwrap_or_default();
    Some(AgentPush { install, core })
}

/// Fills `push` with `validate`'s linked outputs (`linked`). `changed` and `protected` are kept only for an update by
/// an `ActorKind::Agent` or under an agent prefix (the design's "for each ref update by an `Agent` or under an agent
/// prefix"); a plain App's push keeps only its provenance, so one with no recognized trailer keeps its format and
/// bytes. `leases` is set once 32.14 is enabled, so a push with `changed` may write `PathSet` and be refused `Leased`.
pub(crate) fn fill_push(
    c: &Ctx<'_>,
    a: &AgentPush,
    push: &mut graftyard_git::repo::Push,
    detail: &[graftyard_git::agent::RefDetail],
    path: &str,
) {
    let origin = format!("https://{}/{path}", c.cfg.host);
    let v = linked(&push.actor, &a.core, detail, &origin);
    let install = a.install.as_ref().map(|f| f as &dyn AuthzRead);
    let agent =
        matches!(graftyard_git::agent::actor_kind(&push.actor, install), graftyard_git::agent::ActorKind::Agent(_));
    let prefixes: Vec<&graftyard_identity::glob::RefPattern> = a
        .install
        .as_ref()
        .map(|f| f.more.more.agent_apps.values().filter_map(|r| r.confine.as_ref()).collect())
        .unwrap_or_default();
    let under = |r: &graftyard_git::refs::RefName| {
        agent || r.starts_with(graftyard_git::task::RUN_PREFIX) || prefixes.iter().any(|p| p.matches(r.as_bytes()))
    };
    push.provenance = v.provenance;
    push.changed = v.changed.into_iter().filter(|x| under(&x.refname)).collect();
    push.protected = v.protected.into_iter().filter(|x| under(&x.refname)).collect();
    push.leases = c.cfg.enabled.contains(&graftyard_git::repo::REPO_FORMAT_LEASES);
}

/// `POST /git/refs` and `PATCH /git/refs/{ref}` as a one-update `Push` (`design/DESIGN.md`, Shape, `ChangedPaths`:
/// "REST ref writes ... by an `Agent` or under an agent prefix run `validate` on an empty staged pack, so they get all
/// three outputs"). Every write asks the git cache, on an empty pack, that the new tip exists and, without `force`,
/// that a `PATCH` is a fast-forward; an agent's also carries the linked outputs. Replies are GHES's `git-ref`.
pub(crate) async fn write_ref(
    c: &Ctx<'_>,
    rd: &crate::api::Read,
    r: &graftyard_api::view::Repo<'_>,
    who: &Principal,
    u: graftyard_git::refs::RefUpdate,
    force: bool,
) -> Result<Reply> {
    use graftyard_git::group::{CommitReq, PushOutcome};
    let doc = "rest/git/refs";
    let mut push = graftyard_git::repo::Push {
        updates: vec![u.clone()],
        atomicity: graftyard_git::refs::Atomicity::PerRef,
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
    let agent = push_context(c, &rd.repo, &push).await;
    let lines = graftyard_git::agent::detail_lines(&push.updates, true);
    let detail = match crate::validate(c, &rd.repo, &[], &[], &lines).await {
        Ok((_, d)) => d,
        Err(_) => return Ok(Reply::error(422, "Object does not exist", doc)),
    };
    if u.expect.is_some() && !force && !detail.first().is_some_and(|d| d.ff) {
        return Ok(Reply::error(422, "Update is not a fast forward", doc));
    }
    if let Some(a) = &agent {
        fill_push(c, a, &mut push, &detail, &format!("{}/{}", r.at.owner, r.at.name));
    }
    let f = graftyard_raft::Command::format(&push);
    if !c.cfg.enabled.contains(&f) && f != graftyard_git::repo::REPO_FORMAT_V1 {
        return Ok(unavailable(&format!(
            "this write needs record format {}.{}, which FormatsEnable has not enabled",
            f.family, f.version
        )));
    }
    let admit = crate::admission(c, who, &push.provenance, true, 0).await;
    let mut body = serde_json::to_value(CommitReq { rid: c.rid.to_string(), cmd: push })?;
    if let Some(a) = admit {
        body["admit"] = serde_json::to_value(a)?;
    }
    let created = u.expect.is_none();
    let new = u.new.expect("a write names a new tip");
    Ok(match crate::commit(c, &rd.repo, &body, None, None).await? {
        Ok(PushOutcome::Ok(ok)) => match ok.out.refs.first() {
            Some(graftyard_git::repo::RefOutcome::Ok) => {
                Reply::json(if created { 201 } else { 200 }, graftyard_api::rest::ref_json(r, &u.name, &new))
            }
            Some(graftyard_git::repo::RefOutcome::Rejected(graftyard_git::repo::RejectReason::Agent(w))) => {
                Reply::error(422, &w.text, doc)
            }
            _ => Reply::error(422, "Reference cannot be updated", doc),
        },
        Ok(PushOutcome::Refused(x)) if x.status == 403 => {
            Reply::error(403, "Resource not accessible by integration", doc)
        }
        Ok(PushOutcome::Refused(x)) => Reply::error(422, &x.err, doc),
        Ok(PushOutcome::Full(f)) => Reply::secondary_rate_limit(f.retry_after_s),
        Ok(PushOutcome::NotLeader(_)) | Err(_) => unavailable("the repository's group has no leader; retry"),
    })
}

// ---- the activity feed's routing (AM1's hosted half) ----

/// A group's feed members as an edge isolate caches them: when read, and each (site, object, learner).
type FeedMembers = (u64, Vec<(graftyard_types::SiteId, String, bool)>);

thread_local! {
    /// Per isolate: each group's members (learners first, then voters), from its committed config.
    static FEED_MEMBERS: std::cell::RefCell<std::collections::BTreeMap<i64, FeedMembers>> =
        const { std::cell::RefCell::new(std::collections::BTreeMap::new()) };
}

/// The group's members a poll may go to: learners first, then the voters but the one the status names as leader (the
/// member route also refuses a leader or witness). Read from a voter's committed config at most once a minute per
/// isolate.
async fn feed_members(c: &Ctx<'_>, repo: &crate::Repo) -> Vec<(graftyard_types::SiteId, String, bool)> {
    let now = crate::now().0;
    if let Some((at, m)) = FEED_MEMBERS.with(|f| f.borrow().get(&repo.id.0.0).cloned())
        && at + 60_000 > now
    {
        return m;
    }
    let chain = ChainId::Repo(repo.id);
    // Every voter's `/status` at once: the committed config (every member carries it) names the learners, and a
    // voter whose status says it leads is left out, so the leader sees no feed request (L0-15). A leader elected
    // since answers `notServing`, which drops this cache (`activity`).
    let status = |site: graftyard_types::SiteId, name: String| async move {
        let stub = c.bindings().member(site, &name).ok()?;
        let mut r = stub.fetch_with_str("https://member/status").await.ok()?;
        r.json::<Value>().await.ok()
    };
    let voters = repo.record.bootstrap_members(chain);
    let seen = futures_util::future::join_all(voters.iter().map(|m| status(m.site, m.do_name.to_string()))).await;
    let leading: Vec<graftyard_types::SiteId> = voters
        .iter()
        .zip(&seen)
        .filter(|(_, v)| v.as_ref().is_some_and(|v| v["role"] == "leader"))
        .map(|(m, _)| m.site)
        .collect();
    let mut out = Vec::new();
    if let Some(cfg) = seen.iter().flatten().find_map(|v| v.pointer("/config/config").cloned())
        && let Ok(cfg) = serde_json::from_value::<graftyard_raft::GroupConfig>(cfg)
    {
        let p = match &cfg {
            graftyard_raft::GroupConfig::Normal(p) | graftyard_raft::GroupConfig::Degraded { placement: p, .. } => p,
            graftyard_raft::GroupConfig::Joint { new, .. } => new,
        };
        out.extend(p.learners().iter().map(|l| (l.site, l.do_name.to_string(), true)));
        out.extend(
            p.voters().iter().filter(|l| !leading.contains(&l.site)).map(|l| (l.site, l.do_name.to_string(), false)),
        );
    }
    FEED_MEMBERS.with(|f| f.borrow_mut().insert(repo.id.0.0, (now, out.clone())));
    out
}

/// `GET /activity` (`design/AGENT-LAYER.md`, "Activity feed"): sent to the group's learners first, then followers
/// with `learner_answers` set when a learner answered; a member that refuses as lagging sends the poll on, up to three
/// tried. If every one tried refused as lagging and the least lagged is a learner, that learner holds it; otherwise,
/// or at once from a full member, 429 with `Retry-After` of FEED_BATCH_ALL (`graftyard_git::feed::route`). Never the
/// leader: the member route refuses it before reading anything.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn activity(
    c: &Ctx<'_>,
    rd: &crate::api::Read,
    view: &graftyard_api::view::Repo<'_>,
    q: &graftyard_git::feed::FeedQuery,
    contents: bool,
    token: &str,
    ip: &str,
) -> Result<Reply> {
    use graftyard_git::feed::{Admit, FeedRole};
    if !contents {
        return Ok(Reply::not_found());
    }
    let members = feed_members(c, &rd.repo).await;
    let body = |learner_answers: bool, wait: u64| {
        json!({ "cursor": q.cursor, "wait_s": wait, "paths": q.paths, "learner_answers": learner_answers,
                "token": token, "ip": ip })
        .to_string()
    };
    let mut answers: Vec<(Admit, FeedRole, std::time::Duration, usize)> = Vec::new();
    let mut learner_answered = false;
    for (i, (site, name, learner)) in members.iter().enumerate() {
        if answers.len() >= 3 {
            break;
        }
        let deadline = std::time::Duration::from_secs(q.wait.as_secs() + 5);
        let b = body(learner_answered && !learner, q.wait.as_secs());
        let Some((200, v)) =
            graftyard_lookup::post_within(&c.bindings(), *site, name, "/feed", b.as_bytes(), deadline).await
        else {
            continue;
        };
        let role = if *learner { FeedRole::Learner } else { FeedRole::Follower };
        match v["admit"].as_str() {
            Some("hold") => return Ok(render_feed(view, &v)),
            Some("gone") => {
                let Some(o) = v["oldest"].as_str().and_then(graftyard_git::feed::FeedCursor::decode) else { continue };
                return Ok(graftyard_api::leases::gone(o));
            }
            Some("tooMany") => return Ok(graftyard_api::leases::too_many()),
            Some("full") => answers.push((Admit::Full, role, std::time::Duration::ZERO, i)),
            Some("lagging") => {
                // A lagging learner does not answer for the feed: followers may still serve.
                let lag = std::time::Duration::from_millis(v["lagMs"].as_u64().unwrap_or(u64::MAX / 2));
                answers.push((Admit::Lagging, role, lag, i));
            }
            Some("notServing") if *learner => learner_answered = true,
            // A voter that leads now: the cached members are stale.
            Some("notServing") => {
                FEED_MEMBERS.with(|f| f.borrow_mut().remove(&rd.repo.id.0.0));
            }
            _ => {}
        }
    }
    let routed: Vec<(Admit, FeedRole, std::time::Duration)> = answers.iter().map(|(a, r, l, _)| (*a, *r, *l)).collect();
    match graftyard_git::feed::route(&routed) {
        graftyard_git::feed::Route::Held(k) => {
            // Every tried member lagged and the least lagged is a learner: it holds the poll under FEED_HELD_MAX.
            let (site, name, _) = &members[answers[k].3];
            let b = json!({ "cursor": q.cursor, "wait_s": q.wait.as_secs(), "paths": q.paths, "learner_answers": false,
                            "token": token, "ip": ip, "lagging_ok": true });
            let deadline = std::time::Duration::from_secs(q.wait.as_secs() + 5);
            match graftyard_lookup::post_within(&c.bindings(), *site, name, "/feed", b.to_string().as_bytes(), deadline)
                .await
            {
                Some((200, v)) if v["admit"] == "hold" => Ok(render_feed(view, &v)),
                _ => Ok(graftyard_api::leases::too_many()),
            }
        }
        graftyard_git::feed::Route::TooMany { .. } => Ok(graftyard_api::leases::too_many()),
    }
}

fn render_feed(view: &graftyard_api::view::Repo<'_>, v: &Value) -> Reply {
    let events: Vec<Value> = v["events"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|e| {
                    let at = graftyard_types::Timestamp(e[1].as_u64()?);
                    let ev = graftyard_git::feed::ActivityEvent::from_json(&e[2])?;
                    Some(graftyard_api::leases::event_json(view, &ev, at, true))
                })
                .collect()
        })
        .unwrap_or_default();
    Reply::json(200, json!({ "cursor": v["cursor"], "events": events }))
}
