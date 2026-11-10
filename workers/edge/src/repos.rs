//! `rest:repos`' lifecycle (`design/SHARED.md`, the `rest:repos` row; branch `m3-repos`): what
//! `TestAccGithubRepositoryCollaborator`'s `github_repository` and `gh repo create` send.
//!
//! - `POST /orgs/{org}/repos` and `POST /user/repos`, and gh's GraphQL `createRepository` (`create`);
//! - `GET`, `PATCH`, and `DELETE /repos/{o}/{r}` (`repo_route`, from `api::rest_repo`).
//!
//! A create is three commits, in this order:
//! 1. `CreateRepo` on the owner's `Authz` chain under the caller's credential: the commit point. Its `decide` takes the
//!    repository id from the group's id block, refuses a taken or held name (`Taken`, answered 422), and refuses a
//!    caller who does not administer the owner (403). An org member who is not an admin is refused, as GHES refuses
//!    one when the org lets only owners create repositories [inference: the install has no `members_can_create`
//!    policy yet].
//! 2. `RepoOwner` on the repository's `Names` shard, with no account and no key, as the transfer's finisher writes
//!    it, so the new chain's members can check the owner they join under (`Source::Repo`).
//! 3. With `auto_init`, the README commit as an ordinary `Push` of `main` (`crate::push_init`); then `InitRepo`
//!    (repository format 32.13) with the body's settings, which records `created_at`. Its leader renders the 201.
//!
//! Once `FormatsEnable` enabled Authz format 0x12.11 and repository format 32.17 (F59), step 1 is `CreateRepoWith`, whose entry also records the
//! settings asked for (`AuthzEvent::RepoCreate`), so an edge that dies after it leaves the rest to the `Audit` consumer
//! on the owner's chain: `RepoOwner`, then the chain's first settings and `Created` (repository format 32.17), or,
//! when those cannot commit within `graftyard_identity::finish::CREATE_ABORT_AFTER`, the abort that releases the name.
//! The finisher does not redo the README commit, so a create it completed has none. Before 0x12.11 a crash between 1
//! and 2 leaves a name whose group cannot be built, as F59 found. A re-sent create answers 422 for the name it already
//! took [unverified: GHES's answer to that re-send].
//!
//! A delete is `DeleteRepo { key, retain_until }` on the owner's `Authz` chain first, the commit point (F59): it turns
//! the name `Deleted`, so it stops resolving (404), and holds it until `retain_until`, 90 days, as SHARED's retention
//! table says. Then `DeleteRepo { retain_until }` on the repository chain, the `Deleted` tombstone, which refuses every
//! later write; an edge that dies in between leaves the tombstone to the `Audit` consumer on the owner's chain. The
//! order was the other way before F59, which left a name resolving to a deleted chain, and a delete by a repository
//! admin who does not administer the owner tombstoned the chain before the name refused it. A rename in `PATCH` is
//! `RenameRepo` on the owner's chain, which leaves the old name an `Alias`.
//!
//! With `test-faults`, `x-graft-kill` stops a create after `name`, `repo_owner`, or `readme`, and a delete after
//! `delete_name`, answering 503, as an edge that died there would leave it.
//!
//! Sent only once `FormatsEnable` (the test edge's `FORMATS_ENABLED`) enabled 32.13: until a site day put every member
//! on a release that reads it, no member could apply `InitRepo` or `DeleteRepo`.

use graftyard_api::hosted::What;
use graftyard_api::repos::{DOC_CREATE, DOC_CREATE_USER, DOC_DELETE, DOC_UPDATE, apply_body, defaults, name_of, taken};
use graftyard_git::issue_ops::IssueOp;
use graftyard_git::repo::REPO_FORMAT_REPOS;
use graftyard_identity::{AccountKind, Actor, NameKey, Principal, RepoName, name_shard};
use graftyard_types::{AccountId, ChainId, DbId, RepoId, Timestamp};
use graftyard_wire::Reply;
use serde_json::{Value, json};
use worker::*;

use crate::Ctx;
use crate::api::{Read, commit_issue, unavailable};
use crate::ident::{account_of, commit_owner, commit_system, refused};

/// How long a deleted repository is kept (`design/SHARED.md`, "Repository repair, tombstones, and retention").
const RETAIN_MS: u64 = 90 * 86_400_000;

fn enabled(c: &Ctx<'_>) -> std::result::Result<(), Reply> {
    if c.cfg.enabled.contains(&REPO_FORMAT_REPOS) {
        Ok(())
    } else {
        Err(unavailable(&format!(
            "repository create and delete need record format {}.{}, which FormatsEnable has not enabled",
            REPO_FORMAT_REPOS.family, REPO_FORMAT_REPOS.version
        )))
    }
}

/// Whether the test-faults `x-graft-kill` header stops this request after `step` (F59's hosted proof).
fn killed(c: &Ctx<'_>, step: &str) -> bool {
    #[cfg(feature = "test-faults")]
    return c.kill.as_deref() == Some(step);
    #[cfg(not(feature = "test-faults"))]
    {
        let _ = (c, step);
        false
    }
}

fn killed_reply(step: &str) -> Reply {
    unavailable(&format!("x-graft-kill: stopped after {step}"))
}

/// The caller as a user, or GitHub's 401 or 403.
fn user(who: Option<Principal>, doc: &str) -> std::result::Result<(Principal, AccountId), Reply> {
    let who = who.ok_or_else(|| Reply::error(401, "Requires authentication", doc))?;
    match who.actor {
        Actor::User(u) => Ok((who.clone(), u.0)),
        _ => Err(Reply::error(403, "Resource not accessible by integration", doc)),
    }
}

/// Where a create came from: its owner, and how the reply is rendered.
pub enum Via {
    /// `POST /orgs/{org}/repos`.
    Org(String),
    /// `POST /user/repos`.
    User,
    /// gh's `createRepository(input:)`, whose `ownerId` is a node id, or the viewer when it has none.
    Graphql { owner: Option<AccountId>, query: String, variables: Value },
}

/// `POST /orgs/{org}/repos`, `POST /user/repos`, or gh's `createRepository`.
pub async fn create(c: &Ctx<'_>, req: &mut Request, via: Via, body: Value) -> Result<Reply> {
    let doc = if matches!(via, Via::Org(_)) { DOC_CREATE } else { DOC_CREATE_USER };
    if let Err(r) = enabled(c) {
        return Ok(r);
    }
    let (who, me) = match user(crate::authenticate(c, req).await?, doc) {
        Ok(x) => x,
        Err(r) => return Ok(r),
    };
    let name = match name_of(&body, doc) {
        Ok(n) => n,
        Err(r) => return Ok(r),
    };
    // The owner, its login, and whether it is an org.
    let (owner, login, org) = match &via {
        Via::Org(o) => match account_of(c, o).await.map_err(Error::RustError)? {
            Some((a, AccountKind::Org)) => (a, crate::ident::login(c, a).await.unwrap_or_else(|| o.clone()), true),
            _ => return Ok(Reply::not_found()),
        },
        Via::User => (me, crate::ident::login(c, me).await.unwrap_or_default(), false),
        Via::Graphql { owner, .. } => {
            let a = owner.unwrap_or(me);
            let Some(l) = crate::ident::login(c, a).await else { return Ok(Reply::not_found()) };
            let org = crate::ident::authz_of(c, a)
                .await
                .map_err(Error::RustError)?
                .is_some_and(|f| graftyard_identity::AuthzRead::opened(&f) == Some(AccountKind::Org));
            (a, l, org)
        }
    };
    let settings = match apply_body(&defaults(owner, None), &body, org, doc) {
        Ok(s) => s,
        Err(r) => return Ok(r),
    };
    // 1. The name, the commit point; with 0x12.11 and 32.17, with the settings the `Audit` consumer finishes from.
    let audited = [graftyard_identity::AUTHZ_FORMAT_V11, graftyard_git::lifecycle::REPO_FORMAT_LIFECYCLE];
    let op = if audited.iter().all(|f| c.cfg.enabled.contains(f)) {
        let init = serde_json::to_string(&settings)?;
        json!({ "CreateRepoWith": { "name": name.as_str(), "init": init } })
    } else {
        json!({ "CreateRepo": { "name": name.as_str() } })
    };
    let cmd = json!({ "cred": who.credential_shard(), "op": op });
    let done = match commit_owner(c, ChainId::Authz(owner), owner, cmd).await {
        Ok(d) => d,
        Err(x) => return Ok(x),
    };
    match refused(&done).as_deref() {
        None => {}
        Some("Taken") => return Ok(taken(doc)),
        Some("Forbidden") => {
            return Ok(Reply::error(403, "You do not have permission to create repositories on this account", doc));
        }
        Some("Unauthorized") => return Ok(Reply::error(401, "Bad credentials", "rest")),
        // `CreateRepoWith`'s settings over `CREATE_INIT_MAX` (F59): a description of several KB.
        Some("BadRecord") => {
            let e = graftyard_wire::FieldError { resource: "Repository", field: "description", code: "invalid" };
            return Ok(Reply::json(422, graftyard_wire::validation_error("Validation Failed", doc, &[e])));
        }
        Some(e) => return Ok(unavailable(&format!("the repository name did not commit: {e}"))),
    }
    let Some(id) = done.pointer("/ok/out/Repo/repo").and_then(Value::as_i64) else {
        return Ok(unavailable(&format!("CreateRepo answered {done}")));
    };
    let repo = RepoId(DbId(id));
    if killed(c, "name") {
        return Ok(killed_reply("name"));
    }
    // 2. The owner row the new chain's members check.
    let row =
        json!({ "cred": { "account": null, "key": null }, "op": { "RepoOwner": { "repo": repo, "owner": owner } } });
    match commit_system(c, ChainId::Names(name_shard(&NameKey::Repo(repo))), row).await {
        Ok(d) if refused(&d).is_none() => {}
        Ok(d) => return Ok(unavailable(&format!("RepoOwner was refused: {d}"))),
        Err(x) => return Ok(x),
    }
    if killed(c, "repo_owner") {
        return Ok(killed_reply("repo_owner"));
    }
    let record = match crate::placement(c, owner).await {
        Ok(Some(r)) => r,
        Ok(None) => return Ok(Reply::not_found()),
        Err(e) => return Ok(unavailable(&e)),
    };
    let r = crate::Repo { id: repo, owner, record };
    // 3. The README commit, then the settings, whose leader renders the reply.
    if body.get("auto_init").and_then(Value::as_bool) == Some(true) {
        let sig = graftyard_git::init::Signature {
            name: &login,
            email: &format!("{me}+{login}@users.noreply.{}", c.cfg.host, me = me.0.0),
            at: crate::now(),
        };
        let init = graftyard_git::init::readme_commit(name.as_str(), settings.description.as_deref(), &sig);
        match crate::push_init(c, &r, &who, &init).await {
            Ok(true) => {}
            Ok(false) => return Ok(unavailable("the initial commit was refused")),
            Err(e) => return Ok(unavailable(&format!("the initial commit: {e}"))),
        }
        if killed(c, "readme") {
            return Ok(killed_reply("readme"));
        }
    }
    let rd = match crate::api::read_new(c, r, &login, name.as_str(), Some(&who)).await? {
        Ok(rd) => rd,
        Err(x) => return Ok(x),
    };
    let what = match via {
        Via::Graphql { query, variables, .. } => What::RepoCreatedGraphql { query, variables },
        _ => What::RepoSaved { created: true },
    };
    // The enterprise the owner's `Authz` names, which `InitRepo` must name too.
    let mut s = settings;
    s.enterprise = rd.dir.authz.as_ref().and_then(graftyard_identity::AuthzRead::enterprise);
    commit_issue(c, &rd, &who, IssueOp::InitRepo(s), what).await
}

/// `GET`, `PATCH`, and `DELETE /repos/{o}/{r}`: `None` for any other path.
pub async fn repo_route(
    c: &Ctx<'_>,
    req: &mut Request,
    rd: &Read,
    who: Option<&Principal>,
    tail: &[&str],
    index: u64,
    host: &graftyard_wire::Host,
) -> Result<Option<Reply>> {
    if !tail.is_empty() {
        return Ok(None);
    }
    // `rd` came through `api::read`'s read gate (`authorize_read`), so a caller who cannot read was answered 404, or
    // 401 or 503, before this route.
    let method = req.method();
    let view = rd.view(host);
    Ok(Some(match method {
        Method::Get => graftyard_api::repos::get(&view, graftyard_types::Index(index)),
        Method::Patch => {
            if let Err(r) = enabled(c) {
                return Ok(Some(r));
            }
            let (who, _) = match user(who.cloned(), DOC_UPDATE) {
                Ok(x) => x,
                Err(r) => return Ok(Some(r)),
            };
            let body: Value = req.json().await.unwrap_or(Value::Null);
            update(c, rd, &who, body).await?
        }
        Method::Delete => {
            if let Err(r) = enabled(c) {
                return Ok(Some(r));
            }
            let (who, _) = match user(who.cloned(), DOC_DELETE) {
                Ok(x) => x,
                Err(r) => return Ok(Some(r)),
            };
            delete(c, rd, &who).await?
        }
        _ => Reply::not_found(),
    }))
}

/// `PATCH /repos/{o}/{r}`: the body over the current settings as one `Settings`, after a `RenameRepo` on the owner's
/// chain when `name` changes. A body that changes nothing still commits, as the reply comes only from `&Acked`.
async fn update(c: &Ctx<'_>, rd: &Read, who: &Principal, body: Value) -> Result<Reply> {
    let org = rd.dir.authz.as_ref().and_then(graftyard_identity::AuthzRead::opened) == Some(AccountKind::Org);
    let enterprise = rd.dir.authz.as_ref().and_then(graftyard_identity::AuthzRead::enterprise);
    let cur =
        graftyard_git::tiers::get_rec(&rd.core, "settings", &[]).unwrap_or_else(|| defaults(rd.repo.owner, enterprise));
    let s = match apply_body(&cur, &body, org, DOC_UPDATE) {
        Ok(s) => s,
        Err(r) => return Ok(r),
    };
    let mut at = rd.at.clone();
    if let Some(n) = body.get("name").and_then(Value::as_str).filter(|n| *n != rd.at.name) {
        let Ok(to) = RepoName::parse(n) else {
            let e = graftyard_wire::FieldError { resource: "Repository", field: "name", code: "invalid" };
            return Ok(Reply::json(422, graftyard_wire::validation_error("Validation Failed", DOC_UPDATE, &[e])));
        };
        let Ok(from) = RepoName::parse(&rd.at.name) else { return Ok(Reply::not_found()) };
        let cmd = json!({ "cred": who.credential_shard(),
            "op": { "RenameRepo": { "from": from.key().as_str(), "to": to.as_str() } } });
        let done = match commit_owner(c, ChainId::Authz(rd.repo.owner), rd.repo.owner, cmd).await {
            Ok(d) => d,
            Err(x) => return Ok(x),
        };
        match refused(&done).as_deref() {
            None => at.name = to.as_str().to_owned(),
            Some("Taken") => return Ok(taken(DOC_UPDATE)),
            Some("Forbidden") => return Ok(Reply::error(403, "Must have admin rights to Repository.", DOC_UPDATE)),
            Some(e) => return Ok(unavailable(&format!("the rename did not commit: {e}"))),
        }
    }
    let mut rd2 = rd.clone_at(at);
    rd2.dir.home = Some(rd2.at.clone());
    commit_issue(c, &rd2, who, IssueOp::Settings(s), What::RepoSaved { created: false }).await
}

/// `DELETE /repos/{o}/{r}`: the name's `Deleted`, the commit point, then the chain's tombstone. 204 once both
/// committed.
async fn delete(c: &Ctx<'_>, rd: &Read, who: &Principal) -> Result<Reply> {
    let Ok(name) = RepoName::parse(&rd.at.name) else { return Ok(Reply::not_found()) };
    let owner = rd.repo.owner;
    let mut retain_until = Timestamp(crate::now().0 + RETAIN_MS);
    let cmd = json!({ "cred": who.credential_shard(),
        "op": { "DeleteRepo": { "key": name.key().as_str(), "retain_until": retain_until } } });
    let done = match commit_owner(c, ChainId::Authz(owner), owner, cmd).await {
        Ok(d) => d,
        Err(x) => return Ok(x),
    };
    match refused(&done).as_deref() {
        None => {}
        // `NoRepo`: a delete racing this one turned the name `Deleted` first; the tombstone takes that name's
        // `retain_until`, the one the finisher checks.
        Some("NoRepo") => {
            let authz = crate::ident::authz_of(c, owner).await.map_err(Error::RustError)?;
            match authz.and_then(|a| a.deleted(rd.repo.id)) {
                Some(t) => retain_until = t,
                None => return Ok(Reply::not_found()),
            }
        }
        Some("Forbidden") => return Ok(Reply::error(403, "Must have admin rights to Repository.", DOC_DELETE)),
        Some(e) => return Ok(unavailable(&format!("the name's delete did not commit: {e}"))),
    }
    if killed(c, "delete_name") {
        return Ok(killed_reply("delete_name"));
    }
    let r = commit_issue(c, rd, who, IssueOp::DeleteRepo { retain_until }, What::Deleted).await?;
    Ok(if r.status == 403 { Reply::error(403, "Must have admin rights to Repository.", DOC_DELETE) } else { r })
}
