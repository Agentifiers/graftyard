//! The agent settings endpoints (AM1-WP2; `design/AGENT-LAYER.md`, "API reference"): the install's `AgentApp` rows,
//! an org's agent settings, and a repository's `AgentPolicy`, under `/api/graftyard/v1`. Each write commits one
//! command on its chain under the caller's credential, and the leader's `decide` checks who may write it. Served only
//! once `FormatsEnable` enabled the command's format (Authz 0x12.3, Repo 32.8); until then 503.

use graftyard_api::agents::{self, Route};
use graftyard_identity::{AuthzCmd, AuthzFold, AuthzOp, AuthzRead, Principal};
use graftyard_types::ChainId;
use graftyard_wire::{Body, Reply};
use serde_json::{Value, json};
use worker::*;

use crate::Ctx;

fn respond(c: &Ctx<'_>, r: Reply) -> Result<Response> {
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
        Method::Put | Method::Patch => serde_json::from_slice::<Value>(&req.bytes().await?).unwrap_or(Value::Null),
        _ => Value::Null,
    };
    let r = match route {
        Route::Policy { owner, repo } => crate::api::agent_policy(c, req, owner, repo, who, method, &body).await?,
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
