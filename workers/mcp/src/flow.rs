//! Journey A7 over the edge (`design/AGENT-LAYER.md`, journey A7, steps 2 to 6): every route `graft-mcp` serves, as
//! plain request-to-response code over `Env`, which the Workers host implements with its bindings and the tests with
//! real members.
//!
//! ```text
//! GET  /.well-known/oauth-protected-resource     RFC 9728
//! GET  /.well-known/oauth-authorization-server   RFC 8414: PKCE S256, CIMD, RFC 9207 iss
//! GET  /authorize    rate limit, CIMD fetch and checks, PKCE S256, RFC 8707 resource, scope; then the consent page
//! POST /consent      same origin only; then the __Host- cookie bound to state, and the redirect to the host's OAuth
//! GET  /callback     the cookie must match state; the host's code to a ghu_ pair; the grant row; token/scoped
//! POST /token        authorization_code (PKCE, the grant's generation once) and refresh_token (narrow only)
//! GET  /settings, POST /settings/revoke          the user's grants, signed in through the host's OAuth
//! POST /             MCP: the sealed McpToken, audience https://mcp.<host>/, or 401 naming the RFC 9728 metadata
//! ```
//!
//! - **No state of its own.** "Once" is the host's: the callback spends the host's code, so a replayed callback gets no
//!   token, and redeeming our code moves the grant's generation, so a second redemption is a stale generation, which
//!   revokes the grant as RFC 6749 4.1.2 asks of a code used twice. No isolate-local set decides either.
//! - **The held `ghu_`.** The unscoped `ghu_` stays sealed in the `McpRefresh` beside its `ghr_` and is deleted with
//!   `DELETE /applications/{client_id}/token` only after the next refresh spent the `ghr_` (A4's fallback, built as the
//!   behavior), because on Graftyard's host revoking either of a pair revokes both.
//! - **Generation first.** A refresh checks the grant's generation before it spends the `ghr_`, so a replayed
//!   `McpRefresh` revokes the grant and never reaches the upstream refresh (M0-WP15's finding 2).

use std::collections::BTreeMap;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64S;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    CODE_TTL_MS, CONSENT_TTL_MS, CimdDoc, Consent, Fetched, McpRefresh, McpToken, REFRESH_TTL_MS, Scope, Sealed,
    Sealer, TOKEN_TTL_MS, Url, cimd_doc, cimd_url, parse_repos, redirect_hosts, s256,
};

/// The cookie that binds the host's `state` to one consent (or one settings sign-in).
pub const FLOW_COOKIE: &str = "__Host-graft_mcp";
/// The settings page's session.
pub const SESSION_COOKIE: &str = "__Host-graft_mcp_session";
/// A settings session lasts as long as the `ghu_` it holds may [doc: GitHub's 8 hours].
pub const SESSION_TTL_MS: u64 = 8 * 3_600_000;

/// One request, as the host hands it over: lower-case header names, the raw query, and the body.
#[derive(Clone, Debug, Default)]
pub struct Req {
    pub method: String,
    pub path: String,
    pub query: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Req {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    fn pairs(s: &[u8]) -> BTreeMap<String, String> {
        url::form_urlencoded::parse(s).into_owned().collect()
    }

    pub fn q(&self) -> BTreeMap<String, String> {
        Req::pairs(self.query.as_bytes())
    }

    pub fn form(&self) -> BTreeMap<String, String> {
        Req::pairs(&self.body)
    }

    pub fn cookie(&self, name: &str) -> Option<&str> {
        self.header("cookie")?.split(';').find_map(|c| c.trim().strip_prefix(name)?.strip_prefix('='))
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Resp {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Resp {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    fn with(mut self, k: &str, v: impl Into<String>) -> Resp {
        self.headers.push((k.to_owned(), v.into()));
        self
    }

    fn json(status: u16, v: &Value) -> Resp {
        Resp { status, headers: Vec::new(), body: v.to_string().into_bytes() }
            .with("content-type", "application/json")
            .with("cache-control", "no-store")
    }

    /// A page of `graft-mcp`'s own, which no other origin may frame.
    fn page(status: u16, html: String) -> Resp {
        Resp { status, headers: Vec::new(), body: html.into_bytes() }
            .with("content-type", "text/html; charset=utf-8")
            .with("content-security-policy", CSP)
            .with("x-frame-options", "DENY")
            .with("cache-control", "no-store")
            .with("referrer-policy", "no-referrer")
    }

    fn text(status: u16, s: &str) -> Resp {
        Resp::page(status, format!("<!doctype html><meta charset=utf-8><p>{}</p>", esc(s)))
    }

    fn redirect(to: &str) -> Resp {
        Resp { status: 302, headers: vec![("location".into(), to.into())], body: Vec::new() }
            .with("cache-control", "no-store")
    }
}

const CSP: &str = "frame-ancestors 'none'; default-src 'none'; style-src 'unsafe-inline'; form-action 'self'";

/// What the flow needs from its host: the clock, the origins, the client credentials and seal key from Secrets Store,
/// randomness, the per-IP `/authorize` count, CIMD fetches and their cache, and calls to the edge.
#[allow(async_fn_in_trait)]
pub trait Env {
    fn now(&self) -> u64;
    /// `https://mcp.<host>`, the issuer and the resource's origin.
    fn origin(&self) -> &str;
    /// `https://<host>`, whose OAuth and API the edge serves.
    fn host_origin(&self) -> &str;
    /// The install's `Graftyard MCP` client id and secret.
    fn client(&self) -> (&str, &str);
    fn sealer(&self) -> &Sealer;
    /// The zones a CIMD URL may not name: the install's own.
    fn deny(&self) -> &[String];
    fn random(&self) -> [u8; 16];
    /// Counts one `/authorize` call from `ip`; false past `AUTHORIZE_PER_MIN`.
    fn allow_authorize(&self, ip: &str) -> bool;
    fn cimd_cached(&self, url: &str) -> Option<CimdDoc>;
    fn cimd_keep(&self, url: &str, doc: &CimdDoc, secs: u64);
    /// GET with no redirect followed, a `CIMD_TIMEOUT_MS` timeout, and at most `CIMD_MAX_BYTES + 1` bytes read.
    async fn fetch(&self, url: &url::Url) -> Result<Fetched, String>;
    /// A request to the edge over the service binding, at `host_origin`.
    async fn edge(
        &self,
        method: &str,
        path: &str,
        headers: &[(&str, String)],
        body: Option<Vec<u8>>,
    ) -> Result<(u16, Vec<u8>), String>;
}

/// The consent form's hidden field: what `/authorize` checked, until the user answers.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Pending {
    consent: Consent,
    redirect_uri: String,
    state: Option<String>,
    challenge: String,
    exp: u64,
}

/// The `__Host-` cookie: the `state` the host's OAuth will echo, and what it is for.
#[derive(Clone, Debug, Serialize, Deserialize)]
enum Flow {
    Consent { upstate: String, pending: Pending, exp: u64 },
    Settings { upstate: String, exp: u64 },
}

/// Our authorization code: the consent, the host's tokens behind it, and the grant's generation it will spend.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Code {
    consent: Consent,
    sub: i64,
    grant: String,
    generation: u32,
    redirect_uri: String,
    challenge: String,
    scoped: String,
    ghr: String,
    held: String,
    exp: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Session {
    token: String,
    exp: u64,
}

pub fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' | '<' | '>' | '"' | '\'' => o.push_str(&format!("&#{};", c as u32)),
            c => o.push(c),
        }
    }
    o
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn nonce<E: Env>(e: &E) -> [u8; 12] {
    e.random()[..12].try_into().expect("12")
}

fn seal<E: Env, T: Serialize>(e: &E, kind: Sealed, v: &T) -> String {
    e.sealer().seal(kind, v, nonce(e))
}

fn oauth_error(status: u16, error: &str, desc: &str) -> Resp {
    Resp::json(status, &json!({ "error": error, "error_description": desc }))
}

fn basic<E: Env>(e: &E) -> String {
    let (id, secret) = e.client();
    format!("Basic {}", B64S.encode(format!("{id}:{secret}")))
}

/// `redirect_uri` with the answer's parameters and RFC 9207's `iss`.
fn back(redirect: &str, params: &[(&str, &str)], state: Option<&str>, iss: &str) -> Resp {
    let Ok(mut u) = url::Url::parse(redirect) else { return Resp::text(400, "redirect_uri is not a URL") };
    {
        let mut q = u.query_pairs_mut();
        for (k, v) in params {
            q.append_pair(k, v);
        }
        if let Some(s) = state {
            q.append_pair("state", s);
        }
        q.append_pair("iss", iss);
    }
    Resp::redirect(u.as_str())
}

fn set_cookie(name: &str, value: &str, max_age_s: u64) -> String {
    format!("{name}={value}; Path=/; Secure; HttpOnly; SameSite=Lax; Max-Age={max_age_s}")
}

/// Every route.
pub async fn serve<E: Env>(e: &E, req: &Req) -> Resp {
    match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/.well-known/oauth-protected-resource" | "/.well-known/oauth-protected-resource/") => {
            Resp::json(200, &crate::resource_metadata(e.origin())).with("cache-control", "max-age=300")
        }
        ("GET", "/.well-known/oauth-authorization-server") => Resp::json(200, &crate::server_metadata(e.origin())),
        ("GET", "/authorize") => authorize(e, req).await,
        ("POST", "/consent") => consent(e, req),
        ("GET", "/callback") => callback(e, req).await,
        ("POST", "/token") => token(e, req).await,
        ("GET", "/settings") => settings(e, req).await,
        ("POST", "/settings/revoke") => revoke(e, req).await,
        (_, "/") => mcp(e, req),
        _ => Resp::text(404, "Not Found"),
    }
}

/// The client's metadata document: from the cache, or fetched and checked.
async fn client_doc<E: Env>(e: &E, id: &str) -> Result<CimdDoc, String> {
    let u = cimd_url(id, e.deny())?;
    if let Some(d) = e.cimd_cached(id) {
        return Ok(d);
    }
    let f = e.fetch(&u).await.map_err(|why| format!("client metadata fetch failed: {why}"))?;
    let (doc, secs) = cimd_doc(id, &f)?;
    if secs > 0 {
        e.cimd_keep(id, &doc, secs);
    }
    Ok(doc)
}

async fn authorize<E: Env>(e: &E, req: &Req) -> Resp {
    if !e.allow_authorize(req.header("cf-connecting-ip").unwrap_or("?")) {
        return Resp::text(429, "Too many authorization requests").with("retry-after", "60");
    }
    let q = req.q();
    let get = |k: &str| q.get(k).map(String::as_str).unwrap_or_default();
    let client = get("client_id");
    // Until the redirect URI is the client's own, errors are shown here, never redirected (RFC 6749 4.1.2.1).
    let doc = match client_doc(e, client).await {
        Ok(d) => d,
        Err(why) => return Resp::text(400, &format!("invalid_client: {why}")),
    };
    let redirect = get("redirect_uri");
    if !doc.redirect_uris.iter().any(|r| r == redirect) {
        return Resp::text(400, "invalid_request: redirect_uri is not in the client's metadata");
    }
    let state = q.get("state").map(String::as_str);
    let iss = e.origin();
    let fail = |error: &str, desc: &str| back(redirect, &[("error", error), ("error_description", desc)], state, iss);
    if get("response_type") != "code" {
        return fail("unsupported_response_type", "only code");
    }
    let challenge = get("code_challenge");
    if get("code_challenge_method") != "S256" || !crate::challenge_ok(challenge) {
        return fail("invalid_request", "PKCE S256 is required");
    }
    if get("resource").trim_end_matches('/') != e.origin() {
        return fail("invalid_target", &format!("resource must be {iss}/"));
    }
    let Some(scope) = Scope::parse(get("scope")) else { return fail("invalid_scope", "unknown scope") };
    let (hosts, local) = redirect_hosts(&doc);
    let consent = Consent {
        client: Url(client.into()),
        redirect_hosts: hosts,
        localhost_only: local,
        repos: parse_repos(get("repos")).unwrap_or_default(),
        tools: scope.tools,
        write: scope.write,
    };
    let pending = Pending {
        consent,
        redirect_uri: redirect.to_owned(),
        state: state.map(str::to_owned),
        challenge: challenge.to_owned(),
        exp: e.now() + CONSENT_TTL_MS,
    };
    consent_page(&pending, &seal(e, Sealed::Pending, &pending), doc.client_name.as_deref())
}

fn consent_page(p: &Pending, sealed: &str, name: Option<&str>) -> Resp {
    let c = &p.consent;
    let host = url::Url::parse(&c.client.0).ok().and_then(|u| u.host_str().map(str::to_owned)).unwrap_or_default();
    let hosts: String = c.redirect_hosts.iter().map(|h| format!("<li><code>{}</code></li>", esc(h))).collect();
    let warn = if c.localhost_only {
        "<p><b>Warning:</b> every redirect goes to this computer (localhost). Continue only if you started this \
         client yourself.</p>"
    } else {
        ""
    };
    let name = name.map(|n| format!(" calls itself \u{201c}{}\u{201d}; that name is its own claim", esc(n)));
    let html = format!(
        "<!doctype html><meta charset=utf-8><title>Authorize an MCP client</title>\n<h1>Authorize an MCP client</h1>\n\
         <p>The client's metadata comes from <b><code>{host}</code></b> (<code>{client}</code>){name}.</p>\n\
         <p>It will be sent back to:</p><ul>{hosts}</ul>\n{warn}\n\
         <form method=post action=\"/consent\"><input type=hidden name=p value=\"{sealed}\">\n\
         <p><label>Repositories, <code>owner/name</code>, one owner, comma-separated: \
         <input name=repos required value=\"{repos}\"></label></p>\n\
         <p>Toolsets: {tools}<br>Write scope: <b>{write}</b></p>\n\
         <button name=decision value=allow>Allow</button> <button name=decision value=deny>Deny</button></form>",
        host = esc(&host),
        client = esc(&c.client.0),
        name = name.unwrap_or_default(),
        sealed = esc(sealed),
        repos = esc(&c.repos.join(",")),
        tools = esc(&c.tools.wire()),
        write = c.write.wire(),
    );
    Resp::page(200, html)
}

/// The form must come from our own page: a cross-site POST, or one from a page that framed ours, carries another
/// Origin or `Sec-Fetch-Site: cross-site`.
fn same_origin<E: Env>(e: &E, req: &Req) -> bool {
    req.header("origin") == Some(e.origin()) && req.header("sec-fetch-site").is_none_or(|s| s == "same-origin")
}

/// The host's authorize URL for `Graftyard MCP`, with our callback and `upstate`.
fn host_authorize<E: Env>(e: &E, upstate: &str) -> String {
    let mut to = url::Url::parse(&format!("{}/login/oauth/authorize", e.host_origin())).expect("host origin");
    to.query_pairs_mut()
        .append_pair("client_id", e.client().0)
        .append_pair("redirect_uri", &format!("{}/callback", e.origin()))
        .append_pair("state", upstate);
    to.into()
}

fn consent<E: Env>(e: &E, req: &Req) -> Resp {
    if !same_origin(e, req) {
        return Resp::text(403, "consent must be submitted from this origin");
    }
    let f = req.form();
    let Some(mut p) = f.get("p").and_then(|p| e.sealer().open::<Pending>(Sealed::Pending, p)) else {
        return Resp::text(400, "the consent form is not ours");
    };
    if p.exp <= e.now() {
        return Resp::text(400, "consent expired; start again from your client");
    }
    if f.get("decision").map(String::as_str) != Some("allow") {
        return back(&p.redirect_uri, &[("error", "access_denied")], p.state.as_deref(), e.origin());
    }
    let Some(repos) = f.get("repos").and_then(|r| parse_repos(r)) else {
        return Resp::text(400, "name 1 to 4 repositories of one owner");
    };
    p.consent.repos = repos;
    // Only now: the cookie bound to the host's state, and the redirect to the host's OAuth.
    let upstate = hex(&e.random());
    let exp = p.exp;
    let cookie = seal(e, Sealed::Cookie, &Flow::Consent { upstate: upstate.clone(), pending: p, exp });
    Resp::redirect(&host_authorize(e, &upstate)).with("set-cookie", set_cookie(FLOW_COOKIE, &cookie, 600))
}

/// The host's code for a `ghu_` and its `ghr_`, or a refresh of the pair, at `POST /login/oauth/access_token`.
async fn host_token<E: Env>(e: &E, form: &[(&str, &str)]) -> Result<(String, String), String> {
    let (id, secret) = e.client();
    let mut s = url::form_urlencoded::Serializer::new(String::new());
    s.append_pair("client_id", id).append_pair("client_secret", secret);
    for (k, v) in form {
        s.append_pair(k, v);
    }
    let h = [("accept", "application/json".to_owned()), ("content-type", "application/x-www-form-urlencoded".into())];
    let (_, body) = e.edge("POST", "/login/oauth/access_token", &h, Some(s.finish().into_bytes())).await?;
    let v: Value = serde_json::from_slice(&body).map_err(|_| "the host's token answer is not JSON".to_owned())?;
    match (v["access_token"].as_str(), v["refresh_token"].as_str()) {
        (Some(a), Some(r)) => Ok((a.to_owned(), r.to_owned())),
        _ => Err(v["error"].as_str().unwrap_or("no token").to_owned()),
    }
}

/// `token/scoped` with the grant's fields: the consent's owner and repositories, and its write scope.
async fn narrow<E: Env>(
    e: &E,
    ghu: &str,
    grant: &str,
    write: crate::WriteScope,
    repos: &[String],
) -> Result<String, String> {
    let owner = repos.first().and_then(|r| r.split_once('/')).map(|(o, _)| o).unwrap_or_default();
    let names: Vec<&str> = repos.iter().filter_map(|r| r.split_once('/').map(|(_, n)| n)).collect();
    let body = json!({ "access_token": ghu, "target": owner, "repositories": names,
        "graftyard_grant": grant, "graftyard_write": write.wire() });
    let h = [("authorization", basic(e)), ("content-type", "application/json".into())];
    let path = format!("/api/v3/applications/{}/token/scoped", e.client().0);
    let (status, b) = e.edge("POST", &path, &h, Some(body.to_string().into_bytes())).await?;
    let v: Value = serde_json::from_slice(&b).unwrap_or(Value::Null);
    match (status, v["token"].as_str()) {
        (200, Some(t)) => Ok(t.to_owned()),
        _ => Err(format!("token/scoped answered {status}")),
    }
}

async fn callback<E: Env>(e: &E, req: &Req) -> Resp {
    let q = req.q();
    let flow = req.cookie(FLOW_COOKIE).and_then(|c| e.sealer().open::<Flow>(Sealed::Cookie, c));
    let (upstate, exp) = match &flow {
        Some(Flow::Consent { upstate, exp, .. } | Flow::Settings { upstate, exp }) => (upstate.as_str(), *exp),
        None => ("", 0),
    };
    // Skipped consent has no cookie; another flow's cookie names another state.
    if flow.is_none() || q.get("state").map(String::as_str) != Some(upstate) || exp <= e.now() {
        return Resp::text(400, "state does not match a consent from this browser");
    }
    let clear = set_cookie(FLOW_COOKIE, "", 0);
    // The host spends its code once, so a replayed callback stops here.
    let code = q.get("code").cloned().unwrap_or_default();
    let (ghu, ghr) = match host_token(e, &[("code", &code)]).await {
        Ok(p) => p,
        Err(why) => return Resp::text(400, &format!("the host refused the code: {why}")).with("set-cookie", clear),
    };
    let p = match flow.expect("checked") {
        Flow::Settings { .. } => {
            let s = seal(e, Sealed::Session, &Session { token: ghu, exp: e.now() + SESSION_TTL_MS });
            let set = set_cookie(SESSION_COOKIE, &s, SESSION_TTL_MS / 1000);
            return Resp::redirect("/settings").with("set-cookie", clear).with("set-cookie", set);
        }
        Flow::Consent { pending, .. } => pending,
    };
    let fail = |why: String| Resp::text(502, &why).with("set-cookie", set_cookie(FLOW_COOKIE, "", 0));
    let h = [("authorization", format!("Bearer {ghu}"))];
    let sub = match e.edge("GET", "/api/v3/user", &h, None).await {
        Ok((200, b)) => serde_json::from_slice::<Value>(&b).ok().and_then(|v| v["id"].as_i64()),
        _ => None,
    };
    let Some(sub) = sub else { return fail("the host did not name the user".into()) };
    let c = &p.consent;
    let body = json!({ "client": c.client.0, "redirect_hosts": c.redirect_hosts, "write": c.write.wire() });
    let h = [
        ("authorization", basic(e)),
        ("x-graftyard-user-token", ghu.clone()),
        ("content-type", "application/json".into()),
    ];
    let grant = match e.edge("POST", "/api/graftyard/v1/mcp/grants", &h, Some(body.to_string().into_bytes())).await {
        Ok((201, b)) => serde_json::from_slice::<Value>(&b).ok(),
        Ok((s, _)) => return fail(format!("the grant answered {s}")),
        Err(why) => return fail(why),
    };
    let (Some(id), Some(generation)) = (
        grant.as_ref().and_then(|g| g["id"].as_str()).map(str::to_owned),
        grant.as_ref().and_then(|g| g["generation"].as_u64()).and_then(|g| u32::try_from(g).ok()),
    ) else {
        return fail("the grant answer has no id".into());
    };
    let scoped = match narrow(e, &ghu, &id, c.write, &c.repos).await {
        Ok(t) => t,
        Err(why) => return fail(why),
    };
    let code = Code {
        consent: p.consent.clone(),
        sub,
        grant: id,
        generation,
        redirect_uri: p.redirect_uri.clone(),
        challenge: p.challenge.clone(),
        scoped,
        ghr,
        held: ghu,
        exp: e.now() + CODE_TTL_MS,
    };
    back(&p.redirect_uri, &[("code", &seal(e, Sealed::Code, &code))], p.state.as_deref(), e.origin())
        .with("set-cookie", clear)
}

/// `POST /mcp/grants/{id}/generation`: `Ok(next)` when `generation` was the grant's, `Err(true)` when it was stale and
/// the host revoked the grant, `Err(false)` when the host could not answer.
async fn spend_generation<E: Env>(e: &E, grant: &str, generation: u32) -> Result<u32, bool> {
    let h = [("authorization", basic(e)), ("content-type", "application/json".into())];
    let path = format!("/api/graftyard/v1/mcp/grants/{grant}/generation");
    let body = json!({ "generation": generation }).to_string().into_bytes();
    match e.edge("POST", &path, &h, Some(body)).await {
        Ok((200, b)) => serde_json::from_slice::<Value>(&b)
            .ok()
            .and_then(|v| v["generation"].as_u64())
            .and_then(|g| u32::try_from(g).ok())
            .ok_or(false),
        Ok((409, _)) => Err(true),
        _ => Err(false),
    }
}

fn issue<E: Env>(e: &E, t: McpToken, r: McpRefresh, scope: Scope) -> Resp {
    Resp::json(
        200,
        &json!({
            "access_token": seal(e, Sealed::Access, &t), "token_type": "Bearer", "expires_in": TOKEN_TTL_MS / 1000,
            "refresh_token": seal(e, Sealed::Refresh, &r), "scope": scope.wire(),
        }),
    )
}

async fn token<E: Env>(e: &E, req: &Req) -> Resp {
    let f = req.form();
    let get = |k: &str| f.get(k).map(String::as_str).unwrap_or_default();
    let aud = Url(format!("{}/", e.origin()).into());
    let now = e.now();
    match get("grant_type") {
        "authorization_code" => {
            let Some(c) = e.sealer().open::<Code>(Sealed::Code, get("code")).filter(|c| c.exp > now) else {
                return oauth_error(400, "invalid_grant", "code is invalid or expired");
            };
            if get("client_id") != c.consent.client.0.as_ref() {
                return oauth_error(400, "invalid_grant", "code was issued to another client");
            }
            if get("redirect_uri") != c.redirect_uri {
                return oauth_error(400, "invalid_grant", "redirect_uri differs from the authorization request");
            }
            if s256(get("code_verifier")) != c.challenge {
                return oauth_error(400, "invalid_grant", "PKCE verification failed");
            }
            if !get("resource").is_empty() && get("resource").trim_end_matches('/') != e.origin() {
                return oauth_error(400, "invalid_target", "resource mismatch");
            }
            // The code's one use: it moves the grant's generation, so a second redemption is stale and revokes it.
            let generation = match spend_generation(e, &c.grant, c.generation).await {
                Ok(g) => g,
                Err(true) => {
                    return oauth_error(400, "invalid_grant", "code was already used, so the grant is revoked");
                }
                Err(false) => return oauth_error(503, "temporarily_unavailable", "the grant could not be read"),
            };
            let k = &c.consent;
            let scope = Scope { write: k.write, tools: k.tools };
            let t = McpToken {
                aud: aud.clone(),
                sub: c.sub,
                client: k.client.clone(),
                grant: c.grant.clone(),
                repos: k.repos.clone(),
                tools: k.tools,
                write: k.write,
                upstream: c.scoped,
                exp: now + TOKEN_TTL_MS,
            };
            let r = McpRefresh {
                sub: c.sub,
                aud,
                client: k.client.clone(),
                grant: c.grant,
                generation,
                repos: k.repos.clone(),
                tools: k.tools,
                write: k.write,
                upstream: c.ghr,
                held: c.held,
                exp: now + REFRESH_TTL_MS,
            };
            issue(e, t, r, scope)
        }
        "refresh_token" => {
            let r = e.sealer().open::<McpRefresh>(Sealed::Refresh, get("refresh_token"));
            let Some(r) = r.filter(|r| r.exp > now && r.aud == aud) else {
                return oauth_error(400, "invalid_grant", "refresh token is invalid");
            };
            if !get("client_id").is_empty() && get("client_id") != r.client.0.as_ref() {
                return oauth_error(400, "invalid_grant", "refresh token was issued to another client");
            }
            // A refresh can only narrow: a scope wider than the grant's is refused, never widened.
            let had = Scope { write: r.write, tools: r.tools };
            let want = match get("scope") {
                "" => had,
                s => match Scope::parse(s).filter(|w| w.within(had)) {
                    Some(w) => w,
                    None => return oauth_error(400, "invalid_scope", "a refresh cannot widen its grant"),
                },
            };
            let generation = match spend_generation(e, &r.grant, r.generation).await {
                Ok(g) => g,
                Err(true) => {
                    return oauth_error(
                        400,
                        "invalid_grant",
                        "this refresh token was already used, so the grant is revoked",
                    );
                }
                Err(false) => return oauth_error(503, "temporarily_unavailable", "the grant could not be read"),
            };
            let (ghu, ghr) =
                match host_token(e, &[("grant_type", "refresh_token"), ("refresh_token", &r.upstream)]).await {
                    Ok(p) => p,
                    Err(why) => return oauth_error(400, "invalid_grant", &format!("upstream refresh: {why}")),
                };
            // The `ghr_` is spent, so the held `ghu_` may go now; on Graftyard's host it went with its pair already.
            let h = [("authorization", basic(e)), ("content-type", "application/json".into())];
            let path = format!("/applications/{}/token", e.client().0);
            let body = json!({ "access_token": r.held }).to_string().into_bytes();
            let _ = e.edge("DELETE", &format!("/api/v3{path}"), &h, Some(body)).await;
            let scoped = match narrow(e, &ghu, &r.grant, want.write, &r.repos).await {
                Ok(t) => t,
                Err(why) => return oauth_error(400, "invalid_grant", &why),
            };
            let t = McpToken {
                aud: aud.clone(),
                sub: r.sub,
                client: r.client.clone(),
                grant: r.grant.clone(),
                repos: r.repos.clone(),
                tools: want.tools,
                write: want.write,
                upstream: scoped,
                exp: now + TOKEN_TTL_MS,
            };
            let next = McpRefresh {
                generation,
                tools: want.tools,
                write: want.write,
                upstream: ghr,
                held: ghu,
                exp: now + REFRESH_TTL_MS,
                ..r
            };
            issue(e, t, next, want)
        }
        other => oauth_error(400, "unsupported_grant_type", other),
    }
}

/// The MCP endpoint's door: a sealed `McpToken` whose audience is this server and which has not lapsed. The tools
/// behind it are AM2-WP2's; until then a good token is answered 501.
fn mcp<E: Env>(e: &E, req: &Req) -> Resp {
    let prm = format!("{}/.well-known/oauth-protected-resource", e.origin());
    let unauth = |desc: &str| {
        Resp::json(401, &json!({ "error": "invalid_token", "error_description": desc })).with(
            "www-authenticate",
            format!("Bearer resource_metadata=\"{prm}\", error=\"invalid_token\", error_description=\"{desc}\""),
        )
    };
    let bearer =
        req.header("authorization").and_then(|a| a.strip_prefix("Bearer ").or_else(|| a.strip_prefix("bearer ")));
    let Some(bearer) = bearer else {
        return Resp { status: 401, ..Resp::default() }
            .with("www-authenticate", format!("Bearer resource_metadata=\"{prm}\""));
    };
    let Some(t) = e.sealer().open::<McpToken>(Sealed::Access, bearer) else {
        return unauth("not a token this server issued");
    };
    if t.aud.0.as_ref() != format!("{}/", e.origin()) {
        return unauth("the token's audience is not this server");
    }
    if t.exp <= e.now() {
        return unauth("the token expired");
    }
    Resp::json(501, &json!({ "error": "not_implemented", "error_description": "graft-mcp's tools land in AM2-WP2" }))
}

/// The signed-in user's `ghu_`, or the redirect that signs them in through the host's OAuth.
fn session<E: Env>(e: &E, req: &Req) -> Result<String, Resp> {
    let s = req.cookie(SESSION_COOKIE).and_then(|c| e.sealer().open::<Session>(Sealed::Session, c));
    if let Some(s) = s.filter(|s| s.exp > e.now()) {
        return Ok(s.token);
    }
    let upstate = hex(&e.random());
    let cookie = seal(e, Sealed::Cookie, &Flow::Settings { upstate: upstate.clone(), exp: e.now() + CONSENT_TTL_MS });
    Err(Resp::redirect(&host_authorize(e, &upstate)).with("set-cookie", set_cookie(FLOW_COOKIE, &cookie, 600)))
}

async fn settings<E: Env>(e: &E, req: &Req) -> Resp {
    let user = match session(e, req) {
        Ok(u) => u,
        Err(r) => return r,
    };
    let h = [("authorization", basic(e)), ("x-graftyard-user-token", user)];
    let rows = match e.edge("GET", "/api/graftyard/v1/mcp/grants", &h, None).await {
        Ok((200, b)) => serde_json::from_slice::<Vec<Value>>(&b).unwrap_or_default(),
        Ok((401, _)) => {
            return Resp::redirect("/settings").with("set-cookie", set_cookie(SESSION_COOKIE, "", 0));
        }
        Ok((s, _)) => return Resp::text(502, &format!("the grants answered {s}")),
        Err(why) => return Resp::text(502, &why),
    };
    let items: String = rows
        .iter()
        .map(|g| {
            let s = |k: &str| esc(g[k].as_str().unwrap_or_default());
            format!(
                "<li><code>{}</code>, {}, since {}<form method=post action=\"/settings/revoke\">\
                 <input type=hidden name=grant value=\"{}\"><button>Revoke</button></form></li>",
                s("client"),
                s("write"),
                s("created_at"),
                s("id")
            )
        })
        .collect();
    let list =
        if rows.is_empty() { "<p>No MCP client holds a grant.</p>".to_owned() } else { format!("<ul>{items}</ul>") };
    Resp::page(200, format!("<!doctype html><meta charset=utf-8><title>MCP clients</title><h1>MCP clients</h1>{list}"))
}

async fn revoke<E: Env>(e: &E, req: &Req) -> Resp {
    if !same_origin(e, req) {
        return Resp::text(403, "revoke from the settings page");
    }
    let user = match session(e, req) {
        Ok(u) => u,
        Err(r) => return r,
    };
    let grant = req.form().get("grant").cloned().unwrap_or_default();
    if crate::flow::grant_id_ok(&grant) {
        let h = [("authorization", basic(e)), ("x-graftyard-user-token", user)];
        let path = format!("/api/graftyard/v1/mcp/grants/{grant}");
        match e.edge("DELETE", &path, &h, None).await {
            Ok((204, _)) => return Resp::redirect("/settings"),
            Ok((s, _)) => return Resp::text(s.max(400), "the grant was not revoked"),
            Err(why) => return Resp::text(502, &why),
        }
    }
    Resp::text(400, "no such grant")
}

/// A grant id as the host writes it, `<account>.<n>`, so nothing else reaches the edge's path.
pub fn grant_id_ok(s: &str) -> bool {
    s.split_once('.').is_some_and(|(a, n)| {
        [a, n].iter().all(|p| !p.is_empty() && p.len() <= 18 && p.bytes().all(|b| b.is_ascii_digit()))
    })
}
