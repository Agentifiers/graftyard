//! Repository webhooks on the edge (`build/BUILD-PLAN.md`, M4-WP1; `design/SHARED.md`, "Event fan-out"): GHES 3.22's
//! `/repos/{o}/{r}/hooks` routes.
//!
//! - `GET .../hooks` and `GET .../hooks/{id}` read the core `hook` rows (`graftyard_api::webhooks::hook_json`, the
//!   secret masked). Admin only, as on GHES; anyone else gets 404.
//! - `POST .../hooks`, `PATCH .../hooks/{id}`, and `DELETE .../hooks/{id}` are `IssueOp::PutHook` and `DeleteHook`
//!   (format 32.16), which their leader renders from `&Acked`. A body's secret is wrapped by `graft-ops` over the `OPS`
//!   binding (`POST /wrap`) before the command leaves the edge, so no secret reaches a chain or a log.
//! - `POST .../pings`, `GET .../deliveries[/{id}]`, and `POST .../deliveries/{id}/attempts` run on the owner's
//!   `HookSequencer` (`graftyard_api::webhooks::sequencer_route`), which the hosted half builds; until then they answer
//!   503.
//!
//! Until `FormatsEnable` (the test edge's `FORMATS_ENABLED`) enabled 32.16 every route falls through to the reply it
//! had before, so no reply changes, and after it only these routes' replies do.

use graftyard_api::hosted::What;
use graftyard_api::webhooks::{hook_body, hook_json};
use graftyard_git::hooks::{HookId, REPO_FORMAT_HOOKS, hook_row, hooks};
use graftyard_git::issue_ops::IssueOp;
use graftyard_identity::Principal;
use graftyard_identity::sealed::Wrapped;
use graftyard_types::DbId;
use graftyard_wire::{Host, Reply};
use serde_json::Value;
use worker::*;

use crate::Ctx;
use crate::api::{Read, commit_issue, unavailable};

/// `graft-ops` wraps a hook secret under `SecretsWrapKey`'s current slot. The secret travels only in this request's
/// body over the service binding, never in a URL or a log line.
async fn wrap(c: &Ctx<'_>, secret: &str) -> std::result::Result<Wrapped, Reply> {
    let ops = c.env.service("OPS").map_err(|_| unavailable("no graft-ops binding"))?;
    let mut init = RequestInit::new();
    let body = serde_json::json!({ "secret": secret }).to_string();
    init.with_method(Method::Post).with_body(Some(body.into()));
    let mut res = ops.fetch("https://graft-ops/wrap", Some(init)).await.map_err(|_| unavailable("graft-ops"))?;
    if res.status_code() != 200 {
        return Err(unavailable(&format!("graft-ops did not wrap the secret ({})", res.status_code())));
    }
    res.json::<Wrapped>().await.map_err(|_| unavailable("graft-ops answered no wrapped secret"))
}

/// The routes under `/repos/{o}/{r}/hooks`, or `None` for another tail.
pub async fn repo_route(
    c: &Ctx<'_>,
    req: &mut Request,
    rd: &Read,
    who: Option<&Principal>,
    tail: &[&str],
    host: &Host,
) -> Result<Option<Reply>> {
    // Before 32.16 is enabled the routes fall through to the reply every client got before M4-WP1.
    if tail.first() != Some(&"hooks") || !c.cfg.enabled.contains(&REPO_FORMAT_HOOKS) {
        return Ok(None);
    }
    let Some(who) = who.cloned() else { return Ok(Some(Reply::error(401, "Requires authentication", "rest"))) };
    let view = rd.view(host);
    // GHES shows a repository's hooks only to its admins (an App with `repository_hooks`); everyone else sees 404.
    // Each write's leader checks the same again in `decide`.
    let access = graftyard_git::issue_ops::Access { repo: rd.repo.id, owner: rd.repo.owner, core: &rd.core };
    let folds: Vec<&dyn graftyard_identity::AuthzRead> =
        rd.dir.authz.iter().map(|a| a as &dyn graftyard_identity::AuthzRead).collect();
    let admin =
        graftyard_identity::permission(&who, &folds, &access) == Some(graftyard_identity::RepoPermission::Admin);
    let id = |s: &str| s.parse::<i64>().ok().map(|i| HookId(DbId(i)));
    let r = match (req.method(), &tail[1..]) {
        (Method::Get, []) => {
            if !admin {
                return Ok(Some(Reply::not_found()));
            }
            let all: Vec<Value> = hooks(&rd.core)
                .into_iter()
                .filter_map(|(i, _)| Some(hook_json(&view, i, &hook_row(&rd.core, i)?)))
                .collect();
            Reply::json(200, Value::Array(all))
        }
        (Method::Get, [n]) => match id(n).and_then(|i| Some((i, hook_row(&rd.core, i)?))).filter(|_| admin) {
            Some((i, row)) => Reply::json(200, hook_json(&view, i, &row)),
            None => Reply::not_found(),
        },
        (Method::Post, []) | (Method::Patch, [_]) => {
            let target = match tail.get(1) {
                Some(n) => match id(n).and_then(|i| Some((i, hook_row(&rd.core, i)?))).filter(|_| admin) {
                    Some(x) => Some(x),
                    None => return Ok(Some(Reply::not_found())),
                },
                None => None,
            };
            let body: Value = serde_json::from_slice(&req.bytes().await?).unwrap_or(Value::Null);
            let (mut record, secret) = match hook_body(target.as_ref().map(|(_, r)| &r.record), &body) {
                Ok(x) => x,
                Err(r) => return Ok(Some(r)),
            };
            match secret {
                Some(Some(s)) => match wrap(c, &s).await {
                    Ok(w) => record.secret = Some(w),
                    Err(r) => return Ok(Some(r)),
                },
                Some(None) => record.secret = None,
                None => {}
            }
            let created = target.is_none();
            let op = IssueOp::PutHook { id: target.map(|(i, _)| i), record };
            commit_issue(c, rd, &who, op, What::Hook { created }).await?
        }
        (Method::Delete, [n]) => match id(n) {
            Some(i) => commit_issue(c, rd, &who, IssueOp::DeleteHook { id: i }, What::Deleted).await?,
            None => Reply::not_found(),
        },
        (Method::Post, [_, "pings"])
        | (Method::Get, [_, "deliveries"])
        | (Method::Get, [_, "deliveries", _])
        | (Method::Post, [_, "deliveries", _, "attempts"]) => {
            if !admin {
                return Ok(Some(Reply::not_found()));
            }
            unavailable("the owner's HookSequencer is not deployed on this install")
        }
        _ => return Ok(None),
    };
    Ok(Some(r))
}
