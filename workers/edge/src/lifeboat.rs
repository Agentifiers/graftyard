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
    let registered = || graftyard_lookup::view::with(|r| r.fold.lifeboats.contains_key(host));
    // A host the cached view does not hold gets one more look after a refresh, as a signer does on the release
    // endpoint: its `LifeboatAdd` may have committed since.
    if !registered() {
        Box::pin(graftyard_lookup::view::refresh_now(&c.bindings(), &anchor)).await;
    }
    if !registered() {
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

// ---- the lifeboat's front (`decisions/DECISIONS.md`, "`lifeboat-bundle`") ----
//
// The same edge build, run in the lifeboat bundle with `GRAFTYARD_LIFEBOAT=1`: every member call reaches the chain's
// one local learner (`graftyard_lookup::set_lifeboat`), so the edge's own read routes answer from the copy. Writes
// answer 503, a token is a caller only through its `LifeboatRead` row, and every response carries the copy's lag.

/// Whether this edge is the lifeboat's front (`GRAFTYARD_LIFEBOAT=1`).
pub fn mode(env: &Env) -> bool {
    env.var("GRAFTYARD_LIFEBOAT").is_ok_and(|v| v.to_string() == "1")
}

/// Whether the lifeboat's front serves `method` on `path`: a read, a git fetch's POST, or a GraphQL query (a mutation
/// is refused once parsed, `refuses_graphql`). Never a write, the export endpoints, the release endpoint, a token
/// mint, or a test route: the lifeboat is read-only, and it is no export source.
pub fn serves(method: &str, path: &str) -> bool {
    let off = [
        graftyard_git::export::EXPORT_PREFIX,
        crate::release::ENTRIES_PATH,
        crate::release::ERRORS_PATH,
        TOKENS_PATH,
        "/_bed/",
    ];
    if off.iter().any(|p| path.starts_with(p)) {
        return false;
    }
    match method {
        "GET" | "HEAD" => true,
        "POST" => path.ends_with("/git-upload-pack") || path == "/api/graphql",
        _ => false,
    }
}

/// The answer to a request the lifeboat does not serve, as GitHub answers a write it cannot take.
pub fn read_only(c: &Ctx<'_>, path: &str) -> Result<Response> {
    if path.ends_with("/git-receive-pack") || path.ends_with("/info/refs") {
        return crate::text(c.rid, 503, "the lifeboat is read-only\n");
    }
    crate::agents::respond(c, Reply::error(503, "the lifeboat is read-only", "rest"))
}

/// The caller of a lifeboat request: the account of the token's `LifeboatRead` row on its own `Tokens` shard
/// (`{"lifeboat": hash}`), and never a `Stored` row's, so a main-host token is no caller here (D1).
pub async fn principal(c: &Ctx<'_>, t: &str) -> Result<Option<graftyard_identity::Principal>> {
    let Ok(tok) = OpaqueToken::parse(t) else { return Ok(None) };
    let hash = tok.hash();
    let q = json!({ "lifeboat": graftyard_types::Digest(hash).hex() });
    let v = crate::system_read(c, ChainId::Tokens(tok.shard()), q).await.map_err(Error::RustError)?;
    Ok(v["lifeboat"].as_i64().map(|a| graftyard_identity::Principal {
        actor: Actor::User(graftyard_identity::UserId(graftyard_types::AccountId(graftyard_types::DbId(a)))),
        scopes: graftyard_identity::ScopeSet(1),
        credential: graftyard_identity::Credential::Stored(hash),
        sso_orgs: smallvec::SmallVec::new(),
    }))
}

/// The copy's status from the puller (`PULLER`, `crate::lifeboat` of the site build), or `None` if it did not answer.
pub async fn status(env: &Env) -> Option<graftyard_lifeboat::status::LifeboatStatus> {
    let stub = env.durable_object("PULLER").ok()?.get_by_name(graftyard_lifeboat::PULLER_OBJECT).ok()?;
    let mut r = stub.fetch_with_str("https://puller/status").await.ok()?;
    let v: Value = r.json().await.ok()?;
    let decoder = graftyard_types::Digest::parse_hex(v["decoder"].as_str()?)?;
    let at = v["current_at"].as_u64().unwrap_or(0);
    Some(graftyard_lifeboat::status::LifeboatStatus::at(graftyard_types::ReleaseId(decoder.0), at, crate::now().0))
}

/// `res` with the lag header, and for `/api/v3/meta` the status beside GitHub's own fields. A copy whose puller does
/// not answer reads as never current (`LifeboatStatus::at` from 0), so no reader takes it for fresh.
pub async fn stamp(env: &Env, res: Response, path: &str) -> Result<Response> {
    let st = status(env).await.unwrap_or_else(|| {
        graftyard_lifeboat::status::LifeboatStatus::at(graftyard_types::ReleaseId([0; 32]), 0, crate::now().0)
    });
    let headers = res.headers().clone();
    headers.set(graftyard_lifeboat::status::LAG_HEADER, &st.header())?;
    let status = res.status_code();
    if path == "/api/v3/meta" && status == 200 {
        let mut res = res;
        let mut v: Value = res.json().await?;
        if let (Some(o), Value::Object(m)) = (v.as_object_mut(), st.meta()) {
            o.extend(m);
        }
        return Ok(Response::from_json(&v)?.with_status(status).with_headers(headers));
    }
    Ok(Response::from_body(res.body().clone())?.with_status(status).with_headers(headers))
}

#[cfg(test)]
mod tests {
    use super::serves;

    #[test]
    fn the_lifeboat_front_serves_reads_and_fetches_and_nothing_else() {
        for (m, p) in [
            ("GET", "/api/v3/repos/o/r/pulls"),
            ("HEAD", "/api/v3/meta"),
            ("GET", "/o/r.git/info/refs"),
            ("POST", "/o/r.git/git-upload-pack"),
            ("POST", "/api/graphql"),
        ] {
            assert!(serves(m, p), "{m} {p}");
        }
        for (m, p) in [
            ("POST", "/api/v3/repos/o/r/issues"),
            ("PATCH", "/api/v3/repos/o/r/pulls/1"),
            ("DELETE", "/api/v3/repos/o/r"),
            ("PUT", "/api/v3/repos/o/r/pulls/1/merge"),
            ("POST", "/o/r.git/git-receive-pack"),
            ("GET", "/api/graftyard/v1/export/batches"),
            ("GET", "/api/graftyard/v1/export/chains/ids/entries?from=1"),
            ("POST", "/api/graftyard/v1/release/entries"),
            ("POST", "/api/graftyard/v1/lifeboat/tokens"),
            ("GET", "/_bed/status"),
        ] {
            assert!(!serves(m, p), "{m} {p}");
        }
    }
}
