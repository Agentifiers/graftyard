//! `POST /api/graftyard/v1/lifeboat/tokens` (`design/DESIGN.md`, Shape, "Lifeboat feed"; `decisions/DECISIONS.md`,
//! "`lifeboat-revoke`"): `{"host": ...}` under the developer's own token mints a `lifeboat:read` token for a
//! registered lifeboat. The edge verifies the developer and commits `StoreLifeboat` as the operator on
//! `lifeboat_shard(account)`, one group write and no Cloudflare API call. The entry revokes every earlier lifeboat
//! token of the developer. Answers 503 until `FormatsEnable` (the test edge's `FORMATS_ENABLED`) enabled 0x11.5.

use graftyard_identity::{Actor, OpaqueToken, TOKENS_FORMAT_V5, TokenKind, lifeboat_shard};
use graftyard_types::ChainId;
use graftyard_wire::Reply;
use serde_json::{Value, json};
use worker::*;

use crate::Ctx;
use crate::ident::commit_system;

pub const TOKENS_PATH: &str = "/api/graftyard/v1/lifeboat/tokens";

/// Draws a `ghp_` whose hash lands on `lifeboat_shard(account)`: one shard in 3,072, about 3,072 draws.
fn draw(account: graftyard_types::AccountId) -> Option<OpaqueToken> {
    let rand = || u64::from_le_bytes(crate::rand10()[..8].try_into().expect("8"));
    let (s, _) = graftyard_identity::draw_secret_on(rand, lifeboat_shard(account), 200_000)?;
    OpaqueToken::new(TokenKind::Pat, s)
}

pub async fn mint(c: &Ctx<'_>, req: &mut Request) -> Result<Reply> {
    if req.method() != Method::Post {
        return Ok(Reply::error(405, "Method Not Allowed", "rest"));
    }
    if !c.cfg.enabled.contains(&TOKENS_FORMAT_V5) {
        return Ok(crate::api::unavailable(
            "lifeboat tokens need record format 17.5, which FormatsEnable has not enabled",
        ));
    }
    let Some(who) = crate::authenticate(c, req).await? else {
        return Ok(Reply::error(401, "Requires authentication", "rest"));
    };
    let Actor::User(me) = who.actor else { return Ok(Reply::error(403, "Forbidden", "rest")) };
    let body: Value = req.json().await.unwrap_or(Value::Null);
    let Some(host) = body["host"].as_str() else { return Ok(Reply::error(422, "host is required", "rest")) };
    let Some(anchor) = c.anchor else { return Ok(crate::api::unavailable("no trust anchor")) };
    Box::pin(graftyard_lookup::view::ensure(&c.bindings(), &anchor, Some(c.ctx))).await;
    if !graftyard_lookup::view::with(|r| r.fold.lifeboats.contains_key(host)) {
        return Ok(Reply::error(422, &format!("no lifeboat {host} is registered"), "rest"));
    }
    let Some(tok) = draw(me.0) else { return Ok(crate::api::unavailable("no token landed on the lifeboat shard")) };
    let cmd = json!({ "cred": { "account": null, "key": null },
        "op": { "StoreLifeboat": { "hash": tok.hash(), "account": me.0 } } });
    let done = match commit_system(c, ChainId::Tokens(tok.shard()), cmd).await {
        Ok(d) => d,
        Err(x) => return Ok(x),
    };
    if let Some(e) = done.pointer("/refused/err") {
        return Ok(Reply::json(422, json!({ "message": format!("the token was refused: {e}") })));
    }
    Ok(Reply::json(201, json!({ "token": tok.text(), "host": host })))
}
