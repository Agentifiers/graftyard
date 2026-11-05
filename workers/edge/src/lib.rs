//! The edge Worker's git over HTTPS (`build/BUILD-PLAN.md`, M1-WP6; `design/DESIGN.md`, journey 1 and "Usage
//! (caller's view)"): smart HTTP only, receive-pack staged, validated, placed, committed, and reported, and
//! upload-pack through the git cache shard with the clone cache in front.
//!
//! The edge mints one `RequestId` per client request and reuses it on every re-send. It reaches the site members
//! through each site's `MEMBER_<n>` and `REGISTRY_<n>` bindings and finds groups by the M1-WP1 lookup rule
//! (`graftyard_lookup`), with the member routes and JSON of `graftyard_git::group`:
//!
//! - `o/r` resolves through `Names` (the owner's login row, then its `Placement` row) and then the owner's `Authz`
//!   (`RepoName`); system chains' records come from the `Release` view, accepted against this script's trust anchor.
//! - A repository chain's group is created on first use from the owner's record; its members check the owner against
//!   the repository's `RepoOwner` row before they store anything.
//! - The git cache shard is the `GitShard` class of the owner record's home site (`SHARD_<n>`).
//!
//! Authentication is a stored token as the Basic password (any username) or as `Authorization: token` / `Bearer`, as
//! GitHub accepts, checked offline by prefix and CRC32 and then on its row, read through `fresh` on its `Tokens`
//! shard; the leader re-reads the row at `decide` either way. `POST /api/graftyard/v1/release/entries` takes signed
//! `Release` entries (`release`).
//!
//! With the `test-faults` feature only: the `/_bed/` driver routes, BF-16's `FORCE_FORMAT`, BF-12's `x-graft-kill`
//! on pushes, and the `GRAFTYARD_GENESIS` stand-in for the `Release` view.

use futures_util::StreamExt;
use graftyard_git::clone::{CloneKey, cacheable_response, tags_digest};
use graftyard_git::group::{CallErrorWire, CommitReq, PushOutcome, ReadReply};
use graftyard_git::host::R2;
use graftyard_git::refs::{Atomicity, ObjectId, RefName};
use graftyard_git::repo::{ATOMIC_REFS_MAX, Push, REPO_FORMAT_V1, REPO_FORMAT_V2};
use graftyard_git::smart::{
    self, Plan, PushCaps, ReceivePack, Refusal, Route, Service, cap_refusal, parse_receive_pack, parse_v2,
    receive_pack_advertisement, receive_pack_caps, upload_pack_v2_advertisement, wants_v2, wrap,
};
use graftyard_git::{Copies, INLINE_MAX, ObjectMeta, PART_SIZE, PackId, PackIndex, PackStore, Stage, TeeError};
use graftyard_identity::{
    Actor, Credential, Login, LoginRow, NameKey, OpaqueToken, Principal, RepoName, ScopeSet, UserId, account_shard,
    name_shard,
};
use graftyard_raft::{COMMAND_MAX, Format, Record};
use graftyard_release::Anchor;
use graftyard_types::{
    AccountId, ChainId, DbId, Digest, NetworkId, PlacementRecord, RepoId, RequestId, SiteId, SiteTable, Timestamp, Ulid,
};
use serde::Deserialize;
use serde_json::json;
use worker::*;

mod api;
mod release;

const AGENT: &str = "git/graftyard-m1";
/// `design/DESIGN.md`, Shape, `Unsupported`: "The edge renders it as 503 with `Retry-After: 5` [guess]".
const UNSUPPORTED_RETRY_AFTER: &str = "5";

fn now() -> Timestamp {
    Timestamp(Date::now().as_millis())
}

fn rand10() -> [u8; 10] {
    let mut b = [0u8; 10];
    let crypto: web_sys::Crypto = js_sys::Reflect::get(&js_sys::global(), &"crypto".into()).unwrap().into();
    crypto.get_random_values_with_u8_array(&mut b).unwrap();
    b
}

/// The edge's configuration, from the script's version.
struct Config {
    plan: Plan,
    ssh_option: bool,
    /// The edge's own cap, below the plan's body limit.
    cap: u64,
    host: String,
    /// The formats `FormatsEnable` has enabled. A command is sent in its newest enabled format, never one the
    /// `Release` group has not enabled, so an edge on N+1 talks format k to leaders on N.
    enabled: Vec<Format>,
    /// Test fault for BF-16: send every push in this format whatever is enabled. Read only with `test-faults`.
    force_format: Option<Format>,
    /// Multipart part size of the tee.
    part: usize,
}

fn parse_format(s: &str) -> Option<Format> {
    let (f, v) = s.split_once('.')?;
    Some(Format { family: f.parse().ok()?, version: v.parse().ok()? })
}

impl Config {
    fn read(env: &Env) -> Result<Config> {
        let var = |k: &str| env.var(k).map(|v| v.to_string()).ok();
        let plan = match var("PLAN").as_deref() {
            Some("business") => Plan::Business,
            Some("enterprise") => Plan::Enterprise,
            _ => Plan::Pro,
        };
        let cap = var("EDGE_CAP_BYTES").and_then(|v| v.parse().ok()).unwrap_or(plan.body_limit() * 9 / 10);
        if cap >= plan.body_limit() {
            return Err(Error::RustError(format!("EDGE_CAP_BYTES {cap} must sit below the plan limit")));
        }
        Ok(Config {
            plan,
            ssh_option: var("SSH_OPTION").as_deref() == Some("on"),
            cap,
            host: var("HOST").unwrap_or_else(|| "graftyard.dev".into()),
            enabled: var("FORMATS_ENABLED")
                .unwrap_or_else(|| "32.1".into())
                .split(',')
                .filter_map(parse_format)
                .collect(),
            #[cfg(feature = "test-faults")]
            force_format: var("FORCE_FORMAT").as_deref().and_then(parse_format),
            #[cfg(not(feature = "test-faults"))]
            force_format: None,
            part: var("PART_BYTES").and_then(|v| v.parse().ok()).unwrap_or(PART_SIZE),
        })
    }

    /// The format a push is sent in: format 2 carries the client's agent, and is used only once enabled.
    fn push_format(&self) -> Format {
        if let Some(f) = self.force_format {
            return f;
        }
        if self.enabled.contains(&REPO_FORMAT_V2) { REPO_FORMAT_V2 } else { REPO_FORMAT_V1 }
    }
}

/// Every site's bucket, by its `BUCKET_<n>` binding, which the edge binds for every site of `GRAFTYARD_SITES`.
fn buckets(env: &Env, sites: &SiteTable) -> Result<Vec<(SiteId, R2)>> {
    sites.0.iter().map(|s| Ok((s.id, R2(env.bucket(&format!("BUCKET_{}", s.id.0))?)))).collect()
}

/// A repository the edge serves: its id, its owner, and the owner's placement record.
#[derive(Clone)]
struct Repo {
    id: RepoId,
    owner: AccountId,
    record: PlacementRecord,
}

/// `o/r` through `Names` then `Authz` (`design/DESIGN.md`, Shape, the lookup rule: "the owner's `Names` rows for
/// `o/r`"): the owner's login row on `name_shard(Login(o))`, the owner's `Placement` row on `account_shard(owner)`, then
/// the repository's name on the owner's `Authz` chain. `None` for an unknown owner or repository, which git sees as 404.
async fn resolve(c: &Ctx<'_>, owner: &str, repo: &str) -> Result<std::result::Result<Option<Repo>, String>> {
    let (Ok(login), Ok(name)) = (Login::parse(owner), RepoName::parse(repo)) else { return Ok(Ok(None)) };
    let names = ChainId::Names(name_shard(&NameKey::Login(login.key())));
    let row = match system_read(c, names, json!({ "login": owner })).await {
        Ok(v) => v,
        Err(e) => return Ok(Err(e)),
    };
    let account = match serde_json::from_value::<Option<LoginRow>>(row["login"].clone()) {
        Ok(Some(LoginRow::Claimed { account, .. })) => account,
        _ => return Ok(Ok(None)),
    };
    let record = match placement(c, account).await {
        Ok(Some(r)) => r,
        Ok(None) => return Ok(Ok(None)),
        Err(e) => return Ok(Err(e)),
    };
    let authz = ChainId::Authz(account);
    let b = c.bindings();
    let q = json!({ "q": { "repo": name.as_str() } }).to_string();
    let v = match graftyard_lookup::call_opt(&b, authz, &record, "/read", q.as_bytes()).await {
        Ok(Some((v, _, _))) => v,
        // An owner whose `Authz` chain was never created holds no repository.
        Ok(None) => return Ok(Ok(None)),
        Err(e) => return Ok(Err(e)),
    };
    Ok(Ok(v.pointer("/value/repo/repo").and_then(serde_json::Value::as_i64).map(|r| Repo {
        id: RepoId(DbId(r)),
        owner: account,
        record,
    })))
}

/// The owner's `Placement` row on `account_shard(owner)`.
async fn placement(c: &Ctx<'_>, owner: AccountId) -> std::result::Result<Option<PlacementRecord>, String> {
    let v = system_read(c, ChainId::Names(account_shard(owner)), json!({ "placement": owner.0.0 })).await?;
    serde_json::from_value(v["placement"].clone()).map_err(|e| e.to_string())
}

/// A `fresh` read of a system chain's group, found by its record in the `Release` view. A shard with no group holds
/// no rows, so it reads as empty (`null`).
async fn system_read(
    c: &Ctx<'_>,
    chain: ChainId,
    q: serde_json::Value,
) -> std::result::Result<serde_json::Value, String> {
    let record = c.system_record(chain).await?;
    let body = json!({ "q": q }).to_string();
    Ok(match graftyard_lookup::call_opt(&c.bindings(), chain, &record, "/read", body.as_bytes()).await? {
        Some((v, _, _)) => v["value"].clone(),
        None => serde_json::Value::Null,
    })
}

// ---- responses ----

/// Every response carries `X-GitHub-Request-Id` and `X-GitHub-Enterprise-Version`, from the shared wire layer
/// (`design/SHARED.md`, "Cross-cutting conventions").
fn headers(rid: RequestId) -> Result<Headers> {
    let h = Headers::new();
    h.set("x-github-request-id", &graftyard_wire::request_id_header(rid))?;
    h.set("x-github-enterprise-version", graftyard_wire::ENTERPRISE_VERSION)?;
    Ok(h)
}

fn git_headers(rid: RequestId, content_type: &str) -> Result<Headers> {
    let h = headers(rid)?;
    h.set("content-type", content_type)?;
    h.set("cache-control", "no-cache, max-age=0, must-revalidate")?;
    h.set("expires", "Fri, 01 Jan 1980 00:00:00 GMT")?;
    h.set("pragma", "no-cache")?;
    Ok(h)
}

fn text(rid: RequestId, status: u16, body: &str) -> Result<Response> {
    let h = headers(rid)?;
    h.set("content-type", "text/plain; charset=utf-8")?;
    Ok(Response::ok(body)?.with_status(status).with_headers(h))
}

fn unauthorized(rid: RequestId, body: &str) -> Result<Response> {
    let h = headers(rid)?;
    h.set("www-authenticate", "Basic realm=\"GitHub\"")?;
    h.set("content-type", "text/plain; charset=utf-8")?;
    Ok(Response::ok(body)?.with_status(401).with_headers(h))
}

/// `Unsupported` (`design/DESIGN.md`, Shape, the write path): 503 with `Retry-After`, a retryable error, never a
/// refusal, and before any byte of a git result, so git prints the HTTP error and nothing it could read as a ref's fate.
fn unsupported(rid: RequestId, family: u16, version: u16) -> Result<Response> {
    let h = headers(rid)?;
    h.set("retry-after", UNSUPPORTED_RETRY_AFTER)?;
    h.set("content-type", "text/plain; charset=utf-8")?;
    let body = format!("The service is updating (record format {family}.{version}). Retry in a few seconds.\n");
    Ok(Response::ok(body)?.with_status(503).with_headers(h))
}

fn git_result(rid: RequestId, service: Service, body: Vec<u8>) -> Result<Response> {
    Ok(Response::from_bytes(body)?.with_headers(git_headers(rid, &service.result_type())?))
}

// ---- authentication ----

fn b64_decode(s: &str) -> Option<Vec<u8>> {
    graftyard_git::pack::b64::decode(s.trim())
}

/// The token in `Authorization`: Basic with the token as the password (any username), or `token`/`Bearer`.
fn token_of(req: &Request) -> Result<Option<String>> {
    let Some(a) = req.headers().get("authorization")? else { return Ok(None) };
    let (scheme, rest) = a.split_once(' ').unwrap_or((&a, ""));
    Ok(match scheme.to_ascii_lowercase().as_str() {
        "basic" => b64_decode(rest)
            .and_then(|b| String::from_utf8(b).ok())
            .and_then(|s| s.split_once(':').map(|(_, p)| p.to_owned())),
        "token" | "bearer" => Some(rest.trim().to_owned()),
        _ => None,
    })
}

#[derive(Deserialize)]
struct TokenRow {
    account: i64,
    revoked: bool,
}

/// The caller, or `None` for no credential or a refused one: the token's row read through `fresh` on its `Tokens`
/// shard, never a cache. The leader re-reads the row at `decide` either way.
async fn authenticate(c: &Ctx<'_>, req: &Request) -> Result<Option<Principal>> {
    let Some(t) = token_of(req)? else { return Ok(None) };
    let Ok(tok) = OpaqueToken::parse(&t) else { return Ok(None) };
    let hash = tok.hash();
    let chain = ChainId::Tokens(tok.shard());
    let v = system_read(c, chain, json!({ "token": Digest(hash).hex() })).await.map_err(Error::RustError)?;
    let row: Option<TokenRow> = serde_json::from_value(v["token"].clone())?;
    Ok(row.filter(|r| !r.revoked).map(|r| Principal {
        actor: Actor::User(UserId(AccountId(DbId(r.account)))),
        scopes: ScopeSet(1),
        credential: Credential::Stored(hash),
        sso_orgs: smallvec::SmallVec::new(),
    }))
}

// ---- the group and the shard ----

/// The network's home shard on the owner record's home site. A network id is its root repository's id; every M1
/// repository is a root.
fn shard(c: &Ctx<'_>, repo: &Repo) -> Result<Stub> {
    #[cfg(feature = "test-faults")]
    return shard_at(c, repo, c.shard);
    #[cfg(not(feature = "test-faults"))]
    shard_at(c, repo, None)
}

/// Site `at`'s shard of the network, the home's when `None`. Only test installs pick another site (BF-18).
fn shard_at(c: &Ctx<'_>, repo: &Repo, at: Option<SiteId>) -> Result<Stub> {
    let site = at.unwrap_or(repo.record.home);
    let region = c.sites.get(site).ok_or("unknown home site")?.region;
    c.env
        .durable_object(&format!("SHARD_{}", site.0))?
        .get_by_name_with_location_hint(&format!("git#{}#net:{}", site.0, repo.id.0.0), region.hint())
}

/// A repository chain's group, created on first use from the owner's record: its members join with the owner, which
/// each checks against the repository's `RepoOwner` row before it stores anything.
async fn ensure_group(c: &Ctx<'_>, repo: &Repo) -> std::result::Result<(), String> {
    graftyard_lookup::create(&c.bindings(), ChainId::Repo(repo.id), &repo.record, &json!({ "owner": repo.owner })).await
}

async fn read_refs(
    c: &Ctx<'_>,
    repo: &Repo,
) -> Result<std::result::Result<(u64, Vec<(String, String)>), CallErrorWire>> {
    let chain = ChainId::Repo(repo.id);
    let mut tried_create = false;
    loop {
        match graftyard_lookup::call(&c.bindings(), chain, &repo.record, "/read", &json!({ "q": {} })).await {
            Ok((v, _)) => {
                return Ok(match serde_json::from_value::<ReadReply>(v)? {
                    ReadReply::Refs(r) => Ok((r.index, r.value)),
                    ReadReply::Error { error } => Err(error),
                });
            }
            Err(_) if !tried_create => {
                tried_create = true;
                ensure_group(c, repo).await.map_err(Error::RustError)?;
            }
            Err(e) => return Err(Error::RustError(e)),
        }
    }
}

/// The edge's `commit` (`graftyard_lookup::commit_hedged`): under one request id, to the leader only, a missed
/// receipt hedged by `suspect`. A reply that does not decode as `graftyard_git::group`'s shapes is answered as
/// `Unsupported` (503), never as a lost reply: a fallback that replays would turn a newer member's answer into
/// `fetch first` (`tests/m1/results/m1-wp6.md`, t5541 #20).
async fn commit(
    c: &Ctx<'_>,
    repo: &Repo,
    body: &serde_json::Value,
    first: Option<&serde_json::Value>,
    trace: Option<&std::cell::RefCell<Vec<serde_json::Value>>>,
) -> Result<std::result::Result<PushOutcome, CallErrorWire>> {
    ensure_group(c, repo).await.map_err(Error::RustError)?;
    let bytes = body.to_string().into_bytes();
    let first = first.map(|f| f.to_string().into_bytes());
    let r = graftyard_lookup::commit_hedged(
        &c.bindings(),
        ChainId::Repo(repo.id),
        &repo.record,
        &bytes,
        c.colo,
        60_000,
        graftyard_lookup::Hooks { first: first.as_deref(), trace },
    )
    .await;
    Ok(match r {
        Ok(done) => match serde_json::from_value::<PushOutcome>(done) {
            Ok(o) => Ok(o),
            Err(_) => Err(unreadable_reply()),
        },
        Err(Ok(error)) => Err(serde_json::from_value::<CallErrorWire>(error).unwrap_or_else(|_| unreadable_reply())),
        Err(Err(e)) => return Err(Error::RustError(e)),
    })
}

fn unreadable_reply() -> CallErrorWire {
    CallErrorWire::Unsupported(graftyard_git::group::UnsupportedWire { family: 0, version: 0 })
}

// ---- receive-pack ----

/// Reads the body up to the edge's cap. `Err(n)` when the stream passed the cap at `n` bytes; the edge then stops
/// reading and answers, which reaches git because the plan limit is above the cap.
async fn read_capped(req: &mut Request, cap: u64) -> Result<std::result::Result<Vec<u8>, (Vec<u8>, u64)>> {
    let mut s = req.stream()?;
    let mut buf = Vec::new();
    while let Some(c) = s.next().await {
        let c = c?;
        buf.extend_from_slice(&c);
        if buf.len() as u64 > cap {
            let n = buf.len() as u64;
            buf.truncate(64 << 10);
            return Ok(Err((buf, n)));
        }
    }
    Ok(Ok(buf))
}

/// git sends a gzip-compressed receive-pack body only for small requests (`http.c`, below `postBuffer`); the edge
/// decompresses it after the cap check, which counts the bytes on the wire.
fn gunzip(b: &[u8]) -> Result<Vec<u8>> {
    use std::io::Read;
    let mut out = Vec::new();
    flate2::read::GzDecoder::new(b).read_to_end(&mut out).map_err(|e| Error::RustError(format!("gzip: {e}")))?;
    Ok(out)
}

struct Ctx<'a> {
    env: &'a Env,
    cfg: &'a Config,
    rid: RequestId,
    /// The install's sites, from `GRAFTYARD_SITES`.
    sites: &'a SiteTable,
    /// This script's trust anchor, which the `Release` view is accepted against.
    anchor: Option<Anchor>,
    /// The client colo, packed as the `Colo` a `suspect` report carries.
    colo: u32,
    /// BF-12 on pushes: the `x-graft-kill` and `x-graft-down-ms` request headers.
    #[cfg(feature = "test-faults")]
    kill: Option<String>,
    #[cfg(feature = "test-faults")]
    down_ms: u64,
    /// BF-18: `x-graft-shard: N` with the bed token sends this request's git cache calls to site N's shard of the
    /// network, a second shard behind its own `IngestCursor`.
    #[cfg(feature = "test-faults")]
    shard: Option<SiteId>,
}

impl Ctx<'_> {
    fn bindings(&self) -> graftyard_lookup::Bindings<'_> {
        graftyard_lookup::Bindings { env: self.env, sites: self.sites }
    }

    /// A system chain's record: from the `Release` view, accepted against this script's anchor. With `test-faults`
    /// only, `GRAFTYARD_GENESIS` stands in for the view on a test install with no `Release` group.
    async fn system_record(&self, chain: ChainId) -> std::result::Result<PlacementRecord, String> {
        #[cfg(feature = "test-faults")]
        if let Ok(v) = self.env.var("GRAFTYARD_GENESIS") {
            let g: serde_json::Value = serde_json::from_str(&v.to_string()).map_err(|e| e.to_string())?;
            let k = match chain {
                ChainId::Names(_) => "names",
                ChainId::Tokens(_) => "tokens",
                ChainId::Ids => "ids",
                _ => "release",
            };
            return serde_json::from_value(g[k].clone()).map_err(|e| e.to_string());
        }
        let anchor = self.anchor.as_ref().ok_or("this edge has no trust anchor")?;
        graftyard_lookup::system_record(&self.bindings(), anchor, chain).await
    }
}

async fn receive_pack(c: &Ctx<'_>, mut req: Request, repo: &Repo, path: &str) -> Result<Response> {
    let Some(actor) = authenticate(c, &req).await? else { return unauthorized(c.rid, "No anonymous write access.") };
    let gz = req.headers().get("content-encoding")?.is_some_and(|e| e == "gzip");
    let body = match read_capped(&mut req, c.cfg.cap).await? {
        Ok(b) => b,
        Err((head, _)) => {
            let head = if gz { gunzip(&head).unwrap_or(head) } else { head };
            let rp = parse_receive_pack(&head).unwrap_or(ReceivePack {
                updates: Vec::new(),
                caps: PushCaps { report_status: true, side_band_64k: true, ..PushCaps::default() },
                options: Vec::new(),
                pack_at: 0,
            });
            let r = cap_refusal(c.cfg.plan, c.cfg.ssh_option, c.cfg.cap, &c.cfg.host, path);
            return git_result(c.rid, Service::ReceivePack, r.render(&rp));
        }
    };
    let body = if gz { gunzip(&body)? } else { body };
    // git's `probe_rpc` (remote-curl.c): before streaming a request over `http.postBuffer` it posts a bare flush,
    // after authentication, and streams the real body only on a 200. Nothing reaches the group.
    if body == b"0000" {
        return git_result(c.rid, Service::ReceivePack, Vec::new());
    }
    let rp = match parse_receive_pack(&body) {
        Ok(r) => r,
        Err(e) => return text(c.rid, 400, &format!("{e}\n")),
    };
    let atomicity = if rp.caps.atomic { Atomicity::AllOrNothing } else { Atomicity::PerRef };
    let refuse = |r: Refusal| git_result(c.rid, Service::ReceivePack, r.render(&rp));
    if atomicity == Atomicity::AllOrNothing && rp.updates.len() > ATOMIC_REFS_MAX {
        return refuse(Refusal::too_many_atomic(rp.updates.len()));
    }
    let mut push = Push {
        updates: rp.updates.clone(),
        atomicity,
        pack: None,
        actor,
        options: rp.options.iter().map(|s| s.as_str().into()).collect(),
        request: c.rid,
        agent: None,
    };
    // Format 2 carries the client's agent. A forced unknown format is sent with format 2's fields and its own tag.
    let format = c.cfg.push_format();
    if format != REPO_FORMAT_V1 {
        push.agent = Some(rp.caps.agent.clone().unwrap_or_default().into());
    }
    // `design/SHARED.md`, "Refs and pushes", Size: the updates alone over the cap refuse the whole push.
    let alone = push.encoded_len_without_pack();
    if alone > COMMAND_MAX {
        return refuse(Refusal::updates_too_large(alone, COMMAND_MAX, push.updates.len()));
    }
    let pack = &body[rp.pack_at..];
    let deletes_only = push.updates.iter().all(|u| u.new.is_none());
    if !deletes_only {
        if pack.is_empty() {
            return refuse(Refusal::unpack_failed("no pack"));
        }
        // Inline only when the pack fits INLINE_MAX and the push with it stays under COMMAND_MAX.
        let inline = pack.len() <= INLINE_MAX && alone + pack.len() + 16 < COMMAND_MAX;
        let tips: Vec<String> = push.updates.iter().filter_map(|u| u.new.map(|o| o.hex())).collect();
        let raw = buckets(c.env, c.sites)?;
        // The three site buckets of the network's placement.
        let refs: Vec<(SiteId, &R2)> =
            raw.iter().filter(|(s, _)| repo.record.sites.contains(s)).map(|(s, b)| (*s, b)).collect();
        let store = PackStore { sites: c.sites, buckets: &refs, copies: Copies::TwoRegions };
        let stage = Stage {
            network: NetworkId(repo.id.0),
            id: PackId(Ulid::new(now(), rand10())),
            meta: ObjectMeta { chain: ChainId::Repo(repo.id), request: c.rid },
            bytes: pack,
            inline,
            part: c.cfg.part,
        };
        let validate = validate(c, repo, pack, &tips);
        match store.place_teed(stage, validate).await {
            Ok((d, _)) => push.pack = Some(d.into_pack()),
            Err(TeeError::Invalid(e)) => return refuse(Refusal::unpack_failed(&e)),
            Err(TeeError::Place(e)) => return text(c.rid, 503, &format!("pack store: {e}\n")),
        }
    }
    let len = push.encoded_len();
    if len > COMMAND_MAX {
        return refuse(Refusal::updates_too_large(len, COMMAND_MAX, push.updates.len()));
    }
    let mut body = serde_json::to_value(CommitReq { rid: c.rid.to_string(), cmd: push.clone() })?;
    // BF-16's forced unknown variant: the same command, its format tag rewritten to one no release reads.
    if format != REPO_FORMAT_V1 && format != REPO_FORMAT_V2 {
        let mut wire = push.to_wire();
        wire[..2].copy_from_slice(&format.family.to_be_bytes());
        wire[2..4].copy_from_slice(&format.version.to_be_bytes());
        body["cmd"] = json!(graftyard_git::pack::b64::encode(&wire));
    }
    // BF-12 on pushes: the leader that decides this push is killed between quorum and reply. The kill rides on the
    // first send only.
    #[allow(unused_mut)]
    let mut first: Option<serde_json::Value> = None;
    #[cfg(feature = "test-faults")]
    if let Some(k) = c.kill.as_deref() {
        let mut f = body.clone();
        f["kill"] = json!(k);
        f["down_ms"] = json!(c.down_ms);
        first = Some(f);
    }
    let reply = commit(c, repo, &body, first.as_ref(), None).await?;
    match reply {
        Ok(PushOutcome::Ok(ok)) => {
            // The member rendered these bytes from its `&Acked`; the edge only frames them.
            let status = ok.out.status_bytes().ok_or("status")?;
            let r = git_result(c.rid, Service::ReceivePack, wrap(&rp.caps, &status, &[]))?;
            // BF-12's driver compares the outcome's index with the one the killed leader recorded.
            #[cfg(feature = "test-faults")]
            {
                r.headers().set("x-graft-outcome-index", &ok.index.to_string())?;
                r.headers().set("x-graft-rid", &c.rid.to_string())?;
            }
            Ok(r)
        }
        Ok(PushOutcome::Refused(r)) => match r.status {
            401 => unauthorized(c.rid, "Bad credentials"),
            403 => text(c.rid, 403, "Permission denied\n"),
            _ => refuse(Refusal::refused(&r.err)),
        },
        Ok(PushOutcome::NotLeader(_)) | Err(CallErrorWire::NotLeader(_)) => {
            text(c.rid, 503, "the repository's group has no leader; retry\n")
        }
        Err(CallErrorWire::Unsupported(u)) => unsupported(c.rid, u.family, u.version),
        Err(CallErrorWire::TooLarge(t)) => refuse(Refusal::updates_too_large(t.len, t.max, rp.updates.len())),
    }
}

/// `GitCacheShard::validate` on the network's home shard: tips in the body ahead of the pack.
async fn validate(c: &Ctx<'_>, repo: &Repo, pack: &[u8], tips: &[String]) -> std::result::Result<PackIndex, String> {
    let run = async {
        let mut head = tips.join("\n").into_bytes();
        head.push(b'\n');
        let n = head.len();
        head.extend_from_slice(pack);
        let mut init = RequestInit::new();
        init.with_method(Method::Post).with_body(Some(js_sys::Uint8Array::from(head.as_slice()).into()));
        let url = format!("https://shard/validate?repo={}&tipsn={n}", repo.id.0.0);
        let mut r = shard(c, repo)?.fetch_with_request(Request::new_with_init(&url, &init)?).await?;
        if r.status_code() != 200 {
            return Ok(Err(r.text().await.unwrap_or_default()));
        }
        Ok::<_, Error>(Ok(r.json::<PackIndex>().await?))
    };
    match run.await {
        Ok(r) => r,
        Err(e) => Err(e.to_string()),
    }
}

// ---- upload-pack ----

async fn upload_pack(c: &Ctx<'_>, req: &mut Request, repo: &Repo, ctx: &Context) -> Result<Response> {
    let proto = req.headers().get("git-protocol")?.unwrap_or_default();
    let enc = req.headers().get("content-encoding")?.unwrap_or_default();
    let raw = req.bytes().await?;
    let body = if enc == "gzip" { gunzip(&raw)? } else { raw.clone() };
    // The clone cache: a v2 fetch with no haves, read from R2 by its `CloneKey`, else upload-pack on the shard with
    // its output teed to the client and to R2.
    // BF-18 clones from a chosen shard (`x-graft-shard`) bypass the clone cache, so the shard serves every one.
    #[cfg(feature = "test-faults")]
    let cache_ok = c.shard.is_none();
    #[cfg(not(feature = "test-faults"))]
    let cache_ok = true;
    let key = if cache_ok && wants_v2(Some(&proto)) {
        match parse_v2(&body) {
            Ok(v) => {
                let tags = match read_refs(c, repo).await {
                    Ok(Ok((_, refs))) => tags_digest(refs.iter().map(|(n, o)| (n.as_str(), o.as_str()))),
                    _ => return text(c.rid, 503, "the repository's group has no leader; retry\n"),
                };
                CloneKey::of(NetworkId(repo.id.0), &v, tags)
            }
            Err(_) => None,
        }
    } else {
        None
    };
    // The clone cache lives in the home site's bucket.
    let cache_binding = format!("BUCKET_{}", repo.record.home.0);
    let cache = c.env.bucket(&cache_binding)?;
    if let Some(k) = &key {
        let r2 = k.r2_key(0).to_string();
        if let Some(o) = cache.get(&r2).execute().await? {
            let h = git_headers(c.rid, &Service::UploadPack.result_type())?;
            h.set("x-graft-clone-cache", "hit")?;
            let b = o.body().ok_or("cache object has no body")?.bytes().await?;
            return Ok(Response::from_bytes(b)?.with_headers(h));
        }
    }
    let mut init = RequestInit::new();
    let h = Headers::new();
    h.set("git-protocol", &proto)?;
    init.with_method(Method::Post).with_headers(h).with_body(Some(js_sys::Uint8Array::from(body.as_slice()).into()));
    let url = format!("https://shard/upload-pack?repo={}", repo.id.0.0);
    let mut r = shard(c, repo)?.fetch_with_request(Request::new_with_init(&url, &init)?).await?;
    let hh = git_headers(c.rid, &Service::UploadPack.result_type())?;
    if r.status_code() != 200 {
        return text(c.rid, 503, &format!("git cache: {} {}\n", r.status_code(), r.text().await.unwrap_or_default()));
    }
    if let Some(cursor) = r.headers().get("x-graft-cursor")? {
        hh.set("x-graft-cursor", &cursor)?;
    }
    let Some(k) = key else {
        return Ok(Response::from_body(r.body().clone())?.with_headers(hh));
    };
    // Tee: the client's copy streams as the shard sends; the other is written to R2 after it ends, if whole.
    hh.set("x-graft-clone-cache", "miss")?;
    let ResponseBody::Stream(s) = r.body().clone() else {
        return Ok(Response::from_body(r.body().clone())?.with_headers(hh));
    };
    let parts = s.tee();
    let (client, store): (web_sys::ReadableStream, web_sys::ReadableStream) =
        (parts.get(0).into(), parts.get(1).into());
    let r2 = k.r2_key(0).to_string();
    let env = c.env.clone();
    ctx.wait_until(async move {
        let mut st = ByteStream::from(store);
        let mut buf = Vec::new();
        while let Some(Ok(c)) = st.next().await {
            buf.extend_from_slice(&c);
        }
        if cacheable_response(&buf)
            && let Ok(b) = env.bucket(&cache_binding)
        {
            let _ = b.put(&r2, buf).execute().await;
        }
    });
    Ok(Response::from_body(ResponseBody::Stream(client))?.with_headers(hh))
}

// ---- advertisements ----

async fn info_refs(c: &Ctx<'_>, req: &Request, repo: &Repo, service: Service) -> Result<Response> {
    let proto = req.headers().get("git-protocol")?;
    match service {
        Service::UploadPack if wants_v2(proto.as_deref()) => {
            Ok(Response::from_bytes(upload_pack_v2_advertisement(AGENT))?
                .with_headers(git_headers(c.rid, &service.advertisement_type())?))
        }
        Service::UploadPack => {
            // v0: the shard's own advertisement, after it caught up to the leader's refs.
            let h = Headers::new();
            h.set("git-protocol", proto.as_deref().unwrap_or(""))?;
            let mut init = RequestInit::new();
            init.with_method(Method::Get).with_headers(h);
            let url = format!("https://shard/info-refs?repo={}", repo.id.0.0);
            let mut r = shard(c, repo)?.fetch_with_request(Request::new_with_init(&url, &init)?).await?;
            let b = r.bytes().await?;
            Ok(Response::from_bytes(b)?.with_headers(git_headers(c.rid, &service.advertisement_type())?))
        }
        Service::ReceivePack => {
            if authenticate(c, req).await?.is_none() {
                return unauthorized(c.rid, "No anonymous write access.");
            }
            let refs = match read_refs(c, repo).await? {
                Ok((_, r)) => r,
                Err(CallErrorWire::Unsupported(u)) => return unsupported(c.rid, u.family, u.version),
                Err(_) => return text(c.rid, 503, "the repository's group has no leader; retry\n"),
            };
            let refs: Vec<(RefName, ObjectId)> = refs
                .iter()
                .filter_map(|(n, o)| Some((RefName::parse(n.as_bytes()).ok()?, ObjectId::parse_hex(o.as_bytes())?)))
                .collect();
            let caps = receive_pack_caps(AGENT, &graftyard_wire::request_id_header(c.rid));
            Ok(Response::from_bytes(receive_pack_advertisement(&refs, &caps))?
                .with_headers(git_headers(c.rid, &service.advertisement_type())?))
        }
    }
}

// ---- the test driver's routes (test installs only) ----

/// A repository by id: its owner from `RepoOwner` on `Names`, and the owner's record.
#[cfg(feature = "test-faults")]
async fn repo_by_id(c: &Ctx<'_>, id: RepoId) -> std::result::Result<Repo, String> {
    repo_of(c, id).await
}

/// A repository by id, as an LFS transfer path names it: its owner from `RepoOwner` on `Names`, and the owner's
/// record.
async fn repo_of(c: &Ctx<'_>, id: RepoId) -> std::result::Result<Repo, String> {
    let v = system_read(c, ChainId::Names(name_shard(&NameKey::Repo(id))), json!({ "repoOwner": id.0.0 })).await?;
    let owner: AccountId = serde_json::from_value::<Option<AccountId>>(v["repoOwner"].clone())
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("{id:?} has no RepoOwner row"))?;
    let record = placement(c, owner).await?.ok_or_else(|| format!("{owner:?} has no placement record"))?;
    Ok(Repo { id, owner, record })
}

/// Sends `path` to every member the caller knows for the repository's chain: the config's voters, else the record's
/// bootstrap names. Each answer is the member's site, its object name, and JSON.
#[cfg(feature = "test-faults")]
async fn each_member(c: &Ctx<'_>, repo: &Repo, path: &str) -> Vec<(u8, String, serde_json::Value)> {
    let chain = ChainId::Repo(repo.id);
    let mut vs = graftyard_lookup::known_voters(chain);
    if vs.is_empty() {
        vs = repo
            .record
            .bootstrap_members(chain)
            .into_iter()
            .map(|m| graftyard_lookup::Target { site: m.site, do_name: m.do_name.into() })
            .collect();
    }
    let mut out = Vec::new();
    for v in vs {
        let Ok(stub) = c.bindings().member(v.site, &v.do_name) else { continue };
        if let Ok(mut r) = stub.fetch_with_str(&format!("https://member{path}")).await
            && let Ok(j) = r.json::<serde_json::Value>().await
        {
            out.push((v.site.0, v.do_name.to_string(), j));
        }
    }
    out
}

/// BF-18's archive of `from..=to` on the repository's group, as the design orders it (`design/DESIGN.md`, Shape,
/// "Packs"): the home shard packs the objects of the range's inline packs into one complete pack, which is validated
/// on the shard and placed as a stored pack; the range's stored packs' markers are re-put; the leader records
/// `Promoted` and the promoted pack's marker is put; the range's entries go into an `Archive` object, put create-only
/// in two buckets; and the leader records `Archived`, after which each member truncates its stored log through `to`.
/// A test-installs route: the design has the leader run this, with no trigger yet.
#[cfg(feature = "test-faults")]
async fn bed_archive(c: &Ctx<'_>, repo: &Repo, from: u64, to: u64) -> Result<serde_json::Value> {
    use sha2::Digest as _;
    let chain = ChainId::Repo(repo.id);
    let net = NetworkId(repo.id.0);
    let fail = |e: String| Error::RustError(e);
    // The range's entries from the leader, every page.
    let mut entries: Vec<serde_json::Value> = Vec::new();
    let mut at = from;
    while at <= to {
        let (v, _) = graftyard_lookup::call(
            &c.bindings(),
            chain,
            &repo.record,
            &format!("/entries?from={at}&to={to}"),
            &json!({}),
        )
        .await
        .map_err(fail)?;
        let page: Vec<serde_json::Value> = serde_json::from_value(v["entries"].clone())?;
        let Some(last) = page.last().and_then(|e| e["index"].as_u64()) else { break };
        entries.extend(page);
        at = last + 1;
    }
    if entries.len() as u64 != to + 1 - from {
        return Err(fail(format!("entries {from}..={to}: the leader answered {} (commit below {to}?)", entries.len())));
    }
    let logs: Vec<graftyard_git::LogPacks> = entries
        .iter()
        .map(|e| {
            Ok(graftyard_git::LogPacks {
                index: graftyard_types::Index(e["index"].as_u64().unwrap_or(0)),
                packs: serde_json::from_value(e["packs"].clone())?,
                promoted: None,
                consumed: Vec::new(),
            })
        })
        .collect::<Result<_>>()?;
    let (inline, stored) =
        graftyard_git::promotion_inputs(&logs, graftyard_types::Index(from), graftyard_types::Index(to));
    // The range's stored packs' objects go into the promotion too. Pushed packs are thin: a stored pack in the range
    // can have its delta bases only in the range's inline packs, while a later inline pack's trees name its objects,
    // so a promotion of the inline packs alone and that stored pack each need the other, and a rehydrate's
    // `index-pack --strict` refuses both (BF-18 on the hosted install, `tests/m1/results/m1-wp5.md`). The home shard
    // ingested each stored pack under its checksum, so it can name them.
    let sums: Vec<String> = inline
        .iter()
        .map(|(_, b)| Digest(sha2::Sha256::digest(b).into()).hex())
        .chain(stored.iter().filter_map(|p| match p {
            graftyard_git::PackRef::Stored { checksum, .. } => Some(checksum.hex()),
            graftyard_git::PackRef::Inline(_) => None,
        }))
        .collect();
    // The home shard's promotion, then its validation of the promoted pack.
    let call_shard = |path: String, body: Vec<u8>| async move {
        let mut init = RequestInit::new();
        init.with_method(Method::Post).with_body(Some(js_sys::Uint8Array::from(body.as_slice()).into()));
        let mut r =
            shard(c, repo)?.fetch_with_request(Request::new_with_init(&format!("https://shard{path}"), &init)?).await?;
        if r.status_code() != 200 {
            return Err(fail(format!("shard {path}: {} {}", r.status_code(), r.text().await.unwrap_or_default())));
        }
        r.bytes().await
    };
    let promoted = call_shard(format!("/promote?repo={}", repo.id.0.0), serde_json::to_vec(&sums)?).await?;
    let ix: PackIndex = serde_json::from_slice(
        &call_shard(format!("/validate?repo={}&tipsn=0", repo.id.0.0), promoted.clone()).await?,
    )?;
    let raw = buckets(c.env, c.sites)?;
    let refs: Vec<(SiteId, &R2)> =
        raw.iter().filter(|(s, _)| repo.record.sites.contains(s)).map(|(s, b)| (*s, b)).collect();
    let store = PackStore { sites: c.sites, buckets: &refs, copies: Copies::TwoRegions };
    let place = graftyard_git::Place {
        network: net,
        id: PackId(Ulid::new(now(), rand10())),
        meta: ObjectMeta { chain, request: c.rid },
        bytes: &promoted,
        index: &ix,
        idx: None,
        inline_max: 0,
    };
    let pack = store.place(place).await.map_err(|e| fail(e.to_string()))?.into_pack();
    // Markers complete below the promotion: the range's stored packs' markers again.
    let mut remarked = 0;
    for e in &logs {
        for p in e.packs.iter().filter(|p| p.stored_id().is_some()) {
            graftyard_git::mark_committed(&refs, net, p, chain, e.index).await;
            remarked += 1;
        }
    }
    let record = |events: serde_json::Value| async move {
        let rid = graftyard_lookup::request_id();
        let body = json!({ "rid": rid.to_string(), "events": events });
        let (v, _) =
            graftyard_lookup::call(&c.bindings(), chain, &repo.record, "/record", &body).await.map_err(fail)?;
        v["recorded"].as_u64().ok_or_else(|| fail(format!("record: {v}")))
    };
    let p = graftyard_git::Promoted {
        from: graftyard_types::Index(from),
        to: graftyard_types::Index(to),
        pack: pack.clone(),
    };
    let pi = record(json!([{ "Promoted": p }])).await?;
    let marker_failures = graftyard_git::mark_committed(&refs, net, &pack, chain, graftyard_types::Index(pi)).await;
    // The archive object: the range's entries as the leader serves them, keyed by range and digest, in two buckets.
    let bytes = serde_json::to_vec(&entries)?;
    let digest = Digest(sha2::Sha256::digest(&bytes).into());
    let key = graftyard_git::R2Key::Archive {
        chain,
        from: graftyard_types::Index(from),
        to: graftyard_types::Index(to),
        digest,
    };
    let mut put = Vec::new();
    for (s, b) in refs.iter().take(2) {
        graftyard_git::Bucket::put_new(*b, &key.to_string(), &bytes, digest, &[]).await.map_err(|e| fail(e.0))?;
        put.push(s.0);
    }
    let at_of = |i: usize| Timestamp(entries[i]["at"].as_u64().unwrap_or(0));
    let a = graftyard_git::Archived {
        key: key.clone(),
        from: graftyard_types::Index(from),
        to: graftyard_types::Index(to),
        from_at: at_of(0),
        to_at: at_of(entries.len() - 1),
        digest,
    };
    let ai = record(json!([{ "Archived": a }])).await?;
    Ok(json!({
        "inline_packs": inline.len(),
        "stored_in_range": stored.len(),
        "remarked": remarked,
        "promoted": { "id": pack.stored_id().map(|i| i.to_string()), "bytes": promoted.len(), "objects": ix.objects, "index": pi, "marker_failures": marker_failures },
        "archived_index": ai,
        "archive_key": key.to_string(),
        "archive_buckets": put,
    }))
}

/// BF-19's placement: `n` random bytes as a stored pack of the repository's network, placed through
/// `PackStore::place_teed` with the shard's validation stood in by the bytes' own index (they are no git pack, and
/// nothing reads them as one). With `fail`, only the record's home bucket takes the put and the others refuse it, so
/// the two-region rule fails, as a placement whose other puts failed; the one copy stays behind for the sweeper.
#[cfg(feature = "test-faults")]
async fn bed_place(
    c: &Ctx<'_>,
    repo: &Repo,
    n: usize,
    fail: bool,
) -> Result<std::result::Result<graftyard_git::PackRef, serde_json::Value>> {
    use sha2::Digest as _;
    let mut bytes = vec![0u8; n];
    let crypto: web_sys::Crypto =
        js_sys::Reflect::get(&js_sys::global(), &"crypto".into()).map_err(|_| "crypto")?.into();
    for chunk in bytes.chunks_mut(65_536) {
        crypto.get_random_values_with_u8_array(chunk).map_err(|_| "random")?;
    }
    let raw = buckets(c.env, c.sites)?;
    let refs: Vec<(SiteId, &R2)> = raw
        .iter()
        .filter(|(s, _)| repo.record.sites.contains(s) && (!fail || *s == repo.record.home))
        .map(|(s, b)| (*s, b))
        .collect();
    let store = PackStore { sites: c.sites, buckets: &refs, copies: Copies::TwoRegions };
    let checksum = Digest(sha2::Sha256::digest(&bytes).into());
    let ix = PackIndex { checksum, bytes: n as u64, objects: 0, trailer: "".into() };
    let stage = Stage {
        network: NetworkId(repo.id.0),
        id: PackId(Ulid::new(now(), rand10())),
        meta: ObjectMeta { chain: ChainId::Repo(repo.id), request: c.rid },
        bytes: &bytes,
        inline: false,
        part: c.cfg.part,
    };
    let id = stage.id;
    Ok(match store.place_teed(stage, async { Ok::<_, String>(ix) }).await {
        Ok((d, _)) => Ok(d.into_pack()),
        Err(e) => Err(json!({ "id": id.to_string(), "error": e.to_string() })),
    })
}

/// `/_bed/...` behind the bed token, the routes `tests/m1/wp6/bf16.sh` and `bf17.sh` read, now answered from the
/// repository's real group and its home site's shard: `log` (the leader's stored log with each entry's packs and ref
/// updates), `status` (the members' receive counters, summed), `refs`, `markers` (each stored pack's object and
/// `Committed` marker in every bucket, for the entries from `index`), and the shard's `fsck`, `kill`, `teardown`,
/// and `catchup`.
#[cfg(feature = "test-faults")]
async fn bed(c: &Ctx<'_>, req: &mut Request, path: &str) -> Result<Response> {
    let token = c.env.secret("BED_TOKEN")?.to_string();
    if req.headers().get("x-bed-token")?.as_deref() != Some(token.as_str()) {
        return text(c.rid, 404, "not found\n");
    }
    let url = req.url()?;
    let qp = |k: &str| url.query_pairs().find(|(n, _)| n == k).map(|(_, v)| v.into_owned());
    // BM-1's site fault: `?site=N&after_ms=A&for_ms=F[&rate=R]` makes site N's member class fail every call (or, with
    // `rate`, that share of calls) from A ms from now for F ms (default: until cleared); `?off=1` clears it. Written to every site's `Registry` object `faults`, which
    // each member isolate rereads at most once a second (`workers/site/src/faults.rs`, `SiteFault`).
    if path == "/_bed/fault" {
        let now_ms = now().0;
        let plan = if qp("off").is_some() {
            json!({ "site": null, "from": 0, "until": 0 })
        } else {
            let site: u8 = qp("site").and_then(|v| v.parse().ok()).ok_or("site")?;
            let from = now_ms + qp("after_ms").and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
            // "Until cleared" is 30 days, so the bound stays an exact JSON number.
            let for_ms = qp("for_ms").and_then(|v| v.parse::<u64>().ok()).unwrap_or(30 * 86_400_000);
            let until = from + for_ms;
            let rate = qp("rate").and_then(|v| v.parse::<f64>().ok());
            json!({ "site": site, "from": from, "until": until, "rate": rate })
        };
        let mut set = Vec::new();
        for s in &c.sites.0 {
            let mut init = RequestInit::new();
            init.with_method(Method::Put).with_body(Some(plan.to_string().into()));
            let r = async {
                let stub = c.bindings().registry(s.id, "faults")?;
                let mut res =
                    stub.fetch_with_request(Request::new_with_init("https://registry/site_fault", &init)?).await?;
                res.json::<serde_json::Value>().await
            }
            .await;
            set.push(json!({ "site": s.id.0, "plan": r.map_err(|e| e.to_string()) .unwrap_or_else(|e| json!({ "error": e })) }));
        }
        return Response::from_json(&json!({ "now": now_ms, "plan": plan, "sites": set }));
    }
    // The sweeper cron's cursor and last report on site `?site=N` (`workers/site/src/sweep.rs`); with `&now=MS`, one
    // run of the site's sweeper with its clock at MS (BF-19), in that site's `Registry` object `sweep`.
    if path == "/_bed/sweep" {
        let site: u8 = qp("site").and_then(|v| v.parse().ok()).ok_or("site")?;
        let stub = c.bindings().registry(SiteId(site), "sweep")?;
        return match qp("now") {
            Some(n) => {
                let mut init = RequestInit::new();
                init.with_method(Method::Post);
                stub.fetch_with_request(Request::new_with_init(&format!("https://registry/sweep_run?now={n}"), &init)?)
                    .await
            }
            None => stub.fetch_with_str("https://registry/sweep").await,
        };
    }
    // BF-14 on identity commands: `{"chain": "names:N", "cmd": {...}, "receipt"?}` committed through `commit_hedged`
    // on a system chain, the receipt fault on the first send, with the edge's trace; and every member's arrivals.
    if path == "/_bed/identity" || path == "/_bed/identity_arrivals" {
        let b: serde_json::Value = if path == "/_bed/identity" { req.json().await? } else { json!({}) };
        let chain_s = b["chain"].as_str().map(str::to_owned).or_else(|| qp("chain")).ok_or("chain")?;
        let chain: ChainId = chain_s.parse().map_err(|_| "chain")?;
        let record = c.system_record(chain).await.map_err(Error::RustError)?;
        if path == "/_bed/identity_arrivals" {
            let since = qp("since").unwrap_or_else(|| "0".into());
            let mut out = Vec::new();
            // The config's voters this isolate learned, and the record's bootstrap names, once each.
            let mut names: Vec<(SiteId, String)> =
                graftyard_lookup::known_voters(chain).into_iter().map(|t| (t.site, t.do_name)).collect();
            for m in record.bootstrap_members(chain) {
                if !names.iter().any(|(_, n)| **n == *m.do_name) {
                    names.push((m.site, m.do_name.to_string()));
                }
            }
            for (site, name) in names {
                let stub = c.bindings().member(site, &name)?;
                let mut r = stub.fetch_with_str(&format!("https://member/arrivals?since={since}")).await?;
                out.push(json!({ "site": site.0, "name": name, "rows": r.json::<serde_json::Value>().await? }));
            }
            return Response::from_json(&out);
        }
        let body = json!({ "rid": c.rid.to_string(), "cmd": b["cmd"] });
        let first = b.get("receipt").and_then(serde_json::Value::as_str).map(|k| {
            let mut f = body.clone();
            f["receipt"] = json!(k);
            f
        });
        let (bytes, first) = (body.to_string().into_bytes(), first.map(|f| f.to_string().into_bytes()));
        let trace = std::cell::RefCell::new(Vec::new());
        let t0 = now().0;
        let r = graftyard_lookup::commit_hedged(
            &c.bindings(),
            chain,
            &record,
            &bytes,
            c.colo,
            60_000,
            graftyard_lookup::Hooks { first: first.as_deref(), trace: Some(&trace) },
        )
        .await;
        return Response::from_json(&json!({
            "rid": c.rid.to_string(),
            "ms": now().0 - t0,
            "outcome": r.as_ref().ok(),
            "error": r.err().map(|e| format!("{e:?}")),
            "trace": trace.into_inner(),
        }));
    }
    let id = qp("repo").and_then(|v| v.parse::<i64>().ok()).map(|r| RepoId(DbId(r))).ok_or("repo")?;
    let repo = repo_by_id(c, id).await.map_err(Error::RustError)?;
    let chain = ChainId::Repo(repo.id);
    let record = repo.record.clone();
    let leader = |path: String| {
        let record = record.clone();
        async move {
            let b = c.bindings();
            graftyard_lookup::call(&b, chain, &record, &path, &json!({}))
                .await
                .map(|(v, _)| v)
                .map_err(Error::RustError)
        }
    };
    match path {
        "/_bed/log" => {
            let from = qp("from").unwrap_or_else(|| "1".into());
            let stub = {
                read_refs(c, &repo).await?.map_err(|e| Error::RustError(format!("{e:?}")))?;
                let lead = each_member(c, &repo, "/status").await.into_iter().find(|(_, _, s)| s["role"] == "leader");
                let (site, name, _) = lead.ok_or("no leader")?;
                c.bindings().member(SiteId(site), &name)?
            };
            stub.fetch_with_str(&format!("https://member/log?from={from}&n=20000")).await
        }
        "/_bed/status" => {
            let mut received: std::collections::BTreeMap<String, u64> = std::collections::BTreeMap::new();
            for (_, _, s) in each_member(c, &repo, "/stats").await {
                if let Some(m) = s["received"].as_object() {
                    for (k, v) in m {
                        *received.entry(k.clone()).or_insert(0) += v.as_u64().unwrap_or(0);
                    }
                }
            }
            Response::from_json(&json!({ "received": received }))
        }
        "/_bed/refs" => {
            let refs = read_refs(c, &repo).await?.map_err(|e| Error::RustError(format!("{e:?}")))?.1;
            Response::from_json(&refs)
        }
        "/_bed/markers" => {
            let from = qp("index").and_then(|v| v.parse::<u64>().ok()).unwrap_or(1);
            let v = leader(format!("/entries?from={from}")).await?;
            let entries: Vec<serde_json::Value> = serde_json::from_value(v["entries"].clone())?;
            let raw = buckets(c.env, c.sites)?;
            let mut out = Vec::new();
            for e in entries {
                let packs: Vec<graftyard_git::PackRef> = serde_json::from_value(e["packs"].clone())?;
                for id in packs.iter().filter_map(graftyard_git::PackRef::stored_id) {
                    let pack = graftyard_git::R2Key::Pack { network: NetworkId(repo.id.0), id };
                    let marker = graftyard_git::R2Key::committed(pack.clone()).ok_or("marker")?.to_string();
                    let mut per = Vec::new();
                    for (site, b) in &raw {
                        let se = |e: graftyard_git::StoreError| Error::RustError(e.0);
                        let has_pack = graftyard_git::Bucket::head(b, &pack.to_string()).await.map_err(se)?.is_some();
                        let has_marker = graftyard_git::Bucket::head(b, &marker).await.map_err(se)?.is_some();
                        per.push(json!({ "site": site.0, "pack": has_pack, "marker": has_marker }));
                    }
                    out.push(json!({ "id": id.to_string(), "index": e["index"], "stored": true, "buckets": per }));
                }
            }
            Response::from_json(&out)
        }
        // Every member's `fault` rows for a request id, and every member's BF-12 audit (request ids at two indexes,
        // `dup` rows written by apply).
        "/_bed/faults" | "/_bed/audit" => {
            let inner = match path {
                "/_bed/faults" => format!("/faults?rid={}", qp("rid").unwrap_or_default()),
                _ => "/audit?prefix=".into(),
            };
            let rows: Vec<serde_json::Value> =
                each_member(c, &repo, &inner).await.into_iter().map(|(s, _, v)| json!({ "site": s, "r": v })).collect();
            Response::from_json(&rows)
        }
        // BF-12, BF-14, and BF-19 on pushes: one `Push` of a ref update (`{ref, expect, new}`, object ids as hex), under
        // the caller's token, through the edge's commit path (`commit_hedged`). The first send carries `kill` or
        // `receipt` (BF-14's missed receipt); `no_marker` makes the deciding leader skip its marker puts; `stored`
        // (bytes) places that many random bytes as a stored pack first, through `PackStore::place_teed` with no
        // validation, refused by every bucket but the home's with `fail_place`, which then stops before the commit.
        // The answer carries the edge's trace of sends, misses, `suspect` answers, and `NotLeader` redirects.
        "/_bed/push" => {
            let Some(actor) = authenticate(c, req).await? else { return unauthorized(c.rid, "Bad credentials") };
            let b: serde_json::Value = req.json().await?;
            let oid = |k: &str| b[k].as_str().and_then(|h| ObjectId::parse_hex(h.as_bytes()));
            let name = RefName::parse(b["ref"].as_str().unwrap_or_default().as_bytes()).map_err(|e| e.to_string())?;
            let mut push = Push {
                updates: vec![graftyard_git::refs::RefUpdate { name, expect: oid("expect"), new: oid("new") }],
                atomicity: Atomicity::PerRef,
                pack: None,
                actor,
                options: Vec::new(),
                request: c.rid,
                agent: None,
            };
            let t0 = now().0;
            if let Some(n) = b.get("stored").and_then(serde_json::Value::as_u64) {
                let fail = b.get("fail_place").and_then(serde_json::Value::as_bool).unwrap_or(false);
                match bed_place(c, &repo, n as usize, fail).await? {
                    Ok(p) => push.pack = Some(p),
                    Err(e) => {
                        return Response::from_json(
                            &json!({ "rid": c.rid.to_string(), "ms": now().0 - t0, "placed": e }),
                        );
                    }
                }
            }
            let mut body = serde_json::to_value(CommitReq { rid: c.rid.to_string(), cmd: push.clone() })?;
            if b.get("no_marker").and_then(serde_json::Value::as_bool) == Some(true) {
                body["no_marker"] = json!(true);
            }
            let mut first: Option<serde_json::Value> = None;
            for k in ["kill", "receipt"] {
                if let Some(v) = b.get(k).and_then(serde_json::Value::as_str) {
                    let f = first.get_or_insert_with(|| body.clone());
                    f[k] = json!(v);
                    f["down_ms"] = b.get("down_ms").cloned().unwrap_or(json!(0));
                }
            }
            let trace = std::cell::RefCell::new(Vec::new());
            let r = commit(c, &repo, &body, first.as_ref(), Some(&trace)).await;
            Response::from_json(&json!({
                "rid": c.rid.to_string(),
                "ms": now().0 - t0,
                "pack": push.pack,
                "outcome": r.as_ref().ok().map(|o| serde_json::to_value(o).unwrap_or_default()),
                "error": r.err().map(|e| e.to_string()),
                "trace": trace.into_inner(),
            }))
        }
        // BF-19: for each pack id of `{"ids": [...]}` (at most 500 a call), whether each bucket holds the pack and its
        // `Committed` marker.
        "/_bed/heads" => {
            let b: serde_json::Value = req.json().await?;
            let ids: Vec<String> = serde_json::from_value(b["ids"].clone())?;
            if ids.len() > 500 {
                return text(c.rid, 400, "at most 500 ids\n");
            }
            let raw = buckets(c.env, c.sites)?;
            let net = NetworkId(repo.id.0);
            // Every id's six HEADs at once, 25 ids at a time (the HEADs of one id run serially in 1.1 s).
            let one = |id: String| {
                let raw = &raw;
                async move {
                    let Ok(pid) = id.parse::<Ulid>().map(PackId) else { return Ok(None) };
                    let pack = graftyard_git::R2Key::Pack { network: net, id: pid };
                    let marker = graftyard_git::R2Key::committed(pack.clone()).ok_or("marker")?.to_string();
                    let pack = pack.to_string();
                    let se = |e: graftyard_git::StoreError| Error::RustError(e.0);
                    let per = futures_util::future::join_all(raw.iter().map(|(site, bk)| {
                        let (pack, marker) = (&pack, &marker);
                        async move {
                            let (p, m) = futures_util::future::join(
                                graftyard_git::Bucket::head(bk, pack),
                                graftyard_git::Bucket::head(bk, marker),
                            )
                            .await;
                            Ok::<_, Error>(json!({
                                "site": site.0, "pack": p.map_err(se)?.is_some(), "marker": m.map_err(se)?.is_some()
                            }))
                        }
                    }))
                    .await
                    .into_iter()
                    .collect::<Result<Vec<_>>>()?;
                    Ok::<_, Error>(Some(json!({ "id": id, "buckets": per })))
                }
            };
            let mut out = Vec::new();
            let mut ids = ids.into_iter();
            loop {
                let batch: Vec<String> = ids.by_ref().take(25).collect();
                if batch.is_empty() {
                    break;
                }
                for r in futures_util::future::join_all(batch.into_iter().map(one)).await {
                    out.extend(r?);
                }
            }
            Response::from_json(&out)
        }
        // Every member's BF-14 arrivals since `?since=` ms.
        "/_bed/arrivals" => {
            let since = qp("since").unwrap_or_else(|| "0".into());
            let rows: Vec<serde_json::Value> = each_member(c, &repo, &format!("/arrivals?since={since}"))
                .await
                .into_iter()
                .map(|(s, n, v)| json!({ "site": s, "name": n, "rows": v }))
                .collect();
            Response::from_json(&rows)
        }
        // BF-18: archive `?from=F&to=T` on the repository's real group (`bed_archive`).
        "/_bed/archive" => {
            let num = |k: &str| qp(k).and_then(|v| v.parse::<u64>().ok()).ok_or_else(|| Error::RustError(k.into()));
            let (from, to) = (num("from")?, num("to")?);
            Response::from_json(&bed_archive(c, &repo, from, to).await?)
        }
        // The home shard's routes, or with `?shard=N` site N's shard of the same network (BF-18's second shard,
        // behind its own `IngestCursor`).
        "/_bed/fsck" | "/_bed/kill" | "/_bed/teardown" | "/_bed/catchup" | "/_bed/shard_status" => {
            let inner = match path {
                "/_bed/kill" => "/teardown",
                "/_bed/shard_status" => "/status",
                p => &p[5..],
            };
            let at = qp("shard").and_then(|v| v.parse::<u8>().ok()).map(SiteId);
            let mut init = RequestInit::new();
            init.with_method(if inner == "/status" { Method::Get } else { Method::Post });
            shard_at(c, &repo, at)?
                .fetch_with_request(Request::new_with_init(
                    &format!("https://shard{inner}?repo={}", repo.id.0.0),
                    &init,
                )?)
                .await
        }
        _ => text(c.rid, 404, "not found\n"),
    }
}

/// Packs the colo's IATA code into the integer `Colo` a `suspect` report carries.
fn colo_id(req: &Request) -> u32 {
    let colo = req.cf().map(|c| c.colo()).unwrap_or_default();
    colo.bytes().take(4).fold(0u32, |n, b| n.wrapping_mul(256).wrapping_add(u32::from(b)))
}

#[event(fetch)]
async fn fetch(mut req: Request, env: Env, ctx: Context) -> Result<Response> {
    let rid = RequestId(Ulid::new(now(), rand10()));
    let cfg = Config::read(&env)?;
    let url = req.url()?;
    let path = url.path().to_owned();
    let sites: SiteTable = serde_json::from_str(&env.var("GRAFTYARD_SITES")?.to_string())?;
    let anchor = match (env.var("GRAFTYARD_INSTALL"), env.var("GRAFTYARD_CUSTOMER_KEY")) {
        (Ok(i), Ok(k)) => Anchor::from_vars(&i.to_string(), &k.to_string()).ok(),
        _ => None,
    };
    graftyard_lookup::set_colo(colo_id(&req));
    let c = Ctx {
        env: &env,
        cfg: &cfg,
        rid,
        sites: &sites,
        anchor,
        colo: colo_id(&req),
        #[cfg(feature = "test-faults")]
        kill: req.headers().get("x-graft-kill")?,
        #[cfg(feature = "test-faults")]
        down_ms: req.headers().get("x-graft-down-ms")?.and_then(|d| d.parse().ok()).unwrap_or(0),
        #[cfg(feature = "test-faults")]
        shard: match (req.headers().get("x-graft-shard")?, req.headers().get("x-bed-token")?, env.secret("BED_TOKEN")) {
            (Some(n), Some(t), Ok(s)) if t == s.to_string() => n.parse().ok().map(SiteId),
            _ => None,
        },
    };
    if path == release::ENTRIES_PATH {
        return release::entries(&c, &mut req).await;
    }
    #[cfg(feature = "test-faults")]
    if path.starts_with("/_bed/") {
        return bed(&c, &mut req, &path).await;
    }
    // REST, GraphQL, LFS, uploads, and codeload (`api`); git smart HTTP below.
    let authority = url.host_str().unwrap_or_default().to_owned();
    if let Ok(host) = graftyard_wire::Host::new(&cfg.host, false)
        && let Some(e) = host.endpoint(&authority, &path)
        && api::serves(&e)
    {
        return api::serve(&c, &mut req, &host, e).await;
    }
    let ua = req.headers().get("user-agent")?;
    let incoming = graftyard_wire::Incoming { user_agent: ua.as_deref(), ..Default::default() };
    if let Err(refused) = graftyard_wire::serve(&incoming, graftyard_wire::Surface::Git) {
        let body = match refused.body {
            graftyard_wire::Body::Bytes(b) => String::from_utf8(b).unwrap_or_default(),
            _ => String::new(),
        };
        return text(rid, refused.status, &body);
    }
    let Some(route) = smart::route(&path, url.query().unwrap_or("")) else { return text(rid, 404, "Not Found\n") };
    let (owner, name) = match &route {
        Route::InfoRefs { owner, repo, .. } | Route::Rpc { owner, repo, .. } | Route::Dumb { owner, repo } => {
            (owner.clone(), repo.clone())
        }
    };
    let repo = match resolve(&c, &owner, &name).await? {
        Ok(Some(r)) => r,
        Ok(None) => return text(rid, 404, "Repository not found.\n"),
        Err(e) => return text(rid, 503, &format!("the repository lookup failed; retry ({e})\n")),
    };
    let repo = &repo;
    let r = match route {
        // Dumb HTTP is refused: info/refs without a service is 403 and object paths 404, as on github.com.
        Route::InfoRefs { service: None, .. } => text(rid, 403, "Please upgrade your git client.\n"),
        Route::Dumb { .. } => text(rid, 404, "Not Found\n"),
        Route::InfoRefs { service: Some(s), .. } => info_refs(&c, &req, repo, s).await,
        Route::Rpc { service: Service::UploadPack, .. } => {
            if req.method() != Method::Post {
                return text(rid, 405, "Method Not Allowed\n");
            }
            upload_pack(&c, &mut req, repo, &ctx).await
        }
        Route::Rpc { service: Service::ReceivePack, .. } => {
            if req.method() != Method::Post {
                return text(rid, 405, "Method Not Allowed\n");
            }
            receive_pack(&c, req, repo, &format!("{owner}/{name}")).await
        }
    };
    match r {
        Ok(r) => Ok(r),
        Err(e) => text(rid, 500, &format!("{e}\n")),
    }
}
