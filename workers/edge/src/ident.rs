//! The edge's identity routes (`build/BUILD-PLAN.md`, M3-WP3 and M3-WP4, the hosted half): `ghs_` authentication,
//! `POST /app/installations/{id}/access_tokens`, `DELETE /installation/token`, and the REST identity routes journey 4
//! needs. Every route reads its folds through `fresh` and commits through the lookup rule, as git does.
//!
//! An M3 command is sent only once its format is in `FORMATS_ENABLED`: a member on a release that does not read it
//! would refuse it (`design/SHARED.md`, the two-release rule).

use graftyard_identity::stateless::{self, AccessTokenRequest, Grant, InstallationClaims, TokenRequestError};
use graftyard_identity::{
    Access, AppId, AppRecord, AuthzFold, AuthzRead, InstallationId, NameKey, PermissionSet, Principal, RepoName,
    RepoScope, TokensFold, name_shard,
};
use graftyard_raft::Format;
use graftyard_types::{AccountId, ChainId, DbId, RepoId, TokenShard};
use graftyard_wire::Reply;
use serde_json::{Value, json};
use worker::*;

use crate::{Ctx, now, system_read};

/// A read of a system chain's whole fold, as a leader's `dep` read is: `None` for a chain with no group.
async fn dep_fold<T: serde::de::DeserializeOwned>(
    c: &Ctx<'_>,
    chain: ChainId,
) -> std::result::Result<Option<T>, String> {
    let v = system_read(c, chain, json!({ "dep": true })).await?;
    let Some(hex) = v.as_str() else { return Ok(None) };
    let bytes = graftyard_lookup::unhex(hex).ok_or("dep: not hex")?;
    graftyard_raft::codec::from_bytes(&bytes).map(Some).map_err(|e| format!("{chain}: {e}"))
}

/// An owner's `Authz` fold by read-index (`crate::authz_read`), under the owner's `Placement` record.
pub async fn authz_of(c: &Ctx<'_>, owner: AccountId) -> std::result::Result<Option<AuthzFold>, String> {
    let Some(record) = crate::placement(c, owner).await? else { return Ok(None) };
    crate::authz_read(c, owner, &record).await
}

/// `ghs_` authentication (`design/SHARED.md`, "Identity and tokens"): checked offline against the `kid`'s
/// `VerifyingKey` row on `TokenShard(0)`, read through `fresh`. A revoked `jti` is refused here too, from a `fresh`
/// read of its `jti_shard`, so the next write and the next read both see a `DELETE` that returned 204; every write's
/// leader re-reads the row at `decide`. A token that fails any check is no caller, which the routes answer with 401.
pub async fn stateless(c: &Ctx<'_>, text: &str) -> Result<Option<Principal>> {
    let Ok(unverified) = stateless::parse(text) else { return Ok(None) };
    let (read_class, at) = (c.read_class.get(), now());
    // The verifying keys and the `jti`'s revocation row are read at once: the `jti` is in the claims before the
    // signature is checked, and a token whose signature fails is refused whatever the row says. A read-class request
    // may use either answer up to `READ_CACHE` old (`crate::rcache`), and only an answer that lets the token through
    // is kept.
    let jti = unverified.jti();
    let keys = crate::rcache::through(
        &crate::rcache::KEYS,
        (),
        at,
        read_class,
        &c.timing,
        dep_fold::<TokensFold>(c, ChainId::Tokens(TokenShard(0))),
    );
    let read_jti = async {
        // Most `jti` shards hold no group until a token there is revoked. One is created on first use, as every
        // sharded system chain is, so the leader of the caller's next write can read it as a dependency; until then
        // it holds no revocation.
        let chain = ChainId::Tokens(graftyard_identity::jti_shard(&jti));
        let b = c.bindings();
        let q = json!({ "q": { "jti": jti.0.to_string() } }).to_string();
        loop {
            let record = c.system_record(chain).await?;
            match graftyard_lookup::call_opt(&b, chain, &record, "/read", q.as_bytes()).await {
                Ok(Some((v, _, _))) => {
                    let exp = v.pointer("/value/jti").and_then(Value::as_u64);
                    return Ok::<_, String>(Some(exp.is_some_and(|exp| exp > now().0)));
                }
                // A miss under the genesis binding: the view's record, if it differs, before creating anything.
                _ if c.rebind().await => continue,
                Ok(None) => {
                    graftyard_lookup::create(&b, chain, &record, &json!({})).await?;
                    return Ok(Some(false));
                }
                Err(e) => return Err(e),
            }
        }
    };
    let revoked = crate::rcache::through(&crate::rcache::JTIS, jti.0.0, at, read_class, &c.timing, read_jti);
    let (keys, revoked) = futures_util::future::join(keys, revoked).await;
    let Some(keys) = keys.map_err(Error::RustError)? else { return Ok(None) };
    let claims = match stateless::verify(text, &keys, now()) {
        Ok(claims) => claims,
        // Refused on keys up to `READ_CACHE` old: read them again, so a key added since (a rotation) verifies at once.
        Err(_) if read_class => {
            let Some(keys) =
                dep_fold::<TokensFold>(c, ChainId::Tokens(TokenShard(0))).await.map_err(Error::RustError)?
            else {
                return Ok(None);
            };
            crate::rcache::put(&crate::rcache::KEYS, (), now(), keys.clone());
            let Ok(claims) = stateless::verify(text, &keys, now()) else { return Ok(None) };
            claims
        }
        Err(_) => return Ok(None),
    };
    if revoked.map_err(Error::RustError)? != Some(false) {
        return Ok(None);
    }
    Ok(Some(Principal::installation(&claims)))
}

fn enabled(c: &Ctx<'_>, f: Format) -> std::result::Result<(), Reply> {
    if c.cfg.enabled.contains(&f) {
        Ok(())
    } else {
        Err(crate::api::unavailable(&format!(
            "this write needs record format {}.{}, which FormatsEnable has not enabled on this install",
            f.family, f.version
        )))
    }
}

fn bad_credentials() -> Reply {
    Reply::error(401, "Bad credentials", "rest")
}

/// Commits `cmd` on a system chain through the lookup rule, under this request's id; `Err` is the reply to send.
pub async fn commit_system(c: &Ctx<'_>, chain: ChainId, cmd: Value) -> std::result::Result<Value, Reply> {
    // A write may create the group, so an isolate on the genesis binding checks it against the view first (once).
    c.rebind().await;
    let record = c.system_record(chain).await.map_err(|e| crate::api::unavailable(&e))?;
    let b = c.bindings();
    if let Err(e) = graftyard_lookup::create(&b, chain, &record, &json!({})).await {
        return Err(crate::api::unavailable(&e));
    }
    graftyard_lookup::commit_as(&b, chain, &record, c.rid, cmd).await.map_err(|e| crate::api::unavailable(&e))
}

/// Commits `cmd` on an owner's chain (`Authz` or `Identity`) under the owner's `Placement` record.
pub async fn commit_owner(
    c: &Ctx<'_>,
    chain: ChainId,
    owner: AccountId,
    cmd: Value,
) -> std::result::Result<Value, Reply> {
    let record = match crate::placement(c, owner).await {
        Ok(Some(r)) => r,
        Ok(None) => return Err(Reply::not_found()),
        Err(e) => return Err(crate::api::unavailable(&e)),
    };
    let b = c.bindings();
    if let Err(e) = graftyard_lookup::create(&b, chain, &record, &json!({})).await {
        return Err(crate::api::unavailable(&e));
    }
    graftyard_lookup::commit_as(&b, chain, &record, c.rid, cmd).await.map_err(|e| crate::api::unavailable(&e))
}

/// GitHub's level name for an `Access`.
fn level(a: Access) -> &'static str {
    match a {
        Access::Read => "read",
        Access::Write => "write",
        Access::Admin => "admin",
    }
}

fn permissions_json(p: &PermissionSet) -> Value {
    Value::Object(p.0.iter().map(|(k, a)| (k.to_string(), json!(level(*a)))).collect())
}

/// The request body's three narrowing fields, or GitHub's 422 for a malformed one.
fn token_request(body: &Value) -> std::result::Result<AccessTokenRequest, Reply> {
    let bad = |m: &str| Reply::error(422, m, "rest/apps/apps#create-an-installation-access-token-for-an-app");
    let mut req = AccessTokenRequest::default();
    if let Some(r) = body.get("repositories") {
        let names = r.as_array().ok_or_else(|| bad("repositories must be an array of names"))?;
        for n in names {
            req.repositories.push(n.as_str().ok_or_else(|| bad("repositories must be names"))?.into());
        }
    }
    if let Some(r) = body.get("repository_ids") {
        let ids = r.as_array().ok_or_else(|| bad("repository_ids must be an array of ids"))?;
        for i in ids {
            req.repository_ids.push(RepoId(DbId(i.as_i64().ok_or_else(|| bad("repository_ids must be ids"))?)));
        }
    }
    if let Some(p) = body.get("permissions") {
        let m = p.as_object().ok_or_else(|| bad("permissions must be an object"))?;
        let mut set = PermissionSet::default();
        for (k, v) in m {
            let a = match v.as_str() {
                Some("read") => Access::Read,
                Some("write") => Access::Write,
                Some("admin") => Access::Admin,
                _ => return Err(bad(&format!("permission {k} has no level"))),
            };
            set.0.insert(k.as_str().into(), a);
        }
        req.permissions = Some(set);
    }
    Ok(req)
}

/// The App's owner and record from the owner's `Authz` fold, reached through `NameKey::App` on `Names`.
async fn app_record(c: &Ctx<'_>, app: AppId) -> std::result::Result<Option<(AccountId, AppRecord)>, Reply> {
    let unavailable = |e: String| crate::api::unavailable(&e);
    let v = system_read(c, ChainId::Names(name_shard(&NameKey::App(app))), json!({ "app": app.0.0 }))
        .await
        .map_err(unavailable)?;
    let Some(owner) = v["app"].as_i64().map(|o| AccountId(DbId(o))) else { return Ok(None) };
    let a = authz_of(c, owner).await.map_err(unavailable)?;
    Ok(a.and_then(|a| a.app(app).cloned()).map(|r| (owner, r)))
}

/// `GET` and `PATCH /app/hook/config` (`design/SHARED.md`, "Identity and tokens"): the App JWT verified against the
/// record's keys, read fresh from the App owner's `Authz` fold; a new secret wrapped by `graft-ops` as a repository
/// hook's is; then `AuthzOp::SetAppHook` on that chain under the operator's credential, since the JWT is the check.
/// Before `FormatsEnable` enabled 0x12.13 both answer the 404 they did before.
pub async fn app_hook_config(c: &Ctx<'_>, req: &mut Request, method: Method) -> Result<Reply> {
    if !c.cfg.enabled.contains(&graftyard_identity::AUTHZ_FORMAT_V13) {
        return Ok(Reply::not_found());
    }
    let doc = "rest/apps/webhooks#update-a-webhook-configuration-for-an-app";
    let Some(jwt) = crate::token_of(req)? else { return Ok(Reply::error(401, "Requires authentication", doc)) };
    let jwt_401 =
        |e: &dyn std::fmt::Display| Reply::error(401, &format!("A JSON web token could not be decoded ({e})"), doc);
    let Some(app) = graftyard_identity::appauth::claimed_app(&jwt) else { return Ok(jwt_401(&"no App in iss")) };
    let (owner, rec) = match app_record(c, app).await {
        Ok(Some(x)) => x,
        Ok(None) => return Ok(jwt_401(&"no such App")),
        Err(x) => return Ok(x),
    };
    if let Err(e) = graftyard_identity::appauth::verify_app_jwt(&jwt, &rec, now()) {
        return Ok(jwt_401(&e));
    }
    if method == Method::Get {
        return Ok(Reply::json(200, graftyard_api::webhooks::app_hook_config(&rec)));
    }
    let body: Value = serde_json::from_slice(&req.bytes().await?).unwrap_or(Value::Null);
    let (webhook, secret) = match graftyard_api::webhooks::app_hook_body(&rec, &body) {
        Ok(x) => x,
        Err(r) => return Ok(r),
    };
    let secret = match secret {
        Some(Some(s)) => match crate::hooks::wrap(c, &s).await {
            Ok(w) => Some(graftyard_api::webhooks::app_secret_bytes(&w)),
            Err(r) => return Ok(r),
        },
        Some(None) => None,
        None => rec.webhook_secret.clone(),
    };
    let op = graftyard_identity::AuthzOp::SetAppHook { app, webhook: webhook.clone(), secret: secret.clone() };
    let cmd = json!({ "cred": graftyard_types::CredentialShard::NONE, "op": op });
    let done = match commit_owner(c, ChainId::Authz(owner), owner, cmd).await {
        Ok(d) => d,
        Err(x) => return Ok(x),
    };
    if done.get("ok").is_none() {
        return Ok(crate::api::unavailable(&format!("the webhook configuration did not commit: {done}")));
    }
    let set = AppRecord { webhook, webhook_secret: secret, ..rec };
    Ok(Reply::json(200, graftyard_api::webhooks::app_hook_config(&set)))
}

/// `POST /app/installations/{id}/access_tokens` (BF-32): the App JWT verified against `AppRecord.keys`, the
/// installation found through `NameKey::Installation` and read fresh on its owner's `Authz` chain, the request
/// narrowed (`access_token_grant`), and the token minted by `graft-ops` over the `OPS` binding.
pub async fn access_tokens(c: &Ctx<'_>, req: &mut Request, id: &str) -> Result<Reply> {
    let doc = "rest/apps/apps#create-an-installation-access-token-for-an-app";
    let Some(jwt) = crate::token_of(req)? else { return Ok(bad_credentials()) };
    let Ok(id) = id.parse::<i64>().map(|i| InstallationId(DbId(i))) else { return Ok(Reply::not_found()) };
    let body: Value = req.json().await.unwrap_or(Value::Null);
    let r = match token_request(&body) {
        Ok(r) => r,
        Err(x) => return Ok(x),
    };
    let route =
        system_read(c, ChainId::Names(name_shard(&NameKey::Installation(id))), json!({ "installation": id.0.0 }))
            .await
            .map_err(Error::RustError)?;
    let Some((app, owner)) = serde_json::from_value::<Option<(AppId, AccountId)>>(route["installation"].clone())?
    else {
        return Ok(Reply::error(404, "Not Found", doc));
    };
    let (rec, inst) = futures_util::future::join(app_record(c, app), authz_of(c, owner)).await;
    let rec = match rec {
        Ok(Some((_, r))) => r,
        Ok(None) => return Ok(Reply::error(404, "Not Found", doc)),
        Err(x) => return Ok(x),
    };
    let inst = match inst {
        Ok(Some(a)) => a,
        Ok(None) => return Ok(Reply::error(404, "Not Found", doc)),
        Err(e) => return Ok(crate::api::unavailable(&e)),
    };
    let resolve = |n: &str| RepoName::parse(n).ok().and_then(|n| inst.resolve(&n.key())).map(|(r, _)| r);
    let mut grant = match stateless::access_token_grant(&jwt, &rec, id, &inst, &r, &resolve, now()) {
        Ok(g) => g,
        Err(TokenRequestError::Jwt(e)) => {
            return Ok(Reply::error(401, &format!("A JSON web token could not be decoded ({e})"), doc));
        }
        Err(TokenRequestError::NoInstallation) => return Ok(Reply::error(404, "Not Found", doc)),
        Err(TokenRequestError::Narrow(e)) => return Ok(Reply::error(422, &e.to_string(), doc)),
    };
    // AM1-WP3's additive `graftyard_task`: checked against the task's row on its repository, read fresh, once
    // `FormatsEnable` enabled 32.12, the format a command under such a token takes. A request without it is unchanged.
    if let Some(t) = body.get("graftyard_task") {
        match crate::api::task_claim(c, &inst, &grant, owner, t).await? {
            Ok(s) => grant.task = Some(s),
            Err(x) => return Ok(x),
        }
    }
    mint(c, &grant).await
}

/// Asks `graft-ops` to sign `grant`, and renders GitHub's `installation-token`.
async fn mint(c: &Ctx<'_>, grant: &Grant) -> Result<Reply> {
    let mut init = RequestInit::new();
    init.with_method(Method::Post).with_body(Some(serde_json::to_string(grant)?.into()));
    let mut res = c.ops("https://graft-ops/mint", init).await?;
    if res.status_code() != 200 {
        return Ok(crate::api::unavailable(&format!("graft-ops did not mint ({})", res.status_code())));
    }
    let v: Value = res.json().await?;
    let exp = graftyard_types::Timestamp(v["expires_at"].as_u64().unwrap_or(0));
    let selection = match grant.repos {
        RepoScope::All => "all",
        RepoScope::Selected(_) => "selected",
    };
    Ok(Reply::json(
        201,
        json!({
            "token": v["token"],
            "expires_at": graftyard_wire::iso(exp),
            "permissions": permissions_json(&grant.permissions),
            "repository_selection": selection,
        }),
    ))
}

/// `DELETE /installation/token`: the token revokes itself with `RevokeJti` on its `jti_shard` (journey 4, step 1),
/// and the 204 follows its quorum. A token that does not verify is 401.
pub async fn delete_installation_token(c: &Ctx<'_>, req: &Request) -> Result<Reply> {
    if let Err(x) = enabled(c, graftyard_identity::TOKENS_FORMAT_V2) {
        return Ok(x);
    }
    let Some(text) = crate::token_of(req)? else { return Ok(bad_credentials()) };
    let keys: Option<TokensFold> = dep_fold(c, ChainId::Tokens(TokenShard(0))).await.map_err(Error::RustError)?;
    let Some(keys) = keys else { return Ok(bad_credentials()) };
    let claims: InstallationClaims = match stateless::verify(&text, &keys, now()) {
        Ok(c) => c,
        Err(_) => return Ok(bad_credentials()),
    };
    let cmd = stateless::revoke_command(&claims);
    let chain = ChainId::Tokens(graftyard_identity::jti_shard(&claims.jti));
    let done = match commit_system(c, chain, serde_json::to_value(&cmd)?).await {
        Ok(d) => d,
        Err(x) => return Ok(x),
    };
    Ok(match done.get("refused").and_then(|r| r["status"].as_u64()) {
        None if done.get("ok").is_some() => Reply::empty(204),
        Some(401) => bad_credentials(),
        Some(403) => Reply::error(403, "Forbidden", "rest"),
        _ => crate::api::unavailable(&format!("the revocation did not commit: {done}")),
    })
}

// ---- OAuth: the device flow and the web flow (`design/SHARED.md`, "Identity and tokens", OAuth) ----

fn rand_u64() -> u64 {
    let b = crate::rand10();
    u64::from_le_bytes(b[..8].try_into().expect("8"))
}

/// The client an OAuth request names: a first-party client (public, its secret ignored) or an `OAuthAppRecord` or
/// `AppRecord` found through its client id's `Names` route, with the secret hashes to check.
enum Client {
    FirstParty,
    Registered { secrets: Vec<graftyard_identity::SecretHash>, device_flow: bool, app: Option<AppId> },
}

async fn client_of(
    c: &Ctx<'_>,
    id: &str,
) -> std::result::Result<Option<(graftyard_identity::ClientId, Client)>, String> {
    let Ok(client) = graftyard_identity::ClientId::parse(id) else { return Ok(None) };
    if graftyard_wire::FirstPartyClient::by_client_id(id).is_some() {
        return Ok(Some((client, Client::FirstParty)));
    }
    let v = system_read(c, ChainId::Names(name_shard(&NameKey::Client(client))), json!({ "client": id })).await?;
    let Some(route) = serde_json::from_value::<Option<graftyard_identity::ClientRoute>>(v["client"].clone())
        .map_err(|e| e.to_string())?
    else {
        return Ok(None);
    };
    let Some(a) = authz_of(c, route.owner).await? else { return Ok(None) };
    Ok(match route.app {
        Some(app) => a.app(app).map(|r| Client::Registered {
            secrets: r.client_secrets.to_vec(),
            device_flow: true,
            app: Some(app),
        }),
        None => a.oauth_app(&client).map(|r| Client::Registered {
            secrets: r.client_secrets.to_vec(),
            device_flow: r.device_flow,
            app: None,
        }),
    }
    .map(|k| (client, k)))
}

/// An OAuth endpoint's form or JSON body as one map.
async fn form(req: &mut Request) -> Result<std::collections::BTreeMap<String, String>> {
    let ct = req.headers().get("content-type")?.unwrap_or_default();
    let body = req.text().await?;
    let mut out = std::collections::BTreeMap::new();
    if ct.contains("json") {
        if let Ok(Value::Object(m)) = serde_json::from_str::<Value>(&body) {
            for (k, v) in m {
                out.insert(k, v.as_str().map(str::to_owned).unwrap_or_else(|| v.to_string()));
            }
        }
    } else {
        for (k, v) in url::form_urlencoded::parse(body.as_bytes()) {
            out.insert(k.into_owned(), v.into_owned());
        }
    }
    if let Ok(u) = req.url() {
        for (k, v) in u.query_pairs() {
            out.entry(k.into_owned()).or_insert_with(|| v.into_owned());
        }
    }
    Ok(out)
}

/// An OAuth answer, as JSON when the client asked for it and form-encoded otherwise, as GitHub does.
fn oauth_reply(json_wanted: bool, status: u16, v: Value) -> Reply {
    if json_wanted {
        return Reply::json(status, v);
    }
    let mut s = url::form_urlencoded::Serializer::new(String::new());
    for (k, x) in v.as_object().into_iter().flatten() {
        s.append_pair(k, &x.as_str().map(str::to_owned).unwrap_or_else(|| x.to_string()));
    }
    Reply::bytes(status, "application/x-www-form-urlencoded; charset=utf-8", s.finish().into_bytes())
}

fn oauth_error(json_wanted: bool, code: &str, description: &str) -> Reply {
    oauth_reply(
        json_wanted,
        200,
        json!({ "error": code, "error_description": description, "error_uri": format!("https://docs.github.com/developers/apps/troubleshooting-oauth-app-access-token-request-errors#{}", code.replace('_', "-")) }),
    )
}

/// A user code as GitHub prints it: 8 characters from an alphabet without look-alikes, `XXXX-XXXX`.
fn user_code() -> String {
    const A: &[u8] = b"BCDFGHJKLMNPQRSTVWXZ";
    let mut s = String::with_capacity(9);
    let mut r = rand_u64();
    for i in 0..8 {
        if i == 4 {
            s.push('-');
        }
        s.push(A[(r % A.len() as u64) as usize] as char);
        r /= A.len() as u64;
    }
    s
}

fn code_shard(code: &[u8]) -> ChainId {
    ChainId::Tokens(graftyard_identity::shard_of_hash(&graftyard_identity::code_hash(code)))
}

/// The refusal variant of a committed outcome, if refused.
pub(crate) fn refused(done: &Value) -> Option<String> {
    let e = done.get("refused")?.get("err")?.as_str()?;
    Some(e.split(['(', ' ', '{']).next().unwrap_or(e).to_owned())
}

/// `POST /login/device/code`: a device code and a user code, both rows in one entry on the user code's shard.
pub async fn device_code(c: &Ctx<'_>, req: &mut Request) -> Result<Reply> {
    let json_wanted = req.headers().get("accept")?.is_some_and(|a| a.contains("json"));
    if let Err(x) = enabled(c, graftyard_identity::TOKENS_FORMAT_V2) {
        return Ok(x);
    }
    let f = form(req).await?;
    let id = f.get("client_id").cloned().unwrap_or_default();
    let (client, k) = match client_of(c, &id).await.map_err(Error::RustError)? {
        Some(x) => x,
        None => return Ok(oauth_error(json_wanted, "unauthorized_client", "The client_id is not valid.")),
    };
    if let Client::Registered { device_flow: false, .. } = k {
        return Ok(oauth_error(json_wanted, "device_flow_disabled", "Device flow must be enabled for this App."));
    }
    let user = user_code();
    let Some((device, _)) = graftyard_identity::draw_device_code(rand_u64, user.as_bytes(), 20_000) else {
        return Ok(crate::api::unavailable("no device code landed on the user code's shard"));
    };
    let interval: u8 = 5;
    let cmd = json!({ "cred": { "account": null, "key": null }, "op": { "PutDeviceCode": {
        "device": graftyard_identity::code_hash(&device), "user": graftyard_identity::code_hash(user.as_bytes()),
        "client": client, "scopes": scopes_of(f.get("scope")).0, "interval": interval } } });
    let done = match commit_system(c, code_shard(user.as_bytes()), cmd).await {
        Ok(d) => d,
        Err(x) => return Ok(x),
    };
    if done.get("ok").is_none() {
        return Ok(crate::api::unavailable(&format!("the device code did not commit: {done}")));
    }
    Ok(oauth_reply(
        json_wanted,
        200,
        json!({
            "device_code": String::from_utf8(device).unwrap_or_default(),
            "user_code": user,
            "verification_uri": format!("https://{}/login/device", c.cfg.host),
            "expires_in": graftyard_identity::DEVICE_CODE_TTL.as_secs(),
            "interval": interval,
        }),
    ))
}

/// The classic scope bits [inference: Milestone 1 stores scopes without naming bits]: `repo` is bit 0, so a token
/// from the device flow carries what a `ghp_` from the seed does.
fn scopes_of(s: Option<&String>) -> graftyard_identity::ScopeSet {
    let _ = s;
    graftyard_identity::ScopeSet(1)
}

/// `POST /login/device` with `user_code` (test installs' stand-in for the browser page, which is not built): the
/// signed-in caller approves the code for itself. The caller authenticates with a stored token.
pub async fn device_approve(c: &Ctx<'_>, req: &mut Request) -> Result<Reply> {
    let Some(who) = crate::authenticate(c, req).await? else { return Ok(bad_credentials()) };
    let graftyard_identity::Actor::User(me) = who.actor else { return Ok(bad_credentials()) };
    let f = form(req).await?;
    let code = f.get("user_code").cloned().unwrap_or_default().to_ascii_uppercase();
    let cmd = json!({ "cred": who.credential_shard(), "op": { "ApproveCode": {
        "user": graftyard_identity::code_hash(code.as_bytes()), "account": me.0 } } });
    let done = match commit_system(c, code_shard(code.as_bytes()), cmd).await {
        Ok(d) => d,
        Err(x) => return Ok(x),
    };
    Ok(match refused(&done) {
        None => Reply::json(200, json!({ "approved": true })),
        Some(e) => Reply::json(422, json!({ "message": e })),
    })
}

/// A new stored token for `account` with `kind`, drawn onto the account's family's shard, committed after the code's
/// `CodeUsed`; then the `Identity` side (`crate::edge::after_credential`). Returns the token text.
async fn issue(
    c: &Ctx<'_>,
    account: AccountId,
    kind: graftyard_identity::TokenKind,
    client: graftyard_identity::ClientId,
    title: &str,
    kill_on: Option<&str>,
) -> std::result::Result<String, Reply> {
    let fam = graftyard_identity::Family::Na;
    let (secret, _) =
        graftyard_identity::draw_secret(rand_u64, fam).map_err(|e| crate::api::unavailable(&e.to_string()))?;
    let tok =
        graftyard_identity::OpaqueToken::new(kind, secret).ok_or_else(|| crate::api::unavailable("token kind"))?;
    let fields = graftyard_identity::TokenFields { client: Some(client), ..Default::default() };
    let cmd = json!({ "cred": { "account": null, "key": null }, "op": { "StoreWith": {
        "hash": tok.hash(), "kind": kind, "account": account, "scopes": 1, "expires": null, "fields": fields } } });
    let done = commit_killable(c, ChainId::Tokens(tok.shard()), cmd, "token", kill_on).await?;
    if done.get("ok").is_none() {
        return Err(crate::api::unavailable(&format!("the token did not commit: {done}")));
    }
    let cred = graftyard_identity::CredentialRef::Token { hash: tok.hash(), kind };
    list_credential(c, account, cred, title).await;
    Ok(tok.text())
}

/// The `Identity` side of a new credential, right after its `Tokens` commit; `Audit` finishes it if this edge dies
/// first. A failure here is not the client's: the credential is live either way.
pub async fn list_credential(c: &Ctx<'_>, account: AccountId, cred: graftyard_identity::CredentialRef, title: &str) {
    let graftyard_identity::finish::Step::Identity(chain, cmd) =
        graftyard_identity::edge::after_credential(account, graftyard_types::CredentialShard::NONE, cred, title)
    else {
        unreachable!("an Identity step")
    };
    if c.cfg.enabled.contains(&graftyard_identity::IDENTITY_FORMAT)
        && let Ok(v) = serde_json::to_value(&cmd)
    {
        let _ = commit_owner(c, chain, account, v).await;
    }
}

/// `commit_system`, but on a test install the first send may carry this request's `x-graft-kill` when
/// `x-graft-kill-on` names `step` (`code`: the exchange's `UseCode`; `token`: its new token's `StoreWith`), so BF-33
/// kills the leader of that shard while it decides the exchange's own command.
async fn commit_killable(
    c: &Ctx<'_>,
    chain: ChainId,
    cmd: Value,
    step: &str,
    on: Option<&str>,
) -> std::result::Result<Value, Reply> {
    #[cfg(not(feature = "test-faults"))]
    let _ = (step, on);
    #[cfg(feature = "test-faults")]
    if let (Some(k), true) = (c.kill.as_deref(), on == Some(step)) {
        let record = c.system_record(chain).await.map_err(|e| crate::api::unavailable(&e))?;
        let b = c.bindings();
        if let Err(e) = graftyard_lookup::create(&b, chain, &record, &json!({})).await {
            return Err(crate::api::unavailable(&e));
        }
        let body = json!({ "rid": c.rid.to_string(), "cmd": cmd });
        let mut first = body.clone();
        first["kill"] = json!(k);
        first["down_ms"] = json!(c.down_ms);
        let (bytes, first) = (body.to_string().into_bytes(), first.to_string().into_bytes());
        let trace = std::cell::RefCell::new(Vec::new());
        let hooks = graftyard_lookup::Hooks { first: Some(&first), trace: Some(&trace), lookup: None };
        let r = graftyard_lookup::commit_hedged(&b, chain, &record, &bytes, c.colo, 60_000, hooks).await;
        // The hedge's sends and misses, in the reply's `x-graft-trace` (BF-33 shows the kill fired).
        let mut t = trace.into_inner();
        t.insert(0, json!({ "killOn": step, "chain": chain.to_string() }));
        crate::api::trace(c, t);
        return r.map_err(|e| crate::api::unavailable(&format!("{e:?}")));
    }
    commit_system(c, chain, cmd).await
}

/// `POST /login/oauth/access_token`: a device code, an authorization code, or a refresh. The code is spent with
/// `UseCode` before the token is stored, so one code never yields two tokens.
pub async fn access_token(c: &Ctx<'_>, req: &mut Request) -> Result<Reply> {
    let json_wanted = req.headers().get("accept")?.is_some_and(|a| a.contains("json"));
    if let Err(x) = enabled(c, graftyard_identity::TOKENS_FORMAT_V2) {
        return Ok(x);
    }
    let kill_on = req.headers().get("x-graft-kill-on")?;
    let kill_on = kill_on.as_deref();
    let f = form(req).await?;
    let id = f.get("client_id").cloned().unwrap_or_default();
    let (client, k) = match client_of(c, &id).await.map_err(Error::RustError)? {
        Some(x) => x,
        None => {
            return Ok(oauth_error(
                json_wanted,
                "incorrect_client_credentials",
                "The client_id and/or client_secret passed are incorrect.",
            ));
        }
    };
    let device = f.get("grant_type").is_some_and(|g| g == "urn:ietf:params:oauth:grant-type:device_code");
    let refresh = f.get("grant_type").is_some_and(|g| g == "refresh_token");
    // A registered client's secret is checked on the web flow; public first-party clients' are ignored.
    if let Client::Registered { secrets, .. } = &k
        && !device
        && !graftyard_identity::appauth::client_secret_ok(secrets, f.get("client_secret").map_or("", String::as_str))
    {
        return Ok(Reply::json(
            401,
            json!({ "error": "incorrect_client_credentials", "error_description": "The client_id and/or client_secret passed are incorrect." }),
        ));
    }
    // BF-31: an App's `ghr_` for a new pair (`crate::usertok`), after the client secret checked above.
    if refresh {
        let Client::Registered { app: Some(app), .. } = k else {
            return Ok(oauth_error(json_wanted, "unsupported_grant_type", "Only a GitHub App's tokens refresh."));
        };
        let text = f.get("refresh_token").cloned().unwrap_or_default();
        return Ok(match crate::usertok::refresh(c, client, app, &text).await {
            Ok(v) => oauth_reply(json_wanted, 200, v),
            Err(x) => x,
        });
    }
    let code = match (device, f.get("device_code"), f.get("code")) {
        (true, Some(d), _) => d.clone(),
        (false, _, Some(c)) => c.clone(),
        _ => return Ok(oauth_error(json_wanted, "bad_verification_code", "The code passed is incorrect or expired.")),
    };
    let hash = graftyard_identity::code_hash(code.as_bytes());
    let chain = code_shard(code.as_bytes());
    // Who the code is for, read before it is spent: the device code's user code row, or the authorization code.
    let v = system_read(c, chain, json!({ "code": graftyard_lookup::hex(&hash) })).await.map_err(Error::RustError)?;
    let row: Option<graftyard_identity::CodeRow> = serde_json::from_value(v["code"].clone())?;
    let Some(row) = row.filter(|r| r.expires > now()) else {
        let e = if device { "expired_token" } else { "bad_verification_code" };
        return Ok(oauth_error(json_wanted, e, "The code passed is incorrect or expired."));
    };
    let account = match &row.kind {
        graftyard_identity::CodeKind::Device { user_code, .. } => {
            let u = system_read(c, chain, json!({ "code": graftyard_lookup::hex(user_code) }))
                .await
                .map_err(Error::RustError)?;
            serde_json::from_value::<Option<graftyard_identity::CodeRow>>(u["code"].clone())?.and_then(|r| r.approved)
        }
        _ => row.account,
    };
    let cmd =
        json!({ "cred": { "account": null, "key": null }, "op": { "UseCode": { "hash": hash, "client": client } } });
    let done = match commit_killable(c, chain, cmd, "code", kill_on).await {
        Ok(d) => d,
        Err(x) => return Ok(x),
    };
    match refused(&done).as_deref() {
        None => {}
        Some("Pending") => {
            return Ok(oauth_error(
                json_wanted,
                "authorization_pending",
                "The authorization request is still pending.",
            ));
        }
        Some("CodeUsed") | Some("NoCode") | Some("WrongCode") => {
            let e = if device { "expired_token" } else { "bad_verification_code" };
            return Ok(oauth_error(json_wanted, e, "The code passed is incorrect or expired."));
        }
        Some(_) => return Ok(crate::api::unavailable(&format!("the code did not commit: {done}"))),
    }
    let Some(account) = account else {
        return Ok(oauth_error(json_wanted, "authorization_pending", "The authorization request is still pending."));
    };
    // An App's code yields a `ghu_` and its `ghr_` (`crate::usertok`); an OAuth app's, a `gho_`.
    if let Client::Registered { app: Some(app), .. } = k {
        return Ok(match crate::usertok::issue_pair(c, account, app, client).await {
            Ok(v) => oauth_reply(json_wanted, 200, v),
            Err(x) => x,
        });
    }
    let kind = graftyard_identity::TokenKind::OAuth;
    match issue(c, account, kind, client, &format!("OAuth {}", client.as_str()), kill_on).await {
        Ok(token) => {
            Ok(oauth_reply(json_wanted, 200, json!({ "access_token": token, "token_type": "bearer", "scope": "repo" })))
        }
        Err(x) => Ok(x),
    }
}

/// `GET /login/oauth/authorize` stands in for the browser's consent page on a test install: the caller, signed in by
/// a stored token, authorizes `client_id`, and is sent to `redirect_uri` with a code and the `state` it gave. The
/// code is an `Authorization` row on its hash's shard for 10 minutes.
pub async fn authorize(c: &Ctx<'_>, req: &mut Request) -> Result<Reply> {
    if let Err(x) = enabled(c, graftyard_identity::TOKENS_FORMAT_V2) {
        return Ok(x);
    }
    let Some(who) = crate::authenticate(c, req).await? else { return Ok(bad_credentials()) };
    let graftyard_identity::Actor::User(me) = who.actor else { return Ok(bad_credentials()) };
    let f = form(req).await?;
    let id = f.get("client_id").cloned().unwrap_or_default();
    let Some((client, _)) = client_of(c, &id).await.map_err(Error::RustError)? else { return Ok(Reply::not_found()) };
    let redirect = f.get("redirect_uri").cloned().unwrap_or_else(|| "http://localhost/".into());
    let code = String::from_utf8(
        graftyard_identity::draw_secret(rand_u64, graftyard_identity::Family::Na)
            .map_err(|e| e.to_string())?
            .0
            .to_vec(),
    )
    .unwrap_or_default();
    let hash = graftyard_identity::code_hash(code.as_bytes());
    let cmd = json!({ "cred": who.credential_shard(), "op": { "PutCode": { "hash": hash,
        "kind": { "Authorization": { "redirect": redirect, "pkce": null } }, "client": client, "scopes": 1, "account": me.0 } } });
    let done = match commit_system(c, code_shard(code.as_bytes()), cmd).await {
        Ok(d) => d,
        Err(x) => return Ok(x),
    };
    if let Some(e) = refused(&done) {
        return Ok(Reply::json(422, json!({ "message": e })));
    }
    let mut to = url::Url::parse(&redirect).map_err(|e| e.to_string())?;
    to.query_pairs_mut().append_pair("code", &code);
    if let Some(s) = f.get("state") {
        to.query_pairs_mut().append_pair("state", s);
    }
    Ok(Reply::empty(302).with("location", to.to_string()))
}

/// `POST /login/oauth/link` with `link`, and `install` naming the install account: spends a `graftyard admin
/// bootstrap` sign-in link once (`UseCode`, no client) and starts a `SignInMethod::Link` session on the account's
/// `Identity` chain (`crate::edge::sign_in`), which every `InstanceAuth` admits. Answers the session id; the browser
/// cookie and the page after it are not built [inference: DESIGN names the link, not what it opens].
pub async fn sign_in_link(c: &Ctx<'_>, req: &mut Request) -> Result<Reply> {
    if let Err(x) = enabled(c, graftyard_identity::IDENTITY_FORMAT) {
        return Ok(x);
    }
    let f = form(req).await?;
    let link = f.get("link").cloned().unwrap_or_default();
    let Some(install) = f.get("install").and_then(|i| i.parse::<i64>().ok()).map(|i| AccountId(DbId(i))) else {
        return Ok(Reply::json(422, json!({ "message": "install names the install account's id" })));
    };
    let hash = graftyard_identity::code_hash(link.as_bytes());
    let chain = code_shard(link.as_bytes());
    let v = system_read(c, chain, json!({ "code": graftyard_lookup::hex(&hash) })).await.map_err(Error::RustError)?;
    let row: Option<graftyard_identity::CodeRow> = serde_json::from_value(v["code"].clone())?;
    let Some((account, graftyard_identity::CodeKind::SignInLink)) =
        row.filter(|r| r.expires > now()).and_then(|r| Some((r.account?, r.kind)))
    else {
        return Ok(bad_credentials());
    };
    let spend =
        json!({ "cred": { "account": null, "key": null }, "op": { "UseCode": { "hash": hash, "client": null } } });
    let done = match commit_system(c, chain, spend).await {
        Ok(d) => d,
        Err(x) => return Ok(x),
    };
    match refused(&done).as_deref() {
        None => {}
        Some("CodeUsed") | Some("NoCode") | Some("WrongCode") => return Ok(bad_credentials()),
        Some(_) => return Ok(crate::api::unavailable(&format!("the link did not commit: {done}"))),
    }
    let id = graftyard_identity::WebSessionId(graftyard_types::Ulid::new(now(), crate::rand10()));
    let graftyard_identity::finish::Step::Identity(chain, cmd) =
        graftyard_identity::edge::sign_in(account, id, graftyard_identity::SignInMethod::Link, install)
    else {
        unreachable!("an Identity step")
    };
    let done = commit_owner(c, chain, account, serde_json::to_value(&cmd)?).await;
    Ok(match done {
        Ok(d) if refused(&d).is_none() => {
            Reply::json(201, json!({ "account": account.0.0, "session": id.0.to_string(), "method": "Link" }))
        }
        Ok(d) => Reply::json(403, json!({ "message": format!("the sign-in was refused: {}", d["refused"]["err"]) })),
        Err(x) => x,
    })
}

/// `DELETE /applications/{client_id}/token`: revokes the stored token in the body, after the client's secret, sent
/// as Basic auth, checks; a first-party client's embedded secret is ignored, so the holder of a token from `gh` may
/// revoke it. 204 after the revocation's quorum; a token already revoked is 204 too.
pub async fn revoke_app_token(c: &Ctx<'_>, req: &mut Request, client_id: &str) -> Result<Reply> {
    let doc = "rest/apps/oauth-applications#delete-an-app-token";
    let basic = req.headers().get("authorization")?.unwrap_or_default();
    let secret = basic
        .strip_prefix("Basic ")
        .and_then(crate::b64_decode)
        .and_then(|b| String::from_utf8(b).ok())
        .and_then(|s| s.split_once(':').filter(|(u, _)| *u == client_id).map(|(_, p)| p.to_owned()));
    let Some(secret) = secret else { return Ok(Reply::error(404, "Not Found", doc)) };
    let (_, k) = match client_of(c, client_id).await.map_err(Error::RustError)? {
        Some(x) => x,
        None => return Ok(Reply::error(404, "Not Found", doc)),
    };
    if let Client::Registered { secrets, .. } = &k
        && !graftyard_identity::appauth::client_secret_ok(secrets, &secret)
    {
        return Ok(Reply::error(404, "Not Found", doc));
    }
    let body: Value = req.json().await.unwrap_or(Value::Null);
    let Some(tok) = body["access_token"].as_str().and_then(|t| graftyard_identity::OpaqueToken::parse(t).ok()) else {
        return Ok(Reply::error(422, "Validation Failed", doc));
    };
    let cmd = json!({ "cred": { "account": null, "key": null }, "op": { "Revoke": { "hash": tok.hash() } } });
    let done = match commit_system(c, ChainId::Tokens(tok.shard()), cmd).await {
        Ok(d) => d,
        Err(x) => return Ok(x),
    };
    Ok(match refused(&done).as_deref() {
        None | Some("AlreadyRevoked") => Reply::empty(204),
        Some("NoToken") => Reply::error(404, "Not Found", doc),
        Some(e) => crate::api::unavailable(&format!("the revocation did not commit: {e}")),
    })
}

// ---- users and org membership (journey 4) ----

/// An account's login, from its `AccountLogin` row (F50).
pub async fn login(c: &Ctx<'_>, a: AccountId) -> Option<String> {
    login_of(c, a).await
}

async fn login_of(c: &Ctx<'_>, a: AccountId) -> Option<String> {
    let v = system_read(c, ChainId::Names(graftyard_identity::account_shard(a)), json!({ "accountLogins": [a.0.0] }))
        .await
        .ok()?;
    v["accountLogins"][a.0.0.to_string()].as_str().map(str::to_owned)
}

/// A login's account and kind, from its login row.
pub(crate) async fn account_of(
    c: &Ctx<'_>,
    login: &str,
) -> std::result::Result<Option<(AccountId, graftyard_identity::AccountKind)>, String> {
    let Ok(l) = graftyard_identity::Login::parse(login) else { return Ok(None) };
    let v = system_read(c, ChainId::Names(name_shard(&NameKey::Login(l.key()))), json!({ "login": login })).await?;
    Ok(match serde_json::from_value::<Option<graftyard_identity::LoginRow>>(v["login"].clone()) {
        Ok(Some(graftyard_identity::LoginRow::Claimed { account, kind })) => Some((account, kind)),
        _ => None,
    })
}

/// GitHub's `public-user` for an account [inference: profile fields this install does not hold are `null`, and the
/// counters 0].
pub(crate) fn user_json(c: &Ctx<'_>, id: AccountId, login: &str, org: bool) -> Value {
    let (api, web) = (format!("https://{}/api/v3", c.cfg.host), format!("https://{}", c.cfg.host));
    let ty = if org { graftyard_wire::NodeType::Organization } else { graftyard_wire::NodeType::User };
    json!({
        "login": login, "id": id.0.0, "node_id": graftyard_wire::NodeId::top(ty, id.0.0).encode(),
        "avatar_url": format!("{web}/identicons/{login}.png"), "gravatar_id": "",
        "url": format!("{api}/users/{login}"), "html_url": format!("{web}/{login}"),
        "followers_url": format!("{api}/users/{login}/followers"),
        "following_url": format!("{api}/users/{login}/following{{/other_user}}"),
        "gists_url": format!("{api}/users/{login}/gists{{/gist_id}}"),
        "starred_url": format!("{api}/users/{login}/starred{{/owner}}{{/repo}}"),
        "subscriptions_url": format!("{api}/users/{login}/subscriptions"),
        "organizations_url": format!("{api}/users/{login}/orgs"),
        "repos_url": format!("{api}/users/{login}/repos"),
        "events_url": format!("{api}/users/{login}/events{{/privacy}}"),
        "received_events_url": format!("{api}/users/{login}/received_events"),
        "type": if org { "Organization" } else { "User" }, "site_admin": false,
        "name": null, "company": null, "blog": null, "location": null, "email": null, "hireable": null, "bio": null,
        "public_repos": 0, "public_gists": 0, "followers": 0, "following": 0,
        "created_at": "2026-11-01T00:00:00Z", "updated_at": "2026-11-01T00:00:00Z",
    })
}

/// `GET /user`: the caller, by its `AccountLogin` row. An installation token is 403, as on GitHub.
pub async fn get_user(c: &Ctx<'_>, req: &Request) -> Result<Reply> {
    let Some(who) = crate::authenticate(c, req).await? else {
        return Ok(Reply::error(401, "Requires authentication", "rest"));
    };
    let graftyard_identity::Actor::User(me) = who.actor else {
        return Ok(Reply::error(
            403,
            "Resource not accessible by integration",
            "rest/users/users#get-the-authenticated-user",
        ));
    };
    let Some(login) = login_of(c, me.0).await else { return Ok(Reply::not_found()) };
    Ok(Reply::json(200, user_json(c, me.0, &login, false)).with("x-oauth-scopes", "repo"))
}

/// `GET /users/{username}`.
pub async fn get_named_user(c: &Ctx<'_>, login: &str) -> Result<Reply> {
    match account_of(c, login).await.map_err(Error::RustError)? {
        Some((id, kind)) => {
            let shown = login_of(c, id).await.unwrap_or_else(|| login.to_owned());
            Ok(Reply::json(200, user_json(c, id, &shown, kind == graftyard_identity::AccountKind::Org)))
        }
        None => Ok(Reply::error(404, "Not Found", "rest/users/users#get-a-user")),
    }
}

/// The org and the user an `/orgs/{org}/...{username}` path names, with the caller, or the reply to send.
async fn org_and_user(
    c: &Ctx<'_>,
    req: &Request,
    org: &str,
    user: &str,
) -> Result<std::result::Result<(Principal, AccountId, AccountId, AuthzFold), Reply>> {
    let Some(who) = crate::authenticate(c, req).await? else {
        return Ok(Err(Reply::error(401, "Requires authentication", "rest")));
    };
    let (o, u) = futures_util::future::join(account_of(c, org), account_of(c, user)).await;
    let (Some((o, graftyard_identity::AccountKind::Org)), Some((u, _))) =
        (o.map_err(Error::RustError)?, u.map_err(Error::RustError)?)
    else {
        return Ok(Err(Reply::not_found()));
    };
    let Some(a) = authz_of(c, o).await.map_err(Error::RustError)? else { return Ok(Err(Reply::not_found())) };
    Ok(Ok((who, o, u, a)))
}

pub(crate) fn membership_json(
    c: &Ctx<'_>,
    org: &str,
    o: AccountId,
    user: &str,
    u: AccountId,
    role: graftyard_identity::OrgRole,
) -> Value {
    let api = format!("https://{}/api/v3", c.cfg.host);
    let mut org_json = user_json(c, o, org, true);
    let m = org_json.as_object_mut().expect("object");
    m.retain(|k, _| ["login", "id", "node_id", "url", "avatar_url"].contains(&k.as_str()));
    m.insert("repos_url".into(), json!(format!("{api}/orgs/{org}/repos")));
    m.insert("events_url".into(), json!(format!("{api}/orgs/{org}/events")));
    m.insert("hooks_url".into(), json!(format!("{api}/orgs/{org}/hooks")));
    m.insert("issues_url".into(), json!(format!("{api}/orgs/{org}/issues")));
    m.insert("members_url".into(), json!(format!("{api}/orgs/{org}/members{{/member}}")));
    m.insert("public_members_url".into(), json!(format!("{api}/orgs/{org}/public_members{{/member}}")));
    m.insert("description".into(), Value::Null);
    m.insert("url".into(), json!(format!("{api}/orgs/{org}")));
    let mut user_v = user_json(c, u, user, false);
    for k in [
        "name",
        "company",
        "blog",
        "location",
        "email",
        "hireable",
        "bio",
        "public_repos",
        "public_gists",
        "followers",
        "following",
        "created_at",
        "updated_at",
    ] {
        user_v.as_object_mut().expect("object").remove(k);
    }
    json!({
        "url": format!("{api}/orgs/{org}/memberships/{user}"), "state": "active",
        "role": if role == graftyard_identity::OrgRole::Admin { "admin" } else { "member" },
        "organization_url": format!("{api}/orgs/{org}"), "organization": org_json, "user": user_v,
    })
}

/// `PUT /orgs/{org}/memberships/{username}` adds a member, or changes the role, with `SetMember` on the org's
/// `Authz` chain [inference: GitHub sends an invitation to a non-member; an install's admin adds members directly,
/// as GHES does with `ghe-org-admin`]; `GET` reads it; `DELETE /orgs/{org}/members/{username}` and
/// `DELETE /orgs/{org}/memberships/{username}` remove it, journey 4's first step, and answer 204 after its quorum.
pub async fn membership(c: &Ctx<'_>, req: &mut Request, method: Method, org: &str, user: &str) -> Result<Reply> {
    let (who, o, u, a) = match org_and_user(c, req, org, user).await? {
        Ok(x) => x,
        Err(r) => return Ok(r),
    };
    let me = match who.actor {
        graftyard_identity::Actor::User(me) => Some(me),
        _ => None,
    };
    let uid = graftyard_identity::UserId(u);
    match method {
        Method::Get => match a.role(uid) {
            Some(r) if me.is_some_and(|m| m == uid || a.role(m).is_some()) => {
                Ok(Reply::json(200, membership_json(c, org, o, user, u, r)))
            }
            _ => Ok(Reply::error(404, "Not Found", "rest/orgs/members#get-organization-membership-for-a-user")),
        },
        Method::Put | Method::Delete => {
            let role = if method == Method::Delete {
                None
            } else {
                let body: Value = req.json().await.unwrap_or(Value::Null);
                Some(if body["role"].as_str() == Some("admin") {
                    graftyard_identity::OrgRole::Admin
                } else {
                    graftyard_identity::OrgRole::Member
                })
            };
            if method == Method::Delete && a.role(uid).is_none() {
                return Ok(Reply::empty(204));
            }
            let cmd = json!({ "cred": who.credential_shard(), "op": { "SetMember": { "user": u, "role": role } } });
            let done = match commit_owner(c, ChainId::Authz(o), o, cmd).await {
                Ok(d) => d,
                Err(x) => return Ok(x),
            };
            Ok(match (refused(&done).as_deref(), role) {
                (None, Some(r)) => Reply::json(200, membership_json(c, org, o, user, u, r)),
                (None, None) => Reply::empty(204),
                (Some("Forbidden"), _) => {
                    Reply::error(403, "You must be an admin to change memberships.", "rest/orgs/members")
                }
                (Some("Unauthorized"), _) => bad_credentials(),
                (Some(e), _) => crate::api::unavailable(&format!("the membership did not commit: {e}")),
            })
        }
        _ => Ok(Reply::not_found()),
    }
}
