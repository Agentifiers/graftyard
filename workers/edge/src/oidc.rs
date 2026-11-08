//! The OIDC sign-in route (`build/BUILD-PLAN.md`, M3-WP3, BF-36; `design/SHARED.md`, "Instance sign-in in
//! installs") and the site admin's `InstanceAuth` write.
//!
//! - `GET /login/oidc` reads the install's `InstanceAuth` from `Authz(Genesis.account)` through `fresh`. Only when it
//!   is `Oidc` does it send the browser to the issuer's `authorization_endpoint` (from its discovery document) with
//!   `response_type=code`, `scope=openid profile`, a `state`, the `nonce` derived from it, and a PKCE `S256`
//!   challenge; the `state` and the verifier ride in a cookie scoped to the callback, HttpOnly and Secure. Otherwise
//!   it answers 404, so a Builtin install offers no OIDC sign-in.
//! - `GET /login/oidc/callback` checks the `state` against the cookie, exchanges the code at the `token_endpoint`
//!   with the client secret and the verifier, verifies the `id_token` against the issuer's JWKS
//!   (`graftyard_identity::oidc::verify_id_token`), finds the account whose login is the token's
//!   `preferred_username`, and commits `IdentityOp::SignIn { method: Oidc }` on its `Identity` chain. That leader
//!   re-reads `InstanceAuth`, so a switch back to Builtin refuses the sign-in even if this edge read the old value.
//!   201 with the session id, as `POST /login/oauth/link` answers; the cookie and the page after it are not built.
//! - `PUT /api/graftyard/v1/admin/instance-auth` with `{"type": "builtin"}` or `{"type": "oidc", "issuer", "client",
//!   "secret"}` commits `SetInstanceAuth` under the caller's stored token; the leader admits only a site admin, an
//!   admin of the install's own org. `GET` reads it, with the secret left out.
//!
//! The client secret is stored as `OidcConfig.secret`, which SHARED calls ciphertext; this install stores the bytes
//! the admin sent, because no wrap key for it exists yet [inference] (see the results file's design notes).

use graftyard_identity::oidc::{nonce_for, pkce_challenge, verify_id_token};
use graftyard_identity::{
    AuthzRead, InstanceAuth, OidcConfig, SignInMethod, Url as IdUrl, WebSessionId, install_account,
};
use graftyard_types::{AccountId, ChainId};
use graftyard_wire::Reply;
use serde_json::{Value, json};
use worker::*;

use crate::Ctx;

pub const INSTANCE_AUTH_PATH: &str = "/api/graftyard/v1/admin/instance-auth";
const START: &str = "/login/oidc";
const CALLBACK: &str = "/login/oidc/callback";
const COOKIE: &str = "graftyard_oidc";

fn random_text() -> String {
    let mut s = String::new();
    for _ in 0..3 {
        for b in crate::rand10() {
            s.push_str(&format!("{b:02x}"));
        }
    }
    s
}

async fn instance_auth_now(c: &Ctx<'_>) -> std::result::Result<InstanceAuth, String> {
    Ok(crate::ident::authz_of(c, install_account()).await?.map(|a| a.instance_auth()).unwrap_or_default())
}

async fn get_json(url: &str) -> std::result::Result<Value, String> {
    let mut r =
        Fetch::Url(url.parse().map_err(|e: url::ParseError| e.to_string())?).send().await.map_err(|e| e.to_string())?;
    if r.status_code() != 200 {
        return Err(format!("{url}: {}", r.status_code()));
    }
    r.json().await.map_err(|e| e.to_string())
}

async fn discovery(issuer: &str) -> std::result::Result<Value, String> {
    get_json(&format!("{}/.well-known/openid-configuration", issuer.trim_end_matches('/'))).await
}

fn respond(c: &Ctx<'_>, r: Reply) -> Result<Option<Response>> {
    crate::agents::respond(c, r).map(Some)
}

fn cookie(req: &Request) -> Result<Option<(String, String)>> {
    let h = req.headers().get("cookie")?.unwrap_or_default();
    Ok(h.split(';').map(str::trim).find_map(|kv| kv.strip_prefix(&format!("{COOKIE}="))).and_then(|v| {
        let (s, v) = v.split_once('.')?;
        Some((s.to_owned(), v.to_owned()))
    }))
}

/// `GET /login/oidc` and `/login/oidc/callback`, or `None` for another path.
pub async fn route(c: &Ctx<'_>, req: &mut Request, path: &str) -> Result<Option<Response>> {
    if path != START && path != CALLBACK {
        return Ok(None);
    }
    let auth = match instance_auth_now(c).await {
        Ok(a) => a,
        Err(e) => return respond(c, crate::api::unavailable(&e)),
    };
    let InstanceAuth::Oidc(cfg) = auth else {
        return respond(c, Reply::error(404, "This install does not sign in with OIDC.", "graftyard/oidc"));
    };
    let issuer = cfg.issuer.0.as_ref();
    let disc = match discovery(issuer).await {
        Ok(d) => d,
        Err(e) => return respond(c, crate::api::unavailable(&format!("the OIDC issuer: {e}"))),
    };
    let redirect = format!("https://{}{CALLBACK}", c.cfg.host);
    if path == START {
        let (state, verifier) = (random_text(), random_text());
        let Some(ep) = disc["authorization_endpoint"].as_str() else {
            return respond(c, crate::api::unavailable("the issuer names no authorization_endpoint"));
        };
        let mut to = url::Url::parse(ep).map_err(|e| e.to_string())?;
        to.query_pairs_mut()
            .append_pair("response_type", "code")
            .append_pair("client_id", &cfg.client)
            .append_pair("redirect_uri", &redirect)
            .append_pair("scope", "openid profile")
            .append_pair("state", &state)
            .append_pair("nonce", &nonce_for(&state))
            .append_pair("code_challenge", &pkce_challenge(&verifier))
            .append_pair("code_challenge_method", "S256");
        let set = format!("{COOKIE}={state}.{verifier}; Path={CALLBACK}; Max-Age=600; HttpOnly; Secure; SameSite=Lax");
        let h = crate::headers(c.rid)?;
        h.set("location", to.as_str())?;
        h.set("set-cookie", &set)?;
        return Ok(Some(Response::empty()?.with_status(302).with_headers(h)));
    }
    // The callback.
    let url = req.url()?;
    let q = |k: &str| url.query_pairs().find(|(n, _)| n == k).map(|(_, v)| v.into_owned());
    let (Some(code), Some(state)) = (q("code"), q("state")) else {
        return respond(c, Reply::error(400, "code and state are required", "graftyard/oidc"));
    };
    let Some((want, verifier)) = cookie(req)?.filter(|(s, _)| *s == state) else {
        return respond(c, Reply::error(400, "the state does not match this browser's sign-in", "graftyard/oidc"));
    };
    let _ = want;
    let Some(token_ep) = disc["token_endpoint"].as_str() else {
        return respond(c, crate::api::unavailable("the issuer names no token_endpoint"));
    };
    let secret = String::from_utf8(cfg.secret.to_vec()).unwrap_or_default();
    let body = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("grant_type", "authorization_code")
        .append_pair("code", &code)
        .append_pair("redirect_uri", &redirect)
        .append_pair("client_id", &cfg.client)
        .append_pair("client_secret", &secret)
        .append_pair("code_verifier", &verifier)
        .finish();
    let h = Headers::new();
    h.set("content-type", "application/x-www-form-urlencoded")?;
    h.set("accept", "application/json")?;
    let mut init = RequestInit::new();
    init.with_method(Method::Post).with_headers(h).with_body(Some(body.into()));
    let mut r = Fetch::Request(Request::new_with_init(token_ep, &init)?).send().await?;
    let tok: Value = r.json().await.unwrap_or(Value::Null);
    let Some(id_token) = tok["id_token"].as_str() else {
        return respond(c, Reply::error(401, "the issuer returned no id_token", "graftyard/oidc"));
    };
    let Some(jwks_uri) = disc["jwks_uri"].as_str() else {
        return respond(c, crate::api::unavailable("the issuer names no jwks_uri"));
    };
    let jwks = match get_json(jwks_uri).await {
        Ok(j) => j.to_string().into_bytes(),
        Err(e) => return respond(c, crate::api::unavailable(&format!("the issuer's JWKS: {e}"))),
    };
    let ident = match verify_id_token(id_token, &jwks, issuer, &cfg.client, &nonce_for(&state), crate::now()) {
        Ok(i) => i,
        Err(e) => return respond(c, Reply::error(401, &format!("the id_token was refused: {e}"), "graftyard/oidc")),
    };
    let Some((account, graftyard_identity::AccountKind::User)) =
        crate::ident::account_of(c, &ident.username).await.map_err(Error::RustError)?
    else {
        return respond(c, Reply::error(403, "no account has this identity's login", "graftyard/oidc"));
    };
    respond(c, sign_in(c, account).await?)
}

async fn sign_in(c: &Ctx<'_>, account: AccountId) -> Result<Reply> {
    let id = WebSessionId(graftyard_types::Ulid::new(crate::now(), crate::rand10()));
    let graftyard_identity::finish::Step::Identity(chain, cmd) =
        graftyard_identity::edge::sign_in(account, id, SignInMethod::Oidc, install_account())
    else {
        unreachable!("an Identity step")
    };
    Ok(match crate::ident::commit_owner(c, chain, account, serde_json::to_value(&cmd)?).await {
        Ok(d) if d.get("ok").is_some() => {
            Reply::json(201, json!({ "account": account.0.0, "session": id.0.to_string(), "method": "Oidc" }))
        }
        Ok(d) => Reply::json(403, json!({ "message": format!("the sign-in was refused: {}", d["refused"]["err"]) })),
        Err(x) => x,
    })
}

/// `GET` and `PUT /api/graftyard/v1/admin/instance-auth`.
pub async fn instance_auth(c: &Ctx<'_>, req: &mut Request) -> Result<Response> {
    let Some(who) = crate::authenticate(c, req).await? else {
        return crate::agents::respond(c, Reply::error(401, "Requires authentication", "rest"));
    };
    let r = match req.method() {
        Method::Get => match instance_auth_now(c).await {
            Ok(InstanceAuth::Oidc(o)) => {
                Reply::json(200, json!({ "type": "oidc", "issuer": o.issuer.0.as_ref(), "client": o.client.as_ref() }))
            }
            Ok(InstanceAuth::Builtin) => Reply::json(200, json!({ "type": "builtin" })),
            Ok(_) => Reply::json(200, json!({ "type": "other" })),
            Err(e) => crate::api::unavailable(&e),
        },
        Method::Put => {
            let b: Value = req.json().await.unwrap_or(Value::Null);
            let auth = match b["type"].as_str() {
                Some("builtin") => InstanceAuth::Builtin,
                Some("oidc") => {
                    let (Some(issuer), Some(client), Some(secret)) =
                        (b["issuer"].as_str(), b["client"].as_str(), b["secret"].as_str())
                    else {
                        return crate::agents::respond(c, Reply::error(422, "issuer, client, and secret", "graftyard"));
                    };
                    InstanceAuth::Oidc(OidcConfig {
                        issuer: IdUrl(issuer.into()),
                        client: client.into(),
                        secret: secret.as_bytes().into(),
                    })
                }
                _ => return crate::agents::respond(c, Reply::error(422, "type is builtin or oidc", "graftyard")),
            };
            let install = install_account();
            let cmd = json!({ "cred": who.credential_shard(), "op": { "SetInstanceAuth": auth } });
            match crate::ident::commit_owner(c, ChainId::Authz(install), install, cmd).await {
                Ok(d) => match crate::ident::refused(&d).as_deref() {
                    None => Reply::empty(204),
                    Some("Forbidden") => {
                        Reply::error(403, "Only a site admin sets the install's sign-in.", "graftyard")
                    }
                    Some("Unauthorized") => Reply::error(401, "Bad credentials", "rest"),
                    Some(e) => crate::api::unavailable(&format!("the sign-in choice did not commit: {e}")),
                },
                Err(x) => x,
            }
        }
        _ => Reply::error(405, "Method Not Allowed", "graftyard"),
    };
    crate::agents::respond(c, r)
}
