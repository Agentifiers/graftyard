//! Actions OIDC on the edge (`graftyard_actions::oidc`; `decisions/DECISIONS.md`, "`actions-oidc`"): the issuer
//! `https://HOST/_services/token`, its discovery document and JWKS, and `GET run/idtoken`.
//!
//! - **Discovery and JWKS** take no token, since a relying party fetches them unauthenticated. The JWKS is both
//!   `ActionsOidcKey` slots' public keys from the `oidc` Registry object of the first non-spare site that lists any
//!   (`/oidc_slots`), kept `SLOTS_FOR_MS`, so a token signed under either slot verifies across a `Rotate`.
//! - **`run/idtoken`** carries the job token the runner exports as `ACTIONS_ID_TOKEN_REQUEST_TOKEN`. The edge reads
//!   what the shard does not hold, the repository's name, visibility and enterprise and whether the job's runner is
//!   hosted, and the shard leader checks the grant and signs (`workers/site/src/actions/oidc.rs`). A read that fails
//!   answers 503, which the toolkit retries.

use graftyard_actions::runsvc::Grant;
use graftyard_actions::shard::WorkflowRun;
use graftyard_actions::{JobId, OWNER_SCOPE_SHARD, oidc};
use graftyard_identity::AuthzRead as _;
use graftyard_types::{AccountId, ChainId};
use serde_json::{Value, json};
use worker::*;

use super::{answer, job_row, json_reply, service_error, shard_read};
use crate::Ctx;

/// How long an isolate keeps the slots a site's registry answered [guess: as the SSH host-key slots].
const SLOTS_FOR_MS: u64 = 60_000;

thread_local! {
    static SLOTS: std::cell::RefCell<Option<(u64, Value)>> = const { std::cell::RefCell::new(None) };
}

/// `GET token/.well-known/openid-configuration` and `GET token/.well-known/jwks`, else `None`.
pub async fn well_known(c: &Ctx<'_>, rest: &str) -> Result<Option<Response>> {
    let issuer = oidc::issuer(&format!("https://{}", c.cfg.host));
    match rest {
        "token/.well-known/openid-configuration" => Ok(Some(json_reply(200, &oidc::discovery(&issuer))?)),
        "token/.well-known/jwks" => match site_slots(c).await {
            Some(p) => Ok(Some(json_reply(200, &oidc::jwks(&p))?)),
            None => Ok(Some(service_error(503)?)),
        },
        _ => Ok(None),
    }
}

/// Both slots as a site publishes them (`oidc::published`), from the first non-spare site whose bindings hold a key.
async fn site_slots(c: &Ctx<'_>) -> Option<Value> {
    let now = graftyard_lookup::now_ms();
    let kept = SLOTS.with(|s| s.borrow().as_ref().filter(|(at, _)| now < at + SLOTS_FOR_MS).map(|(_, v)| v.clone()));
    if kept.is_some() {
        return kept;
    }
    let b = c.bindings();
    for site in c.sites.0.iter().filter(|s| !s.spare) {
        let Ok(stub) = b.registry(site.id, "oidc") else { continue };
        let call = async {
            let mut r = stub.fetch_with_str("https://registry/oidc_slots").await.ok()?;
            if r.status_code() != 200 {
                return None;
            }
            r.json::<Value>().await.ok()
        };
        let Some(Some(v)) = graftyard_lookup::within(graftyard_lookup::CALL, call).await else { continue };
        if v["keys"].as_array().is_some_and(|k| !k.is_empty()) {
            SLOTS.with(|s| *s.borrow_mut() = Some((now, v.clone())));
            return Some(v);
        }
    }
    None
}

/// `GET run/idtoken?api-version=2.0&audience=...` with the job's token.
pub async fn idtoken(c: &Ctx<'_>, req: &Request, chain: ChainId, g: Grant) -> Result<Response> {
    let (Grant::Job { job, .. }, ChainId::Actions(owner, _)) = (g, chain) else { return service_error(401) };
    let audience = req.url()?.query_pairs().find(|(k, _)| k == "audience").map(|(_, v)| v.into_owned());
    let facts = match facts(c, chain, owner, job).await {
        Ok(f) => f,
        Err(e) => {
            console_error!("idtoken {chain} job {}: {e}", job.0.0);
            return service_error(503);
        }
    };
    let mut body = json!({ "op": "idtoken", "grant": g, "audience": audience });
    for (k, v) in facts {
        body[k] = v;
    }
    answer(c, chain, body).await
}

/// The job's repository `o/r`, its visibility, its owner's enterprise as `[login, id]` (none when the login did not
/// answer), and whether its runner is a hosted one, registered on `OWNER_SCOPE_SHARD` with `hosted`.
async fn facts(
    c: &Ctx<'_>,
    chain: ChainId,
    owner: AccountId,
    job: JobId,
) -> std::result::Result<Vec<(&'static str, Value)>, String> {
    let row = job_row(c, chain, job).await.ok_or("no such job")?;
    let v = shard_read(c, chain, json!({ "run": row.run.0.0 })).await?;
    let run: WorkflowRun = serde_json::from_value(v["run"]["run"].clone()).map_err(|e| format!("run: {e}"))?;
    let repo = crate::repo_by_id(c, run.repo).await?;
    let (login, name, repo, authz) = crate::api::repo_named_why(c, repo).await?;
    let core = crate::api::core_rows(c, &repo, "actions").await.map_err(|e| e.to_string())??;
    let visibility = graftyard_git::tiers::get_rec::<graftyard_git::issues::RepoSettings>(&core, "settings", &[])
        .map_or(graftyard_identity::Visibility::Private, |s| s.visibility);
    let enterprise = match authz.enterprise() {
        Some(e) => {
            let logins = crate::api::account_logins(c, &std::iter::once(e).collect(), 1).await;
            logins.get(&e.0.0).map(|l| json!([l, e.0.0]))
        }
        None => None,
    };
    let hosted = match row.state.runner() {
        Some(r) => {
            let v = shard_read(c, ChainId::Actions(owner, OWNER_SCOPE_SHARD), json!({ "runner": r.0.0 })).await?;
            v["runner"]["hosted"].as_bool() == Some(true)
        }
        None => false,
    };
    Ok(vec![
        ("repository", json!(format!("{login}/{name}"))),
        ("visibility", json!(visibility)),
        ("enterprise", enterprise.unwrap_or(Value::Null)),
        ("hosted", json!(hosted)),
    ])
}
