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
//! Every other entry must decode, and is checked against this isolate's cached `Release` view before `commit`
//! (`graftyard_release::view::Replica::precheck`): bound to this install and group, every signer a key of the cached
//! `Keys` (or the new customer key of a `Keys`, or a lifeboat's key for its `LifeboatDecoder`), and every signature
//! valid. A signer the view does not hold gets one more look, after a refresh, since a `Keys` committed after the last
//! refresh may name it. The group's `decide` then runs every signer and state rule
//! (`graftyard_release::ReleaseFold::decide`), so a view older than the group's newest `Keys` can pass an entry the
//! group then refuses, never commit one it would not.
//!
//! A `StaleMode` that the group did not commit (no outcome within the commit's bound, or a 503) is verified against
//! the cached `Keys` and written to every reachable `Release` member's `pending` row, outside the log, and answered
//! 202 `{"pending": n, "of": m}` once at least one member kept it (`design/DESIGN.md`, Shape, `STALE_MAX`).
//!
//! Answers: 201 `{"index", "digest"}` after `Acked`; 202 for an uncommitted `StaleMode`; 409 `{"message": "stale
//! prev", "prev", "index"}` when `prev` is not the group's last digest (for a genesis, when the group holds one); 422
//! `{"message", "refusal"}` when the edge or `decide` refuses.

use graftyard_identity::{AccountKind, account_shard};
use graftyard_lookup::{create, system_of, view};
use graftyard_release::{ReleaseEntry, SignedEntry, accept_genesis};
use graftyard_types::{ChainId, PlacementRecord, SystemChain, TokenShard};
use serde_json::{Value, json};
use worker::*;

use crate::Ctx;

pub const ENTRIES_PATH: &str = "/api/graftyard/v1/release/entries";
pub const ERRORS_PATH: &str = "/api/graftyard/v1/release/errors";

fn answer(status: u16, v: Value) -> Result<Response> {
    Ok(Response::from_json(&v)?.with_status(status))
}

/// 422 with the refusal. A refusal the group pages on (`graftyard_release::pages`: a release, pointer, `Rollback`, or
/// `Abort` below an enabled format, or a second drain-key drain) also writes a `PAGE` line, which the install's log
/// alert matches (`design/DESIGN.md`, Shape, "Release group").
fn refused(variant: &str, message: String) -> Result<Response> {
    if graftyard_release::pages(variant) {
        console_error!("PAGE release {variant}: {message}");
    }
    answer(422, json!({ "message": message, "refusal": variant }))
}

pub async fn entries(c: &Ctx<'_>, req: &mut Request) -> Result<Response> {
    if req.method() == Method::Get {
        return read(c, req).await;
    }
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
        let b = c.bindings();
        view::ensure(&b, &anchor, Some(c.ctx)).await;
        // A signer the view does not hold may be one a `Keys` committed since the last refresh added: look again.
        if let Err(graftyard_release::Refusal::UnknownKey(_)) = view::with(|v| v.precheck(&entry)) {
            view::refresh_now(&b, &anchor).await;
        }
        if let Err(e) = view::with(|v| v.precheck(&entry)) {
            return refused(e.variant(), e.to_string());
        }
        let record = match view::release_record() {
            Some(r) => r,
            None => match c.system_record(ChainId::Release).await {
                Ok(r) => r,
                Err(e) => return answer(503, json!({ "message": format!("no Release view: {e}") })),
            },
        };
        let stale = matches!(entry.body, ReleaseEntry::StaleMode { .. });
        // A StaleMode is the entry that must land when groups cannot commit, the Release group among them: its
        // commit is bounded so the pending path below runs while the client still waits.
        let for_ms = if stale { STALE_COMMIT_MS } else { 60_000 };
        return match submit(c, &record, &bytes, for_ms).await {
            Ok((status, out)) => answer(status, out),
            // Only a group that did not commit; one whose `decide` refused the flag answers 422 as usual.
            Err(Ok(r)) if stale && r.status_code() == 503 => pending(c, &record, &bytes).await,
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
    let (status, out) = match submit(c, &record, &bytes, 60_000).await {
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

/// `GET /api/graftyard/v1/release/entries?after=<index>&per_page=<n>` (`design/AGENT-LAYER.md`, "API reference";
/// AM3-WP1): the group's committed entries after index `after` (0 reads from the genesis, index 1), at most `per_page`
/// (default 30, at most 100), each `{index, entry, at}` with `entry` the stored bytes in base64 and `at` the time of the
/// entry it committed in, which the agents view checks it at. A `Link` `rel="next"` names the next page while more
/// remain. Only an App JWT of the install's `graftyard-agent` reads it (`ident::graftyard_agent`); the bytes are
/// signed entries, which carry no secret. Any `Release` member answers from what it applied (`/view`), with no
/// read-index round, so the agents view, which replays and checks every entry itself and never moves backward
/// (`graftyard_release::agents::AgentsReleaseView`), may read one entry behind the group and catches up on its next
/// read. 503 when no member answered.
async fn read(c: &Ctx<'_>, req: &Request) -> Result<Response> {
    use base64::Engine as _;
    if let Err(r) = crate::ident::graftyard_agent(c, req).await {
        return crate::agents::respond(c, r);
    }
    let url = req.url()?;
    let q = |k: &str| url.query_pairs().find(|(n, _)| n == k).and_then(|(_, v)| v.parse::<u64>().ok());
    let after = q("after").unwrap_or(0);
    let per_page = q("per_page").map_or(30, |n| n as usize);
    let record = match c.system_record(ChainId::Release).await {
        Ok(r) => r,
        Err(e) => return answer(503, json!({ "message": format!("no Release group: {e}") })),
    };
    let mut members: Vec<(graftyard_types::SiteId, String)> =
        graftyard_lookup::known_voters(ChainId::Release).into_iter().map(|t| (t.site, t.do_name)).collect();
    if members.is_empty() {
        members =
            record.bootstrap_members(ChainId::Release).into_iter().map(|m| (m.site, m.do_name.to_string())).collect();
    }
    let b = c.bindings();
    let body = json!({ "from": after }).to_string();
    for (site, name) in &members {
        let deadline = std::time::Duration::from_millis(graftyard_release::view::VIEW_CALL_MS);
        let Some((200, v)) = graftyard_lookup::post_within(&b, *site, name, "/view", body.as_bytes(), deadline).await
        else {
            continue;
        };
        let Some(count) = v["count"].as_u64() else { continue };
        let page: Vec<(Vec<u8>, graftyard_types::Timestamp)> = v["entries"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|e| {
                Some((graftyard_lookup::unhex(e[0].as_str()?)?, graftyard_types::Timestamp(e[1].as_u64()?)))
            })
            .collect();
        let out = graftyard_release::agents::feed(after, &page, per_page);
        let last = out.last().map_or(after, |(i, _, _)| i.0);
        let rows: Vec<Value> = out
            .iter()
            .map(|(i, e, at)| {
                json!({ "index": i.0, "entry": base64::engine::general_purpose::STANDARD.encode(e), "at": at.0 })
            })
            .collect();
        let mut r = Response::from_json(&rows)?;
        if last < count {
            let per = per_page.clamp(1, graftyard_release::agents::ENTRIES_PER_PAGE);
            let next = format!("<https://{}{ENTRIES_PATH}?per_page={per}&after={last}>; rel=\"next\"", c.cfg.host);
            r.headers_mut().set("link", &next)?;
        }
        return Ok(r);
    }
    answer(503, json!({ "message": "no Release member answered" }))
}

/// `POST /api/graftyard/v1/release/errors` (AM3-WP1, LM-3; OPEN-QUESTIONS A52, R47): one flush of
/// `graft-agents-front`'s counts of `graft-controller`'s answers, `{"counts": ScriptErrors}`, stored under the
/// `controller` key in the install's `errors` objects as `graft-front` stores its own, so the agents `graft-ops` judges a
/// `ControllerRoll` on `GATE_ERRORS`. Only an App JWT of the install's `graftyard-agent` sends one
/// (`ident::graftyard_agent`), and only a flush `gate::accept_flush` takes against this isolate's `Release` view is
/// stored: every version a `Controller` version the group uploaded, at most `FLUSH_CALLS_MAX` calls. Anything else is
/// 401 or 422 and stores nothing. 204 once one site's object kept it, else 503. `GET ?since=<ms>` under the same JWT
/// answers the `controller` counts since then, summed over every voter site that answered, with `sites` the number
/// that did: what the agents `graft-ops`, in another account, judges the roll on. No Cloudflare API is called.
pub async fn errors(c: &Ctx<'_>, req: &mut Request) -> Result<Response> {
    use graftyard_release::Script;
    use graftyard_release::gate::{self, ScriptErrors};
    if req.method() != Method::Post && req.method() != Method::Get {
        return answer(405, json!({ "message": "POST a flush of graft-controller's counts, or GET them" }));
    }
    if let Err(r) = crate::ident::graftyard_agent(c, req).await {
        return crate::agents::respond(c, r);
    }
    let key = gate::errors_key(Script::Controller);
    let voters: Vec<_> = c.sites.0.iter().filter(|s| !s.spare).map(|s| s.id).collect();
    let deadline = std::time::Duration::from_millis(graftyard_release::view::VIEW_CALL_MS);
    let b = c.bindings();
    if req.method() == Method::Get {
        let since = req.url()?.query_pairs().find(|(k, _)| k == "since").and_then(|(_, v)| v.parse::<u64>().ok());
        let Some(since) = since else { return answer(422, json!({ "message": "GET needs ?since=<ms>" })) };
        // Every voter site at once, each within VIEW_CALL_MS, so the read takes one site's call (OPEN-QUESTIONS A57).
        let (b, path) = (&b, &format!("https://registry/errors?key={key}&since={since}"));
        let read = |site| async move {
            let stub = b.registry(site, "errors").ok()?;
            let read = async {
                let mut r = stub.fetch_with_str(path).await.ok()?;
                (r.status_code() == 200).then_some(())?;
                r.json::<ScriptErrors>().await.ok()
            };
            graftyard_lookup::within(deadline, read).await.flatten()
        };
        let Some((sum, sites)) = gate::read_sites(&voters, read).await else {
            return answer(503, json!({ "message": "no errors object answered" }));
        };
        return answer(200, json!({ "counts": sum, "sites": sites, "of": voters.len() }));
    }
    let Some(anchor) = c.anchor else { return answer(503, json!({ "message": "this edge has no trust anchor" })) };
    let counts: ScriptErrors =
        match req.json::<Value>().await.ok().and_then(|v| serde_json::from_value(v["counts"].clone()).ok()) {
            Some(c) => c,
            None => return answer(422, json!({ "message": "the body is not {\"counts\": ...}" })),
        };
    view::ensure(&b, &anchor, Some(c.ctx)).await;
    // A version the view does not hold may be one an upload committed since the last refresh: look again.
    if let Err(gate::FlushRefused::UnknownVersion(_)) = view::with(|v| gate::accept_flush(&v.fold, &counts)) {
        view::refresh_now(&b, &anchor).await;
    }
    let body = match view::with(|v| gate::accept_flush(&v.fold, &counts)) {
        Ok(Some(put)) => put.to_string(),
        Ok(None) => return Ok(Response::empty()?.with_status(204)),
        Err(e) => return answer(422, json!({ "message": e.to_string() })),
    };
    for site in gate::errors_sites(key, &voters) {
        let Ok(stub) = b.registry(site, "errors") else { continue };
        let mut init = RequestInit::new();
        init.with_method(Method::Put).with_body(Some(body.clone().into()));
        let Ok(put) = Request::new_with_init("https://registry/errors", &init) else { continue };
        if matches!(graftyard_lookup::within(deadline, stub.fetch_with_request(put)).await, Some(Ok(r)) if r.status_code() == 200)
        {
            return Ok(Response::empty()?.with_status(204));
        }
    }
    answer(503, json!({ "message": "no errors object answered" }))
}

/// How long a `StaleMode` waits for the group's commit before it goes to the members' `pending` rows [guess].
const STALE_COMMIT_MS: u64 = 10_000;

/// Writes an uncommitted `StaleMode` to every `Release` member's `pending` row it reaches, each of which verifies it
/// against its own applied `Keys`: 202 when at least one kept it, else 503.
async fn pending(c: &Ctx<'_>, record: &PlacementRecord, bytes: &[u8]) -> Result<Response> {
    if view::with(|v| v.fold.keys.is_none()) {
        return answer(
            503,
            json!({ "message": "the StaleMode did not commit, and this edge holds no Release view to verify it against" }),
        );
    }
    if let Err(e) = view::with(|v| {
        graftyard_release::view::verify_stale(&v.fold, bytes, graftyard_types::Timestamp(graftyard_lookup::now_ms()))
    }) {
        return refused(e.variant(), e.to_string());
    }
    let b = c.bindings();
    let mut members: Vec<(graftyard_types::SiteId, String)> =
        graftyard_lookup::known_voters(ChainId::Release).into_iter().map(|t| (t.site, t.do_name)).collect();
    if members.is_empty() {
        members =
            record.bootstrap_members(ChainId::Release).into_iter().map(|m| (m.site, m.do_name.to_string())).collect();
    }
    let body = json!({ "entry": graftyard_lookup::hex(bytes) }).to_string();
    let calls = members.iter().map(|(site, name)| {
        graftyard_lookup::post_within(&b, *site, name, "/pending", body.as_bytes(), std::time::Duration::from_secs(2))
    });
    let answers = futures_util::future::join_all(calls).await;
    let kept: Vec<Value> = answers
        .iter()
        .flatten()
        .filter_map(|(s, v)| (*s == 200).then(|| v.get("pending")).flatten().cloned())
        .collect();
    let refusal = answers
        .iter()
        .flatten()
        .find_map(|(_, v)| v.pointer("/refused/err").and_then(Value::as_str).map(str::to_owned));
    match (kept.len(), refusal) {
        (0, Some(r)) => refused(&r, "every member that answered refused the StaleMode".into()),
        (0, None) => {
            answer(503, json!({ "message": "the StaleMode did not commit, and no Release member kept it pending" }))
        }
        (n, _) => answer(202, json!({ "pending": n, "of": members.len(), "rows": kept })),
    }
}

/// Commits one entry's bytes on the `Release` group within `for_ms`: 201 or 409 with the body to send, or the 422 or
/// 503 reply.
async fn submit(
    c: &Ctx<'_>,
    record: &PlacementRecord,
    bytes: &[u8],
    for_ms: u64,
) -> std::result::Result<(u16, Value), Result<Response>> {
    let b = c.bindings();
    let body =
        json!({ "rid": graftyard_lookup::request_id().to_string(), "cmd": graftyard_lookup::hex(bytes) }).to_string();
    let hooks = graftyard_lookup::Hooks::default();
    let done =
        match graftyard_lookup::commit_hedged(&b, ChainId::Release, record, body.as_bytes(), c.colo, for_ms, hooks)
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
