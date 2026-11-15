//! M4-WP4's install-side routes for `graft-runners` (`design/SHARED.md`, "Actions", Hosted runners; `design/DESIGN.md`,
//! Shape, `graft-runners`). The caller is the `graftyard-hosted-runners` App, by an installation token on the owner:
//!
//! - `GET /api/graftyard/v1/owners/{owner}/actions/hosted/demand`: `Broker::demand`, the owner's `HostedDemand`, read
//!   by the owner's `Owner`-scope broker (`RunnerBroker`, `broker:actions:<owner>.0`) from its 16 shards.
//! - `POST .../hosted/runners` `{"label", "key", "name"}`: a hosted registration on `OWNER_SCOPE_SHARD`, which
//!   `decide` refuses past the owner's cap (409), and the runner's `--jitconfig` pieces, which `graft-runners` writes
//!   with the private half of `key`. GHES's `generate-jitconfig` takes the same request through
//!   `POST /orgs/{org}/actions/runners/generate-jitconfig` and answers `encoded_jit_config` built from a key the server
//!   holds; this route keeps the private key in the runner account [inference: `runsvc::jit_config` needs the private
//!   key, and the install never holds a runner's].
//! - `DELETE .../hosted/runners/{id}`: a stale runner's registration, refused (422) while it holds a job.
//!
//! The installation needs `organization_self_hosted_runners` write (SHARED: "self-hosted-runner write permission").
//! Nothing here calls the Cloudflare API, and no GHES reply changes.

use graftyard_actions::OWNER_SCOPE_SHARD;
use graftyard_actions::hosted::{self, HostedDemand, HostedLabel};
use graftyard_actions::runsvc::{self, RunnerAt};
use graftyard_identity::{Access, Actor, AuthzRead as _, Credential, Principal};
use graftyard_types::{AccountId, ChainId};
use graftyard_wire::Reply;
use serde_json::{Value, json};
use worker::*;

use crate::Ctx;
use crate::api::unavailable;

/// The installation permission a caller needs [doc: GitHub's App permission for org self-hosted runners].
pub const PERMISSION: &str = "organization_self_hosted_runners";
/// The runner version the install pins (`tests/m4/wp3/runner-protocol.md`), as the admin route registers.
const RUNNER_VERSION: &str = "2.338.2";

/// The routes above, or `None` for another path under `/api/graftyard/v1`.
pub async fn route(c: &Ctx<'_>, req: &mut Request, segs: &[&str]) -> Result<Option<Reply>> {
    let ["owners", login, "actions", "hosted", tail @ ..] = segs else { return Ok(None) };
    let Some(who) = crate::authenticate(c, req).await? else {
        return Ok(Some(Reply::error(401, "Requires authentication", "rest")));
    };
    let owner = match crate::account_of(c, login).await {
        Ok(Some(a)) => a,
        Ok(None) => return Ok(Some(Reply::not_found())),
        Err(e) => return Ok(Some(unavailable(&e))),
    };
    if !may_run(&who, owner, c).await {
        return Ok(Some(Reply::not_found()));
    }
    let method = req.method();
    Ok(Some(match (method, tail) {
        (Method::Get, ["demand"]) => match demand(c, owner).await {
            Ok(d) => Reply::json(200, serde_json::to_value(d)?),
            Err(e) => unavailable(&e),
        },
        (Method::Post, ["runners"]) => {
            let b: Value = serde_json::from_slice(&req.bytes().await?).unwrap_or(Value::Null);
            register(c, login, owner, &b).await
        }
        (Method::Delete, ["runners", id]) => match id.parse::<i64>() {
            Ok(id) => {
                let chain = ChainId::Actions(owner, OWNER_SCOPE_SHARD);
                match crate::actions::runsvc_call(c, chain, &json!({ "op": "delete_hosted", "runner": id })).await {
                    Ok((204, _)) => Reply::empty(204),
                    Ok((s, v)) => Reply::json(s, v["body"].clone()),
                    Err(e) => unavailable(&e),
                }
            }
            Err(_) => Reply::not_found(),
        },
        _ => Reply::not_found(),
    }))
}

/// An installation token of an App installed on `owner` with self-hosted-runner write. The App's identity is not
/// checked here: any App the owner granted that permission may run hosted runners for it, as on GitHub any such App
/// may register self-hosted runners [inference].
async fn may_run(who: &Principal, owner: AccountId, c: &Ctx<'_>) -> bool {
    let (Actor::App { installation, .. }, Credential::Stateless(claims)) = (who.actor, &who.credential) else {
        return false;
    };
    if claims.permissions.0.get(PERMISSION).is_none_or(|a| *a < Access::Write) {
        return false;
    }
    let Ok(Some(f)) = crate::ident::authz_of(c, owner).await else { return false };
    f.installation(installation).is_some_and(|s| s.row.is_some())
}

/// `Broker::demand` on the owner's `Owner`-scope broker, on its registration shard's leader site.
async fn demand(c: &Ctx<'_>, owner: AccountId) -> std::result::Result<HostedDemand, String> {
    let chain = ChainId::Actions(owner, OWNER_SCOPE_SHARD);
    let record = crate::actions::shard_record(c, chain).await?;
    let site = crate::actions::leader_site(c, chain, &record).await;
    let stub = c.bindings().object("BROKER", site, &format!("broker:{chain}"), None).map_err(|e| e.to_string())?;
    let mut init = RequestInit::new();
    let body = json!({ "owner": owner, "record": record });
    init.with_method(Method::Post).with_body(Some(body.to_string().into()));
    let req = Request::new_with_init("https://broker/demand", &init).map_err(|e| e.to_string())?;
    let mut r = stub.fetch_with_request(req).await.map_err(|e| e.to_string())?;
    if r.status_code() != 200 {
        return Err(format!("broker demand: {}", r.status_code()));
    }
    r.json().await.map_err(|e| e.to_string())
}

/// `POST .../hosted/runners`: one hosted registration and its JIT config pieces.
async fn register(c: &Ctx<'_>, login: &str, owner: AccountId, b: &Value) -> Reply {
    use base64::Engine as _;
    let row = b["label"].as_str().and_then(HostedLabel::named);
    let key = b["key"].as_str().and_then(|k| base64::engine::general_purpose::STANDARD.decode(k).ok());
    let (Some(row), Some(key)) = (row, key) else {
        return Reply::error(422, "label (a hosted label) and key are required", "rest");
    };
    let name = b["name"].as_str().unwrap_or("graftyard-hosted");
    let rec = hosted::record(row, name, key.into(), RUNNER_VERSION);
    let chain = ChainId::Actions(owner, OWNER_SCOPE_SHARD);
    match crate::actions::runsvc_call(c, chain, &json!({ "op": "register_hosted", "record": rec })).await {
        Ok((201, v)) => {
            let Ok(at) = serde_json::from_value::<RunnerAt>(v["body"]["runner"].clone()) else {
                return unavailable("the shard's register answer");
            };
            let urls = runsvc::Urls::new(&format!("https://{}", c.cfg.host));
            Reply::json(
                201,
                json!({
                    "runner": at.runner.0.0,
                    "client_id": runsvc::client_id(at),
                    "server_url_v2": urls.broker(),
                    "authorization_url": urls.token(),
                    "github_url": format!("https://{}/{login}", c.cfg.host),
                    "labels": row.labels,
                }),
            )
        }
        Ok((s, v)) => Reply::json(s, v["body"].clone()),
        Err(e) => unavailable(&e),
    }
}
