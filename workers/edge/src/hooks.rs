//! Repository webhooks on the edge (`build/BUILD-PLAN.md`, M4-WP1; `design/SHARED.md`, "Event fan-out"): GHES 3.22's
//! `/repos/{o}/{r}/hooks` routes.
//!
//! - `GET .../hooks` and `GET .../hooks/{id}` read the core `hook` rows (`graftyard_api::webhooks::hook_json`, the
//!   secret masked). Admin only, as on GHES; anyone else gets 404.
//! - `POST .../hooks`, `PATCH .../hooks/{id}`, and `DELETE .../hooks/{id}` are `IssueOp::PutHook` and `DeleteHook`
//!   (format 32.16), which their leader renders from `&Acked`. A body's secret is wrapped by `graft-ops` over the `OPS`
//!   binding (`POST /wrap`) before the command leaves the edge, so no secret reaches a chain or a log.
//! - `POST .../pings`, `GET .../deliveries[/{id}]`, and `POST .../deliveries/{id}/attempts` run on the owner's
//!   `HookSequencer` (`graftyard_api::webhooks::sequencer_route`), the object `hookseq:<owner>` on the owner's home site
//!   through the `HOOKSEQ_<n>` bindings; an edge without them answers 503. A home site that does not answer is
//!   503 too: the history lives there [inference: the consumer falls back to the next site only to deliver].
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
pub(crate) async fn wrap(c: &Ctx<'_>, secret: &str) -> std::result::Result<Wrapped, Reply> {
    let mut init = RequestInit::new();
    let body = serde_json::json!({ "secret": secret }).to_string();
    init.with_method(Method::Post).with_body(Some(body.into()));
    let mut res = c.ops("https://graft-ops/wrap", init).await.map_err(|_| unavailable("graft-ops"))?;
    if res.status_code() != 200 {
        return Err(unavailable(&format!("graft-ops did not wrap the secret ({})", res.status_code())));
    }
    res.json::<Wrapped>().await.map_err(|_| unavailable("graft-ops answered no wrapped secret"))
}

/// `/route` on the owner's `HookSequencer` on its home site.
async fn sequencer(c: &Ctx<'_>, rd: &Read, body: &Value) -> std::result::Result<Reply, String> {
    let home = rd.repo.record.home;
    let region = c.sites.get(home).ok_or("unknown home site")?.region;
    let stub = c
        .env
        .durable_object(&format!("HOOKSEQ_{}", home.0))
        .and_then(|ns| ns.get_by_name_with_location_hint(&format!("hookseq:{}", rd.repo.owner.0.0), region.hint()))
        .map_err(|e| e.to_string())?;
    let mut init = RequestInit::new();
    init.with_method(Method::Post).with_body(Some(body.to_string().into()));
    let req = Request::new_with_init("https://hookseq/route", &init).map_err(|e| e.to_string())?;
    let mut r = stub.fetch_with_request(req).await.map_err(|e| e.to_string())?;
    if r.status_code() != 200 {
        return Err(format!("{} {}", r.status_code(), r.text().await.unwrap_or_default()));
    }
    let w: graftyard_api::hosted::WireReply = r.json().await.map_err(|e| e.to_string())?;
    Ok(w.reply())
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
            Some(i) => {
                let doc = "rest/repos/webhooks#delete-a-repository-webhook";
                crate::api::commit_deleted(c, rd, &who, IssueOp::DeleteHook { id: i }, doc).await?
            }
            None => Reply::not_found(),
        },
        (Method::Post, [_, "pings"])
        | (Method::Get, [_, "deliveries"])
        | (Method::Get, [_, "deliveries", _])
        | (Method::Post, [_, "deliveries", _, "attempts"]) => {
            if !admin {
                return Ok(Some(Reply::not_found()));
            }
            let Some((i, row)) = id(tail[1]).and_then(|i| Some((i, hook_row(&rd.core, i)?))) else {
                return Ok(Some(Reply::not_found()));
            };
            let ping = (tail.get(2) == Some(&"pings"))
                .then(|| serde_json::to_vec(&graftyard_api::webhooks::ping(&view, i, &row)).unwrap_or_default());
            let cx = graftyard_api::webhooks::RouteCtx {
                repo: rd.repo.id,
                url: graftyard_api::webhooks::deliveries_url(&view, i),
                ping,
            };
            let url = req.url()?;
            let body = serde_json::json!({
                "cx": cx, "id": i, "row": row, "method": req.method().to_string(),
                "tail": tail[2..].to_vec(), "query": url.query().unwrap_or(""),
            });
            match sequencer(c, rd, &body).await {
                Ok(r) => r,
                Err(e) => unavailable(&format!("the owner's HookSequencer did not answer: {e}")),
            }
        }
        _ => return Ok(None),
    };
    Ok(Some(r))
}
