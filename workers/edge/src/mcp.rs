//! The edge's host for `graftyard_api::mcp` (`build/BUILD-PLAN.md`, AM2-WP1): the grant endpoints under
//! `/api/graftyard/v1/mcp/grants` and `POST /api/v3/applications/{client_id}/token/scoped` for the install's
//! `Graftyard MCP`. The rules and replies are that module's; this one reads the folds fresh, never through `rcache`,
//! and commits on the `Identity` and `Tokens` chains.

use graftyard_api::mcp::{self, Call, Route};
use graftyard_identity::{
    AppRecord, AuthzRead as _, CredentialRef, IdentityCmd, IdentityFold, IdentityOut, OpaqueToken, TokenFields,
    TokenKind, TokenRow, TokensCmd, install_account,
};
use graftyard_types::{AccountId, ChainId, DbId, Digest, RepoId, Timestamp};
use graftyard_wire::Reply;
use serde_json::{Value, json};
use worker::*;

use crate::Ctx;
use crate::ident::{commit_owner, commit_system, refused};

struct Edge<'a, 'c>(&'a Ctx<'c>);

fn unavailable(e: impl std::fmt::Display) -> Reply {
    crate::api::unavailable(&e.to_string())
}

impl mcp::Host for Edge<'_, '_> {
    fn now(&self) -> Timestamp {
        crate::now()
    }
    fn web(&self) -> String {
        format!("https://{}", self.0.cfg.host)
    }
    fn enabled(&self) -> bool {
        let e = &self.0.cfg.enabled;
        e.contains(&graftyard_identity::IDENTITY_FORMAT_V4) && e.contains(&graftyard_identity::TOKENS_FORMAT_V4)
    }
    fn draw(&self) -> OpaqueToken {
        let rand = || u64::from_le_bytes(crate::rand10()[..8].try_into().expect("8"));
        loop {
            if let Ok((s, _)) = graftyard_identity::draw_secret(rand, graftyard_identity::Family::Na)
                && let Some(t) = OpaqueToken::new(TokenKind::UserToServer, s)
            {
                return t;
            }
        }
    }
    async fn mcp_app(&self) -> std::result::Result<Option<AppRecord>, Reply> {
        let Some(f) = crate::ident::authz_of(self.0, install_account()).await.map_err(unavailable)? else {
            return Ok(None);
        };
        Ok(f.more
            .more
            .agent_apps
            .values()
            .filter(|a| a.kind == graftyard_identity::AgentKind::Mcp)
            .filter_map(|a| f.app(a.app))
            .find(|r| r.slug.as_str() == mcp::MCP_SLUG)
            .cloned())
    }
    async fn token(&self, text: &str) -> std::result::Result<Option<(TokenRow, Option<TokenFields>)>, Reply> {
        let Ok(tok) = OpaqueToken::parse(text) else { return Ok(None) };
        let q = json!({ "token": Digest(tok.hash()).hex() });
        let v = crate::system_read(self.0, ChainId::Tokens(tok.shard()), q).await.map_err(unavailable)?;
        let row = &v["token"];
        let parsed = (|| {
            Some(TokenRow {
                kind: serde_json::from_value(row["kind"].clone()).ok()?,
                account: AccountId(DbId(row["account"].as_i64()?)),
                scopes: graftyard_identity::ScopeSet(1),
                expires: serde_json::from_value(row["expires"].clone()).ok()?,
                revoked: row["revoked"].as_bool()?,
            })
        })();
        Ok(parsed.map(|r| (r, serde_json::from_value(row["fields"].clone()).ok().flatten())))
    }
    async fn identity(&self, a: AccountId) -> std::result::Result<Option<IdentityFold>, Reply> {
        let Some(rec) = crate::placement(self.0, a).await.map_err(unavailable)? else { return Ok(None) };
        let body = json!({ "q": { "dep": true } }).to_string();
        let got = graftyard_lookup::call_opt(&self.0.bindings(), ChainId::Identity(a), &rec, "/read", body.as_bytes())
            .await
            .map_err(unavailable)?;
        let Some((v, _, _)) = got else { return Ok(None) };
        let bytes = v["value"].as_str().and_then(graftyard_lookup::unhex).ok_or_else(|| unavailable("not hex"))?;
        graftyard_raft::codec::from_bytes(&bytes).map(Some).map_err(unavailable)
    }
    async fn commit_identity(
        &self,
        a: AccountId,
        cmd: IdentityCmd,
    ) -> std::result::Result<std::result::Result<IdentityOut, String>, Reply> {
        let v = serde_json::to_value(&cmd).map_err(unavailable)?;
        let done = commit_owner(self.0, ChainId::Identity(a), a, v).await?;
        if let Some(e) = refused(&done) {
            return Ok(Err(e));
        }
        serde_json::from_value(done["ok"]["out"].clone()).map(Ok).map_err(unavailable)
    }
    async fn commit_tokens(&self, cmd: TokensCmd) -> std::result::Result<std::result::Result<(), String>, Reply> {
        let chain = ChainId::Tokens(cmd.shard());
        let done = commit_system(self.0, chain, serde_json::to_value(&cmd).map_err(unavailable)?).await?;
        Ok(match refused(&done) {
            Some(e) => Err(e),
            None if done.get("ok").is_some() => Ok(()),
            None => return Err(unavailable(format!("the token command did not commit: {done}"))),
        })
    }
    async fn list_credential(&self, a: AccountId, c: CredentialRef, title: &str) {
        crate::ident::list_credential(self.0, a, c, title).await;
    }
    async fn account_of(&self, login: &str) -> std::result::Result<Option<AccountId>, Reply> {
        crate::account_of(self.0, login).await.map_err(unavailable)
    }
    async fn repo(&self, owner: AccountId, name: &str) -> std::result::Result<Option<RepoId>, Reply> {
        let Some(login) = crate::ident::login(self.0, owner).await else { return Ok(None) };
        match crate::resolve(self.0, &login, name).await {
            Ok(Ok(r)) => Ok(r.map(|r| r.id)),
            Ok(Err(e)) => Err(unavailable(e)),
            Err(e) => Err(unavailable(e)),
        }
    }
    async fn user_json(&self, a: AccountId) -> Value {
        let login = crate::ident::login(self.0, a).await.unwrap_or_default();
        crate::ident::user_json(self.0, a, &login, false)
    }
}

async fn call_parts(req: &mut Request) -> Result<(String, Option<String>, Option<String>, Value)> {
    let method = req.method().to_string();
    let auth = req.headers().get("authorization")?;
    let user = req.headers().get("x-graftyard-user-token")?;
    let body = if method == "POST" { req.json().await.unwrap_or(Value::Null) } else { Value::Null };
    Ok((method, auth, user, body))
}

/// One request under `/api/graftyard/v1/mcp/grants`.
pub async fn grants(c: &Ctx<'_>, req: &mut Request, route: Route) -> Result<Reply> {
    let (method, auth, user, body) = call_parts(req).await?;
    let call = Call { method: &method, authorization: auth.as_deref(), user_token: user.as_deref(), body: &body };
    Ok(mcp::grants(&Edge(c), route, call).await)
}

/// `POST /api/v3/applications/{client_id}/token/scoped`: `Graftyard MCP`'s, or 404 for any other client.
pub async fn token_scoped(c: &Ctx<'_>, req: &mut Request, client_id: &str) -> Result<Reply> {
    let (method, auth, user, body) = call_parts(req).await?;
    let call = Call { method: &method, authorization: auth.as_deref(), user_token: user.as_deref(), body: &body };
    Ok(mcp::token_scoped(&Edge(c), client_id, call).await.unwrap_or_else(Reply::not_found))
}
