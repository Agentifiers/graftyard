//! Actions secrets and variables on the edge (`build/BUILD-PLAN.md`, M4-WP3, BF-59; `design/SHARED.md`, "Actions",
//! Secrets and variables): GHES 3.22's `rest:actions` secret and variable routes, as `gh secret set` and `gh variable
//! set` call them [measured: cli/cli `pkg/cmd/secret/set/http.go`, `pkg/cmd/variable/set/http.go`].
//!
//! - **The public key.** `GET .../secrets/public-key` (repository, environment, and org) answers the owner's current
//!   `SealKey` as `{key_id, key}`. An owner with none gets one: `graft-ops` mints it (`POST /actions/sealkey`, the
//!   private half wrapped under `SecretsWrapKey`), and `AuthzOp::PutSealKey` commits it on the owner's `Authz` chain.
//! - **Repository and environment rows** are `IssueOp::PutActionsSecret` and the rest (format 32.23) on the repository
//!   chain, rendered by the leader from `&Acked` (`What::ActionsConfig`: 201 for a new name, else 204). Org rows are
//!   `AuthzOp::PutOrgSecret` and the rest (0x12.12) on the org's chain, read back after the commit as `crate::agents`
//!   does. Lists and single reads answer names and times only, never a value of a secret.
//! - Admin only, as on GHES; anyone else gets 404 on a repository route and 403 on an org route. Each leader checks
//!   again in `decide`.
//!
//! Until `FormatsEnable` enabled 32.23 (and 0x12.12 for the owner's rows) every route falls through to the reply it
//! had before, so no reply changes.

use graftyard_api::hosted::What;
use graftyard_git::issue_ops::IssueOp;
use graftyard_git::secrets::{self as rows, REPO_FORMAT_SECRETS};
use graftyard_identity::sealed::{ActionsName, EnvName, SealKey, SecretScope, SecretVisibility};
use graftyard_identity::{AUTHZ_FORMAT_V12, AuthzCmd, AuthzFold, AuthzOp, AuthzRead as _, Principal};
use graftyard_types::{AccountId, ChainId, PlacementRecord, RepoId};
use graftyard_wire::Reply;
use serde_json::{Value, json};
use worker::*;

use crate::Ctx;
use crate::api::{Read, commit_issue, commit_issue_as, unavailable};

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

fn iso(t: graftyard_types::Timestamp) -> String {
    graftyard_wire::time::iso(t)
}

fn validation(msg: &str) -> Reply {
    Reply::error(422, msg, "rest/actions/secrets")
}

/// The owner's current `SealKey`, minted and committed if it has none. A concurrent first read may mint two; each
/// commit is a distinct key id, and the later reads pick the newest unretired one (`ActionsRows::current_key`).
async fn current_key(
    c: &Ctx<'_>,
    owner: AccountId,
    record: &PlacementRecord,
    known: Option<&AuthzFold>,
) -> std::result::Result<SealKey, Reply> {
    if let Some(k) = known.and_then(|f| f.actions().current_key()) {
        return Ok(k.clone());
    }
    let ops = c.env.service("OPS").map_err(|_| unavailable("no graft-ops binding"))?;
    let mut init = RequestInit::new();
    init.with_method(Method::Post).with_body(Some("{}".into()));
    let mut res =
        ops.fetch("https://graft-ops/actions/sealkey", Some(init)).await.map_err(|_| unavailable("graft-ops"))?;
    if res.status_code() != 200 {
        return Err(unavailable(&format!("graft-ops minted no seal key ({})", res.status_code())));
    }
    let key: SealKey = res.json().await.map_err(|_| unavailable("graft-ops answered no seal key"))?;
    authz_commit(c, owner, record, AuthzOp::PutSealKey { key: key.clone() }).await?;
    Ok(key)
}

/// One `AuthzOp` on the owner's chain under the operator's credential: the edge already checked the caller is an
/// admin, and a key mint names no account. Org rows go through `org_commit`, under the caller's credential.
async fn authz_commit(
    c: &Ctx<'_>,
    owner: AccountId,
    record: &PlacementRecord,
    op: AuthzOp,
) -> std::result::Result<(), Reply> {
    commit_as(c, owner, record, graftyard_types::CredentialShard::NONE, op).await
}

async fn commit_as(
    c: &Ctx<'_>,
    owner: AccountId,
    record: &PlacementRecord,
    cred: graftyard_types::CredentialShard,
    op: AuthzOp,
) -> std::result::Result<(), Reply> {
    let rid = fresh_rid();
    let body = json!({ "rid": rid.to_string(), "cmd": AuthzCmd { cred, op } }).to_string();
    let r = graftyard_lookup::commit_hedged(
        &c.bindings(),
        ChainId::Authz(owner),
        record,
        body.as_bytes(),
        c.colo,
        60_000,
        graftyard_lookup::Hooks::default(),
    )
    .await;
    let done = match r {
        Ok(d) => d,
        Err(Ok(e)) => return Err(unavailable(&format!("the owner's members refused the call: {e}"))),
        Err(Err(e)) => return Err(unavailable(&e)),
    };
    if let Some(r) = done.get("refused") {
        let err = r["err"].as_str().unwrap_or("");
        return Err(match err {
            "Taken" => Reply::error(409, "Already exists", "rest/actions/variables"),
            "NoRecord" => Reply::not_found(),
            "Forbidden" | "NotAnOrg" => Reply::not_found(),
            _ => validation("Validation Failed"),
        });
    }
    if done.get("ok").is_none() {
        return Err(unavailable(&format!("no outcome: {done}")));
    }
    Ok(())
}

fn key_json(k: &SealKey) -> Value {
    json!({ "key_id": &*k.key_id, "key": base64::Engine::encode(&B64, k.public) })
}

/// `{encrypted_value, key_id}` as `gh secret set` sends it.
fn sealed_body(b: &Value) -> std::result::Result<(Box<[u8]>, Box<str>), Reply> {
    let (Some(v), Some(k)) = (b["encrypted_value"].as_str(), b["key_id"].as_str()) else {
        return Err(validation("encrypted_value and key_id are required"));
    };
    let sealed = base64::Engine::decode(&B64, v).map_err(|_| validation("encrypted_value is not base64"))?;
    Ok((sealed.into(), k.into()))
}

/// The routes under `/repos/{o}/{r}/actions/{secrets,variables}` and `/repos/{o}/{r}/environments/{env}/...`, or
/// `None` for another tail.
pub async fn repo_route(
    c: &Ctx<'_>,
    req: &mut Request,
    rd: &Read,
    who: Option<&Principal>,
    tail: &[&str],
) -> Result<Option<Reply>> {
    let (scope, rest) = match tail {
        ["actions", kind @ ("secrets" | "variables"), rest @ ..] => (SecretScope::Repo, (*kind, rest)),
        ["environments", env, kind @ ("secrets" | "variables"), rest @ ..] => {
            (SecretScope::Env(EnvName((*env).into())), (*kind, rest))
        }
        _ => return Ok(None),
    };
    if !c.cfg.enabled.contains(&REPO_FORMAT_SECRETS) || !c.cfg.enabled.contains(&AUTHZ_FORMAT_V12) {
        return Ok(None);
    }
    let Some(who) = who.cloned() else { return Ok(Some(Reply::error(401, "Requires authentication", "rest"))) };
    let access = graftyard_git::issue_ops::Access { repo: rd.repo.id, owner: rd.repo.owner, core: &rd.core };
    let folds: Vec<&dyn graftyard_identity::AuthzRead> =
        rd.dir.authz.iter().map(|a| a as &dyn graftyard_identity::AuthzRead).collect();
    if graftyard_identity::permission(&who, &folds, &access) != Some(graftyard_identity::RepoPermission::Admin) {
        return Ok(Some(Reply::not_found()));
    }
    let method = req.method();
    let body: Value = match method {
        Method::Put | Method::Post | Method::Patch => {
            serde_json::from_slice(&req.bytes().await?).unwrap_or(Value::Null)
        }
        _ => Value::Null,
    };
    let name = |s: &str| ActionsName::parse(s);
    let r = match (method, rest) {
        (Method::Get, ("secrets", ["public-key"])) => {
            match current_key(c, rd.repo.owner, &rd.repo.record, rd.dir.authz.as_ref()).await {
                Ok(k) => Reply::json(200, key_json(&k)),
                Err(r) => r,
            }
        }
        (Method::Get, ("secrets", [])) => {
            let all: Vec<Value> = rows::secrets(&rd.core, &scope)
                .iter()
                .map(|(n, r)| json!({ "name": n.as_str(), "created_at": iso(r.updated), "updated_at": iso(r.updated) }))
                .collect();
            Reply::json(200, json!({ "total_count": all.len(), "secrets": all }))
        }
        (Method::Get, ("secrets", [n])) => match name(n).and_then(|n| Some((rows::secret(&rd.core, &scope, &n)?, n))) {
            Some((r, n)) => Reply::json(
                200,
                json!({ "name": n.as_str(), "created_at": iso(r.updated), "updated_at": iso(r.updated) }),
            ),
            None => Reply::not_found(),
        },
        (Method::Put, ("secrets", [n])) => {
            let Some(n) = name(n) else { return Ok(Some(validation("Invalid secret name"))) };
            let (sealed, key_id) = match sealed_body(&body) {
                Ok(x) => x,
                Err(r) => return Ok(Some(r)),
            };
            let op = IssueOp::PutActionsSecret { scope, name: n, sealed, key_id };
            commit_issue(c, rd, &who, op, What::ActionsConfig).await?
        }
        (Method::Delete, ("secrets", [n])) => match name(n) {
            Some(n) => {
                commit_issue(c, rd, &who, IssueOp::DeleteActionsSecret { scope, name: n }, What::ActionsConfig).await?
            }
            None => Reply::not_found(),
        },
        (Method::Get, ("variables", [])) => {
            let all: Vec<Value> =
                rows::variables(&rd.core, &scope).iter().map(|(n, r)| variable_json(n, &r.value, r.updated)).collect();
            Reply::json(200, json!({ "total_count": all.len(), "variables": all }))
        }
        (Method::Get, ("variables", [n])) => {
            match name(n).and_then(|n| Some((rows::variable(&rd.core, &scope, &n)?, n))) {
                Some((r, n)) => Reply::json(200, variable_json(&n, &r.value, r.updated)),
                None => Reply::not_found(),
            }
        }
        (Method::Post, ("variables", [])) | (Method::Patch, ("variables", [_])) => {
            let create = rest.1.is_empty();
            let given = if create { body["name"].as_str() } else { rest.1.first().copied() };
            let (Some(n), Some(v)) = (given.and_then(name), body["value"].as_str()) else {
                return Ok(Some(validation("name and value are required")));
            };
            let op = IssueOp::PutActionsVariable { scope, name: n, value: v.into(), create };
            commit_issue(c, rd, &who, op, What::ActionsConfig).await?
        }
        (Method::Delete, ("variables", [n])) => match name(n) {
            Some(n) => {
                commit_issue(c, rd, &who, IssueOp::DeleteActionsVariable { scope, name: n }, What::ActionsConfig)
                    .await?
            }
            None => Reply::not_found(),
        },
        _ => return Ok(None),
    };
    Ok(Some(r))
}

fn variable_json(n: &ActionsName, value: &str, at: graftyard_types::Timestamp) -> Value {
    json!({ "name": n.as_str(), "value": value, "created_at": iso(at), "updated_at": iso(at) })
}

fn visibility_of(b: &Value) -> std::result::Result<SecretScope, Reply> {
    let v = match b["visibility"].as_str().unwrap_or("private") {
        "all" => SecretVisibility::All,
        "private" => SecretVisibility::Private,
        "selected" => SecretVisibility::Selected,
        _ => return Err(validation("visibility must be all, private, or selected")),
    };
    let repos = b["selected_repository_ids"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_i64)
        .map(|i| RepoId(graftyard_types::DbId(i)))
        .collect();
    Ok(SecretScope::Org { visibility: v, repos })
}

fn org_scope_json(s: &SecretScope) -> (&'static str, Option<Vec<i64>>) {
    match s {
        SecretScope::Org { visibility: SecretVisibility::All, .. } => ("all", None),
        SecretScope::Org { visibility: SecretVisibility::Selected, repos } => {
            ("selected", Some(repos.iter().map(|r| r.0.0).collect()))
        }
        _ => ("private", None),
    }
}

/// `/orgs/{org}/actions/{secrets,variables}[/...]`, or `None` for another path.
pub async fn org_route(c: &Ctx<'_>, req: &mut Request, segs: &[&str]) -> Result<Option<Reply>> {
    let ["orgs", login, "actions", kind @ ("secrets" | "variables"), rest @ ..] = segs else { return Ok(None) };
    if !c.cfg.enabled.contains(&AUTHZ_FORMAT_V12) {
        return Ok(None);
    }
    let Some(who) = crate::authenticate(c, req).await? else {
        return Ok(Some(Reply::error(401, "Requires authentication", "rest")));
    };
    let owner = match crate::account_of(c, login).await {
        Ok(Some(a)) => a,
        Ok(None) => return Ok(Some(Reply::not_found())),
        Err(e) => return Ok(Some(unavailable(&e))),
    };
    let Some(record) = crate::placement(c, owner).await.map_err(Error::RustError)? else {
        return Ok(Some(Reply::not_found()));
    };
    let Some(f) = crate::authz_fold(c, owner, &record).await else {
        return Ok(Some(unavailable("the owner's Authz chain did not answer; retry")));
    };
    let admin = match who.actor {
        graftyard_identity::Actor::User(u) => f.is_admin(u),
        _ => false,
    };
    if !admin || f.opened() != Some(graftyard_identity::AccountKind::Org) {
        return Ok(Some(Reply::not_found()));
    }
    let method = req.method();
    let body: Value = match method {
        Method::Put | Method::Post | Method::Patch => {
            serde_json::from_slice(&req.bytes().await?).unwrap_or(Value::Null)
        }
        _ => Value::Null,
    };
    let a = f.actions();
    let cred = who.credential_shard();
    let done = |r: std::result::Result<(), Reply>, ok: u16| match r {
        Ok(()) if ok == 201 => Reply::json(201, json!({})),
        Ok(()) => Reply::empty(ok),
        Err(r) => r,
    };
    let r = match (method, *kind, rest) {
        (Method::Get, "secrets", ["public-key"]) => match current_key(c, owner, &record, Some(&f)).await {
            Ok(k) => Reply::json(200, key_json(&k)),
            Err(r) => r,
        },
        (Method::Get, "secrets", []) => {
            let all: Vec<Value> = a
                .secrets
                .iter()
                .map(|(n, (s, r))| {
                    let (v, _) = org_scope_json(s);
                    json!({ "name": n.as_str(), "created_at": iso(r.updated), "updated_at": iso(r.updated), "visibility": v })
                })
                .collect();
            Reply::json(200, json!({ "total_count": all.len(), "secrets": all }))
        }
        (Method::Get, "secrets", [n]) => match ActionsName::parse(n).and_then(|n| Some((a.secrets.get(&n)?, n))) {
            Some(((s, r), n)) => {
                let (v, _) = org_scope_json(s);
                Reply::json(
                    200,
                    json!({ "name": n.as_str(), "created_at": iso(r.updated), "updated_at": iso(r.updated), "visibility": v }),
                )
            }
            None => Reply::not_found(),
        },
        (Method::Put, "secrets", [n]) => {
            let Some(name) = ActionsName::parse(n) else { return Ok(Some(validation("Invalid secret name"))) };
            let (sealed, key_id) = match sealed_body(&body) {
                Ok(x) => x,
                Err(r) => return Ok(Some(r)),
            };
            let scope = match visibility_of(&body) {
                Ok(s) => s,
                Err(r) => return Ok(Some(r)),
            };
            let had = a.secrets.contains_key(&name);
            let op = AuthzOp::PutOrgSecret { name, scope, sealed, key_id };
            done(commit_as(c, owner, &record, cred, op).await, if had { 204 } else { 201 })
        }
        (Method::Delete, "secrets", [n]) => match ActionsName::parse(n) {
            Some(name) => done(commit_as(c, owner, &record, cred, AuthzOp::DeleteOrgSecret { name }).await, 204),
            None => Reply::not_found(),
        },
        (Method::Get, "variables", []) => {
            let all: Vec<Value> = a
                .variables
                .iter()
                .map(|(n, (s, r))| {
                    let mut v = variable_json(n, &r.value, r.updated);
                    v["visibility"] = json!(org_scope_json(s).0);
                    v
                })
                .collect();
            Reply::json(200, json!({ "total_count": all.len(), "variables": all }))
        }
        (Method::Get, "variables", [n]) => match ActionsName::parse(n).and_then(|n| Some((a.variables.get(&n)?, n))) {
            Some(((s, r), n)) => {
                let mut v = variable_json(&n, &r.value, r.updated);
                v["visibility"] = json!(org_scope_json(s).0);
                Reply::json(200, v)
            }
            None => Reply::not_found(),
        },
        (Method::Post, "variables", []) | (Method::Patch, "variables", [_]) => {
            let create = rest.is_empty();
            let given = if create { body["name"].as_str() } else { rest.first().copied() };
            let (Some(name), Some(v)) = (given.and_then(ActionsName::parse), body["value"].as_str()) else {
                return Ok(Some(validation("name and value are required")));
            };
            let scope = match visibility_of(&body) {
                Ok(s) => s,
                Err(r) => return Ok(Some(r)),
            };
            let op = AuthzOp::PutOrgVariable { name, scope, value: v.into(), create };
            done(commit_as(c, owner, &record, cred, op).await, if create { 201 } else { 204 })
        }
        (Method::Delete, "variables", [n]) => match ActionsName::parse(n) {
            Some(name) => done(commit_as(c, owner, &record, cred, AuthzOp::DeleteOrgVariable { name }).await, 204),
            None => Reply::not_found(),
        },
        _ => return Ok(None),
    };
    Ok(Some(r))
}

// ---- rotation (`design/SHARED.md`, "Actions", Rotation; BF-59)

/// One `graft-ops` call with a JSON body, answered as JSON.
async fn ops_call(c: &Ctx<'_>, path: &str, body: &Value) -> std::result::Result<Value, String> {
    let ops = c.env.service("OPS").map_err(|_| "no graft-ops binding")?;
    let mut init = RequestInit::new();
    init.with_method(Method::Post).with_body(Some(body.to_string().into()));
    let mut res = ops.fetch(&format!("https://graft-ops{path}"), Some(init)).await.map_err(|e| e.to_string())?;
    if res.status_code() != 200 {
        return Err(format!("graft-ops {path} answered {}", res.status_code()));
    }
    res.json().await.map_err(|e| e.to_string())
}

/// Every repository the owner's `Authz` names `Live`, read whole as `who` (an owner admin).
async fn owner_repos(
    c: &Ctx<'_>,
    owner: AccountId,
    login: &str,
    record: &PlacementRecord,
    f: &AuthzFold,
    who: &Principal,
) -> std::result::Result<Vec<Read>, String> {
    let mut out = Vec::new();
    for (k, e) in &f.names {
        let graftyard_identity::RepoEntry::Live { repo, .. } = e else { continue };
        let r = crate::Repo { id: *repo, owner, record: record.clone() };
        let rows = crate::api::Rows::Core;
        match crate::api::read_repo(
            c,
            r,
            login,
            k.as_str(),
            Some(who),
            Some(f.clone()),
            rows,
            crate::api::Gate::Readers,
        )
        .await
        .map_err(|e| e.to_string())?
        {
            Ok(rd) => out.push(rd),
            Err(r) => return Err(format!("{}: {}", k.as_str(), r.status)),
        }
    }
    Ok(out)
}

/// A request id for one of several commands a rotation commits in one request.
fn fresh_rid() -> graftyard_types::RequestId {
    graftyard_types::RequestId(graftyard_types::Ulid::new(crate::now(), crate::rand10()))
}

fn ok2xx(r: &Reply) -> std::result::Result<(), String> {
    if (200..300).contains(&r.status) { Ok(()) } else { Err(format!("a write answered {}", r.status)) }
}

/// `POST /api/graftyard/v1/{orgs,users}/{login}/actions/rotate-seal-key` and `.../rewrap-secrets`, for an owner admin,
/// or `None` for another path. Each answers counts and key ids, never a value.
///
/// - **`rotate-seal-key`** mints a new `SealKey`, which becomes current; reseals every org, repository, and
///   environment secret sealed to an older key to it inside `graft-ops` (`POST /actions/reseal`); then retires each
///   older key and drops it once a fresh read shows no record names it.
/// - **`rewrap-secrets`** is the wrapping key's rotation, after `graft-ops` was given the new slot as current: each
///   `SealKey`'s private half and each hook secret is wrapped again under it (`POST /actions/rewrap`). The operator then
///   clears the old slot from `graft-ops`.
pub async fn graftyard_route(c: &Ctx<'_>, req: &mut Request, rest: &str) -> Result<Option<Reply>> {
    let segs: Vec<&str> = rest.trim_matches('/').split('/').collect();
    let [kind @ ("orgs" | "users"), login, "actions", op @ ("rotate-seal-key" | "rewrap-secrets")] = segs.as_slice()
    else {
        return Ok(None);
    };
    if req.method() != Method::Post {
        return Ok(Some(Reply::error(405, "Method Not Allowed", "graftyard/actions")));
    }
    if !c.cfg.enabled.contains(&REPO_FORMAT_SECRETS) || !c.cfg.enabled.contains(&AUTHZ_FORMAT_V12) {
        return Ok(Some(Reply::not_found()));
    }
    let Some(who) = crate::authenticate(c, req).await? else {
        return Ok(Some(Reply::error(401, "Requires authentication", "rest")));
    };
    let owner = match crate::account_of(c, login).await {
        Ok(Some(a)) => a,
        Ok(None) => return Ok(Some(Reply::not_found())),
        Err(e) => return Ok(Some(unavailable(&e))),
    };
    let Some(record) = crate::placement(c, owner).await.map_err(Error::RustError)? else {
        return Ok(Some(Reply::not_found()));
    };
    let Some(f) = crate::authz_fold(c, owner, &record).await else {
        return Ok(Some(unavailable("the owner's Authz chain did not answer; retry")));
    };
    let is_org = f.opened() == Some(graftyard_identity::AccountKind::Org);
    let admin = matches!(who.actor, graftyard_identity::Actor::User(u) if f.is_admin(u));
    if !admin || is_org != (*kind == "orgs") {
        return Ok(Some(Reply::not_found()));
    }
    let r = if *op == "rotate-seal-key" {
        rotate(c, owner, login, &record, &f, &who).await
    } else {
        rewrap(c, owner, login, &record, &f, &who).await
    };
    Ok(Some(match r {
        Ok(v) => Reply::json(200, v),
        Err(e) => unavailable(&format!("the rotation stopped: {e}; it is safe to run again")),
    }))
}

/// One record resealed to `to` inside `graft-ops`; the answer is sealed bytes only.
async fn reseal(
    c: &Ctx<'_>,
    to: &SealKey,
    keys: &[&SealKey],
    name: &str,
    sealed: &[u8],
    key_id: &str,
) -> std::result::Result<Box<[u8]>, String> {
    let body = json!({ "to": to, "records": { "keys": keys, "secrets": [(name, sealed, key_id)] } });
    #[derive(serde::Deserialize)]
    struct Out {
        sealed: Vec<(String, Box<[u8]>)>,
    }
    let o: Out = serde_json::from_value(ops_call(c, "/actions/reseal", &body).await?).map_err(|e| e.to_string())?;
    o.sealed.into_iter().next().map(|(_, s)| s).ok_or_else(|| format!("{name} did not open under its key"))
}

async fn rotate(
    c: &Ctx<'_>,
    owner: AccountId,
    login: &str,
    record: &PlacementRecord,
    f: &AuthzFold,
    who: &Principal,
) -> std::result::Result<Value, String> {
    let cred = who.credential_shard();
    let new: SealKey =
        serde_json::from_value(ops_call(c, "/actions/sealkey", &json!({})).await?).map_err(|e| e.to_string())?;
    commit_as(c, owner, record, cred, AuthzOp::PutSealKey { key: new.clone() })
        .await
        .map_err(|r| r.status.to_string())?;
    let old: Vec<Box<str>> = f.actions().seal_keys.keys().filter(|k| **k != new.key_id).cloned().collect();
    let keys: Vec<&SealKey> = f.actions().seal_keys.values().collect();
    let mut resealed = 0usize;
    for (name, (scope, r)) in &f.actions().secrets {
        let sealed = reseal(c, &new, &keys, name.as_str(), &r.sealed, &r.key_id).await?;
        let op = AuthzOp::PutOrgSecret { name: name.clone(), scope: scope.clone(), sealed, key_id: new.key_id.clone() };
        commit_as(c, owner, record, cred, op).await.map_err(|r| r.status.to_string())?;
        resealed += 1;
    }
    let repos = owner_repos(c, owner, login, record, f, who).await?;
    for rd in &repos {
        for (scope, name, r) in rows::all_secrets(&rd.core) {
            if r.key_id == new.key_id {
                continue;
            }
            let sealed = reseal(c, &new, &keys, name.as_str(), &r.sealed, &r.key_id).await?;
            let op = IssueOp::PutActionsSecret { scope, name, sealed, key_id: new.key_id.clone() };
            ok2xx(
                &commit_issue_as(c, rd, who, op, What::ActionsConfig, fresh_rid()).await.map_err(|e| e.to_string())?,
            )?;
            resealed += 1;
        }
    }
    // Retire every older key, then drop each one no record names any more, read fresh.
    for k in &old {
        let op = AuthzOp::RetireSealKey { key_id: k.clone(), drop: false };
        commit_as(c, owner, record, cred, op).await.map_err(|r| r.status.to_string())?;
    }
    let f2 = crate::authz_fold(c, owner, record).await.ok_or("the owner's Authz chain did not answer")?;
    let mut named: std::collections::BTreeSet<Box<str>> =
        f2.actions().secrets.values().map(|(_, r)| r.key_id.clone()).collect();
    for rd in owner_repos(c, owner, login, record, &f2, who).await? {
        named.extend(rows::all_secrets(&rd.core).into_iter().map(|(_, _, r)| r.key_id));
    }
    let mut dropped = Vec::new();
    for k in &old {
        if named.contains(k) {
            continue;
        }
        let op = AuthzOp::RetireSealKey { key_id: k.clone(), drop: true };
        commit_as(c, owner, record, cred, op).await.map_err(|r| r.status.to_string())?;
        dropped.push(k.clone());
    }
    Ok(
        json!({ "key_id": &*new.key_id, "retired": old, "dropped": dropped, "resealed": resealed, "repositories": repos.len() }),
    )
}

async fn rewrap(
    c: &Ctx<'_>,
    owner: AccountId,
    login: &str,
    record: &PlacementRecord,
    f: &AuthzFold,
    who: &Principal,
) -> std::result::Result<Value, String> {
    let cred = who.credential_shard();
    let again = |w: &graftyard_identity::sealed::Wrapped| {
        let body = serde_json::to_value(w).unwrap_or(Value::Null);
        async move {
            serde_json::from_value::<graftyard_identity::sealed::Wrapped>(ops_call(c, "/actions/rewrap", &body).await?)
                .map_err(|e| e.to_string())
        }
    };
    let (mut keys, mut hooks) = (0usize, 0usize);
    for k in f.actions().seal_keys.values() {
        let private = again(&k.private).await?;
        if private != k.private {
            let op = AuthzOp::PutSealKey { key: SealKey { private, ..k.clone() } };
            commit_as(c, owner, record, cred, op).await.map_err(|r| r.status.to_string())?;
            keys += 1;
        }
    }
    let repos = owner_repos(c, owner, login, record, f, who).await?;
    for rd in &repos {
        for (id, mut rec) in graftyard_git::hooks::hooks(&rd.core) {
            let Some(w) = rec.secret.clone() else { continue };
            let w2 = again(&w).await?;
            if w2 == w {
                continue;
            }
            rec.secret = Some(w2);
            let op = IssueOp::PutHook { id: Some(id), record: rec };
            ok2xx(
                &commit_issue_as(c, rd, who, op, What::Hook { created: false }, fresh_rid())
                    .await
                    .map_err(|e| e.to_string())?,
            )?;
            hooks += 1;
        }
    }
    Ok(json!({ "seal_keys": keys, "hooks": hooks, "repositories": repos.len() }))
}
