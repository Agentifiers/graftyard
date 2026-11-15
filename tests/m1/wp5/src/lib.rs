//! BF-18 bed, `graft-m1-wp5`: git cache shards from `crates/git` on Cloudflare Containers with real git
//! (`containers/git`), three real buckets (`graft-m1-wp5-a` enam, `-b` weur, `-c` apac), and a stub chain leader.
//!
//! - `Chain` stands for one member chain's leader, which no site script hosts yet: an SQLite log of entries naming
//!   packs (`LogPacks`), the fold's refs with an `expect` check per update, the last `Promoted`, and the first index
//!   still in the log, which `Archived` raises. It implements nothing of Raft; it is the `ChainLog` a shard reads.
//! - `GitShard` is the shard class: a container-backed object under the `durable_object` policy that runs
//!   `ShardHost` (`crates/git/src/host.rs`): catch-up behind its `IngestCursor` before every call, `validate`,
//!   `upload_pack`, `rehydrate`, the start-time region check, and the inactivity alarm.
//! - The Worker's fetch is the test driver's edge, behind a bearer token. A push is validate on the home shard, then
//!   `PackStore::place`, then the stub's append (the commit), then the `Committed` markers. An archive is the home
//!   shard's promotion of the range's inline packs into one `DurablePack`, the markers of the range re-put, `Promoted`,
//!   the archive object in two buckets, and `Archived`, which truncates the stub's log.
//!
//! Routes: `POST /push?u=<ref>:<old>:<new>[&u=...][&kill_marker=1]`, `POST /archive?from=F&to=T`, `POST /kill`,
//! `POST /teardown?objects=<object name>@<site>,...` (container and alarm, after a run),
//! `POST /shard/<name>/{catchup,fsck,status}`, smart HTTP at `/git/<name>/info/refs` and `/git/<name>/git-upload-pack`.

use std::collections::BTreeSet;

use graftyard_git::group::RefsPage;
use graftyard_git::host::{R2, ShardHost};
use graftyard_git::{
    Bucket, ChainLog, Copies, Head, INLINE_MAX, LogPacks, ObjectMeta, PackId, PackIndex, PackRef, PackStore, Place,
    Promoted, R2Key, StoreError, mark_committed, promotion_inputs,
};
use graftyard_types::{
    ChainId, DbId, Digest, Index, NetworkId, Region, RepoId, RequestId, Site, SiteId, SiteLink, SiteTable, Timestamp,
    Ulid,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use worker::*;

/// One network per run of the row, so a rerun starts from empty buckets, a new chain, and new shard objects. Set at
/// build time: `BF18_NET=5000002 worker-build --release`.
const NET_ID: i64 = match option_env!("BF18_NET") {
    Some(s) => parse(s),
    None => 5_000_001,
};
const NET: NetworkId = NetworkId(DbId(NET_ID));
const CHAIN: ChainId = ChainId::Repo(RepoId(DbId(NET_ID)));

const fn parse(s: &str) -> i64 {
    let b = s.as_bytes();
    let mut i = 0;
    let mut v = 0i64;
    while i < b.len() {
        v = v * 10 + (b[i] - b'0') as i64;
        i += 1;
    }
    v
}
const SITES: [(u8, &str, Region); 3] =
    [(0, "BUCKET_A", Region::Enam), (1, "BUCKET_B", Region::Weur), (2, "BUCKET_C", Region::Apac)];
const ZERO: &str = "0000000000000000000000000000000000000000";

fn table() -> SiteTable {
    SiteTable(
        SITES
            .iter()
            .map(|(id, b, r)| Site {
                id: SiteId(*id),
                script: "graft-m1-wp5".into(),
                account: String::new(),
                region: *r,
                jurisdiction: None,
                bucket: (*b).into(),
                release: None,
                link: SiteLink::Binding { member_class: "Member".into() },
                spare: false,
            })
            .collect(),
    )
}

fn buckets(env: &Env) -> Result<Vec<(SiteId, R2)>> {
    SITES.iter().map(|(id, b, _)| Ok((SiteId(*id), R2(env.bucket(b)?)))).collect()
}

fn rand10() -> [u8; 10] {
    let mut b = [0u8; 10];
    let crypto: web_sys::Crypto = js_sys::Reflect::get(&js_sys::global(), &"crypto".into()).unwrap().into();
    crypto.get_random_values_with_u8_array(&mut b).unwrap();
    b
}

fn now() -> Timestamp {
    Timestamp(Date::now().as_millis())
}

fn se(e: StoreError) -> Error {
    Error::RustError(e.0)
}

fn ws(e: Error) -> StoreError {
    StoreError(e.to_string())
}

fn q(url: &Url, k: &str) -> Option<String> {
    url.query_pairs().find(|(n, _)| n == k).map(|(_, v)| v.into_owned())
}

async fn call(stub: &Stub, method: Method, path: &str, body: Option<Vec<u8>>) -> Result<Response> {
    let mut init = RequestInit::new();
    init.with_method(method);
    if let Some(b) = body {
        init.with_body(Some(js_sys::Uint8Array::from(b.as_slice()).into()));
    }
    stub.fetch_with_request(Request::new_with_init(&format!("https://do{path}"), &init)?).await
}

async fn ok_json<T: for<'de> Deserialize<'de>>(mut r: Response) -> Result<T> {
    if r.status_code() >= 300 {
        let t = r.text().await.unwrap_or_default();
        return Err(Error::RustError(format!("{} {t}", r.status_code())));
    }
    r.json().await
}

// ---- the stub chain leader ----

#[durable_object(fetch)]
pub struct Chain {
    state: State,
}

#[derive(Serialize, Deserialize)]
struct Append {
    updates: Vec<(String, String, String)>,
    packs: Vec<PackRef>,
    promoted: Option<Promoted>,
    archived: Option<(u64, u64)>,
}

#[derive(Deserialize)]
struct Kv {
    v: String,
}

impl Chain {
    fn get(&self, k: &str) -> Result<Option<String>> {
        let rows: Vec<Kv> =
            self.state.storage().sql().exec("SELECT v FROM kv WHERE k = ?", vec![k.into()])?.to_array()?;
        Ok(rows.into_iter().next().map(|r| r.v))
    }
    fn set(&self, k: &str, v: &str) -> Result<()> {
        self.state.storage().sql().exec("INSERT OR REPLACE INTO kv (k, v) VALUES (?, ?)", vec![k.into(), v.into()])?;
        Ok(())
    }
    fn applied(&self) -> Result<u64> {
        Ok(self.get("applied")?.map_or(0, |v| v.parse().unwrap_or(0)))
    }
    fn refs(&self) -> Result<Vec<(String, String)>> {
        #[derive(Deserialize)]
        struct R {
            name: String,
            oid: String,
        }
        let rows: Vec<R> =
            self.state.storage().sql().exec("SELECT name, oid FROM refs ORDER BY name", None)?.to_array()?;
        Ok(rows.into_iter().map(|r| (r.name, r.oid)).collect())
    }
}

impl DurableObject for Chain {
    fn new(state: State, _env: Env) -> Self {
        let sql = state.storage().sql();
        for t in [
            "CREATE TABLE IF NOT EXISTS log (idx INTEGER PRIMARY KEY, body TEXT NOT NULL)",
            "CREATE TABLE IF NOT EXISTS refs (name TEXT PRIMARY KEY, oid TEXT NOT NULL)",
            "CREATE TABLE IF NOT EXISTS kv (k TEXT PRIMARY KEY, v TEXT NOT NULL)",
        ] {
            sql.exec(t, None).unwrap();
        }
        Chain { state }
    }

    async fn fetch(&self, mut req: Request) -> Result<Response> {
        let url = req.url()?;
        let sql = self.state.storage().sql();
        match url.path() {
            // One committed entry: every update's `old` must equal the fold's ref (an `expect`), or nothing commits.
            "/append" => {
                let a: Append = req.json().await?;
                let refs: std::collections::BTreeMap<String, String> = self.refs()?.into_iter().collect();
                for (name, old, _) in &a.updates {
                    let have = refs.get(name).map(String::as_str).unwrap_or(ZERO);
                    if have != old {
                        return Response::error(format!("stale: {name} is {have}, not {old}"), 409);
                    }
                }
                let index = self.applied()? + 1;
                let changes =
                    a.updates.iter().map(|(name, _, new)| (name.clone(), (new != ZERO).then(|| new.clone()))).collect();
                let entry = LogPacks {
                    index: Index(index),
                    packs: a.packs,
                    promoted: a.promoted.clone(),
                    consumed: vec![],
                    refs: Some(changes),
                };
                sql.exec(
                    "INSERT INTO log (idx, body) VALUES (?, ?)",
                    vec![(index as i64).into(), serde_json::to_string(&entry)?.into()],
                )?;
                for (name, _, new) in &a.updates {
                    if new == ZERO {
                        sql.exec("DELETE FROM refs WHERE name = ?", vec![name.clone().into()])?;
                    } else {
                        sql.exec(
                            "INSERT OR REPLACE INTO refs (name, oid) VALUES (?, ?)",
                            vec![name.clone().into(), new.clone().into()],
                        )?;
                    }
                }
                if let Some(p) = &a.promoted {
                    self.set("promoted", &serde_json::to_string(&(index, p))?)?;
                }
                // `Archived`: followers truncate through the range once it is committed.
                if let Some((_, to)) = a.archived {
                    sql.exec("DELETE FROM log WHERE idx <= ?", vec![(to as i64).into()])?;
                    self.set("first", &(to + 1).to_string())?;
                }
                self.set("applied", &index.to_string())?;
                Response::from_json(&index)
            }
            // The test driver's view of the chain (`bf18.sh`), and the shard's paged read: its few refs are one page.
            "/refs" => Response::from_json(&serde_json::json!({ "applied": self.applied()?, "refs": self.refs()? })),
            "/first" => Response::from_json(&self.get("first")?.map_or(1, |v| v.parse::<u64>().unwrap_or(1))),
            "/promoted" => {
                let p: Option<(u64, Promoted)> = self.get("promoted")?.map(|v| serde_json::from_str(&v)).transpose()?;
                Response::from_json(&p)
            }
            "/entries" => {
                let from: u64 = q(&url, "from").and_then(|v| v.parse().ok()).unwrap_or(1);
                let to: u64 = q(&url, "to").and_then(|v| v.parse().ok()).unwrap_or(0);
                let first = self.get("first")?.map_or(1, |v| v.parse::<u64>().unwrap_or(1));
                if from < first {
                    return Response::error(format!("entries below {first} are archived"), 410);
                }
                #[derive(Deserialize)]
                struct B {
                    body: String,
                }
                let rows: Vec<B> = sql
                    .exec(
                        "SELECT body FROM log WHERE idx >= ? AND idx <= ? ORDER BY idx",
                        vec![(from as i64).into(), (to as i64).into()],
                    )?
                    .to_array()?;
                let out: Vec<LogPacks> =
                    rows.iter().map(|r| serde_json::from_str(&r.body)).collect::<std::result::Result<_, _>>()?;
                Response::from_json(&out)
            }
            _ => Response::error("not found", 404),
        }
    }
}

struct StubChain {
    stub: Stub,
}

impl ChainLog for StubChain {
    async fn head(&self, _chain: ChainId) -> std::result::Result<Head, StoreError> {
        let v: serde_json::Value =
            ok_json(call(&self.stub, Method::Get, "/refs", None).await.map_err(ws)?).await.map_err(ws)?;
        let first =
            ok_json::<u64>(call(&self.stub, Method::Get, "/first", None).await.map_err(ws)?).await.map_err(ws)?;
        Ok(Head { first: Index(first), applied: Index(v["applied"].as_u64().unwrap_or(0)) })
    }
    async fn refs_page(
        &self,
        _c: ChainId,
        _after: Option<String>,
    ) -> std::result::Result<(RefsPage, Index), StoreError> {
        let v: serde_json::Value =
            ok_json(call(&self.stub, Method::Get, "/refs", None).await.map_err(ws)?).await.map_err(ws)?;
        let refs = serde_json::from_value(v["refs"].clone()).map_err(|e| StoreError(e.to_string()))?;
        Ok((RefsPage { refs, next: None }, Index(v["applied"].as_u64().unwrap_or(0))))
    }
    async fn entries(&self, _chain: ChainId, from: Index, to: Index) -> std::result::Result<Vec<LogPacks>, StoreError> {
        let path = format!("/entries?from={}&to={}", from.0, to.0);
        ok_json(call(&self.stub, Method::Get, &path, None).await.map_err(ws)?).await.map_err(ws)
    }
    async fn last_promoted(&self, _chain: ChainId) -> std::result::Result<Option<(Index, Promoted)>, StoreError> {
        let p: Option<(u64, Promoted)> =
            ok_json(call(&self.stub, Method::Get, "/promoted", None).await.map_err(ws)?).await.map_err(ws)?;
        Ok(p.map(|(i, p)| (Index(i), p)))
    }
    async fn consumed(&self, _network: NetworkId) -> std::result::Result<BTreeSet<PackId>, StoreError> {
        Ok(BTreeSet::new())
    }
}

fn chain_stub(env: &Env) -> Result<Stub> {
    env.durable_object("CHAIN")?.get_by_name_with_location_hint(&format!("chain#{NET_ID}"), "enam")
}

// ---- the shard class ----

#[durable_object]
pub struct GitShard {
    state: State,
    env: Env,
}

#[derive(Serialize, Deserialize)]
struct Me {
    site: u8,
}

impl GitShard {
    async fn me(&self) -> Result<u8> {
        Ok(self.state.storage().get::<Me>("me").await?.map_or(0, |m| m.site))
    }

    fn host<'a>(&'a self, site: u8) -> ShardHost<'a> {
        ShardHost { state: &self.state, region: SITES[site as usize].2, image: "git", instance: "standard-2" }
    }

    /// Its own site's bucket first, then the other copies.
    fn my_buckets(&self, site: u8) -> Result<Vec<(SiteId, R2)>> {
        Ok(graftyard_git::catchup::own_first(SiteId(site), buckets(&self.env)?))
    }
}

impl DurableObject for GitShard {
    fn new(state: State, env: Env) -> Self {
        GitShard { state, env }
    }

    async fn fetch(&self, mut req: Request) -> Result<Response> {
        let url = req.url()?;
        if let Some(site) = q(&url, "site").and_then(|v| v.parse::<u8>().ok()) {
            self.state.storage().put("me", Me { site }).await?;
        }
        let site = self.me().await?;
        let host = self.host(site);
        let owned = self.my_buckets(site)?;
        let bs: Vec<(SiteId, &R2)> = owned.iter().map(|(s, b)| (*s, b)).collect();
        let log = StubChain { stub: chain_stub(&self.env)? };
        let path = url.path().to_owned();
        if !matches!(path.as_str(), "/kill" | "/status" | "/teardown") {
            host.touch(now()).await.map_err(se)?;
        }
        match path.as_str() {
            "/catchup" => Response::from_json(&host.catch_up(NET, &[CHAIN], &bs, &log).await.map_err(se)?),
            "/validate" => {
                host.catch_up(NET, &[CHAIN], &bs, &log).await.map_err(se)?;
                let tips: Vec<String> = q(&url, "tips")
                    .map(|t| t.split(',').filter(|s| !s.is_empty()).map(str::to_owned).collect())
                    .unwrap_or_default();
                let mut head = tips.join("\n").into_bytes();
                head.push(b'\n');
                let tipsn = head.len();
                head.extend_from_slice(&req.bytes().await?);
                let body = Some(js_sys::Uint8Array::from(head.as_slice()).into());
                match host.validate(NET, tipsn, 0, false, false, body).await {
                    Ok(graftyard_git::host::Validated::Done(Some(ix), _)) => Response::from_json(&ix),
                    Ok(_) => Response::error("validate gave no index", 422),
                    Err((status, e)) => Response::error(e.0, graftyard_git::validate_status(status)),
                }
            }
            "/promote" => {
                host.catch_up(NET, &[CHAIN], &bs, &log).await.map_err(se)?;
                let sums: Vec<Digest> =
                    req.json::<Vec<String>>().await?.iter().filter_map(|s| Digest::parse_hex(s)).collect();
                host.promote(NET, &sums).await.map_err(se)
            }
            "/fsck" => Response::from_json(&host.fsck(NET).await.map_err(se)?),
            // The LRU on a real container: `fill` empty filler networks (ids NET_ID + 1..), then `make_room` for
            // `keep` with the disk taken as `disk` bytes, and the disk's listing before and after.
            "/lru" => {
                host.up().await.map_err(se)?;
                let fill: i64 = q(&url, "fill").and_then(|v| v.parse().ok()).unwrap_or(0);
                for i in 1..=fill {
                    host.init(NetworkId(DbId(NET_ID + i)), true).await.map_err(se)?;
                }
                let keep = NetworkId(DbId(q(&url, "keep").and_then(|v| v.parse().ok()).unwrap_or(NET_ID)));
                let disk = q(&url, "disk").and_then(|v| v.parse().ok());
                let before = host.repos().await.map_err(se)?.1;
                let room = host.make_room(keep, 0, disk).await.map_err(se)?;
                let after = host.repos().await.map_err(se)?.1;
                let health = host.health().await.map_err(se)?;
                let cursor = host.cursor(NET, &health.disk).await.map_err(se)?.map(|c| c.at(CHAIN).0);
                Response::from_json(&serde_json::json!({
                    "before": before, "room": room, "after": after, "cursor": cursor,
                }))
            }
            "/status" => {
                let running = self.state.container().is_some_and(|c| c.running());
                let health = if running { host.health().await.ok() } else { None };
                let cursor = match &health {
                    Some(h) => host.cursor(NET, &h.disk).await.map_err(se)?,
                    None => None,
                };
                let applied = log.head(CHAIN).await.map_err(se)?.applied;
                Response::from_json(&serde_json::json!({
                    "running": running, "site": site, "applied": applied.0,
                    "cursor": cursor.map(|c| c.at(CHAIN).0),
                    "health": health.map(|h| serde_json::json!({ "disk": h.disk, "git": h.git, "region": h.region, "location": h.location })),
                    "alarm": self.state.storage().get_alarm().await?,
                }))
            }
            "/kill" => {
                host.destroy().await.map_err(se)?;
                Response::from_json(&serde_json::json!({ "killed": true }))
            }
            // Bed teardown: the container and the renewal alarm, so nothing runs on after the row.
            "/teardown" => {
                host.destroy().await.map_err(se)?;
                self.state.storage().delete_alarm().await?;
                Response::from_json(&serde_json::json!({ "torn_down": true }))
            }
            "/info-refs" | "/upload-pack" => {
                host.catch_up(NET, &[CHAIN], &bs, &log).await.map_err(se)?;
                let proto = req.headers().get("git-protocol")?.unwrap_or_default();
                let enc = req.headers().get("content-encoding")?.unwrap_or_default();
                let body = if path == "/upload-pack" { Some(req.bytes().await?) } else { None };
                host.upload_pack(NET, path == "/info-refs", body, &proto, &enc).await.map_err(se)
            }
            _ => Response::error("not found", 404),
        }
    }

    async fn alarm(&self) -> Result<Response> {
        let site = self.me().await?;
        self.host(site).on_alarm(now()).await.map_err(se)?;
        Response::empty()
    }
}

// ---- the driver's edge ----

/// Shard names are `<site>-<n>`: `0-0` is the home shard on site A, `1-0` the second warm site's shard.
fn shard(env: &Env, name: &str) -> Result<(Stub, u8)> {
    let site: u8 = name.split('-').next().and_then(|s| s.parse().ok()).filter(|s| *s < 3).ok_or("bad shard")?;
    let do_name = format!("git#{site}#{name}#{NET_ID}");
    Ok((env.durable_object("SHARD")?.get_by_name_with_location_hint(&do_name, SITES[site as usize].2.hint())?, site))
}

async fn on_shard(env: &Env, name: &str, path: &str, method: Method, body: Option<Vec<u8>>) -> Result<Response> {
    let (stub, site) = shard(env, name)?;
    let sep = if path.contains('?') { '&' } else { '?' };
    call(&stub, method, &format!("{path}{sep}site={site}"), body).await
}

fn index_of_ok(ix: &PackIndex, body: &[u8]) -> bool {
    Digest(Sha256::digest(body).into()) == ix.checksum
}

async fn push(env: &Env, url: &Url, body: Vec<u8>) -> Result<Response> {
    let updates: Vec<(String, String, String)> = url
        .query_pairs()
        .filter(|(k, _)| k == "u")
        .filter_map(|(_, v)| {
            let p: Vec<&str> = v.split(':').collect();
            (p.len() == 3).then(|| (p[0].to_owned(), p[1].to_owned(), p[2].to_owned()))
        })
        .collect();
    let tips: Vec<&str> = updates.iter().map(|(_, _, n)| n.as_str()).filter(|n| *n != ZERO).collect();
    let mut r =
        on_shard(env, "0-0", &format!("/validate?tips={}", tips.join(",")), Method::Post, Some(body.clone())).await?;
    if r.status_code() != 200 {
        return Response::error(format!("validate: {}", r.text().await?), 422);
    }
    let ix: PackIndex = r.json().await?;
    if !index_of_ok(&ix, &body) {
        return Response::error("validate indexed other bytes", 500);
    }
    let raw = buckets(env)?;
    let refs: Vec<(SiteId, &R2)> = raw.iter().map(|(s, b)| (*s, b)).collect();
    let sites = table();
    let store = PackStore { sites: &sites, buckets: &refs, copies: Copies::TwoRegions };
    let id = PackId(Ulid::new(now(), rand10()));
    let request = RequestId(Ulid::new(now(), rand10()));
    let place = Place {
        network: NET,
        id,
        meta: ObjectMeta { chain: CHAIN, request },
        bytes: &body,
        index: &ix,
        idx: None,
        idx_range: None,
        inline_max: INLINE_MAX,
    };
    let durable = store.place(place).await.map_err(|e| Error::RustError(e.to_string()))?;
    let pack = durable.into_pack();
    let a = Append { updates, packs: vec![pack.clone()], promoted: None, archived: None };
    let mut c = call(&chain_stub(env)?, Method::Post, "/append", Some(serde_json::to_vec(&a)?)).await?;
    if c.status_code() != 200 {
        return Response::error(format!("commit: {}", c.text().await?), 409);
    }
    let index: u64 = c.json().await?;
    let killed = url.query_pairs().any(|(k, v)| k == "kill_marker" && v == "1");
    let failed = if killed { 0 } else { mark_committed(&refs, NET, &pack, CHAIN, Index(index)).await };
    Response::from_json(&serde_json::json!({
        "index": index, "stored": pack.stored_id().map(|i| i.to_string()), "bytes": body.len(),
        "objects": ix.objects, "marker": if killed { "killed" } else if failed == 0 { "put" } else { "failed" },
    }))
}

/// Archive `from..=to`: the home shard packs the range's inline packs into one stored pack, the range's stored packs'
/// markers are re-put, `Promoted` commits and its marker is put, the archive object goes to two buckets, and
/// `Archived` commits and truncates.
async fn archive(env: &Env, from: u64, to: u64) -> Result<Response> {
    let log = StubChain { stub: chain_stub(env)? };
    let entries = log.entries(CHAIN, Index(from), Index(to)).await.map_err(se)?;
    let (inline, stored) = promotion_inputs(&entries, Index(from), Index(to));
    let sums: Vec<String> = inline.iter().map(|(_, b)| Digest(Sha256::digest(b).into()).hex()).collect();
    let mut r = on_shard(env, "0-0", "/promote", Method::Post, Some(serde_json::to_vec(&sums)?)).await?;
    if r.status_code() != 200 {
        return Response::error(format!("promote: {}", r.text().await?), 500);
    }
    let promoted = r.bytes().await?;
    let mut v = on_shard(env, "0-0", "/validate", Method::Post, Some(promoted.clone())).await?;
    if v.status_code() != 200 {
        return Response::error(format!("validate promoted: {}", v.text().await?), 500);
    }
    let ix: PackIndex = v.json().await?;
    let raw = buckets(env)?;
    let refs: Vec<(SiteId, &R2)> = raw.iter().map(|(s, b)| (*s, b)).collect();
    let sites = table();
    let store = PackStore { sites: &sites, buckets: &refs, copies: Copies::TwoRegions };
    let id = PackId(Ulid::new(now(), rand10()));
    let request = RequestId(Ulid::new(now(), rand10()));
    // A promoted pack is always stored, whatever its size.
    let place = Place {
        network: NET,
        id,
        meta: ObjectMeta { chain: CHAIN, request },
        bytes: &promoted,
        index: &ix,
        idx: None,
        idx_range: None,
        inline_max: 0,
    };
    let pack = store.place(place).await.map_err(|e| Error::RustError(e.to_string()))?.into_pack();
    // The range's stored packs: their markers again, so markers are complete below the promotion.
    let mut remarked = 0;
    for e in &entries {
        for p in e.packs.iter().filter(|p| p.stored_id().is_some()) {
            mark_committed(&refs, NET, p, CHAIN, e.index).await;
            remarked += 1;
        }
    }
    let p = Promoted { from: Index(from), to: Index(to), pack: pack.clone() };
    let a = Append { updates: vec![], packs: vec![], promoted: Some(p), archived: None };
    let pi: u64 =
        ok_json(call(&chain_stub(env)?, Method::Post, "/append", Some(serde_json::to_vec(&a)?)).await?).await?;
    let failed = mark_committed(&refs, NET, &pack, CHAIN, Index(pi)).await;
    // The archive object, keyed by range and digest, create-only in two buckets, before `Archived` commits.
    let bytes = serde_json::to_vec(&entries)?;
    let digest = Digest(Sha256::digest(&bytes).into());
    let key = R2Key::Archive { chain: CHAIN, from: Index(from), to: Index(to), digest }.to_string();
    for (_, b) in raw.iter().take(2) {
        b.put_new(&key, &bytes, digest, &[]).await.map_err(se)?;
    }
    let a = Append { updates: vec![], packs: vec![], promoted: None, archived: Some((from, to)) };
    let ai: u64 =
        ok_json(call(&chain_stub(env)?, Method::Post, "/append", Some(serde_json::to_vec(&a)?)).await?).await?;
    Response::from_json(&serde_json::json!({
        "inline_packs": inline.len(), "stored_in_range": stored.len(), "remarked": remarked,
        "promoted": { "id": pack.stored_id().map(|i| i.to_string()), "bytes": promoted.len(), "objects": ix.objects, "index": pi, "marker_failures": failed },
        "archived_index": ai, "archive_key": key,
    }))
}

#[event(fetch)]
async fn fetch(mut req: Request, env: Env, _ctx: Context) -> Result<Response> {
    let token = env.secret("TOKEN")?.to_string();
    if req.headers().get("authorization")?.as_deref() != Some(&format!("Bearer {token}")) {
        return Response::error("unauthorized", 401).map(|r| {
            let h = Headers::new();
            let _ = h.set("www-authenticate", "Bearer");
            r.with_headers(h)
        });
    }
    let url = req.url()?;
    let path = url.path().to_owned();
    let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    match parts.as_slice() {
        ["push"] => {
            let body = req.bytes().await?;
            push(&env, &url, body).await
        }
        ["archive"] => {
            let from = q(&url, "from").and_then(|v| v.parse().ok()).ok_or("from")?;
            let to = q(&url, "to").and_then(|v| v.parse().ok()).ok_or("to")?;
            archive(&env, from, to).await
        }
        ["kill"] => {
            let mut out = Vec::new();
            for name in q(&url, "shards").unwrap_or_else(|| "0-0,1-0".into()).split(',') {
                out.push(ok_json::<serde_json::Value>(on_shard(&env, name, "/kill", Method::Post, None).await?).await?);
            }
            Response::from_json(&out)
        }
        ["refs"] => call(&chain_stub(&env)?, Method::Get, "/refs", None).await,
        // `objects=<do name>@<site>,...`: shard objects of any run, by their object names.
        ["teardown"] => {
            let mut out = Vec::new();
            for o in q(&url, "objects").unwrap_or_default().split(',').filter(|o| !o.is_empty()) {
                let (name, site) = o.rsplit_once('@').ok_or("objects=<name>@<site>")?;
                let site: u8 = site.parse().map_err(|_| "site")?;
                let stub =
                    env.durable_object("SHARD")?.get_by_name_with_location_hint(name, SITES[site as usize].2.hint())?;
                let r: serde_json::Value = ok_json(call(&stub, Method::Post, "/teardown", None).await?).await?;
                out.push(serde_json::json!({ "object": name, "result": r }));
            }
            Response::from_json(&out)
        }
        ["shard", name, op] => {
            let op = match url.query() {
                Some(qs) => format!("/{op}?{qs}"),
                None => format!("/{op}"),
            };
            on_shard(&env, name, &op, Method::Post, None).await
        }
        ["git", name, "info", "refs"] => {
            let (stub, site) = shard(&env, name)?;
            let mut init = RequestInit::new();
            let h = Headers::new();
            if let Some(p) = req.headers().get("git-protocol")? {
                h.set("git-protocol", &p)?;
            }
            init.with_method(Method::Get).with_headers(h);
            stub.fetch_with_request(Request::new_with_init(&format!("https://do/info-refs?site={site}"), &init)?).await
        }
        ["git", name, "git-upload-pack"] => {
            let (stub, site) = shard(&env, name)?;
            let body = req.bytes().await?;
            let mut init = RequestInit::new();
            let h = Headers::new();
            for k in ["git-protocol", "content-encoding", "content-type"] {
                if let Some(v) = req.headers().get(k)? {
                    h.set(k, &v)?;
                }
            }
            init.with_method(Method::Post)
                .with_headers(h)
                .with_body(Some(js_sys::Uint8Array::from(body.as_slice()).into()));
            stub.fetch_with_request(Request::new_with_init(&format!("https://do/upload-pack?site={site}"), &init)?)
                .await
        }
        _ => Response::error("not found", 404),
    }
}
