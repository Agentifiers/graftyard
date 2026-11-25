//! F122's `sub` customization routes (GHES 3.22, "REST API endpoints for GitHub Actions OIDC"), which the ID token's
//! mint reads (`graftyard_actions::oidc::sub_template`):
//!
//! - `GET` and `PUT /orgs/{org}/actions/oidc/customization/sub`, `{"include_claim_keys": [..]}`, for an org admin: the
//!   owner's template, a row on its Actions shard 0.
//! - `GET` and `PUT /repos/{o}/{r}/actions/oidc/customization/sub`, `{"use_default", "include_claim_keys"?}`, for a
//!   repository admin: the repository's setting, a row on the repository's own shard.
//!
//! Each route answers 404 until the edge's `FORMATS_ENABLED` holds 0x15.8, as on an edge before it. A `PUT` answers
//! 201 with an empty body, and 422 for a key GHES refuses.

use graftyard_actions::oidc::RepoSub;
use graftyard_actions::shard::ACTIONS_FORMAT_CLAIMS;
use graftyard_actions::{OWNER_SCOPE_SHARD, actions_shard};
use graftyard_identity::{Actor, AuthzRead as _, Principal, RepoPermission};
use graftyard_types::{AccountId, ChainId};
use graftyard_wire::Reply;
use serde_json::{Value, json};
use worker::*;

use super::{runsvc_call, shard_read};
use crate::Ctx;
use crate::api::{Read, unavailable};

const DOC: &str = "rest/actions/oidc";

/// The owner's template (`orgSub` on shard 0), which `run/idtoken` passes to the mint. `None` when none was put or
/// the shard's members predate 0x15.8.
pub(super) async fn org_template(c: &Ctx<'_>, owner: AccountId) -> std::result::Result<Option<Vec<Box<str>>>, String> {
    if !c.cfg.enabled.contains(&ACTIONS_FORMAT_CLAIMS) {
        return Ok(None);
    }
    let v = shard_read(c, ChainId::Actions(owner, OWNER_SCOPE_SHARD), json!({ "orgSub": true })).await?;
    Ok(serde_json::from_value(v["orgSub"].clone()).unwrap_or_default())
}

/// The keys of a `PUT` body, or the 422 GHES answers.
fn keys(b: &Value) -> std::result::Result<Option<Vec<Box<str>>>, Reply> {
    match &b["include_claim_keys"] {
        Value::Null => Ok(None),
        v => serde_json::from_value(v.clone())
            .map(Some)
            .map_err(|_| Reply::error(422, "include_claim_keys must be an array of strings", DOC)),
    }
}

async fn put(
    c: &Ctx<'_>,
    chain: ChainId,
    repo: Option<graftyard_types::RepoId>,
    sub: RepoSub,
    admin: AccountId,
) -> Reply {
    if let Err(m) = graftyard_actions::oidc::check_sub(&sub) {
        return Reply::error(422, m, DOC);
    }
    match runsvc_call(c, chain, &json!({ "op": "set_sub", "repo": repo, "sub": sub, "admin": admin })).await {
        Ok((201, _)) => Reply::json(201, json!({})),
        Ok((s, v)) => Reply::error(s, v["body"]["message"].as_str().unwrap_or("refused"), DOC),
        Err(e) => unavailable(&e),
    }
}

/// `/repos/{o}/{r}/actions/oidc/customization/sub`, or `None` for another tail or before 0x15.8.
pub async fn repo_route(
    c: &Ctx<'_>,
    req: &mut Request,
    rd: &Read,
    who: Option<&Principal>,
    tail: &[&str],
) -> Result<Option<Reply>> {
    if tail != ["actions", "oidc", "customization", "sub"] || !c.cfg.enabled.contains(&ACTIONS_FORMAT_CLAIMS) {
        return Ok(None);
    }
    let Some(who) = who else { return Ok(Some(Reply::error(401, "Requires authentication", "rest"))) };
    let access = graftyard_git::issue_ops::Access { repo: rd.repo.id, owner: rd.repo.owner, core: &rd.core };
    let folds: Vec<&dyn graftyard_identity::AuthzRead> =
        rd.dir.authz.iter().map(|a| a as &dyn graftyard_identity::AuthzRead).collect();
    let (Actor::User(admin), Some(RepoPermission::Admin)) =
        (who.actor, graftyard_identity::permission(who, &folds, &access))
    else {
        return Ok(Some(Reply::not_found()));
    };
    let chain = ChainId::Actions(rd.repo.owner, actions_shard(rd.repo.id));
    Ok(Some(match req.method() {
        Method::Get => match shard_read(c, chain, json!({ "subRepo": rd.repo.id.0.0 })).await {
            Ok(v) => match serde_json::from_value::<RepoSub>(v["subRepo"].clone()) {
                Ok(s) => Reply::json(200, sub_json(&s)),
                Err(_) => unavailable("the shard's members do not read 0x15.8 yet"),
            },
            Err(e) => unavailable(&e),
        },
        Method::Put => {
            let b: Value = serde_json::from_slice(&req.bytes().await?).unwrap_or(Value::Null);
            let Some(use_default) = b["use_default"].as_bool() else {
                return Ok(Some(Reply::error(422, "use_default is required", DOC)));
            };
            let keys = match keys(&b) {
                Ok(k) => k,
                Err(r) => return Ok(Some(r)),
            };
            put(c, chain, Some(rd.repo.id), RepoSub { use_default, keys }, admin.0).await
        }
        _ => Reply::not_found(),
    }))
}

/// GHES's repository answer: `use_default`, and `include_claim_keys` only when the repository has its own.
fn sub_json(s: &RepoSub) -> Value {
    let mut v = json!({ "use_default": s.use_default });
    if let Some(k) = &s.keys {
        v["include_claim_keys"] = json!(k);
    }
    v
}

/// `/orgs/{org}/actions/oidc/customization/sub`, or `None` for another path or before 0x15.8.
pub async fn org_route(c: &Ctx<'_>, req: &mut Request, segs: &[&str]) -> Result<Option<Reply>> {
    let ["orgs", login, "actions", "oidc", "customization", "sub"] = segs else { return Ok(None) };
    if !c.cfg.enabled.contains(&ACTIONS_FORMAT_CLAIMS) {
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
        Actor::User(u) if f.is_admin(u) => u,
        _ => return Ok(Some(Reply::not_found())),
    };
    if f.opened() != Some(graftyard_identity::AccountKind::Org) {
        return Ok(Some(Reply::not_found()));
    }
    let chain = ChainId::Actions(owner, OWNER_SCOPE_SHARD);
    Ok(Some(match req.method() {
        Method::Get => match org_template(c, owner).await {
            Ok(k) => Reply::json(200, json!({ "include_claim_keys": k.unwrap_or_default() })),
            Err(e) => unavailable(&e),
        },
        Method::Put => {
            let b: Value = serde_json::from_slice(&req.bytes().await?).unwrap_or(Value::Null);
            let keys = match keys(&b) {
                Ok(Some(k)) => k,
                Ok(None) => return Ok(Some(Reply::error(422, "include_claim_keys is required", DOC))),
                Err(r) => return Ok(Some(r)),
            };
            put(c, chain, None, RepoSub { use_default: false, keys: Some(keys) }, admin.0).await
        }
        _ => Reply::not_found(),
    }))
}
