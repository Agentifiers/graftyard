//! The puller's git turn (`graftyard_lifeboat::git`; `decisions/DECISIONS.md`, "`lifeboat-clone`"): each network
//! whose copy moved is caught up into `graft-git` on `GIT_URL` with the shards' own `catch_up`, after the pass's
//! packs, at most `GIT_PIECES_PER_PASS` pieces a pass round the networks.
//!
//! - `Graft` is the `ShardIo`: `graft-git`'s routes over HTTP, and each stored pack streamed from `graft-blob` into
//!   its `/spool`, which checks the SHA-256 again as it writes. It lists no bucket: no markers but the copy's rolls
//!   (`roll_markers`), no stored `.idx`, and no eviction.
//! - `Store` is the `Saved`: the cursor and a rehydrate's place in the puller's own storage, written as `catch_up`
//!   saves them, so a puller reset mid-run resumes where it stopped.
//! - `Copy` is the `Learner`: the network's chain from its member copy, `/entries?local=1` and `/read`.
//! - `ready` answers the front's `GET /git?net=`: whether the network serves, from the facts as they are now.

use std::collections::{BTreeMap, BTreeSet};

use graftyard_git::catchup::{Fetched, IdxRange, Ingested, Rehydrating, Saved, ShardIo};
use graftyard_git::group::RefsPage;
use graftyard_git::pack::{Compacted, Marker, PackId};
use graftyard_git::{IngestCursor, LogPacks, StoreError};
use graftyard_lifeboat::git::{self as lgit, Facts, GIT_PIECES_PER_PASS, Learner, LearnerLog, Ready};
use graftyard_lifeboat::pass::State;
use graftyard_types::{ChainId, Digest, Index, NetworkId};
use serde::Deserialize;
use serde_json::{Value, json};
use worker::*;

fn se(e: impl std::fmt::Display) -> StoreError {
    StoreError(e.to_string())
}

async fn call(url: &str, method: Method, body: Option<Vec<u8>>) -> std::result::Result<Response, StoreError> {
    let mut init = RequestInit::new();
    init.with_method(method);
    if let Some(b) = body {
        init.with_body(Some(js_sys::Uint8Array::from(b.as_slice()).into()));
    }
    Fetch::Request(Request::new_with_init(url, &init).map_err(se)?).send().await.map_err(se)
}

/// `graft-git` for one network, and `graft-blob` for its stored packs.
pub struct Graft<'a> {
    pub git: &'a str,
    pub blob: &'a str,
    pub network: NetworkId,
    pub rolls: &'a [Compacted],
}

impl Graft<'_> {
    fn url(&self, path: &str, q: &str) -> String {
        format!("{}{path}?net={}{q}", self.git, self.network.0.0)
    }
    async fn ok(
        &self,
        method: Method,
        path: &str,
        q: &str,
        body: Option<Vec<u8>>,
    ) -> std::result::Result<Response, StoreError> {
        let mut r = call(&self.url(path, q), method, body).await?;
        if r.status_code() >= 300 {
            let t = r.text().await.unwrap_or_default();
            return Err(StoreError(format!("graft-git {path}: {} {t}", r.status_code())));
        }
        Ok(r)
    }
}

/// `graft-git`'s `/health`: its disk and its run.
pub async fn health(git: &str) -> Option<(String, String)> {
    let mut r = call(&format!("{git}/health"), Method::Get, None).await.ok()?;
    let v: Value = r.json().await.ok()?;
    Some((v["disk"].as_str()?.to_string(), v["boot"].as_str()?.to_string()))
}

fn answers(v: &[String]) -> std::result::Result<Vec<Ingested>, StoreError> {
    v.iter()
        .map(|a| match a.as_str() {
            "done" => Ok(Ingested::Done),
            "later" => Ok(Ingested::Later),
            a => Err(StoreError(format!("graft-git answered {a:?} for a pack"))),
        })
        .collect()
}

impl ShardIo for Graft<'_> {
    async fn up(&self) -> std::result::Result<String, StoreError> {
        health(self.git).await.map(|(d, _)| d).ok_or_else(|| StoreError("graft-git does not answer".into()))
    }
    async fn init(&self, fresh: bool) -> std::result::Result<(), StoreError> {
        self.ok(Method::Post, "/init", &format!("&fresh={}", u8::from(fresh)), None).await.map(|_| ())
    }
    async fn have(&self, sums: &[Digest]) -> std::result::Result<BTreeSet<Digest>, StoreError> {
        #[derive(Deserialize)]
        struct Have {
            have: Vec<String>,
        }
        let body: String = sums.iter().map(|s| format!("{}\n", s.hex())).collect();
        let h: Have = self.ok(Method::Post, "/have", "", Some(body.into_bytes())).await?.json().await.map_err(se)?;
        Ok(h.have.iter().filter_map(|s| Digest::parse_hex(s)).collect())
    }
    async fn ingest(&self, packs: &[(Digest, &[u8])]) -> std::result::Result<Vec<Ingested>, StoreError> {
        #[derive(Deserialize)]
        struct Out {
            results: Vec<String>,
        }
        let mut body = Vec::new();
        for (s, b) in packs {
            body.extend_from_slice(format!("{} {}\n", s.hex(), b.len()).as_bytes());
            body.extend_from_slice(b);
        }
        let o: Out = self.ok(Method::Post, "/ingest-batch", "", Some(body)).await?.json().await.map_err(se)?;
        answers(&o.results)
    }
    /// The pack from `graft-blob`, which reads it only if it hashes to `checksum`, streamed into `/spool`, which checks
    /// it again as it writes. A pack `graft-blob` does not hold yet stops the run: the pass still owes it.
    async fn fetch_stored(&self, id: PackId, checksum: Digest, _idx: bool) -> std::result::Result<Fetched, StoreError> {
        let key = format!("pack/{}/{id}", self.network.0.0);
        let got = call(&format!("{}/{key}?sha256={}", self.blob, checksum.hex()), Method::Get, None).await?;
        if got.status_code() != 200 {
            return Err(StoreError(format!("graft-blob {key}: {} (the pack is not held yet)", got.status_code())));
        }
        let ResponseBody::Stream(st) = got.body().clone() else {
            return Err(StoreError(format!("graft-blob {key}: no body")));
        };
        let mut init = RequestInit::new();
        init.with_method(Method::Post).with_body(Some(st.into()));
        let req =
            Request::new_with_init(&self.url("/spool", &format!("&sha256={}", checksum.hex())), &init).map_err(se)?;
        let mut r = Fetch::Request(req).send().await.map_err(se)?;
        if r.status_code() != 200 {
            return Err(StoreError(format!(
                "graft-git /spool {key}: {} {}",
                r.status_code(),
                r.text().await.unwrap_or_default()
            )));
        }
        let v: Value = r.json().await.map_err(se)?;
        Ok(if v["have"].as_bool() == Some(true) { Fetched::Have } else { Fetched::Spooled })
    }
    async fn ingest_spooled(&self, checksum: Digest) -> std::result::Result<Ingested, StoreError> {
        let mut r =
            call(&self.url("/ingest", &format!("&sha256={}&spooled=1", checksum.hex())), Method::Post, None).await?;
        match r.status_code() {
            200 => Ok(Ingested::Done),
            409 => Ok(Ingested::Later),
            c => Err(StoreError(format!("graft-git /ingest: {c} {}", r.text().await.unwrap_or_default()))),
        }
    }
    async fn ingest_spooled_batch(&self, checksums: &[Digest]) -> std::result::Result<Vec<Ingested>, StoreError> {
        #[derive(Deserialize)]
        struct Out {
            results: Vec<String>,
        }
        let body: String = checksums.iter().map(|s| format!("{}\n", s.hex())).collect();
        let o: Out = self
            .ok(Method::Post, "/ingest-spooled-batch", "", Some(body.into_bytes()))
            .await?
            .json()
            .await
            .map_err(se)?;
        answers(&o.results)
    }
    async fn idx_ranges(&self) -> std::result::Result<BTreeMap<PackId, Option<IdxRange>>, StoreError> {
        Ok(BTreeMap::new())
    }
    async fn connected(&self, repack: bool) -> std::result::Result<std::result::Result<(), String>, StoreError> {
        let v: Value = self
            .ok(Method::Post, "/connected", &format!("&objects=tips&repack={}", u8::from(repack)), None)
            .await?
            .json()
            .await
            .map_err(se)?;
        Ok(if v["connected"].as_bool() == Some(true) {
            Ok(())
        } else {
            Err(v["error"].as_str().unwrap_or("not connected").to_string())
        })
    }
    async fn put_refs_page(
        &self,
        refs: &[(String, String)],
        start: bool,
        done: bool,
        repack: bool,
        head: Option<&str>,
    ) -> std::result::Result<(), StoreError> {
        let body: String = refs.iter().map(|(r, o)| format!("{o} {r}\n")).collect();
        let q = format!(
            "&start={}&done={}&repack={}{}",
            u8::from(start),
            u8::from(done),
            u8::from(repack),
            graftyard_git::catchup::head_param(head)
        );
        self.ok(Method::Put, "/refs", &q, Some(body.into_bytes())).await.map(|_| ())
    }
    async fn apply_refs(
        &self,
        changes: &[(String, Option<String>)],
        head: Option<&str>,
        repack: bool,
    ) -> std::result::Result<(), StoreError> {
        if changes.is_empty() && head.is_none() {
            return Ok(());
        }
        let zero = "0".repeat(40);
        let body: String = changes.iter().map(|(r, o)| format!("{} {r}\n", o.as_deref().unwrap_or(&zero))).collect();
        let q = format!("{}{}", if repack { "" } else { "&repack=0" }, graftyard_git::catchup::head_param(head));
        self.ok(Method::Post, "/refs", &q, Some(body.into_bytes())).await.map(|_| ())
    }
    /// A lifeboat evicts nothing: `start.sh` refuses to start without room for every repository.
    async fn make_room(&self, _incoming: u64) -> std::result::Result<Vec<i64>, StoreError> {
        Ok(Vec::new())
    }
    async fn markers(&self) -> std::result::Result<Vec<(PackId, Marker)>, StoreError> {
        Ok(lgit::roll_markers(lgit::chain_of(self.network), self.rolls))
    }
}

/// The cursor and a rehydrate's place for one network, in the puller's storage.
pub struct Store<'a> {
    pub storage: &'a worker::Storage,
    pub network: NetworkId,
}

pub fn cursor_key(n: NetworkId) -> String {
    format!("git-cursor:{}", n.0.0)
}
pub fn rehydrating_key(n: NetworkId) -> String {
    format!("git-rehydrating:{}", n.0.0)
}

impl Saved for Store<'_> {
    async fn cursor(&self) -> std::result::Result<Option<(String, IngestCursor)>, StoreError> {
        self.storage.get(&cursor_key(self.network)).await.map_err(se)
    }
    async fn put_cursor(&self, disk: &str, c: &IngestCursor) -> std::result::Result<(), StoreError> {
        self.storage.put(&cursor_key(self.network), (disk, c)).await.map_err(se)
    }
    async fn drop_cursor(&self) -> std::result::Result<(), StoreError> {
        self.storage.delete(&cursor_key(self.network)).await.map(|_| ()).map_err(se)
    }
    async fn rehydrating(&self) -> std::result::Result<Option<(String, Rehydrating)>, StoreError> {
        Ok(self.storage.get(&rehydrating_key(self.network)).await.ok().flatten())
    }
    async fn put_rehydrating(&self, disk: &str, r: &Rehydrating) -> std::result::Result<(), StoreError> {
        self.storage.put(&rehydrating_key(self.network), (disk, r)).await.map_err(se)
    }
    async fn drop_rehydrating(&self) -> std::result::Result<(), StoreError> {
        self.storage.delete(&rehydrating_key(self.network)).await.map(|_| ()).map_err(se)
    }
}

/// The network's chain from its member copy.
pub struct Copy<'a> {
    pub env: &'a Env,
}

impl Copy<'_> {
    async fn post(&self, chain: ChainId, path: &str, body: Value) -> std::result::Result<Value, StoreError> {
        let stub = self
            .env
            .durable_object("MEMBER")
            .and_then(|n| n.get_by_name(&graftyard_lookup::lifeboat_object(chain)))
            .map_err(se)?;
        let mut init = RequestInit::new();
        init.with_method(Method::Post).with_body(Some(body.to_string().into()));
        let mut r = stub
            .fetch_with_request(Request::new_with_init(&format!("https://member{path}"), &init).map_err(se)?)
            .await
            .map_err(se)?;
        if r.status_code() != 200 {
            return Err(StoreError(format!(
                "{chain} {path}: {} {}",
                r.status_code(),
                r.text().await.unwrap_or_default()
            )));
        }
        r.json().await.map_err(se)
    }
}

impl Learner for Copy<'_> {
    async fn entries(&self, chain: ChainId, from: Index, to: Index) -> std::result::Result<Vec<LogPacks>, StoreError> {
        let path = format!(
            "/entries?from={}&to={}&local=1&bytes={}",
            from.0,
            to.0,
            graftyard_git::catchup::INGEST_BATCH_BYTES
        );
        lgit::entries_page(&self.post(chain, &path, json!({})).await?)
    }
    async fn refs(&self, chain: ChainId, after: Option<String>) -> std::result::Result<(RefsPage, Index), StoreError> {
        let q = graftyard_git::group::refs_query(b"", after.as_deref(), graftyard_git::repo::REFS_PAGE_MAX);
        let v = self.post(chain, "/read", q).await?;
        let page: RefsPage = serde_json::from_value(v["value"].clone()).map_err(se)?;
        Ok((page, Index(v["index"].as_u64().unwrap_or(0))))
    }
    async fn ranges(&self, chain: ChainId) -> std::result::Result<(Index, Option<String>, Vec<Compacted>), StoreError> {
        let v = self.post(chain, "/read", json!({ "q": { "ranges": true } })).await?;
        let cs: Vec<Compacted> =
            serde_json::from_value(v["value"].get("compactions").cloned().unwrap_or(Value::Array(Vec::new())))
                .map_err(se)?;
        Ok((Index(v["index"].as_u64().unwrap_or(0)), v["value"]["default_branch"].as_str().map(str::to_owned), cs))
    }
}

/// The networks the copy holds, each at its repository chain's index.
fn held(st: &State) -> BTreeMap<NetworkId, Index> {
    st.puller
        .held
        .0
        .iter()
        .filter_map(|(c, i)| match c {
            ChainId::Repo(r) => Some((NetworkId(r.0), *i)),
            _ => None,
        })
        .collect()
}

/// Networks that wait: a pack still owed, a halted or gone copy.
fn waiting(st: &State) -> BTreeSet<NetworkId> {
    let mut w: BTreeSet<NetworkId> = st.owed.iter().map(|o| o.network).collect();
    for c in st.puller.halted.iter().chain(&st.puller.gone) {
        if let ChainId::Repo(r) = c {
            w.insert(NetworkId(r.0));
        }
    }
    w
}

/// One git turn: up to `GIT_PIECES_PER_PASS` `catch_up` pieces round the due networks. The line for the pass log.
pub async fn turn(env: &Env, storage: &worker::Storage, git: &str, blob: &str, st: &mut State, now: u64) -> Value {
    let Some((_, boot)) = health(git).await else { return json!({ "down": true }) };
    let due = st.git.due(&held(st), &waiting(st), &boot, now);
    let copy = Copy { env };
    let mut out = Vec::new();
    for net in due.into_iter().take(GIT_PIECES_PER_PASS) {
        st.git.turn = Some(net);
        let chain = lgit::chain_of(net);
        let entry = st.git.nets.entry(net).or_default();
        let r = async {
            let (_, _, rolls) = copy.ranges(chain).await?;
            let io = Graft { git, blob, network: net, rolls: &rolls };
            let saved = Store { storage, network: net };
            // A run the puller has not seen started without it: the repository is checked before it is trusted, and
            // a failed check drops the cursor, so the catch-up below rehydrates it. A check that errs, such as a 404
            // for a repository directory that is gone, fails it too: kept, that cursor would hold the network at 503.
            if entry.checked.as_deref() != Some(boot.as_str())
                && saved.cursor().await?.is_some()
                && let Err(why) = io.connected(false).await.unwrap_or_else(|e| Err(e.0))
            {
                saved.drop_cursor().await?;
                console_error!("lifeboat git {}: not connected after a restart, rebuilding: {why}", net.0.0);
            }
            let log = LearnerLog { learner: &copy };
            let mut run = graftyard_git::catchup::Run::default();
            graftyard_git::catchup::catch_up(
                &io,
                &saved,
                &log,
                net,
                &[chain],
                graftyard_git::catchup::PIECE,
                &mut run,
                &|| Date::now().as_millis(),
            )
            .await
        }
        .await;
        match r {
            Ok(rep) => {
                entry.checked = Some(boot.clone());
                entry.error = None;
                if !rep.behind {
                    entry.served = Some((rep.cursor.at(chain), now));
                }
                out.push(json!({ "net": net.0.0, "at": rep.cursor.at(chain).0, "behind": rep.behind,
                    "rehydrated": rep.rehydrated.is_some(), "packs": rep.packs }));
            }
            Err(e) => {
                (entry.error, entry.failed_at) = (Some(e.0.clone()), now);
                console_error!("lifeboat git {}: {}", net.0.0, e.0);
                out.push(json!({ "net": net.0.0, "error": e.0 }));
            }
        }
    }
    json!(out)
}

/// The front's `GET /git?net=N`.
pub async fn ready(storage: &worker::Storage, git: &str, st: &State, net: NetworkId, now: u64) -> Ready {
    let chain = lgit::chain_of(net);
    let cursor: Option<(String, Value)> = storage.get(&cursor_key(net)).await.ok().flatten();
    let rehydrating: Option<Value> = storage.get(&rehydrating_key(net)).await.ok().flatten();
    let f = Facts {
        health: health(git).await,
        cursor_disk: cursor.map(|(d, _)| d),
        rehydrating: rehydrating.is_some(),
        halted: st.puller.halted.contains(&chain),
    };
    lgit::ready(st.git.nets.get(&net), &f, st.puller.held.get(chain), now)
}
