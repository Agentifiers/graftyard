//! BF-19 bed, `graft-m1-wp4`: the pack store and the sweeper from `crates/git` against three real R2 buckets
//! (`graft-m1-wp4-a` enam, `-b` weur, `-c` apac), until the site script hosts them.
//!
//! - `Requests` is the stub `request` table: one object holding (RequestId -> index, packs) rows in SQLite. The real
//!   table lives on the chain's Raft group, which no site script hosts yet.
//! - `Bed` drives the row from its alarm, in steps, so no step depends on the caller's connection:
//!   `seed` places 1,000 packs that fail (one bucket refused, so the two-of-three rule fails and the placement's one
//!   copy stays), and 1,000 that place, commit (a row in `Requests`), and whose marker write is killed (the marker
//!   bucket refuses every marker put, as a leader that dies after `Acked` and before its marker puts). Packs are
//!   1 MiB + 1 byte, the smallest size that is stored. Then `sweep` runs the sweeper with `now` = the real time plus
//!   a skew, from the seed's first or last placement time or the real time, set by the caller. Time is shortened only
//!   there: R2's upload times and the `PackId` times are real.
//!
//! Routes (bearer `TOKEN`): `POST /start?run=R&n=1000`, `GET /status?run=R`,
//! `POST /sweep?run=R&base=first|last|now&skew_m=M` (the sweeper's clock is base + M minutes),
//! `GET /count?run=R`, `POST /clean?run=R`.

use std::collections::BTreeMap;

use graftyard_git::host::R2;
use graftyard_git::{
    Bucket, Copies, INLINE_MAX, Marker, ObjectMeta, PackId, PackIndex, PackRef, PackStore, Place, R2Key, RequestRow,
    RequestSource, StoreError, mark_committed, sweep,
};
use graftyard_types::{
    ChainId, DbId, Digest, Index, NetworkId, RepoId, RequestId, Site, SiteId, SiteLink, SiteTable, Timestamp, Ulid,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use worker::*;

const SITES: [(u8, &str, graftyard_types::Region); 3] = [
    (0, "BUCKET_A", graftyard_types::Region::Enam),
    (1, "BUCKET_B", graftyard_types::Region::Weur),
    (2, "BUCKET_C", graftyard_types::Region::Apac),
];

fn table() -> SiteTable {
    SiteTable(
        SITES
            .iter()
            .map(|(id, b, r)| Site {
                id: SiteId(*id),
                script: format!("graft-m1-wp4-{id}"),
                account: String::new(),
                region: *r,
                jurisdiction: None,
                bucket: (*b).to_owned(),
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
    getrandom(&mut b);
    b
}

fn getrandom(b: &mut [u8]) {
    let crypto: web_sys::Crypto = js_sys::Reflect::get(&js_sys::global(), &"crypto".into()).unwrap().into();
    crypto.get_random_values_with_u8_array(b).unwrap();
}

fn now() -> Timestamp {
    Timestamp(Date::now().as_millis())
}

/// One run's network: run "r7" is network 7000007.
fn network(run: &str) -> NetworkId {
    let n: i64 = run.trim_start_matches('r').parse().unwrap_or(0);
    NetworkId(DbId(7_000_000 + n))
}

fn chain(run: &str) -> ChainId {
    ChainId::Repo(RepoId(network(run).0))
}

// ---- the stub request table ----

#[durable_object(fetch)]
pub struct Requests {
    state: State,
}

#[derive(Serialize, Deserialize)]
struct RowJson {
    index: u64,
    packs: Vec<(PackId, Marker)>,
}

impl DurableObject for Requests {
    fn new(state: State, _env: Env) -> Self {
        state
            .storage()
            .sql()
            .exec("CREATE TABLE IF NOT EXISTS request (id TEXT PRIMARY KEY, row TEXT NOT NULL)", None)
            .unwrap();
        Requests { state }
    }

    async fn fetch(&self, mut req: Request) -> Result<Response> {
        let sql = self.state.storage().sql();
        let url = req.url()?;
        let id = url.query_pairs().find(|(k, _)| k == "id").map(|(_, v)| v.into_owned()).unwrap_or_default();
        match req.method() {
            Method::Put => {
                let rows: Vec<(String, RowJson)> = req.json().await?;
                for (id, row) in rows {
                    sql.exec(
                        "INSERT OR REPLACE INTO request (id, row) VALUES (?, ?)",
                        vec![id.into(), serde_json::to_string(&row)?.into()],
                    )?;
                }
                Response::empty()
            }
            _ => {
                #[derive(Deserialize)]
                struct R {
                    row: String,
                }
                let rows: Vec<R> = sql.exec("SELECT row FROM request WHERE id = ?", vec![id.into()])?.to_array()?;
                match rows.first() {
                    Some(r) => Response::ok(r.row.clone()),
                    None => Response::error("no row", 404),
                }
            }
        }
    }
}

/// `RequestSource` over the stub, one object per chain as the real table is one per group.
struct StubRequests {
    stub: Stub,
}

impl RequestSource for StubRequests {
    async fn request(&self, _chain: ChainId, id: RequestId) -> std::result::Result<Option<RequestRow>, StoreError> {
        let e = |e: Error| StoreError(e.to_string());
        let mut r = self.stub.fetch_with_str(&format!("https://requests/?id={id}")).await.map_err(e)?;
        match r.status_code() {
            404 => Ok(None),
            200 => {
                let row: RowJson = r.json().await.map_err(e)?;
                Ok(Some(RequestRow { index: Index(row.index), packs: row.packs }))
            }
            c => Err(StoreError(format!("request table: {c}"))),
        }
    }
}

// ---- a bucket wrapper that kills writes ----

/// Fails every put whose key starts with `deny`: a placement whose site bucket is down, or a leader that dies before
/// its marker puts.
struct Killed<'a> {
    inner: &'a R2,
    deny: Option<&'static str>,
}

impl Bucket for Killed<'_> {
    async fn put_new(
        &self,
        key: &str,
        body: &[u8],
        sha256: Digest,
        meta: &[(String, String)],
    ) -> std::result::Result<graftyard_git::PutNew, StoreError> {
        if self.deny.is_some_and(|d| key.starts_with(d)) {
            return Err(StoreError("killed".into()));
        }
        self.inner.put_new(key, body, sha256, meta).await
    }
    async fn head(&self, key: &str) -> std::result::Result<Option<graftyard_git::ObjectInfo>, StoreError> {
        self.inner.head(key).await
    }
    async fn get(&self, key: &str) -> std::result::Result<Option<Vec<u8>>, StoreError> {
        self.inner.get(key).await
    }
    async fn list(&self, prefix: &str) -> std::result::Result<Vec<graftyard_git::ObjectInfo>, StoreError> {
        self.inner.list(prefix).await
    }
    async fn delete(&self, keys: &[String]) -> std::result::Result<(), StoreError> {
        self.inner.delete(keys).await
    }
    async fn abort_upload(&self, key: &str, id: &str) -> std::result::Result<(), StoreError> {
        self.inner.abort_upload(key, id).await
    }
}

// ---- the bed ----

#[durable_object]
pub struct Bed {
    state: State,
    env: Env,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Progress {
    run: String,
    n: usize,
    failed_done: usize,
    committed_done: usize,
    /// Placements that did not behave as the row needs (a failed one that placed, a committed one that did not).
    surprises: Vec<String>,
    /// Pack ids, as text, of each class.
    failed: Vec<String>,
    committed: Vec<String>,
    /// Committed packs that only two buckets took.
    in_two: usize,
    /// Multipart uploads started and abandoned: (pack id, upload id).
    uploads: Vec<(String, String)>,
    sweeps: Vec<serde_json::Value>,
    /// R2 upload time bounds of the seed: the first placement's start and the last one's end.
    first_at: u64,
    last_at: u64,
    done: bool,
    error: Option<String>,
}

/// Per alarm, this many placements of each class: each is 3 R2 puts of 1 MiB, within one alarm's limits.
const BATCH: usize = 50;

impl Bed {
    async fn progress(&self) -> Result<Progress> {
        Ok(self.state.storage().get("p").await?.unwrap_or_default())
    }

    async fn save(&self, p: &Progress) -> Result<()> {
        self.state.storage().put("p", p).await
    }

    fn requests(&self, run: &str) -> Result<Stub> {
        self.env.durable_object("REQUESTS")?.get_by_name(&format!("requests#{run}"))
    }

    /// One step of the seed: up to BATCH failed placements and BATCH committed ones with killed markers.
    async fn step(&self, p: &mut Progress) -> Result<()> {
        if p.first_at == 0 {
            p.first_at = now().0;
        }
        let sites = table();
        let raw = buckets(&self.env)?;
        let net = network(&p.run);
        let chain = chain(&p.run);
        let mut body = vec![0u8; INLINE_MAX + 1];
        // Failed placements: buckets B and C refuse, so only A takes the pack and the rule fails.
        let fb: Vec<Killed> =
            raw.iter().map(|(s, b)| Killed { inner: b, deny: if s.0 == 0 { None } else { Some("pack/") } }).collect();
        let fbs: Vec<(SiteId, &Killed)> = raw.iter().zip(&fb).map(|((s, _), k)| (*s, k)).collect();
        let store = PackStore { sites: &sites, buckets: &fbs, copies: Copies::TwoRegions };
        let mut i = 0;
        while p.failed_done < p.n && i < BATCH {
            getrandom(&mut body[..64]);
            let id = PackId(Ulid::new(now(), rand10()));
            let rid = RequestId(Ulid::new(now(), rand10()));
            let ix = index_of(&body);
            let place = Place {
                network: net,
                id,
                meta: ObjectMeta { chain, request: rid },
                bytes: &body,
                index: &ix,
                idx: None,
                inline_max: INLINE_MAX,
            };
            match store.place(place).await {
                Err(_) => p.failed.push(id.to_string()),
                Ok(_) => p.surprises.push(format!("failed placement {id} placed")),
            }
            p.failed_done += 1;
            i += 1;
        }
        // Committed placements: all three buckets take the pack, the request row is written (the entry commits), and
        // the marker write is killed in every bucket.
        // Every third committed placement has bucket C refuse its pack, so two buckets hold it and the filler must
        // copy it to C from the markers.
        let two: Vec<Killed> =
            raw.iter().map(|(s, b)| Killed { inner: b, deny: if s.0 == 2 { Some("pack/") } else { None } }).collect();
        let twos: Vec<(SiteId, &Killed)> = raw.iter().zip(&two).map(|((s, _), k)| (*s, k)).collect();
        let all: Vec<Killed> = raw.iter().map(|(_, b)| Killed { inner: b, deny: None }).collect();
        let alls: Vec<(SiteId, &Killed)> = raw.iter().zip(&all).map(|((s, _), k)| (*s, k)).collect();
        let mut rows = Vec::new();
        let mut j = 0;
        while p.committed_done < p.n && j < BATCH {
            let in_two = p.committed_done.is_multiple_of(3);
            let store =
                PackStore { sites: &sites, buckets: if in_two { &twos } else { &alls }, copies: Copies::TwoRegions };
            getrandom(&mut body[..64]);
            let id = PackId(Ulid::new(now(), rand10()));
            let rid = RequestId(Ulid::new(now(), rand10()));
            let ix = index_of(&body);
            let place = Place {
                network: net,
                id,
                meta: ObjectMeta { chain, request: rid },
                bytes: &body,
                index: &ix,
                idx: None,
                inline_max: INLINE_MAX,
            };
            match store.place(place).await {
                Ok(d) => {
                    let index = Index(1 + p.committed_done as u64);
                    let PackRef::Stored { checksum, bytes, .. } = d.pack().clone() else { unreachable!() };
                    rows.push((
                        rid.to_string(),
                        RowJson { index: index.0, packs: vec![(id, Marker { chain, index, checksum, bytes })] },
                    ));
                    // The leader dies after Acked: every marker put fails.
                    let killed: Vec<Killed> =
                        raw.iter().map(|(_, b)| Killed { inner: b, deny: Some("committed/") }).collect();
                    let kb: Vec<(SiteId, &Killed)> = raw.iter().zip(&killed).map(|((s, _), k)| (*s, k)).collect();
                    let failed = mark_committed(&kb, net, d.pack(), chain, index).await;
                    if failed == 0 {
                        p.surprises.push(format!("marker for {id} was not killed"));
                    }
                    p.committed.push(id.to_string());
                    if in_two {
                        p.in_two += 1;
                    }
                }
                Err(e) => p.surprises.push(format!("committed placement {id} failed: {e}")),
            }
            p.committed_done += 1;
            j += 1;
        }
        p.last_at = now().0;
        if !rows.is_empty() {
            let mut init = RequestInit::new();
            init.with_method(Method::Put).with_body(Some(serde_json::to_string(&rows)?.into()));
            let r =
                self.requests(&p.run)?.fetch_with_request(Request::new_with_init("https://requests/", &init)?).await?;
            if r.status_code() != 204 && r.status_code() != 200 {
                return Err(Error::RustError(format!("request table put: {}", r.status_code())));
            }
        }
        Ok(())
    }

    /// Five multipart uploads started on bucket A and abandoned after one part, as an edge that dies mid-push.
    async fn orphan_uploads(&self, p: &mut Progress) -> Result<()> {
        let a = self.env.bucket("BUCKET_A")?;
        for _ in 0..5 {
            let id = PackId(Ulid::new(now(), rand10()));
            let meta = ObjectMeta { chain: chain(&p.run), request: RequestId(Ulid::new(now(), rand10())) }.to_pairs();
            let u = graftyard_git::host::start_upload(&a, network(&p.run), id, &meta)
                .await
                .map_err(|e| Error::RustError(e.0))?;
            u.upload_part(1, vec![1u8; 5 << 20]).await?;
            p.uploads.push((id.to_string(), u.upload_id().await));
        }
        Ok(())
    }

    /// The sweeper's clock is `base + skew`: `first` is the seed's first placement, `last` the end of its last, and
    /// `now` the real time. Nothing else is faked: upload times are R2's own.
    async fn sweep_once(&self, p: &Progress, base: &str, skew_m: u64) -> Result<serde_json::Value> {
        let run = p.run.as_str();
        let base_ms = match base {
            "first" => p.first_at,
            "last" => p.last_at,
            _ => now().0,
        };
        let raw = buckets(&self.env)?;
        let refs: Vec<(SiteId, &R2)> = raw.iter().map(|(s, b)| (*s, b)).collect();
        let src = StubRequests { stub: self.requests(run)? };
        let at = Timestamp(base_ms + skew_m * 60_000);
        let t = Date::now().as_millis();
        let r = sweep(network(run), &refs, &src, at, usize::MAX).await.map_err(|e| Error::RustError(e.0))?;
        let mut kinds: BTreeMap<&str, usize> = BTreeMap::new();
        for a in &r.plan.actions {
            *kinds
                .entry(match a {
                    graftyard_git::Action::PutMarker { .. } => "put_marker",
                    graftyard_git::Action::Fill { .. } => "fill",
                    graftyard_git::Action::Delete { .. } => "delete",
                    graftyard_git::Action::Abort { .. } => "abort",
                })
                .or_default() += 1;
        }
        let mut alarms: BTreeMap<String, usize> = BTreeMap::new();
        for a in &r.plan.alarms {
            *alarms.entry(format!("{a:?}").split([' ', '{']).next().unwrap().to_owned()).or_default() += 1;
        }
        Ok(serde_json::json!({
            "base": base, "skew_m": skew_m, "clock": at.0, "real": now().0, "ms": Date::now().as_millis() - t, "actions": kinds, "alarms": alarms,
            "young": r.plan.young, "done": r.done, "failed": r.failed.len(),
            "first_failures": r.failed.iter().take(3).map(|(a, e)| format!("{a:?}: {e}")).collect::<Vec<_>>(),
        }))
    }

    /// What is in the buckets for this run's packs, by class.
    async fn count(&self, p: &Progress) -> Result<serde_json::Value> {
        let net = network(&p.run);
        let mut out = serde_json::Map::new();
        for (s, b) in buckets(&self.env)? {
            let packs: std::collections::BTreeSet<String> = b
                .list(&R2Key::pack_prefix(net))
                .await
                .map_err(|e| Error::RustError(e.0))?
                .into_iter()
                .map(|o| o.key)
                .collect();
            let marks: std::collections::BTreeSet<String> = b
                .list(&R2Key::marker_prefix(net))
                .await
                .map_err(|e| Error::RustError(e.0))?
                .into_iter()
                .map(|o| o.key)
                .collect();
            let uploads = b.list(&R2Key::upload_prefix(net)).await.map_err(|e| Error::RustError(e.0))?.len();
            let has = |ids: &[String], set: &std::collections::BTreeSet<String>, marker: bool| {
                ids.iter()
                    .filter(|i| {
                        let id: PackId = i.parse().unwrap();
                        let k = R2Key::Pack { network: net, id };
                        let k = if marker { R2Key::committed(k).unwrap() } else { k };
                        set.contains(&k.to_string())
                    })
                    .count()
            };
            out.insert(
                format!("site{}", s.0),
                serde_json::json!({
                    "failed_packs": has(&p.failed, &packs, false),
                    "committed_packs": has(&p.committed, &packs, false),
                    "committed_markers": has(&p.committed, &marks, true),
                    "failed_markers": has(&p.failed, &marks, true),
                    "upload_journals": uploads,
                    "all_pack_keys": packs.len(),
                    "all_marker_keys": marks.len(),
                    "unmarked_pack_keys": packs.iter().filter(|k| {
                        let Ok(R2Key::Pack { network, id }) = k.parse::<R2Key>() else { return false };
                        !marks.contains(&R2Key::committed(R2Key::Pack { network, id }).unwrap().to_string())
                    }).count(),
                }),
            );
        }
        // An upload is still open if a part can be added to it; an aborted one answers NoSuchUpload.
        let a = self.env.bucket("BUCKET_A")?;
        let mut open = 0;
        for (id, upload) in &p.uploads {
            let key = R2Key::Pack { network: net, id: id.parse().unwrap() }.to_string();
            if a.resume_multipart_upload(key, upload)?.upload_part(2, vec![0u8; 1]).await.is_ok() {
                open += 1;
            }
        }
        out.insert("open_uploads".into(), serde_json::json!({ "open": open, "of": p.uploads.len() }));
        Ok(serde_json::Value::Object(out))
    }

    async fn clean(&self, run: &str) -> Result<usize> {
        let net = network(run);
        let mut n = 0;
        for (_, b) in buckets(&self.env)? {
            for prefix in [R2Key::pack_prefix(net), R2Key::marker_prefix(net), R2Key::upload_prefix(net)] {
                let keys: Vec<String> =
                    b.list(&prefix).await.map_err(|e| Error::RustError(e.0))?.into_iter().map(|o| o.key).collect();
                n += keys.len();
                for c in keys.chunks(1000) {
                    b.delete(c).await.map_err(|e| Error::RustError(e.0))?;
                }
            }
        }
        Ok(n)
    }
}

fn index_of(b: &[u8]) -> PackIndex {
    PackIndex { checksum: Digest(Sha256::digest(b).into()), bytes: b.len() as u64, objects: 1, trailer: "".into() }
}

impl DurableObject for Bed {
    fn new(state: State, env: Env) -> Self {
        Bed { state, env }
    }

    async fn fetch(&self, mut req: Request) -> Result<Response> {
        let url = req.url()?;
        let q: BTreeMap<String, String> = url.query_pairs().into_owned().collect();
        let run = q.get("run").cloned().unwrap_or_default();
        match url.path() {
            "/start" => {
                let n = q.get("n").and_then(|v| v.parse().ok()).unwrap_or(1000);
                let mut p = Progress { run: run.clone(), n, ..Default::default() };
                self.orphan_uploads(&mut p).await?;
                self.save(&p).await?;
                self.state.storage().set_alarm(std::time::Duration::from_millis(10)).await?;
                Response::from_json(&serde_json::json!({ "started": run, "n": n, "uploads": p.uploads.len() }))
            }
            "/status" => {
                let p = self.progress().await?;
                Response::from_json(&serde_json::json!({
                    "run": p.run, "n": p.n, "failed_done": p.failed_done, "committed_done": p.committed_done,
                    "failed": p.failed.len(), "committed": p.committed.len(), "in_two": p.in_two, "surprises": p.surprises,
                    "sweeps": p.sweeps, "done": p.done, "error": p.error,
                }))
            }
            "/sweep" => {
                let skew = q.get("skew_m").and_then(|v| v.parse().ok()).unwrap_or(0);
                let base = q.get("base").cloned().unwrap_or_else(|| "now".into());
                let mut p = self.progress().await?;
                let s = self.sweep_once(&p, &base, skew).await?;
                p.sweeps.push(s.clone());
                self.save(&p).await?;
                Response::from_json(&s)
            }
            "/count" => {
                let p = self.progress().await?;
                Response::from_json(&self.count(&p).await?)
            }
            "/clean" => Response::from_json(&serde_json::json!({ "deleted": self.clean(&run).await? })),
            _ => {
                let _ = req.text().await;
                Response::error("not found", 404)
            }
        }
    }

    async fn alarm(&self) -> Result<Response> {
        let mut p = self.progress().await?;
        if p.done {
            return Response::empty();
        }
        if let Err(e) = self.step(&mut p).await {
            p.error = Some(e.to_string());
        }
        p.done = p.failed_done >= p.n && p.committed_done >= p.n;
        self.save(&p).await?;
        if !p.done {
            self.state.storage().set_alarm(std::time::Duration::from_millis(10)).await?;
        }
        Response::empty()
    }
}

#[event(fetch)]
async fn fetch(req: Request, env: Env, _ctx: Context) -> Result<Response> {
    let token = env.secret("TOKEN")?.to_string();
    if req.headers().get("authorization")?.as_deref() != Some(&format!("Bearer {token}")) {
        return Response::error("unauthorized", 401);
    }
    let run = req.url()?.query_pairs().find(|(k, _)| k == "run").map(|(_, v)| v.into_owned()).unwrap_or_default();
    env.durable_object("BED")?
        .get_by_name_with_location_hint(&format!("bed#{run}"), "enam")?
        .fetch_with_request(req)
        .await
}
