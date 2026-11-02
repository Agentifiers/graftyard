//! The Workers host: `Bucket` over an R2 binding, and the git cache shard object's side of its container
//! (`GitCacheShard::validate`, `upload_pack`, `rehydrate`, and catch-up behind the `IngestCursor`). A site script's shard
//! class holds a `ShardHost` and calls these; the throwaway M1 scripts under `tests/m1/` do the same today.

use std::collections::{BTreeMap, BTreeSet};

use graftyard_types::{ChainId, Digest, NetworkId, Region, Timestamp};
use serde::Deserialize;
use worker::{Conditional, Container, Fetcher, Headers, Include, Method, Request, RequestInit, Response, State};

use crate::bucket::{Bucket, ObjectInfo, PutNew, StoreError};
use crate::ingest::{ChainLog, Load, LogPacks, rehydrate_plan, tail_start};
use crate::key::R2Key;
use crate::pack::{Marker, PackId, PackIndex};
use crate::shard::{DiskRepo, IngestCursor, RegionCheck, Renewal, evict_plan, region_check, renewal};
use crate::store::marker_from_meta;

fn err(e: impl std::fmt::Display) -> StoreError {
    StoreError(e.to_string())
}

pub struct R2(pub worker::Bucket);

fn info(o: &worker::Object) -> ObjectInfo {
    let sha256 = o.checksum().sha256.and_then(|b| <[u8; 32]>::try_from(b.as_slice()).ok()).map(Digest);
    let mut meta: Vec<(String, String)> = o.custom_metadata().unwrap_or_default().into_iter().collect();
    meta.sort();
    ObjectInfo { key: o.key(), size: o.size(), uploaded: Timestamp(o.uploaded().as_millis()), sha256, meta }
}

impl Bucket for R2 {
    async fn put_new(
        &self,
        key: &str,
        body: &[u8],
        sha256: Digest,
        meta: &[(String, String)],
    ) -> Result<PutNew, StoreError> {
        let meta: std::collections::HashMap<String, String> = meta.iter().cloned().collect();
        let put = self
            .0
            .put(key, body.to_vec())
            .sha256(sha256.0.to_vec())
            .custom_metadata(meta)
            .only_if(Conditional { etag_does_not_match: Some("*".into()), ..Default::default() })
            .execute()
            .await
            .map_err(err)?;
        match put {
            Some(_) => Ok(PutNew::Created),
            None => Ok(PutNew::Exists(self.head(key).await?.and_then(|o| o.sha256))),
        }
    }

    /// Journal first, then the parts in order, then complete. The key is a fresh `PackId`, so the create-only check
    /// is a HEAD before the upload; R2's multipart API takes no conditional, and it cannot check a SHA-256 of the
    /// whole object, so a reader verifies a multipart copy by its marker's checksum.
    async fn put_new_parts(
        &self,
        key: &str,
        journal: &str,
        body: &[u8],
        sha256: Digest,
        meta: &[(String, String)],
        part: usize,
    ) -> Result<PutNew, StoreError> {
        if let Some(o) = self.head(key).await? {
            return Ok(PutNew::Exists(o.sha256));
        }
        let mut m: std::collections::HashMap<String, String> = meta.iter().cloned().collect();
        m.insert("sha256".into(), sha256.hex());
        let u = self.0.create_multipart_upload(key).custom_metadata(m).execute().await.map_err(err)?;
        let mut jm = std::collections::HashMap::new();
        jm.insert("upload".to_owned(), u.upload_id().await);
        self.0.put(journal, Vec::<u8>::new()).custom_metadata(jm).execute().await.map_err(err)?;
        let mut parts = Vec::new();
        for (i, c) in body.chunks(part).enumerate() {
            match u.upload_part(i as u16 + 1, c.to_vec()).await {
                Ok(p) => parts.push(p),
                Err(e) => {
                    let _ = u.abort().await;
                    return Err(err(e));
                }
            }
        }
        u.complete(parts).await.map_err(err)?;
        self.0.delete(journal).await.map_err(err)?;
        Ok(PutNew::Created)
    }

    async fn head(&self, key: &str) -> Result<Option<ObjectInfo>, StoreError> {
        Ok(self.0.head(key).await.map_err(err)?.as_ref().map(info))
    }

    async fn get(&self, key: &str) -> Result<Option<Vec<u8>>, StoreError> {
        let Some(o) = self.0.get(key).execute().await.map_err(err)? else { return Ok(None) };
        match o.body() {
            Some(b) => Ok(Some(b.bytes().await.map_err(err)?)),
            None => Ok(Some(Vec::new())),
        }
    }

    async fn list(&self, prefix: &str) -> Result<Vec<ObjectInfo>, StoreError> {
        let mut out = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let mut l = self.0.list().prefix(prefix).include(vec![Include::CustomMetadata]);
            if let Some(c) = cursor.take() {
                l = l.cursor(c);
            }
            let page = l.execute().await.map_err(err)?;
            out.extend(page.objects().iter().map(info));
            if !page.truncated() {
                return Ok(out);
            }
            cursor = page.cursor();
        }
    }

    async fn segments(&self, prefix: &str) -> Result<Vec<String>, StoreError> {
        let mut out = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let mut l = self.0.list().prefix(prefix).delimiter("/");
            if let Some(c) = cursor.take() {
                l = l.cursor(c);
            }
            let page = l.execute().await.map_err(err)?;
            out.extend(
                page.delimited_prefixes()
                    .into_iter()
                    .filter_map(|p| p.strip_prefix(prefix).map(|r| r.trim_end_matches('/').to_owned())),
            );
            if !page.truncated() {
                out.sort();
                out.dedup();
                return Ok(out);
            }
            cursor = page.cursor();
        }
    }

    async fn delete(&self, keys: &[String]) -> Result<(), StoreError> {
        if keys.is_empty() {
            return Ok(());
        }
        self.0.delete_multiple(keys.to_vec()).await.map_err(err)
    }

    async fn abort_upload(&self, key: &str, upload_id: &str) -> Result<(), StoreError> {
        let u = self.0.resume_multipart_upload(key, upload_id).map_err(err)?;
        match u.abort().await {
            Ok(()) => Ok(()),
            // NoSuchUpload (10024): already completed, aborted, or expired.
            Err(e) if e.to_string().contains("10024") || e.to_string().contains("NoSuchUpload") => Ok(()),
            Err(e) => Err(err(e)),
        }
    }
}

/// Starts a multipart upload of a pack and journals its upload id in the same bucket first, so the sweeper can abort
/// it if the uploader dies. The Workers binding cannot list multipart uploads.
pub async fn start_upload(
    b: &worker::Bucket,
    network: NetworkId,
    id: PackId,
    meta: &[(String, String)],
) -> Result<worker::MultipartUpload, StoreError> {
    let journal = R2Key::Upload { network, id }.to_string();
    let key = R2Key::Pack { network, id }.to_string();
    let u = b
        .create_multipart_upload(key)
        .custom_metadata(meta.iter().cloned().collect::<std::collections::HashMap<_, _>>())
        .execute()
        .await
        .map_err(err)?;
    let mut jm = std::collections::HashMap::new();
    jm.insert("upload".to_owned(), u.upload_id().await);
    b.put(journal, Vec::<u8>::new()).custom_metadata(jm).execute().await.map_err(err)?;
    Ok(u)
}

/// The pack for each load: inline bytes, or a stored pack read from the first bucket whose copy matches its checksum.
pub async fn fetch_load<B: Bucket>(
    buckets: &[&B],
    network: NetworkId,
    l: &Load,
) -> Result<(Digest, Vec<u8>), StoreError> {
    use sha2::{Digest as _, Sha256};
    match l {
        Load::Inline { bytes, .. } => Ok((Digest(Sha256::digest(bytes).into()), bytes.clone())),
        Load::Stored { id, checksum, .. } => {
            let key = R2Key::Pack { network, id: *id }.to_string();
            for b in buckets {
                if let Some(body) = b.get(&key).await?
                    && Digest(Sha256::digest(&body).into()) == *checksum
                {
                    return Ok((*checksum, body));
                }
            }
            Err(StoreError(format!("{key}: no bucket holds a copy matching its checksum")))
        }
    }
}

/// Every marker of a network in one bucket.
pub async fn markers<B: Bucket>(b: &B, network: NetworkId) -> Result<Vec<(PackId, Marker)>, StoreError> {
    let mut out = Vec::new();
    for o in b.list(&R2Key::marker_prefix(network)).await? {
        if let Ok(R2Key::Committed { of }) = o.key.parse::<R2Key>()
            && let R2Key::Pack { id, .. } = *of
            && let Some(m) = marker_from_meta(&o.meta)
        {
            out.push((id, m));
        }
    }
    Ok(out)
}

#[derive(Debug, Deserialize)]
pub struct Health {
    pub disk: String,
    pub git: String,
    pub region: String,
    pub location: String,
}

#[derive(Debug, Deserialize)]
struct ReposOut {
    disk: u64,
    repos: Vec<RepoOut>,
}

#[derive(Debug, Deserialize)]
struct RepoOut {
    network: String,
    bytes: u64,
    used: u64,
}

/// What `make_room` saw and did.
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct Room {
    /// The disk size the budget was taken from: the container's filesystem, or the caller's override.
    pub disk: u64,
    /// Bytes of repositories on disk before eviction, and the bytes about to be written.
    pub held: u64,
    pub incoming: u64,
    pub evicted: Vec<i64>,
    /// Chosen but in use (the container answered 409), so kept this time.
    pub busy: Vec<i64>,
}

#[derive(Debug, Deserialize)]
struct ValidateOut {
    checksum: String,
    bytes: u64,
    objects: u32,
    trailer: String,
}

/// The shard object's view of its container.
pub struct ShardHost<'a> {
    pub state: &'a State,
    pub region: Region,
    /// The image map's key this shard starts (`git` or `git_prev`).
    pub image: &'a str,
    pub instance: &'a str,
}

#[derive(Debug)]
pub struct Started {
    pub health: Health,
    pub replaced: u8,
    pub check: RegionCheck,
}

/// worker-rs reads an integer alarm time as an offset from now, not as epoch milliseconds.
fn alarm_at(now: Timestamp, at: Timestamp) -> std::time::Duration {
    std::time::Duration::from_millis(at.0.saturating_sub(now.0))
}

fn worker_err(e: worker::Error) -> StoreError {
    err(e)
}

impl ShardHost<'_> {
    fn container(&self) -> Result<Container, StoreError> {
        self.state.container().ok_or_else(|| StoreError("this object has no container".into()))
    }

    fn port(&self) -> Result<Fetcher, StoreError> {
        self.container()?.get_tcp_port(8080).map_err(worker_err)
    }

    async fn call(
        &self,
        method: Method,
        path: &str,
        body: Option<Vec<u8>>,
        headers: &[(&str, &str)],
    ) -> Result<Response, StoreError> {
        let mut init = RequestInit::new();
        init.with_method(method);
        let h = Headers::new();
        for (k, v) in headers {
            h.set(k, v).map_err(worker_err)?;
        }
        init.with_headers(h);
        if let Some(b) = body {
            init.with_body(Some(js_sys::Uint8Array::from(b.as_slice()).into()));
        }
        let req = Request::new_with_init(&format!("http://container{path}"), &init).map_err(worker_err)?;
        self.port()?.fetch_request(req).await.map_err(worker_err)
    }

    async fn ok(&self, method: Method, path: &str, body: Option<Vec<u8>>) -> Result<Response, StoreError> {
        let mut r = self.call(method, path, body, &[]).await?;
        if r.status_code() >= 300 {
            let t = r.text().await.unwrap_or_default();
            return Err(StoreError(format!("{path}: {} {t}", r.status_code())));
        }
        Ok(r)
    }

    pub async fn health(&self) -> Result<Health, StoreError> {
        let mut r = self.ok(Method::Get, "/health", None).await?;
        r.json().await.map_err(worker_err)
    }

    async fn wait_up(&self) -> Result<Health, StoreError> {
        let mut last = String::new();
        for _ in 0..300 {
            match self.health().await {
                Ok(h) => return Ok(h),
                Err(e) => last = e.0,
            }
            worker::Delay::from(std::time::Duration::from_millis(200)).await;
        }
        Err(StoreError(format!("container did not answer in 60 s: {last}")))
    }

    fn start_once(&self) -> Result<(), StoreError> {
        let c = self.container()?;
        if c.running() {
            return Ok(());
        }
        // worker-rs 0.8.7's options carry no image or instance, so the object is built here.
        let o = js_sys::Object::new();
        let set = |k: &str, v: wasm_bindgen::JsValue| js_sys::Reflect::set(&o, &k.into(), &v).map(|_| ());
        let images = js_sys::Reflect::get(c.as_ref(), &"images".into()).map_err(|_| StoreError("images".into()))?;
        let image = js_sys::Reflect::get(&images, &self.image.into()).map_err(|_| StoreError("image".into()))?;
        if image.is_undefined() {
            return Err(StoreError(format!("no image {:?} in this version's image map", self.image)));
        }
        set("image", image)
            .and(set("instance", self.instance.into()))
            .and(set("enableInternet", false.into()))
            .map_err(|_| StoreError("start options".into()))?;
        let start = js_sys::Reflect::get(c.as_ref(), &"start".into()).map_err(|_| StoreError("start".into()))?;
        js_sys::Function::from(start).call1(c.as_ref(), &o).map_err(|e| StoreError(format!("start: {e:?}")))?;
        Ok(())
    }

    /// Sets the container's inactivity timeout (the API's maximum, 6 h).
    pub async fn set_inactivity(&self, ms: u64) -> Result<(), StoreError> {
        let c = self.container()?;
        let f = js_sys::Reflect::get(c.as_ref(), &"setInactivityTimeout".into())
            .map_err(|_| StoreError("timeout".into()))?;
        let p = js_sys::Function::from(f)
            .call1(c.as_ref(), &(ms as f64).into())
            .map_err(|e| StoreError(format!("{e:?}")))?;
        wasm_bindgen_futures_await(p).await
    }

    /// Starts the container if it is not running, waits for its server, and applies the start-time region check:
    /// a container outside the site's container regions is replaced at most `REGION_RETRIES` times, then kept with
    /// an alarm (`check` is `Alarm`).
    pub async fn up(&self) -> Result<Started, StoreError> {
        let was_running = self.container()?.running();
        let mut replaced = 0;
        loop {
            self.start_once()?;
            let health = self.wait_up().await?;
            let check = if was_running {
                RegionCheck::Keep
            } else {
                region_check(self.region, Some(&health.region).filter(|r| !r.is_empty()).map(|s| s.as_str()), replaced)
            };
            match check {
                RegionCheck::Replace => {
                    self.container()?
                        .destroy(Some("outside the site's container regions"))
                        .await
                        .map_err(worker_err)?;
                    replaced += 1;
                }
                _ => {
                    self.set_inactivity(crate::shard::INACTIVITY.as_millis() as u64).await?;
                    return Ok(Started { health, replaced, check });
                }
            }
        }
    }

    pub async fn destroy(&self) -> Result<(), StoreError> {
        let c = self.container()?;
        if c.running() {
            c.destroy(None).await.map_err(worker_err)?;
        }
        Ok(())
    }

    /// Records a request and arms the renewal alarm (`renewal`). The shard's `alarm` handler calls `on_alarm`.
    pub async fn touch(&self, now: Timestamp) -> Result<(), StoreError> {
        let s = self.state.storage();
        s.put("last_request", now.0).await.map_err(worker_err)?;
        if let Renewal::Renew { next } = renewal(now, now) {
            s.set_alarm(alarm_at(now, next)).await.map_err(worker_err)?;
        }
        Ok(())
    }

    pub async fn on_alarm(&self, now: Timestamp) -> Result<Renewal, StoreError> {
        let s = self.state.storage();
        let last: u64 = s.get("last_request").await.map_err(worker_err)?.unwrap_or(0);
        let r = renewal(now, Timestamp(last));
        if let Renewal::Renew { next } = r {
            if self.container()?.running() {
                self.set_inactivity(crate::shard::INACTIVITY.as_millis() as u64).await?;
            }
            s.set_alarm(alarm_at(now, next)).await.map_err(worker_err)?;
        }
        Ok(r)
    }

    /// `GitCacheShard::validate`: `index-pack --strict --fix-thin` into a quarantine and connectivity of `tips`
    /// against the refs the leader just gave, which the container holds once `set_refs` ran. The tips ride in the
    /// body ahead of the pack, because a 10,000-ref push has too many for a URL.
    pub async fn validate(&self, network: NetworkId, pack: Vec<u8>, tips: &[String]) -> Result<PackIndex, StoreError> {
        let mut head: Vec<u8> = tips.join("\n").into_bytes();
        head.push(b'\n');
        let path = format!("/validate?net={}&tipsn={}", network.0.0, head.len());
        head.extend_from_slice(&pack);
        let mut r = self.call(Method::Post, &path, Some(head), &[]).await?;
        if r.status_code() >= 300 {
            let t = r.text().await.unwrap_or_default();
            return Err(StoreError(format!("validate: {} {t}", r.status_code())));
        }
        let v: ValidateOut = r.json().await.map_err(worker_err)?;
        Ok(PackIndex {
            checksum: Digest::parse_hex(&v.checksum).ok_or_else(|| StoreError("checksum".into()))?,
            bytes: v.bytes,
            objects: v.objects,
            trailer: v.trailer.into(),
        })
    }

    pub async fn init(&self, network: NetworkId, fresh: bool) -> Result<(), StoreError> {
        self.ok(Method::Post, &format!("/init?net={}&fresh={}", network.0.0, u8::from(fresh)), None).await.map(|_| ())
    }

    /// The whole ref set of the network's member chains, as their leaders gave it.
    pub async fn set_refs(&self, network: NetworkId, refs: &[(String, String)]) -> Result<(), StoreError> {
        let body: String = refs.iter().map(|(r, o)| format!("{o} {r}\n")).collect();
        self.ok(Method::Put, &format!("/refs?net={}", network.0.0), Some(body.into_bytes())).await.map(|_| ())
    }

    pub async fn fsck(&self, network: NetworkId) -> Result<serde_json::Value, StoreError> {
        let mut r = self.ok(Method::Post, &format!("/fsck?net={}", network.0.0), None).await?;
        r.json().await.map_err(worker_err)
    }

    /// Ingests packs in order. A thin pack whose bases arrive later answers 409 and is retried after the others, until
    /// a full pass makes no progress.
    pub async fn ingest(&self, network: NetworkId, packs: Vec<(Digest, Vec<u8>)>) -> Result<usize, StoreError> {
        let mut left = packs;
        let mut done = 0;
        while !left.is_empty() {
            let mut retry = Vec::new();
            let before = left.len();
            for (sum, body) in left {
                let path = format!("/ingest?net={}&sha256={}", network.0.0, sum.hex());
                let mut r = self.call(Method::Post, &path, Some(body.clone()), &[]).await?;
                match r.status_code() {
                    200 => done += 1,
                    409 => retry.push((sum, body)),
                    c => {
                        return Err(StoreError(format!(
                            "ingest {}: {c} {}",
                            sum.hex(),
                            r.text().await.unwrap_or_default()
                        )));
                    }
                }
            }
            if retry.len() == before {
                return Err(StoreError(format!("{} packs have bases no pack provides", retry.len())));
            }
            left = retry;
        }
        Ok(done)
    }

    /// One complete pack of every object the given ingested packs brought: the home shard's half of `Promoted`.
    pub async fn promote(&self, network: NetworkId, sums: &[Digest]) -> Result<Vec<u8>, StoreError> {
        let body: String = sums.iter().map(|s| format!("{}\n", s.hex())).collect();
        let mut r = self.ok(Method::Post, &format!("/promote?net={}", network.0.0), Some(body.into_bytes())).await?;
        r.bytes().await.map_err(worker_err)
    }

    /// `GitCacheShard::upload_pack`, as smart HTTP: `info_refs` for the advertisement, else one stateless-rpc round.
    /// The caller passes the client's `Git-Protocol` header through.
    pub async fn upload_pack(
        &self,
        network: NetworkId,
        info_refs: bool,
        body: Option<Vec<u8>>,
        git_protocol: &str,
        content_encoding: &str,
    ) -> Result<Response, StoreError> {
        let path = if info_refs {
            format!("/info-refs?net={}", network.0.0)
        } else {
            format!("/upload-pack?net={}", network.0.0)
        };
        let mut h = vec![("git-protocol", git_protocol)];
        if !content_encoding.is_empty() {
            h.push(("content-encoding", content_encoding));
        }
        let method = if info_refs { Method::Get } else { Method::Post };
        self.call(method, &path, body, &h).await
    }

    fn cursor_key(network: NetworkId) -> String {
        format!("cursor:{}", network.0.0)
    }

    /// The cursor for `network`, or `None` when it describes another disk than the container's.
    pub async fn cursor(&self, network: NetworkId, disk: &str) -> Result<Option<IngestCursor>, StoreError> {
        let v: Option<(String, IngestCursor)> =
            self.state.storage().get(&Self::cursor_key(network)).await.map_err(worker_err)?;
        Ok(v.filter(|(d, _)| d == disk).map(|(_, c)| c))
    }

    pub async fn save_cursor(&self, c: &IngestCursor, disk: &str) -> Result<(), StoreError> {
        self.state.storage().put(&Self::cursor_key(c.network), (disk, c)).await.map_err(worker_err)
    }

    /// The container's disk size and the repositories on it, with their sizes and last use.
    pub async fn repos(&self) -> Result<(u64, Vec<DiskRepo>), StoreError> {
        let mut r = self.ok(Method::Get, "/repos", None).await?;
        let listed: ReposOut = r.json().await.map_err(worker_err)?;
        let repos = listed
            .repos
            .iter()
            .filter_map(|o| {
                Some(DiskRepo {
                    network: NetworkId(graftyard_types::DbId(o.network.parse().ok()?)),
                    bytes: o.bytes,
                    used: Timestamp(o.used),
                })
            })
            .collect();
        Ok((listed.disk, repos))
    }

    /// The LRU of bare repositories: before `incoming` bytes of packs are written for `network`, evicts the least
    /// recently used other networks until the disk's repositories fit `DISK_USE_PCT` of `disk_bytes` (the
    /// container's filesystem size when `None`). An evicted network's cursor is deleted first, so its next request
    /// rehydrates it; a network the container reports in use keeps its repository, and its cursor is put back.
    pub async fn make_room(
        &self,
        network: NetworkId,
        incoming: u64,
        disk_bytes: Option<u64>,
    ) -> Result<Room, StoreError> {
        let (fs, repos) = self.repos().await?;
        let disk = disk_bytes.unwrap_or(fs);
        let mut room = Room { disk, held: repos.iter().map(|r| r.bytes).sum(), incoming, ..Room::default() };
        for n in evict_plan(&repos, disk, incoming, network) {
            let key = Self::cursor_key(n);
            let saved: Option<(String, IngestCursor)> = self.state.storage().get(&key).await.map_err(worker_err)?;
            self.state.storage().delete(&key).await.map_err(worker_err)?;
            let mut e = self.call(Method::Post, &format!("/evict?net={}", n.0.0), None, &[]).await?;
            match e.status_code() {
                200 => room.evicted.push(n.0.0),
                409 => {
                    if let Some(c) = saved {
                        self.state.storage().put(&key, c).await.map_err(worker_err)?;
                    }
                    room.busy.push(n.0.0);
                }
                c => return Err(StoreError(format!("evict {}: {c} {}", n.0.0, e.text().await.unwrap_or_default()))),
            }
        }
        Ok(room)
    }

    /// `rehydrate`: every stored pack with a marker, minus consumed packs, plus the packs each member chain names
    /// after its last `Promoted`, read from its leader. Rebuilds the repository from nothing, sets the leader's refs,
    /// and saves the cursor at each chain's applied index.
    pub async fn rehydrate<B: Bucket, L: ChainLog>(
        &self,
        network: NetworkId,
        chains: &[ChainId],
        buckets: &[&B],
        log: &L,
    ) -> Result<(IngestCursor, RehydrateReport), StoreError> {
        let started = self.up().await?;
        self.init(network, true).await?;
        // Markers from every bucket of the placement: a marker missing in one bucket is in another.
        let mut marks: BTreeMap<PackId, Marker> = BTreeMap::new();
        for b in buckets {
            for (id, m) in markers(*b, network).await? {
                marks.entry(id).or_insert(m);
            }
        }
        let marks: Vec<(PackId, Marker)> = marks.into_iter().collect();
        let consumed = log.consumed(network).await?;
        let mut cursor = IngestCursor::new(network);
        let mut tail: Vec<LogPacks> = Vec::new();
        let mut all_refs: Vec<(String, String)> = Vec::new();
        for chain in chains {
            let refs = log.core_refs(*chain).await?;
            let last = log.last_promoted(*chain).await?.map(|(_, p)| p);
            let from = tail_start(last.as_ref());
            if refs.applied >= from {
                tail.extend(log.entries(*chain, from, refs.applied).await?);
            }
            cursor.advance(*chain, refs.applied);
            all_refs.extend(refs.refs);
        }
        let plan = rehydrate_plan(&marks, &consumed, &tail);
        let incoming: u64 = plan
            .iter()
            .map(|l| match l {
                Load::Stored { bytes, .. } => *bytes,
                Load::Inline { bytes, .. } => bytes.len() as u64,
            })
            .sum();
        let room = self.make_room(network, incoming, None).await?;
        let mut packs = Vec::new();
        let mut bytes = 0;
        for l in &plan {
            let p = fetch_load(buckets, network, l).await?;
            bytes += p.1.len();
            packs.push(p);
        }
        let ingested = self.ingest(network, packs).await?;
        self.set_refs(network, &all_refs).await?;
        self.save_cursor(&cursor, &started.health.disk).await?;
        let stored = plan.iter().filter(|l| l.stored_id().is_some()).count();
        Ok((
            cursor,
            RehydrateReport {
                markers: marks.len(),
                stored,
                inline: plan.len() - stored,
                ingested,
                bytes,
                disk: started.health.disk,
                region: started.health.region,
                location: started.health.location,
                replaced: started.replaced,
                room,
            },
        ))
    }

    /// Catch-up before serving: for each chain whose leader's applied index is past the cursor, read the missing
    /// entries, fetch their packs, ingest them, set the refs, and advance the cursor. A cursor for another disk
    /// (the container restarted) means a full rehydrate.
    pub async fn catch_up<B: Bucket, L: ChainLog>(
        &self,
        network: NetworkId,
        chains: &[ChainId],
        buckets: &[&B],
        log: &L,
    ) -> Result<CatchUp, StoreError> {
        let started = self.up().await?;
        let mut cursor = self.cursor(network, &started.health.disk).await?;
        // A cursor below a chain's first log index needs entries the leader has archived and truncated.
        if let Some(c) = cursor.clone() {
            for chain in chains {
                if c.at(*chain).0 + 1 < log.first_index(*chain).await?.0 {
                    cursor = None;
                }
            }
        }
        let Some(mut cursor) = cursor else {
            let (cursor, r) = self.rehydrate(network, chains, buckets, log).await?;
            return Ok(CatchUp { rehydrated: Some(r), entries: 0, packs: 0, cursor, evicted: Vec::new() });
        };
        let mut all_refs = Vec::new();
        let (mut entries, mut packs) = (0, 0);
        let mut evicted = Vec::new();
        let mut seen = BTreeSet::new();
        for chain in chains {
            let refs = log.core_refs(*chain).await?;
            if let Some(range) = cursor.behind(*chain, refs.applied) {
                let es = log.entries(*chain, *range.start(), *range.end()).await?;
                entries += es.len();
                let plan = crate::ingest::catch_up_plan(&cursor, *chain, &es, refs.applied);
                let mut bodies = Vec::new();
                for l in plan.iter().filter(|l| l.stored_id().is_none_or(|id| seen.insert(id))) {
                    bodies.push(fetch_load(buckets, network, l).await?);
                }
                if !bodies.is_empty() {
                    let incoming = bodies.iter().map(|(_, b)| b.len() as u64).sum();
                    let r = self.make_room(network, incoming, None).await?;
                    evicted.extend(r.evicted);
                }
                packs += self.ingest(network, bodies).await?;
            }
            all_refs.extend(refs.refs);
            cursor.advance(*chain, refs.applied);
        }
        self.set_refs(network, &all_refs).await?;
        self.save_cursor(&cursor, &started.health.disk).await?;
        Ok(CatchUp { rehydrated: None, entries, packs, cursor, evicted })
    }
}

#[derive(Debug, serde::Serialize)]
pub struct RehydrateReport {
    pub markers: usize,
    pub stored: usize,
    pub inline: usize,
    pub ingested: usize,
    pub bytes: usize,
    pub disk: String,
    pub region: String,
    pub location: String,
    pub replaced: u8,
    pub room: Room,
}

#[derive(Debug, serde::Serialize)]
pub struct CatchUp {
    pub rehydrated: Option<RehydrateReport>,
    pub entries: usize,
    pub packs: usize,
    pub cursor: IngestCursor,
    /// Networks the LRU evicted to make room for this catch-up's packs.
    pub evicted: Vec<i64>,
}

async fn wasm_bindgen_futures_await(p: wasm_bindgen::JsValue) -> Result<(), StoreError> {
    let p: js_sys::Promise = p.into();
    worker::wasm_bindgen_futures::JsFuture::from(p).await.map(|_| ()).map_err(|e| StoreError(format!("{e:?}")))
}
