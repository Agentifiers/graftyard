//! `POST /api/graftyard/v1/release/entries` (`design/DESIGN.md`, Shape, the release endpoint;
//! `tests/m1/results/m1-cli.md`, "What the edge's release endpoint must accept"): one signed entry, as stored, with no
//! token, because the signatures are the credential.
//!
//! A genesis is checked against this script's trust anchor before `commit` (`accept_genesis`), so a genesis the group
//! would refuse never reaches it. An empty install has no `Release` group: the edge creates it from the genesis's own
//! `Release` record, its members joined with the genesis, which each accepts against its own anchor before it stores
//! anything. After the 201, the edge creates the `Names`, `Tokens`, and `Ids` groups the genesis places and the
//! `Authz(Genesis.account)` chain (`design/DESIGN.md`, "Install operations", Genesis step 5), and opens that chain as an
//! `Org`. A failure there is retried by the next `install` (the 409 path), so every step is idempotent: joins are, and a
//! second `Open` is refused `AlreadyOpened`.
//!
//! Every other entry must decode, then goes to the group, whose `decide` runs every signer and state rule
//! (`graftyard_release::ReleaseFold::decide`). The design's edge-side check against its cached `Keys` waits for the
//! cached `Release` view (M5-WP2); until then the group is the only judge, which refuses the same entries.
//!
//! Answers: 201 `{"index", "digest"}` after `Acked`; 409 `{"message": "stale prev", "prev", "index"}` when `prev` is
//! not the group's last digest (for a genesis, when the group holds one); 422 `{"message", "refusal"}` when the edge
//! or `decide` refuses.

use graftyard_identity::{AccountKind, account_shard};
use graftyard_lookup::{create, system_of};
use graftyard_release::{ReleaseEntry, SignedEntry, accept_genesis};
use graftyard_types::{ChainId, PlacementRecord, SystemChain, TokenShard};
use serde_json::{Value, json};
use worker::*;

use crate::Ctx;

pub const ENTRIES_PATH: &str = "/api/graftyard/v1/release/entries";

fn answer(status: u16, v: Value) -> Result<Response> {
    Ok(Response::from_json(&v)?.with_status(status))
}

fn refused(variant: &str, message: String) -> Result<Response> {
    answer(422, json!({ "message": message, "refusal": variant }))
}

pub async fn entries(c: &Ctx<'_>, req: &mut Request) -> Result<Response> {
    // Reads (`GET`) take an App JWT of the install's `graftyard-agent` and are AM3's (`design/AGENT-LAYER.md`, "API
    // reference"); not built here.
    if req.method() != Method::Post {
        return answer(405, json!({ "message": "POST a signed entry" }));
    }
    let Some(anchor) = c.anchor else {
        return refused(
            "BadGenesis",
            "this edge has no trust anchor (GRAFTYARD_INSTALL, GRAFTYARD_CUSTOMER_KEY)".into(),
        );
    };
    let bytes = req.bytes().await?;
    let entry = match SignedEntry::decode(&bytes) {
        Ok(e) => e,
        Err(e) => return refused("Malformed", format!("malformed entry: {e}")),
    };
    if !matches!(entry.body, ReleaseEntry::Genesis(_)) {
        let record = match c.system_record(ChainId::Release).await {
            Ok(r) => r,
            Err(e) => return answer(503, json!({ "message": format!("no Release view: {e}") })),
        };
        return match submit(c, &record, &bytes).await {
            Ok((status, out)) => answer(status, out),
            Err(r) => r,
        };
    }
    let genesis = match accept_genesis(&anchor, &bytes) {
        Ok(g) => g,
        Err(e) => return refused(e.variant(), e.to_string()),
    };
    let record: PlacementRecord = match genesis.placement(SystemChain::Release) {
        Some(r) => r.clone(),
        None => return refused("BadGenesis", "the genesis has no Release record".into()),
    };
    let b = c.bindings();
    if let Err(e) = create(&b, ChainId::Release, &record, &json!({ "genesis": graftyard_lookup::hex(&bytes) })).await {
        return answer(503, json!({ "message": format!("the Release group did not start: {e}") }));
    }
    let digest = entry.digest().hex();
    let (status, out) = match submit(c, &record, &bytes).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    // Genesis step 5, after the 201, and again on a retried install that met 409 with its own digest.
    if status == 201 || out["prev"].as_str() == Some(digest.as_str()) {
        let created = after_genesis(c, &genesis).await;
        let mut out = out;
        out["created"] = json!(created);
        return answer(status, out);
    }
    answer(status, out)
}

/// Commits one entry's bytes on the `Release` group: 201 or 409 with the body to send, or the 422 or 503 reply.
async fn submit(
    c: &Ctx<'_>,
    record: &PlacementRecord,
    bytes: &[u8],
) -> std::result::Result<(u16, Value), Result<Response>> {
    let b = c.bindings();
    let body =
        json!({ "rid": graftyard_lookup::request_id().to_string(), "cmd": graftyard_lookup::hex(bytes) }).to_string();
    let hooks = graftyard_lookup::Hooks::default();
    let done =
        match graftyard_lookup::commit_hedged(&b, ChainId::Release, record, body.as_bytes(), c.colo, 60_000, hooks)
            .await
        {
            Ok(d) => d,
            Err(Ok(e)) => {
                return Err(answer(503, json!({ "message": format!("the Release group refused the call: {e}") })));
            }
            Err(Err(e)) => return Err(answer(503, json!({ "message": e }))),
        };
    if let Some(ok) = done.get("ok") {
        return Ok((201, json!({ "index": ok["index"], "digest": ok.pointer("/out/digest") })));
    }
    if let Some(r) = done.get("refused") {
        return match r["status"].as_u64() {
            Some(409) => Ok((409, json!({ "message": "stale prev", "prev": r["prev"], "index": r["index"] }))),
            _ => Err(refused(r["err"].as_str().unwrap_or("Malformed"), r["message"].as_str().unwrap_or("").into())),
        };
    }
    Err(answer(503, json!({ "message": format!("no outcome: {done}") })))
}

/// Creates the groups the accepted genesis places, and opens `Authz(Genesis.account)`. Returns what each step did.
/// `Names` and `Tokens` are sharded (1,024 and 3,072 shards); this creates the shard the install account's rows live
/// on and token shard 0. `graftyard install` then creates every other `Tokens` shard in ranges through
/// `precreate::TOKENS_PATH` (R20), and every other `Names` shard is created on first use from the same genesis
/// record, as the lookup finds no group (`graftyard_lookup::create`).
async fn after_genesis(c: &Ctx<'_>, g: &graftyard_release::Genesis) -> Value {
    let b = c.bindings();
    let mut steps = Vec::new();
    let account = g.account;
    let names_rec = g.placement(SystemChain::Names).cloned();
    let mut groups: Vec<(ChainId, Option<PlacementRecord>)> = vec![
        (ChainId::Names(account_shard(account)), names_rec.clone()),
        (ChainId::Ids, g.placement(SystemChain::Ids).cloned()),
        (ChainId::Tokens(TokenShard(0)), g.placement(SystemChain::Tokens).cloned()),
    ];
    for (chain, rec) in groups.drain(..) {
        let r = match rec {
            Some(r) => create(&b, chain, &r, &json!({})).await,
            None => Err(format!("the genesis places no {:?}", system_of(chain))),
        };
        steps.push(json!({ "chain": chain, "created": r.as_ref().is_ok(), "error": r.err() }));
    }
    // `Authz(account)`'s record is the install account's `Placement` row: the system chains' record, as genesis
    // placed them, written once by `InitPlacement` (refused `RecordExists` on a retry).
    let Some(rec) = names_rec else { return json!(steps) };
    let names = ChainId::Names(account_shard(account));
    let init = json!({ "cred": { "account": null, "key": null }, "op": { "InitPlacement": { "owner": account, "record": rec } } });
    let placed = graftyard_lookup::commit(&b, names, &rec, init).await;
    steps.push(json!({ "chain": names, "op": "InitPlacement", "outcome": placed.as_ref().ok(), "error": placed.as_ref().err() }));
    let authz = ChainId::Authz(account);
    let r = create(&b, authz, &rec, &json!({})).await;
    steps.push(json!({ "chain": authz, "created": r.is_ok(), "error": r.err() }));
    let open = json!({ "cred": { "account": null, "key": null }, "op": { "Open": { "kind": AccountKind::Org, "enterprise": null } } });
    let opened = graftyard_lookup::commit(&b, authz, &rec, open).await;
    steps
        .push(json!({ "chain": authz, "op": "Open", "outcome": opened.as_ref().ok(), "error": opened.as_ref().err() }));
    json!(steps)
}
