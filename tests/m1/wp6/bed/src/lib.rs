//! M1-WP6 bed, `graft-m1-wp6-bed`: the classes the edge (`workers/edge`, deployed as `graft-m1-edge`) reaches
//! through cross-script Durable Object bindings until a site script hosts them.
//!
//! - `Group`: the stand-in for a repository chain's group (`graftyard_git::standin`): three real
//!   `graftyard_raft::Member<Push>`s in one object, each member's state row and log in its SQLite, rebuilt on every
//!   start. It serves the member routes a site member serves (`/commit`, `/outcome`, `/outcome_by_request`, `/read`)
//!   in the same JSON (`graftyard_git::group`), plus `/token` (the `Tokens` leader read the edge's authentication
//!   makes), `/entries` (a shard's `ChainLog`), and `/log`, `/status`, `/seed` for the test driver. After each `Acked`
//!   naming a stored pack it puts the pack's `Committed` markers, as the committing leader does.
//! - `GitShard`: the git cache shard on Cloudflare Containers with `containers/git`, as in `tests/m1/wp5`, catching
//!   up behind its `IngestCursor` from `Group` before every call.

use std::cell::RefCell;
use std::collections::BTreeSet;

use graftyard_git::group::{
    CommitReply, CommitReq, OutcomeReply, OutcomeReq, PushOutcome, ReadReply, RefsPage, decode_commit,
};
use graftyard_git::host::{R2, ShardHost};
use graftyard_git::standin::{MemberRows, Seed, StandIn};
use graftyard_git::{Bucket, ChainLog, Head, LogPacks, PackId, PackRef, Promoted, R2Key, StoreError, mark_committed};
use graftyard_raft::{Write, codec};
use graftyard_types::{ChainId, DbId, Index, NetworkId, Region, ReleaseId, RepoId, SiteId, Timestamp};
use serde::Deserialize;
use serde_json::json;
use worker::*;

fn now() -> Timestamp {
    Timestamp(Date::now().as_millis())
}

fn q(url: &Url, k: &str) -> Option<String> {
    url.query_pairs().find(|(n, _)| n == k).map(|(_, v)| v.into_owned())
}

fn buckets(env: &Env) -> Result<Vec<(SiteId, R2)>> {
    ["BUCKET_A", "BUCKET_B", "BUCKET_C"]
        .iter()
        .enumerate()
        .map(|(i, b)| Ok((SiteId(i as u8), R2(env.bucket(b)?))))
        .collect()
}

// ---- the stand-in group ----

#[durable_object(fetch)]
pub struct Group {
    state: State,
    env: Env,
    group: RefCell<Option<StandIn>>,
}

/// The release the stand-in's members run, which `Unsupported` names.
const RELEASE: ReleaseId = ReleaseId([0x4e; 32]);

impl Group {
    fn sql(&self) -> SqlStorage {
        self.state.storage().sql()
    }

    fn seed(&self) -> Result<Option<Seed>> {
        #[derive(Deserialize)]
        struct R {
            v: String,
        }
        let rows: Vec<R> = self.sql().exec("SELECT v FROM kv WHERE k = 'seed'", None)?.to_array()?;
        rows.into_iter().next().map(|r| serde_json::from_str(&r.v).map_err(Error::from)).transpose()
    }

    /// Every member's rows, decoded with the crate's codec.
    fn rows(&self) -> Result<Vec<MemberRows>> {
        let mut out = vec![MemberRows::default(); 3];
        let sql = self.sql();
        for r in sql.exec("SELECT site, row FROM member_state", None)?.raw() {
            let r = r?;
            let (SqlStorageValue::Integer(site), SqlStorageValue::Blob(b)) = (&r[0], &r[1]) else {
                return Err("member_state row".into());
            };
            out[*site as usize].state = Some(codec::from_bytes(b).map_err(|e| Error::RustError(e.0))?);
        }
        for r in sql.exec("SELECT site, idx, row FROM log ORDER BY site, idx", None)?.raw() {
            let r = r?;
            let (SqlStorageValue::Integer(site), SqlStorageValue::Integer(idx), SqlStorageValue::Blob(b)) =
                (&r[0], &r[1], &r[2])
            else {
                return Err("log row".into());
            };
            let log = &mut out[*site as usize].log;
            if *idx as usize != log.len() + 1 {
                return Err(Error::RustError(format!("site {site}: log gap at {idx}")));
            }
            log.push(codec::from_bytes(b).map_err(|e| Error::RustError(e.0))?);
        }
        Ok(out)
    }

    /// The group, restored from its rows on first use after the object started.
    fn with<R>(&self, f: impl FnOnce(&mut StandIn) -> R) -> Result<R> {
        let mut g = self.group.borrow_mut();
        if g.is_none() {
            let seed = self.seed()?.ok_or("the group has no seed: POST /seed first")?;
            *g = Some(StandIn::restore(&seed, self.rows()?, now()));
        }
        let s = g.as_mut().expect("restored");
        s.advance(now());
        let r = f(s);
        self.persist(s.drain_writes())?;
        Ok(r)
    }

    /// Makes the members' writes durable. The output gate holds the reply until they are.
    fn persist(&self, writes: Vec<(SiteId, Write<graftyard_git::repo::RepoEvent>)>) -> Result<()> {
        let sql = self.sql();
        for (site, w) in writes {
            let s = i64::from(site.0);
            match w {
                Write::State(st) => {
                    sql.exec(
                        "INSERT OR REPLACE INTO member_state (site, row) VALUES (?, ?)",
                        vec![s.into(), SqlStorageValue::Blob(codec::to_bytes(&st))],
                    )?;
                }
                Write::Append(e) => {
                    sql.exec(
                        "INSERT OR REPLACE INTO log (site, idx, row) VALUES (?, ?, ?)",
                        vec![s.into(), (e.index.0 as i64).into(), SqlStorageValue::Blob(codec::to_bytes(&e))],
                    )?;
                }
                Write::Truncate(i) => {
                    sql.exec("DELETE FROM log WHERE site = ? AND idx >= ?", vec![s.into(), (i.0 as i64).into()])?;
                }
                // The fold is rebuilt from the log at restore.
                Write::Apply(_) => {}
                Write::Install(_) => unreachable!("the bed never installs a snapshot"),
            }
        }
        Ok(())
    }

    fn count(&self, k: &str) -> Result<()> {
        self.sql().exec(
            "INSERT INTO kv (k, v) VALUES (?, '1') ON CONFLICT(k) DO UPDATE SET v = CAST(v AS INTEGER) + 1",
            vec![k.into()],
        )?;
        Ok(())
    }

    fn counters(&self) -> Result<serde_json::Value> {
        #[derive(Deserialize)]
        struct R {
            k: String,
            v: String,
        }
        let rows: Vec<R> = self.sql().exec("SELECT k, v FROM kv WHERE k LIKE 'recv:%'", None)?.to_array()?;
        Ok(json!(rows.into_iter().map(|r| (r.k, r.v)).collect::<std::collections::BTreeMap<_, _>>()))
    }

    /// After `Acked`: the `Committed` markers of the stored pack the push names, in the buckets its `sites` names.
    async fn mark(&self, repo: RepoId, o: &PushOutcome, pack: Option<&PackRef>) -> Result<usize> {
        let (PushOutcome::Ok(ok), Some(p @ PackRef::Stored { .. })) = (o, pack) else { return Ok(0) };
        if !ok.out.refs.contains(&graftyard_git::repo::RefOutcome::Ok) {
            return Ok(0);
        }
        let raw = buckets(&self.env)?;
        let refs: Vec<(SiteId, &R2)> = raw.iter().map(|(s, b)| (*s, b)).collect();
        Ok(mark_committed(&refs, NetworkId(repo.0), p, ChainId::Repo(repo), Index(ok.index)).await)
    }
}

impl DurableObject for Group {
    fn new(state: State, env: Env) -> Self {
        let sql = state.storage().sql();
        for t in [
            "CREATE TABLE IF NOT EXISTS kv (k TEXT PRIMARY KEY, v TEXT NOT NULL)",
            "CREATE TABLE IF NOT EXISTS member_state (site INTEGER PRIMARY KEY, row BLOB NOT NULL)",
            "CREATE TABLE IF NOT EXISTS log (site INTEGER NOT NULL, idx INTEGER NOT NULL, row BLOB NOT NULL, PRIMARY KEY (site, idx))",
        ] {
            sql.exec(t, None).expect("schema");
        }
        Group { state, env, group: RefCell::new(None) }
    }

    async fn fetch(&self, mut req: Request) -> Result<Response> {
        let url = req.url()?;
        match url.path() {
            "/seed" => {
                let seed: Seed = req.json().await?;
                if let Some(old) = self.seed()?
                    && (old.repo, old.owner) != (seed.repo, seed.owner)
                {
                    return Response::error("a group's repository and owner never change", 409);
                }
                self.sql().exec(
                    "INSERT OR REPLACE INTO kv (k, v) VALUES ('seed', ?)",
                    vec![serde_json::to_string(&seed)?.into()],
                )?;
                *self.group.borrow_mut() = None;
                let leads = self.with(|g| g.ensure_leader())?;
                Response::from_json(&json!({ "seeded": true, "leads": leads }))
            }
            "/commit" => {
                let body = req.bytes().await?;
                let reply = match decode_commit(&body, RELEASE) {
                    Err(e) => {
                        // What a member answers for bytes it cannot decode: no state is touched.
                        self.count("recv:unsupported")?;
                        CommitReply::Error(e)
                    }
                    Ok(CommitReq { rid, cmd }) => {
                        let f = graftyard_raft::Command::format(&cmd);
                        self.count(&format!("recv:{}.{}", f.family, f.version))?;
                        let rid = rid.parse().map_err(|_| "bad rid")?;
                        let pack = cmd.pack.clone();
                        let repo = self.with(|g| g.repo)?;
                        let r = self.with(|g| g.commit(rid, cmd))?;
                        if let CommitReply::Done(o) = &r {
                            self.mark(repo, o, pack.as_ref()).await?;
                        }
                        r
                    }
                };
                Response::from_json(&reply)
            }
            "/outcome" => {
                let o: OutcomeReq = req.json().await?;
                let t = o.ticket.ticket().ok_or("bad ticket")?;
                Response::from_json(&self.with(|g| g.outcome(t))?)
            }
            "/outcome_by_request" => {
                let body = req.bytes().await?;
                let r = match decode_commit(&body, RELEASE) {
                    Err(e) => OutcomeReply::Error(e),
                    Ok(CommitReq { rid, cmd }) => {
                        let rid = rid.parse().map_err(|_| "bad rid")?;
                        self.with(|g| g.outcome_by_request(rid, &cmd))?
                    }
                };
                Response::from_json(&r)
            }
            "/read" => Response::from_json(&self.with(|g| g.read())?),
            "/token" => {
                let hash = q(&url, "hash").and_then(|h| graftyard_types::Digest::parse_hex(&h)).ok_or("hash")?;
                let row = self.with(|g| g.identity().token(&hash.0))?;
                Response::from_json(&row.map(|(a, revoked)| json!({ "account": a.0.0, "revoked": revoked })))
            }
            "/entries" => {
                let from = q(&url, "from").and_then(|v| v.parse().ok()).unwrap_or(1);
                let to = q(&url, "to").and_then(|v| v.parse().ok()).unwrap_or(u64::MAX);
                Response::from_json(&self.with(|g| g.entries(from, to))?)
            }
            "/log" => {
                let from = q(&url, "from").and_then(|v| v.parse().ok()).unwrap_or(1);
                Response::from_json(&self.with(|g| g.log(from))?)
            }
            "/status" => {
                let (members, leads) = self.with(|g| g.status())?;
                let refs = self.with(|g| g.refs().len())?;
                Response::from_json(&json!({
                    "members": members, "leads": leads, "refs": refs, "received": self.counters()?,
                }))
            }
            "/refs" => Response::from_json(&self.with(|g| g.refs())?),
            "/markers" => {
                // Per bucket, whether the pack object and its `Committed` marker exist, for the stored pack each
                // entry from `index` on names.
                let from = q(&url, "index").and_then(|v| v.parse().ok()).unwrap_or(1);
                let repo = self.with(|g| g.repo)?;
                let ids: Vec<PackId> = self
                    .with(|g| g.entries(from, u64::MAX))?
                    .iter()
                    .flat_map(|e| e.packs.iter().filter_map(PackRef::stored_id))
                    .collect();
                let mut out = Vec::new();
                for id in ids {
                    let net = NetworkId(repo.0);
                    let pack = R2Key::Pack { network: net, id };
                    let marker = R2Key::committed(pack.clone()).ok_or("marker")?.to_string();
                    let mut per = Vec::new();
                    for (site, b) in buckets(&self.env)? {
                        let has_pack = b.head(&pack.to_string()).await.map_err(se)?.is_some();
                        let has_marker = b.head(&marker).await.map_err(se)?.is_some();
                        per.push(json!({ "site": site.0, "pack": has_pack, "marker": has_marker }));
                    }
                    out.push(json!({ "id": id.to_string(), "stored": true, "buckets": per }));
                }
                Response::from_json(&out)
            }
            _ => Response::error("not found", 404),
        }
    }
}

// ---- the git cache shard ----

#[durable_object]
pub struct GitShard {
    state: State,
    env: Env,
}

struct GroupLog {
    stub: Stub,
}

async fn get_json<T: for<'de> Deserialize<'de>>(stub: &Stub, path: &str) -> std::result::Result<T, StoreError> {
    let ws = |e: Error| StoreError(e.to_string());
    let mut r = stub.fetch_with_str(&format!("https://group{path}")).await.map_err(ws)?;
    if r.status_code() >= 300 {
        return Err(StoreError(format!("{path}: {} {}", r.status_code(), r.text().await.unwrap_or_default())));
    }
    r.json().await.map_err(ws)
}

/// The stand-in's refs are a few test refs, so its `/read` answers them as one page.
impl ChainLog for GroupLog {
    async fn head(&self, _chain: ChainId) -> std::result::Result<Head, StoreError> {
        match get_json::<ReadReply>(&self.stub, "/read").await? {
            ReadReply::Refs(r) => Ok(Head { first: Index(1), applied: Index(r.index) }),
            ReadReply::Error { error } => Err(StoreError(format!("read: {error:?}"))),
        }
    }
    async fn refs_page(
        &self,
        _c: ChainId,
        _after: Option<String>,
    ) -> std::result::Result<(RefsPage, Index), StoreError> {
        match get_json::<ReadReply>(&self.stub, "/read").await? {
            ReadReply::Refs(r) => Ok((RefsPage { refs: r.value, next: None }, Index(r.index))),
            ReadReply::Error { error } => Err(StoreError(format!("read: {error:?}"))),
        }
    }
    async fn entries(&self, _c: ChainId, from: Index, to: Index) -> std::result::Result<Vec<LogPacks>, StoreError> {
        get_json(&self.stub, &format!("/entries?from={}&to={}", from.0, to.0)).await
    }
    async fn last_promoted(&self, _c: ChainId) -> std::result::Result<Option<(Index, Promoted)>, StoreError> {
        Ok(None)
    }
    async fn consumed(&self, _n: NetworkId) -> std::result::Result<BTreeSet<PackId>, StoreError> {
        Ok(BTreeSet::new())
    }
}

fn se(e: StoreError) -> Error {
    Error::RustError(e.0)
}

impl GitShard {
    fn host(&self) -> ShardHost<'_> {
        ShardHost { state: &self.state, region: Region::Enam, image: "git", instance: "standard-2" }
    }
}

impl DurableObject for GitShard {
    fn new(state: State, env: Env) -> Self {
        GitShard { state, env }
    }

    /// `?repo=<id>` names the network (a root repository's id is its network's) and the group it reads.
    async fn fetch(&self, mut req: Request) -> Result<Response> {
        let url = req.url()?;
        let repo: i64 = q(&url, "repo").and_then(|v| v.parse().ok()).ok_or("repo")?;
        let (net, chain) = (NetworkId(DbId(repo)), ChainId::Repo(RepoId(DbId(repo))));
        let host = self.host();
        let raw = buckets(&self.env)?;
        let bs: Vec<&R2> = raw.iter().map(|(_, b)| b).collect();
        let log = GroupLog {
            stub: self.env.durable_object("GROUP")?.get_by_name_with_location_hint(&format!("repo:{repo}"), "enam")?,
        };
        let path = url.path().to_owned();
        if !matches!(path.as_str(), "/teardown" | "/status") {
            host.touch(now()).await.map_err(se)?;
        }
        match path.as_str() {
            "/validate" => {
                host.catch_up(net, &[chain], &bs, &log).await.map_err(se)?;
                let n: usize = q(&url, "tipsn").and_then(|v| v.parse().ok()).unwrap_or(0);
                let body = req.inner().body().map(Into::into);
                match host.validate(net, n, 0, false, body).await {
                    Ok(graftyard_git::host::Validated::Done(Some(ix), _)) => Response::from_json(&ix),
                    Ok(_) => Response::error("validate gave no index", 422),
                    Err(e) => Response::error(e.0, 422),
                }
            }
            "/info-refs" | "/upload-pack" => {
                let c = host.catch_up(net, &[chain], &bs, &log).await.map_err(se)?;
                let proto = req.headers().get("git-protocol")?.unwrap_or_default();
                let enc = req.headers().get("content-encoding")?.unwrap_or_default();
                let body = if path == "/upload-pack" { Some(req.bytes().await?) } else { None };
                let r = host.upload_pack(net, path == "/info-refs", body, &proto, &enc).await.map_err(se)?;
                let h = Headers::new();
                for (k, v) in r.headers().entries() {
                    h.set(&k, &v)?;
                }
                h.set("x-graft-cursor", &c.cursor.at(chain).0.to_string())?;
                h.set("x-graft-rehydrated", if c.rehydrated.is_some() { "1" } else { "0" })?;
                Ok(r.with_headers(h))
            }
            "/catchup" => Response::from_json(&host.catch_up(net, &[chain], &bs, &log).await.map_err(se)?),
            "/fsck" => Response::from_json(&host.fsck(net).await.map_err(se)?),
            "/status" => {
                let running = self.state.container().is_some_and(|c| c.running());
                Response::from_json(&json!({ "running": running, "alarm": self.state.storage().get_alarm().await? }))
            }
            "/kill" => {
                host.destroy().await.map_err(se)?;
                Response::from_json(&json!({ "killed": true }))
            }
            "/teardown" => {
                host.destroy().await.map_err(se)?;
                self.state.storage().delete_alarm().await?;
                Response::from_json(&json!({ "torn_down": true }))
            }
            _ => Response::error("not found", 404),
        }
    }

    async fn alarm(&self) -> Result<Response> {
        self.host().on_alarm(now()).await.map_err(se)?;
        Response::empty()
    }
}

/// No public route: the edge reaches these classes through bindings.
#[event(fetch)]
async fn fetch(_req: Request, _env: Env, _ctx: Context) -> Result<Response> {
    Response::error("not found", 404)
}
