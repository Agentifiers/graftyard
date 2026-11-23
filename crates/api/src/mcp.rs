//! The host's half of `graft-mcp` (`design/AGENT-LAYER.md`, Shape, "graft-mcp", journey A7, and "API reference";
//! `build/BUILD-PLAN.md`, AM2-WP1): the grant endpoints under `/api/graftyard/v1/mcp/grants` and the additive
//! `graftyard_grant` and `graftyard_write` fields of `POST /api/v3/applications/{client_id}/token/scoped`.
//!
//! Pure request-to-decision code. The edge reads the folds and commits the commands; the rules a reply depends on are
//! here so LF-6 runs them in memory against real members (`tests/mcp_lf6.rs`).
//!
//! - **Client credentials.** Every grant call takes the install's `Graftyard MCP` client id and secret as HTTP Basic
//!   credentials, checked against its `AppRecord.client_secrets` (`client_auth`). Wrong or missing ones are 401 before
//!   anything else is read, so a caller without them learns nothing about grants.
//! - **The user.** `POST` and `GET` also take the user's `ghu_` of `Graftyard MCP` in `X-Graftyard-User-Token`; the
//!   edge resolves its row, and `user_of` accepts only a live `ghu_` whose `app` is `Graftyard MCP`. `/{id}/generation`
//!   takes the client credentials alone, so the check runs before the upstream refresh rotates the `ghr_` (M0-WP15).
//! - **Grant ids.** A grant's wire id is `<account>.<n>` (`GrantRef`): the account names the `Identity` group, which
//!   the generation call, carrying no user token, needs to find; `n` is the leader's per-account counter.
//! - **`token/scoped`.** From `Graftyard MCP`, `graftyard_grant` and `graftyard_write` are required, and a grant that is
//!   not a live grant of the token's user, or a write above the grant's, is 422 (`scoped_request`, then
//!   `IdentityOp::McpScoped` binds the same check at commit). The host serves `token/scoped` for `Graftyard MCP` only;
//!   every other client, and every client while the App cannot be read, gets the 404 it got before.

use graftyard_identity::{
    AppRecord, CredentialRef, GrantId, IdentityCmd, IdentityFold, IdentityOp, IdentityOut, McpGrant, OpaqueToken,
    PermissionSet, RepoScope, TokenFields, TokenKind, TokenRow, TokensCmd, TokensOp, WriteScope,
};
use graftyard_types::{AccountId, CredentialKey, CredentialShard, DbId, RepoId, Timestamp};
use graftyard_wire::{FieldError, Reply, iso, validation_error};
use serde_json::{Value, json};
use smallvec::SmallVec;

/// `/api/graftyard/v1` paths after the prefix.
pub const GRANTS: &str = "/mcp/grants";
const DOC: &str = "graftyard/mcp";
const SCOPED_DOC: &str = "rest/apps/apps#create-a-scoped-access-token";
/// The install's `Graftyard MCP` App's slug (`design/AGENT-LAYER.md`, "First-party Apps"; `graftyard apps init`).
pub const MCP_SLUG: &str = "graftyard-mcp";
/// How many redirect hosts a grant records [guess: a CIMD document names a few; the consent page shows each].
pub const REDIRECT_HOSTS_MAX: usize = 8;

/// A route under `/api/graftyard/v1/mcp/grants`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    /// `/mcp/grants`
    Grants,
    /// `/mcp/grants/{id}/generation`
    Generation(GrantRef),
    /// `/mcp/grants/{id}`
    Grant(GrantRef),
    /// A grant path whose id does not parse: 404.
    Unknown,
}

/// A grant's id on the wire: the account whose `Identity` group holds it, and its id there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GrantRef {
    pub account: AccountId,
    pub grant: GrantId,
}

impl GrantRef {
    pub fn text(self) -> String {
        format!("{}.{}", self.account.0.0, self.grant.0.0)
    }

    pub fn parse(s: &str) -> Option<GrantRef> {
        let (a, g) = s.split_once('.')?;
        let digits = |x: &str| !x.is_empty() && x.len() <= 18 && x.bytes().all(|b| b.is_ascii_digit());
        if !digits(a) || !digits(g) {
            return None;
        }
        let (a, g) = (a.parse::<i64>().ok()?, g.parse::<i64>().ok()?);
        (a > 0 && g > 0).then_some(GrantRef { account: AccountId(DbId(a)), grant: GrantId(DbId(g)) })
    }
}

/// The route of a path after `/api/graftyard/v1`, or `None` when it is not a grant path.
pub fn route(rest: &str) -> Option<Route> {
    let tail = rest.strip_prefix(GRANTS)?;
    let segs: Vec<&str> = tail.split('/').collect();
    Some(match segs.as_slice() {
        [""] => Route::Grants,
        ["", id, "generation"] => GrantRef::parse(id).map_or(Route::Unknown, Route::Generation),
        ["", id] => GrantRef::parse(id).map_or(Route::Unknown, Route::Grant),
        _ => return None,
    })
}

/// The Basic credentials of `Authorization`, as (client id, secret).
pub fn basic(authorization: Option<&str>) -> Option<(String, String)> {
    let (scheme, rest) = authorization?.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("basic") {
        return None;
    }
    let bytes = graftyard_git::pack::b64::decode(rest.trim())?;
    let (id, secret) = String::from_utf8(bytes).ok()?.split_once(':').map(|(a, b)| (a.to_owned(), b.to_owned()))?;
    Some((id, secret))
}

/// Whether the Basic credentials are the `Graftyard MCP` App's: its client id and one of its live secrets. `mcp` is the
/// App's record on the install's chain, `None` before `graftyard apps init` ran, which refuses every caller.
pub fn client_auth(mcp: Option<&AppRecord>, authorization: Option<&str>) -> Result<(), Reply> {
    let ok = match (mcp, basic(authorization)) {
        (Some(app), Some((id, secret))) => {
            id == app.client.as_str() && graftyard_identity::appauth::client_secret_ok(&app.client_secrets, &secret)
        }
        _ => false,
    };
    if ok { Ok(()) } else { Err(Reply::error(401, "Requires the Graftyard MCP client credentials", DOC)) }
}

/// The user behind `X-Graftyard-User-Token`: its row must be a live, unexpired `ghu_` of `Graftyard MCP`.
pub fn user_of(
    row: Option<(&TokenRow, Option<&TokenFields>)>,
    mcp: &AppRecord,
    now: Timestamp,
) -> Result<AccountId, Reply> {
    match row {
        Some((r, f))
            if r.kind == TokenKind::UserToServer
                && !r.revoked
                && r.expires.is_none_or(|e| e > now)
                && f.and_then(|f| f.app) == Some(mcp.id) =>
        {
            Ok(r.account)
        }
        _ => Err(Reply::error(401, "X-Graftyard-User-Token must be a live user token of Graftyard MCP", DOC)),
    }
}

fn invalid(resource: &'static str, field: &'static str, code: &'static str, doc: &str) -> Reply {
    Reply::json(422, validation_error("Validation Failed", doc, &[FieldError { resource, field, code }]))
}

/// A new grant: `POST /mcp/grants` with `{client, redirect_hosts, write?}`. `write` defaults to `read`, the consent
/// page's default.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewGrant {
    pub client: graftyard_identity::Url,
    pub redirect_hosts: SmallVec<[Box<str>; 2]>,
    pub write: WriteScope,
}

pub fn parse_new_grant(body: &Value) -> Result<NewGrant, Reply> {
    let client = body.get("client").and_then(Value::as_str).filter(|c| {
        c.strip_prefix("https://").is_some_and(|r| r.contains('/') && !r.starts_with('/')) && c.len() <= 2_048
    });
    let Some(client) = client else { return Err(invalid("Grant", "client", "invalid", DOC)) };
    let hosts = body.get("redirect_hosts").and_then(Value::as_array);
    let hosts: Option<SmallVec<[Box<str>; 2]>> =
        hosts.filter(|h| !h.is_empty() && h.len() <= REDIRECT_HOSTS_MAX).and_then(|h| {
            h.iter().map(|x| x.as_str().filter(|s| !s.is_empty() && s.len() <= 253).map(Box::from)).collect()
        });
    let Some(redirect_hosts) = hosts else { return Err(invalid("Grant", "redirect_hosts", "invalid", DOC)) };
    let write = match body.get("write") {
        None | Some(Value::Null) => WriteScope::Read,
        Some(w) => {
            w.as_str().and_then(WriteScope::parse_wire).ok_or_else(|| invalid("Grant", "write", "invalid", DOC))?
        }
    };
    Ok(NewGrant { client: graftyard_identity::Url(client.into()), redirect_hosts, write })
}

/// `POST /mcp/grants/{id}/generation` with `{generation}`.
pub fn parse_generation(body: &Value) -> Result<u32, Reply> {
    body.get("generation")
        .and_then(Value::as_u64)
        .and_then(|g| u32::try_from(g).ok())
        .ok_or_else(|| invalid("Grant", "generation", "missing_field", DOC))
}

/// A grant as the reference renders it (`web/src/areas/agents/api.ts` `Grant`).
pub fn grant_json(account: AccountId, g: &McpGrant, created: Timestamp) -> Value {
    json!({
        "id": GrantRef { account, grant: g.id }.text(),
        "client": g.client.0,
        "redirect_hosts": g.redirect_hosts,
        "write": g.write.wire(),
        "generation": g.generation,
        "created_at": iso(created),
    })
}

/// `GET /mcp/grants`: the account's live grants, oldest first.
pub fn list_json(account: AccountId, f: &IdentityFold) -> Value {
    Value::Array(f.grants().values().filter(|r| r.live()).map(|r| grant_json(account, &r.grant, r.created)).collect())
}

/// The 409 of a revoked grant, or of a stale generation, which revoked it in the same entry.
pub fn revoked(stale: bool) -> Reply {
    let message = if stale { "A refresh with an older generation revoked this grant" } else { "This grant is revoked" };
    let mut r = Reply::error(409, message, DOC);
    if let graftyard_wire::Body::Json(Value::Object(m)) = &mut r.body {
        m.insert("revoked".into(), Value::Bool(true));
    }
    r
}

// ---- token/scoped ----

/// The fields of a `token/scoped` request this host reads. `target`, `target_id`, and `permissions` are GHES's;
/// `graftyard_grant` and `graftyard_write` are the additive pair.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScopedRequest {
    pub access_token: String,
    pub target: Option<String>,
    pub target_id: Option<AccountId>,
    pub repositories: Vec<String>,
    pub repository_ids: Vec<graftyard_types::RepoId>,
    pub permissions: Option<PermissionSet>,
    pub grant: GrantRef,
    pub write: WriteScope,
}

/// Parses a `token/scoped` body from `Graftyard MCP`, whose requests must carry a grant and a write scope.
pub fn parse_scoped(body: &Value) -> Result<ScopedRequest, Reply> {
    let bad = |field: &'static str, code: &'static str| invalid("Authorization", field, code, SCOPED_DOC);
    let Some(token) = body.get("access_token").and_then(Value::as_str).filter(|t| !t.is_empty()) else {
        return Err(bad("access_token", "missing_field"));
    };
    let grant = match body.get("graftyard_grant") {
        None => return Err(bad("graftyard_grant", "missing_field")),
        Some(g) => g.as_str().and_then(GrantRef::parse).ok_or_else(|| bad("graftyard_grant", "invalid"))?,
    };
    let write = match body.get("graftyard_write") {
        None => return Err(bad("graftyard_write", "missing_field")),
        Some(w) => w.as_str().and_then(WriteScope::parse_wire).ok_or_else(|| bad("graftyard_write", "invalid"))?,
    };
    let mut r = ScopedRequest {
        access_token: token.to_owned(),
        target: None,
        target_id: None,
        repositories: Vec::new(),
        repository_ids: Vec::new(),
        permissions: None,
        grant,
        write,
    };
    if let Some(t) = body.get("target") {
        r.target = Some(t.as_str().ok_or_else(|| bad("target", "invalid"))?.to_owned());
    }
    if let Some(t) = body.get("target_id") {
        r.target_id = Some(AccountId(DbId(t.as_i64().ok_or_else(|| bad("target_id", "invalid"))?)));
    }
    if let Some(names) = body.get("repositories") {
        for n in names.as_array().ok_or_else(|| bad("repositories", "invalid"))? {
            r.repositories.push(n.as_str().ok_or_else(|| bad("repositories", "invalid"))?.to_owned());
        }
    }
    if let Some(ids) = body.get("repository_ids") {
        for i in ids.as_array().ok_or_else(|| bad("repository_ids", "invalid"))? {
            r.repository_ids
                .push(graftyard_types::RepoId(DbId(i.as_i64().ok_or_else(|| bad("repository_ids", "invalid"))?)));
        }
    }
    if !r.repositories.is_empty() && !r.repository_ids.is_empty() {
        return Err(bad("repository_ids", "invalid"));
    }
    if let Some(p) = body.get("permissions") {
        let mut set = PermissionSet::default();
        for (k, v) in p.as_object().ok_or_else(|| bad("permissions", "invalid"))? {
            let level = match v.as_str() {
                Some("read") => graftyard_identity::Access::Read,
                Some("write") => graftyard_identity::Access::Write,
                Some("admin") => graftyard_identity::Access::Admin,
                _ => return Err(bad("permissions", "invalid")),
            };
            set.0.insert(k.as_str().into(), level);
        }
        r.permissions = Some(set);
    }
    Ok(r)
}

/// The grant rule of `token/scoped` from `Graftyard MCP`: the grant must be a live grant of `user` (the account of the
/// token being narrowed), and `write` within its scope. 422 otherwise, with no hint of whose grant it was.
pub fn check_grant(user: AccountId, grant: GrantRef, write: WriteScope, f: Option<&IdentityFold>) -> Result<(), Reply> {
    let bad = |field| invalid("Authorization", field, "invalid", SCOPED_DOC);
    if grant.account != user {
        return Err(bad("graftyard_grant"));
    }
    let Some(g) = f.and_then(|f| f.live_grant(grant.grant)) else { return Err(bad("graftyard_grant")) };
    if write > g.write {
        return Err(bad("graftyard_write"));
    }
    Ok(())
}

/// GHES's `authorization` for a new scoped token: the token text once, its hash, and the App. `user` is the account's
/// `simple-user`; GHES renders `installation` only for an installation's token [inference: a user token has none].
#[allow(clippy::too_many_arguments)]
pub fn scoped_json(
    api: &str,
    id: i64,
    token: &str,
    hash: &[u8; 32],
    app: &AppRecord,
    app_url: &str,
    user: Value,
    at: Timestamp,
    expires: Option<Timestamp>,
) -> Value {
    json!({
        "id": id, "url": format!("{api}/authorizations/{id}"), "scopes": [],
        "token": token, "token_last_eight": &token[token.len().saturating_sub(8)..],
        "hashed_token": graftyard_types::Digest(*hash).hex(),
        "app": { "client_id": app.client.as_str(), "name": app.name.as_ref(), "url": app_url },
        "note": null, "note_url": null, "fingerprint": null,
        "created_at": iso(at), "updated_at": iso(at), "expires_at": expires.map(iso),
        "user": user, "installation": null,
    })
}

// ---- the handlers, over the host's reads and commits ----

/// What the handlers need from the edge: fresh reads, commits on the account's `Identity` chain and on a `Tokens`
/// shard (an `Err(name)` is the refusal's variant name), and the request's environment. Every read is fresh, never a
/// cache, so a revoked grant or token binds the next call. The edge implements it over its bindings; LF-6's test over
/// real members.
#[allow(async_fn_in_trait)]
pub trait Host {
    fn now(&self) -> Timestamp;
    /// `https://<host>`.
    fn web(&self) -> String;
    /// Whether `FormatsEnable` enabled 0x14.4 and 0x11.4, which the grant and scoped-token commands take.
    fn enabled(&self) -> bool;
    /// A new `ghu_` drawn from the host's randomness.
    fn draw(&self) -> OpaqueToken;
    /// The install's `Graftyard MCP` App record: the `Mcp` `AgentApp` row's App on `Authz(Genesis.account)`.
    async fn mcp_app(&self) -> Result<Option<AppRecord>, Reply>;
    async fn token(&self, text: &str) -> Result<Option<(TokenRow, Option<TokenFields>)>, Reply>;
    async fn identity(&self, a: AccountId) -> Result<Option<IdentityFold>, Reply>;
    async fn commit_identity(&self, a: AccountId, cmd: IdentityCmd) -> Result<Result<IdentityOut, String>, Reply>;
    async fn commit_tokens(&self, cmd: TokensCmd) -> Result<Result<(), String>, Reply>;
    /// Lists a new credential on the account's `Identity` chain, best effort (`crate::ident::list_credential`).
    async fn list_credential(&self, a: AccountId, c: CredentialRef, title: &str);
    async fn account_of(&self, login: &str) -> Result<Option<AccountId>, Reply>;
    async fn repo(&self, owner: AccountId, name: &str) -> Result<Option<RepoId>, Reply>;
    async fn user_json(&self, a: AccountId) -> Value;
}

/// One request's parts the handlers read.
#[derive(Clone, Copy, Debug)]
pub struct Call<'a> {
    pub method: &'a str,
    pub authorization: Option<&'a str>,
    pub user_token: Option<&'a str>,
    pub body: &'a Value,
}

const OPERATOR: CredentialShard = CredentialShard::NONE;

fn unavailable(why: &str) -> Reply {
    Reply::error(503, why, DOC).with("retry-after", "5")
}

fn refusal(e: &str) -> Reply {
    match e {
        "NoGrant" | "NotCreated" => Reply::not_found(),
        "GrantRevoked" => revoked(false),
        "BadGrant" => invalid("Grant", "client", "invalid", DOC),
        "Unauthorized" | "Forbidden" => Reply::error(401, "Bad credentials", DOC),
        _ => unavailable(&format!("the grant did not commit: {e}")),
    }
}

/// Revokes each token on its `Tokens` shard, as the operator; one already revoked is fine.
async fn revoke_all<H: Host>(h: &H, hashes: &[[u8; 32]]) -> Result<(), Reply> {
    for hash in hashes {
        match h.commit_tokens(TokensCmd { cred: OPERATOR, op: TokensOp::Revoke { hash: *hash } }).await? {
            Ok(()) => {}
            Err(e) if e == "AlreadyRevoked" || e == "NoToken" => {}
            Err(e) => return Err(unavailable(&format!("a grant token's revocation did not commit: {e}"))),
        }
    }
    Ok(())
}

/// Commits a grant op and renders its row. After a revocation it reads the row fresh and revokes every token recorded
/// under it, so a replayed or repeated revocation finishes what an edge that died after the commit left.
async fn grant_op<H: Host>(h: &H, a: AccountId, cmd: IdentityCmd, status: u16) -> Result<Reply, Reply> {
    let at = h.now();
    let out = h.commit_identity(a, cmd).await?.map_err(|e| refusal(&e))?;
    let IdentityOut::Grant { grant, revoked: gone, created } = out else {
        return Err(unavailable("no grant in the answer"));
    };
    if gone {
        let row = h.identity(a).await?.and_then(|f| f.grants().get(&grant.id).cloned());
        let revoke: Vec<[u8; 32]> = row.map(|r| r.tokens.into_keys().chain(r.upstream).collect()).unwrap_or_default();
        revoke_all(h, &revoke).await?;
    }
    Ok(match (gone, status) {
        (_, 204) => Reply::empty(204),
        (true, _) => revoked(true),
        (false, _) => Reply::json(status, grant_json(a, &grant, created.unwrap_or(at))),
    })
}

/// `/api/graftyard/v1/mcp/grants...`: the client credentials first, then the route.
pub async fn grants<H: Host>(h: &H, route: Route, call: Call<'_>) -> Reply {
    grants_inner(h, route, call).await.unwrap_or_else(|r| r)
}

async fn grants_inner<H: Host>(h: &H, route: Route, call: Call<'_>) -> Result<Reply, Reply> {
    if !h.enabled() {
        return Err(unavailable("MCP grants need record formats 20.4 and 17.4, which FormatsEnable has not enabled"));
    }
    let app = h.mcp_app().await?;
    client_auth(app.as_ref(), call.authorization)?;
    let app = app.expect("client_auth refuses without the App");
    let user = async || -> Result<AccountId, Reply> {
        let row = h.token(call.user_token.unwrap_or_default()).await?;
        user_of(row.as_ref().map(|(r, f)| (r, f.as_ref())), &app, h.now())
    };
    match (route, call.method) {
        (Route::Grants, "GET") => {
            let a = user().await?;
            Ok(Reply::json(200, h.identity(a).await?.map_or(json!([]), |f| list_json(a, &f))))
        }
        (Route::Grants, "POST") => {
            let a = user().await?;
            let g = parse_new_grant(call.body)?;
            let op = IdentityOp::PutMcpGrant { client: g.client, redirect_hosts: g.redirect_hosts, write: g.write };
            let cred = CredentialShard { account: Some(a), key: None };
            grant_op(h, a, IdentityCmd { cred, op }, 201).await
        }
        (Route::Generation(r), "POST") => {
            let generation = parse_generation(call.body)?;
            let op = IdentityOp::McpGeneration { grant: r.grant, generation };
            grant_op(h, r.account, IdentityCmd { cred: OPERATOR, op }, 200).await
        }
        (Route::Grant(r), "DELETE") => {
            let a = user().await?;
            if a != r.account {
                return Err(Reply::not_found());
            }
            let cred = CredentialShard { account: Some(a), key: None };
            grant_op(h, a, IdentityCmd { cred, op: IdentityOp::RevokeMcpGrant { grant: r.grant } }, 204).await
        }
        _ => Err(Reply::not_found()),
    }
}

/// `POST /api/v3/applications/{client_id}/token/scoped` for the install's `Graftyard MCP`. `None` for any other
/// client, and when the App cannot be read, which the edge answers as it did before (404).
pub async fn token_scoped<H: Host>(h: &H, client_id: &str, call: Call<'_>) -> Option<Reply> {
    let app = h.mcp_app().await.ok()??;
    if app.client.as_str() != client_id {
        return None;
    }
    Some(scoped_inner(h, &app, call).await.unwrap_or_else(|r| r))
}

async fn scoped_inner<H: Host>(h: &H, app: &AppRecord, call: Call<'_>) -> Result<Reply, Reply> {
    let bad = || invalid("Authorization", "repositories", "invalid", SCOPED_DOC);
    if !h.enabled() {
        return Err(unavailable(
            "token/scoped needs record formats 20.4 and 17.4, which FormatsEnable has not enabled",
        ));
    }
    client_auth(Some(app), call.authorization)?;
    let r = parse_scoped(call.body)?;
    let (grant, write) = (r.grant, r.write);
    // "Invalid tokens will return `404 NOT FOUND`" (GHES 3.22): unknown, revoked, lapsed, or another App's.
    let unscoped = OpaqueToken::parse(&r.access_token).map_err(|_| Reply::not_found())?;
    let row = h.token(&r.access_token).await?;
    let user = user_of(row.as_ref().map(|(r, f)| (r, f.as_ref())), app, h.now()).map_err(|_| Reply::not_found())?;
    check_grant(user, grant, write, h.identity(user).await?.as_ref())?;
    let mut fields = TokenFields {
        app: Some(app.id),
        client: Some(app.client),
        grant: Some(grant.grant),
        write: Some(write),
        ..TokenFields::default()
    };
    if let Some(p) = r.permissions {
        if !p.within(&app.permissions) {
            return Err(invalid("Authorization", "permissions", "invalid", SCOPED_DOC));
        }
        fields.permissions = Some(p);
    }
    let owner = match (&r.target, r.target_id) {
        (Some(t), _) => {
            Some(h.account_of(t).await?.ok_or_else(|| invalid("Authorization", "target", "invalid", SCOPED_DOC))?)
        }
        (None, id) => id,
    };
    let mut repos: SmallVec<[RepoId; 4]> = SmallVec::new();
    match owner {
        Some(o) => {
            for name in &r.repositories {
                repos.push(h.repo(o, name).await?.ok_or_else(bad)?);
            }
            repos.extend(r.repository_ids.iter().copied());
            fields.resource_owner = Some(o);
        }
        None if !r.repositories.is_empty() || !r.repository_ids.is_empty() => return Err(bad()),
        None => {}
    }
    if !repos.is_empty() {
        fields.repos = Some(RepoScope::Selected(repos));
    }
    // An hour, as the `McpToken` that seals it, and never past the token it narrows.
    let hour = Timestamp(h.now().0 + SCOPED_TTL_MS);
    let expires = row.and_then(|(r, _)| r.expires).map_or(hour, |e| e.min(hour));
    let tok = h.draw();
    // The scoped token's row first, then the grant's commit point: `McpScoped` re-checks the grant and the write scope
    // on the leader and records both tokens under the grant, so a revocation that commits after it revokes them, and
    // one that committed before refuses it, and the token is revoked here. The token text leaves only after both.
    let op = TokensOp::StoreWith {
        hash: tok.hash(),
        kind: TokenKind::UserToServer,
        account: user,
        scopes: graftyard_identity::ScopeSet(0),
        expires: Some(expires),
        fields,
    };
    if let Err(e) = h.commit_tokens(TokensCmd { cred: OPERATOR, op }).await? {
        return Err(unavailable(&format!("the scoped token did not commit: {e}")));
    }
    let cred = CredentialShard {
        account: Some(user),
        key: Some(CredentialKey::Token { shard: unscoped.shard(), hash: unscoped.hash() }),
    };
    let op = IdentityOp::McpScoped { grant: grant.grant, write, token: tok.hash(), expires };
    if let Err(e) = h.commit_identity(user, IdentityCmd { cred, op }).await? {
        revoke_all(h, &[tok.hash()]).await?;
        return Err(match e.as_str() {
            "Unauthorized" => Reply::not_found(),
            _ => invalid("Authorization", "graftyard_grant", "invalid", SCOPED_DOC),
        });
    }
    let listed = CredentialRef::Token { hash: tok.hash(), kind: TokenKind::UserToServer };
    h.list_credential(user, listed, &format!("App {}", app.client.as_str())).await;
    let web = h.web();
    let id = i64::from(u32::from_be_bytes(tok.hash()[..4].try_into().expect("4")) >> 1);
    let app_url = format!("{web}/apps/{}", app.slug.as_str());
    let reply = scoped_json(
        &format!("{web}/api/v3"),
        id,
        &tok.text(),
        &tok.hash(),
        app,
        &app_url,
        h.user_json(user).await,
        h.now(),
        Some(expires),
    );
    Ok(Reply::json(200, reply))
}

/// A scoped token's lifetime: the `McpToken`'s one hour (A7 step 6), so a token graft-mcp no longer serves is dead
/// upstream too [inference: the design gives the `McpToken` an hour and the scoped token no lifetime of its own].
pub const SCOPED_TTL_MS: u64 = 3_600_000;
