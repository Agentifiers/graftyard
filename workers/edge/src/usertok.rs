//! BF-31's tokens at the edge (`build/BUILD-PLAN.md`, M3-WP2): an App's user-to-server `ghu_` with its `ghr_`
//! refresh token, `grant_type=refresh_token`, and a fine-grained `github_pat_` scoped to one owner's repositories.
//!
//! - A `ghu_` and its `ghr_` commit in one `StorePair` entry on one `Tokens` shard, so revoking either revokes both
//!   (`TokensEvent::Revoked` applies to the pair). The `ghu_` lives 8 hours and the `ghr_` 6 months [doc: GitHub,
//!   "Refreshing user access tokens"; unverified for GHES 3.22].
//! - A refresh first revokes the old pair through the `ghr_` (the commit point: a `ghr_` is used once), then stores a
//!   new pair. An edge that dies between the two leaves the user signed out of that App, never with two live pairs.
//! - A `ghr_` is never a bearer credential, and a lapsed token is no caller (`usable`): `decide` sees no clock, so the
//!   edge checks `expires` when it authenticates, as for every stored token.
//! - A `github_pat_` is drawn and stored by `POST /api/graftyard/v1/user/tokens` (test installs stand in for the
//!   settings page, which is not built) under the caller's own stored token, so the leader's `acts_for` is the check.
//!   Its scope is its row's fields; every write's leader checks it (`graftyard_identity::token_allows`), and the REST
//!   repository reads check it here (`reaches`).

use graftyard_identity::{Access, AppId, ClientId, OpaqueToken, PermissionSet, RepoScope, TokenFields, TokenKind};
use graftyard_types::{AccountId, ChainId, Timestamp};
use graftyard_wire::Reply;
use serde_json::{Value, json};
use worker::*;

use crate::Ctx;
use crate::ident::commit_system;

/// The test installs' stand-in for the fine-grained token settings page.
pub const TOKENS_PATH: &str = "/api/graftyard/v1/user/tokens";
/// A `ghu_`'s lifetime [doc: GitHub's 8 hours].
pub const USER_TOKEN_TTL_MS: u64 = 8 * 3_600_000;
/// A `ghr_`'s lifetime [doc: GitHub's 6 months, taken as 184 days].
pub const REFRESH_TOKEN_TTL_MS: u64 = 184 * 24 * 3_600_000;

/// Whether a stored row may authenticate a request: never a `ghr_`, and only before its `expires`.
pub fn usable(kind: Option<TokenKind>, expires: Option<Timestamp>, now: Timestamp) -> bool {
    kind != Some(TokenKind::Refresh) && expires.is_none_or(|e| e > now)
}

/// Whether the authenticated token reaches the repository for a read: a `github_pat_`'s `resource_owner` and
/// `repos`, as `token_allows` checks them for a write. `Some` is the reply: 404 for a private repository.
pub fn reaches(c: &Ctx<'_>, repo: &dyn graftyard_identity::RepoAccess) -> Option<Reply> {
    let f = c.token_fields.borrow();
    match graftyard_identity::token_allows(f.as_ref(), &[], repo) {
        graftyard_identity::Decision::Allow => None,
        graftyard_identity::Decision::Forbidden => {
            Some(Reply::error(403, "Resource not accessible by personal access token", "rest"))
        }
        graftyard_identity::Decision::Hidden => Some(Reply::not_found()),
    }
}

fn rand_u64() -> u64 {
    let b = crate::rand10();
    u64::from_le_bytes(b[..8].try_into().expect("8"))
}

/// A `ghu_` and a `ghr_` on the same shard: the `ghr_` secret is redrawn onto the `ghu_`'s (`draw_secret_on`).
fn draw_pair() -> std::result::Result<(OpaqueToken, OpaqueToken), Reply> {
    let fam = graftyard_identity::Family::Na;
    let (s, _) = graftyard_identity::draw_secret(rand_u64, fam).map_err(|e| crate::api::unavailable(&e.to_string()))?;
    let u = OpaqueToken::new(TokenKind::UserToServer, s).ok_or_else(|| crate::api::unavailable("token kind"))?;
    let (rs, _) = graftyard_identity::draw_secret_on(rand_u64, u.shard(), 200_000)
        .ok_or_else(|| crate::api::unavailable("no refresh token landed on the user token's shard"))?;
    let r = OpaqueToken::new(TokenKind::Refresh, rs).ok_or_else(|| crate::api::unavailable("token kind"))?;
    Ok((u, r))
}

/// Stores a new `ghu_`/`ghr_` pair for `account` and the App, then lists the `ghu_` on the account's `Identity`
/// chain. Returns the OAuth answer's fields.
pub async fn issue_pair(
    c: &Ctx<'_>,
    account: AccountId,
    app: AppId,
    client: ClientId,
) -> std::result::Result<Value, Reply> {
    let (u, r) = draw_pair()?;
    let now = crate::now().0;
    let fields = TokenFields { app: Some(app), client: Some(client), ..TokenFields::default() };
    let cmd = json!({ "cred": { "account": null, "key": null }, "op": { "StorePair": {
        "user": u.hash(), "refresh": r.hash(), "account": account, "scopes": 1,
        "user_expires": now + USER_TOKEN_TTL_MS, "refresh_expires": now + REFRESH_TOKEN_TTL_MS, "fields": fields } } });
    let done = commit_system(c, ChainId::Tokens(u.shard()), cmd).await?;
    if done.get("ok").is_none() {
        return Err(crate::api::unavailable(&format!("the token pair did not commit: {}", done["refused"]["err"])));
    }
    let cred = graftyard_identity::CredentialRef::Token { hash: u.hash(), kind: TokenKind::UserToServer };
    crate::ident::list_credential(c, account, cred, &format!("App {}", client.as_str())).await;
    Ok(json!({
        "access_token": u.text(), "expires_in": USER_TOKEN_TTL_MS / 1000,
        "refresh_token": r.text(), "refresh_token_expires_in": REFRESH_TOKEN_TTL_MS / 1000,
        "token_type": "bearer", "scope": "",
    }))
}

/// `grant_type=refresh_token`: the `ghr_` is spent by revoking its pair, then a new pair is stored. A `ghr_` that is
/// unknown, revoked, lapsed, or another client's is `bad_refresh_token`.
pub async fn refresh(c: &Ctx<'_>, client: ClientId, app: AppId, text: &str) -> std::result::Result<Value, Reply> {
    let bad = || {
        Reply::json(
            200,
            json!({ "error": "bad_refresh_token", "error_description": "The refresh token passed is incorrect or expired.",
                "error_uri": "https://docs.github.com/apps/managing-oauth-apps/troubleshooting-oauth-app-access-token-request-errors/#bad-refresh-token" }),
        )
    };
    let Ok(tok) = OpaqueToken::parse(text) else { return Err(bad()) };
    if tok.kind() != TokenKind::Refresh {
        return Err(bad());
    }
    let chain = ChainId::Tokens(tok.shard());
    let v = crate::system_read(c, chain, json!({ "token": graftyard_types::Digest(tok.hash()).hex() }))
        .await
        .map_err(|e| crate::api::unavailable(&e))?;
    let row = &v["token"];
    let account = row["account"].as_i64();
    let lapsed = row["expires"].as_u64().is_some_and(|e| e <= crate::now().0);
    let ours = row["fields"]["client"].as_str() == Some(client.as_str());
    let (Some(account), false, false, true) = (account, row["revoked"].as_bool().unwrap_or(true), lapsed, ours) else {
        return Err(bad());
    };
    let cmd = json!({ "cred": { "account": null, "key": null }, "op": { "Revoke": { "hash": tok.hash() } } });
    let done = commit_system(c, chain, cmd).await?;
    if done.get("ok").is_none() {
        // `AlreadyRevoked`: another refresh spent it first.
        return Err(bad());
    }
    issue_pair(c, AccountId(graftyard_types::DbId(account)), app, client).await
}

/// `POST /api/graftyard/v1/user/tokens` with `{"owner": login, "repositories": [name], "permissions": {"contents":
/// "read"}}`: a `github_pat_` for the caller, stored with `StoreWith` under the caller's own stored token. 201 with
/// the token text, shown once.
pub async fn create_fine_grained(c: &Ctx<'_>, req: &mut Request) -> Result<Reply> {
    let Some(who) = crate::authenticate(c, req).await? else {
        return Ok(Reply::error(401, "Requires authentication", "rest"));
    };
    let graftyard_identity::Actor::User(me) = who.actor else { return Ok(Reply::error(403, "Forbidden", "rest")) };
    let body: Value = req.json().await.unwrap_or(Value::Null);
    let Some(owner) = body["owner"].as_str() else { return Ok(Reply::error(422, "owner is required", "rest")) };
    let Some(owner_id) = crate::account_of(c, owner).await.map_err(Error::RustError)? else {
        return Ok(Reply::not_found());
    };
    let mut repos = smallvec::SmallVec::new();
    for name in body["repositories"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        let Some(r) = crate::resolve(c, owner, name).await?.map_err(Error::RustError)? else {
            return Ok(Reply::error(422, &format!("no repository {owner}/{name}"), "rest"));
        };
        repos.push(r.id);
    }
    let mut perms = PermissionSet::default();
    for (k, v) in body["permissions"].as_object().into_iter().flatten() {
        let level = match v.as_str() {
            Some("read") => Access::Read,
            Some("write") => Access::Write,
            Some("admin") => Access::Admin,
            _ => return Ok(Reply::error(422, &format!("permission {k} has no level"), "rest")),
        };
        perms.0.insert(k.as_str().into(), level);
    }
    let fam = graftyard_identity::Family::Na;
    let (s, _) = graftyard_identity::draw_secret(rand_u64, fam).map_err(|e| e.to_string())?;
    let Some(tok) = OpaqueToken::new(TokenKind::FineGrained, s) else { return Ok(crate::api::unavailable("kind")) };
    let fields = TokenFields {
        resource_owner: Some(owner_id),
        repos: Some(if repos.is_empty() { RepoScope::All } else { RepoScope::Selected(repos) }),
        permissions: Some(perms),
        ..TokenFields::default()
    };
    let cmd = json!({ "cred": who.credential_shard(), "op": { "StoreWith": {
        "hash": tok.hash(), "kind": "FineGrained", "account": me.0, "scopes": 0, "expires": null, "fields": fields } } });
    let done = match commit_system(c, ChainId::Tokens(tok.shard()), cmd).await {
        Ok(d) => d,
        Err(x) => return Ok(x),
    };
    if let Some(e) = done.pointer("/refused/err") {
        return Ok(Reply::json(422, json!({ "message": format!("the token was refused: {e}") })));
    }
    let cred = graftyard_identity::CredentialRef::Token { hash: tok.hash(), kind: TokenKind::FineGrained };
    let title = body["name"].as_str().unwrap_or("fine-grained token");
    crate::ident::list_credential(c, me.0, cred, title).await;
    Ok(Reply::json(
        201,
        json!({ "token": tok.text(), "owner": owner, "id": graftyard_types::Digest(tok.hash()).hex() }),
    ))
}
