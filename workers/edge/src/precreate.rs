//! Install creates every `Tokens` shard's group ahead of time (`decisions/OPEN-QUESTIONS.md`, R20, decided by the
//! operator: `Tokens` pre-created, `Names` create-on-first-use). A code's row lands on one of 3,072 shards by its hash,
//! and creating a group on the request path cost a device-code request a median 13.0 s (`tests/m3/results/m3-hosted.md`,
//! "Latency"); most of it is each new member measuring its peers on `/join`.
//!
//! One invocation cannot create 3,072 groups: each create is about ten subrequests, and an invocation may hold six
//! connections waiting at once (`research/cloudflare.md`, Workers limits). So `graftyard install` posts ranges of at
//! most `RANGE_MAX` shards, a few at once, after the genesis commits. `create` is idempotent (a probe that answers
//! returns at once), so a failed range is retried by running `install` again.
//!
//! `POST /api/graftyard/v1/release/tokens?from=F&count=N` carries the signed genesis as its body, the same credential
//! as the entries endpoint: the edge accepts it against its anchor and creates only if it is the genesis the `Release`
//! group holds, so the route fans out for nobody but the install's customer. Answers 200 when every shard in the range
//! leads, 503 with the failures otherwise, 422 for a body that is not the held genesis.

use futures_util::StreamExt;
use graftyard_identity::TOKEN_SHARDS;
use graftyard_lookup::create;
use graftyard_release::accept_genesis;
use graftyard_types::{ChainId, PlacementRecord, SystemChain, TokenShard};
use serde_json::{Value, json};
use worker::*;

use crate::Ctx;

pub const TOKENS_PATH: &str = "/api/graftyard/v1/release/tokens";
/// The most shards one request creates.
pub const RANGE_MAX: u16 = 16;
/// Shards created at once inside one invocation: three `/join`s each, under the six-connection limit.
const AT_ONCE: usize = 2;

fn answer(status: u16, v: Value) -> Result<Response> {
    Ok(Response::from_json(&v)?.with_status(status))
}

/// `from` and `count` from the query, `count` clamped to `RANGE_MAX` and the range to the shards that exist.
fn range(url: &Url) -> std::result::Result<std::ops::Range<u16>, String> {
    let q = |k: &str| url.query_pairs().find(|(n, _)| n == k).and_then(|(_, v)| v.parse::<u16>().ok());
    let from = q("from").ok_or("from")?;
    let count = q("count").unwrap_or(RANGE_MAX).min(RANGE_MAX);
    if from >= TOKEN_SHARDS {
        return Err(format!("from {from} is past the last shard ({})", TOKEN_SHARDS - 1));
    }
    Ok(from..from.saturating_add(count).min(TOKEN_SHARDS))
}

/// Creates the groups of `Tokens(shards)` from `record`, `AT_ONCE` at a time.
async fn create_range(c: &Ctx<'_>, record: &PlacementRecord, shards: std::ops::Range<u16>) -> Result<Response> {
    let b = c.bindings();
    let (from, to) = (shards.start, shards.end);
    let results: Vec<(u16, std::result::Result<(), String>)> = futures_util::stream::iter(shards)
        .map(|s| {
            let b = &b;
            async move { (s, create(b, ChainId::Tokens(TokenShard(s)), record, &json!({})).await) }
        })
        .buffer_unordered(AT_ONCE)
        .collect()
        .await;
    let failed: Vec<Value> =
        results.iter().filter_map(|(s, r)| r.as_ref().err().map(|e| json!({ "shard": s, "error": e }))).collect();
    let status = if failed.is_empty() { 200 } else { 503 };
    answer(status, json!({ "from": from, "to": to, "created": results.len() - failed.len(), "failed": failed }))
}

pub async fn tokens(c: &Ctx<'_>, req: &mut Request) -> Result<Response> {
    if req.method() != Method::Post {
        return answer(405, json!({ "message": "POST the signed genesis" }));
    }
    let shards = match range(&req.url()?) {
        Ok(r) => r,
        Err(e) => return answer(422, json!({ "message": e })),
    };
    let Some(anchor) = c.anchor else { return answer(422, json!({ "message": "this edge has no trust anchor" })) };
    let bytes = req.bytes().await?;
    let genesis = match accept_genesis(&anchor, &bytes) {
        Ok(g) => g,
        Err(e) => return answer(422, json!({ "message": e.to_string(), "refusal": e.variant() })),
    };
    match graftyard_lookup::read_genesis(&c.bindings()).await {
        Ok(held) if held == bytes => {}
        Ok(_) => return answer(422, json!({ "message": "not the genesis the Release group holds" })),
        Err(e) => return answer(503, json!({ "message": e })),
    }
    let Some(record) = genesis.placement(SystemChain::Tokens).cloned() else {
        return answer(422, json!({ "message": "the genesis places no Tokens chain" }));
    };
    create_range(c, &record, shards).await
}

/// `/_bed/precreate?from=F&count=N`: the same ranges on a test install with no `Release` group, under the
/// `GRAFTYARD_GENESIS` stand-in's `Tokens` record.
#[cfg(feature = "test-faults")]
pub const BED_PATH: &str = "/_bed/precreate";

#[cfg(feature = "test-faults")]
pub async fn bed(c: &Ctx<'_>, req: &Request) -> Result<Response> {
    let token = c.env.secret("BED_TOKEN")?.to_string();
    if req.headers().get("x-bed-token")?.as_deref() != Some(token.as_str()) {
        return crate::text(c.rid, 404, "not found\n");
    }
    let shards = match range(&req.url()?) {
        Ok(r) => r,
        Err(e) => return answer(422, json!({ "message": e })),
    };
    let record = c.system_record(ChainId::Tokens(TokenShard(0))).await.map_err(Error::RustError)?;
    create_range(c, &record, shards).await
}
