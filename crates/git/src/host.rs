//! The Workers host: `Bucket` over an R2 binding, and the git cache shard object's side of its container
//! (`GitCacheShard::validate`, `upload_pack`, `rehydrate`, and catch-up behind the `IngestCursor`). A site script's shard
//! class holds a `ShardHost` and calls these; the throwaway M1 scripts under `tests/m1/` do the same today.

use std::collections::BTreeSet;

use graftyard_types::{ChainId, Digest, NetworkId, Region, Timestamp};
use serde::Deserialize;
use worker::{Conditional, Container, Fetcher, Headers, Include, Method, Request, RequestInit, Response, State};

use crate::bucket::{Bucket, ObjectInfo, PutNew, StoreError};
use crate::catchup::{Ingested, Rehydrating, Report, Saved, ShardIo};
use crate::ingest::ChainLog;
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

/// `ShardHost::validate`'s answer: the pack's index (none for a detail-only call) and each update's detail, or the
/// container's slots stayed full past an agent's wait (HTTP 429), to be retried after `retry_after_s`.
#[derive(Debug)]
pub enum Validated {
    Done(Option<PackIndex>, Vec<crate::agent::RefDetail>),
    Busy { retry_after_s: u32 },
}

#[derive(Debug, Deserialize)]
struct ValidateOut {
    #[serde(default)]
    checksum: String,
    #[serde(default)]
    bytes: u64,
    #[serde(default)]
    objects: u32,
    #[serde(default)]
    trailer: String,
    #[serde(default)]
    detail: Vec<crate::agent::RefDetail>,
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

    /// The container's `NetworkLoad`: the last minute's p99 busy fraction of its vCPUs and the validates waiting or
    /// running. A container that is not running reports no load and is not started for it.
    pub async fn load(&self) -> Result<crate::shard::NetworkLoad, StoreError> {
        if !self.container()?.running() {
            return Ok(crate::shard::NetworkLoad::default());
        }
        let mut r = self.ok(Method::Get, "/load", None).await?;
        r.json().await.map_err(worker_err)
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
        set_inactivity(&self.container()?, ms).await
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

    /// `GitCacheShard::validate`: `index-pack --strict --fix-thin` into a quarantine and connectivity of the tips
    /// against the refs the leader just gave, which the container holds once `set_refs` ran. `body` is the caller's
    /// body as it came, streamed to the container and never held here: its first `tipsn` bytes are the tips, one per
    /// line, then `detailn` bytes of `"<new> <old> <ref>"` lines (`crate::agent::RefDetail`), then the pack. Buffered,
    /// a burst of 512 pushes of 256 KiB, or 1 MiB packs at 4 a second, put the shard's isolate over its 128 MB (LM-1
    /// round 6). An agent's validate (`agent`) may wait for one of the container's slots at most 2 s, and is then
    /// `Validated::Busy`; a human's waits. `derived` marks the shard's own archive promotion, whose CPU the container
    /// counts as derived work, outside `NetworkLoad`.
    pub async fn validate(
        &self,
        network: NetworkId,
        tipsn: usize,
        detailn: usize,
        agent: bool,
        derived: bool,
        body: Option<wasm_bindgen::JsValue>,
    ) -> Result<Validated, StoreError> {
        let mut path = format!("/validate?net={}&tipsn={tipsn}", network.0.0);
        if detailn > 0 {
            path.push_str(&format!("&detailn={detailn}"));
        }
        if agent {
            path.push_str("&class=agent");
        }
        if derived {
            path.push_str("&derived=1");
        }
        let mut init = RequestInit::new();
        init.with_method(Method::Post).with_body(body);
        let req = Request::new_with_init(&format!("http://container{path}"), &init).map_err(worker_err)?;
        let mut r = self.port()?.fetch_request(req).await.map_err(worker_err)?;
        if r.status_code() == 429 {
            let retry = r.headers().get("retry-after").ok().flatten().and_then(|v| v.trim().parse().ok());
            return Ok(Validated::Busy { retry_after_s: retry.unwrap_or(1).max(1) });
        }
        if r.status_code() >= 300 {
            let t = r.text().await.unwrap_or_default();
            return Err(StoreError(format!("validate: {} {t}", r.status_code())));
        }
        let v: ValidateOut = r.json().await.map_err(worker_err)?;
        if v.checksum.is_empty() {
            return Ok(Validated::Done(None, v.detail));
        }
        let ix = PackIndex {
            checksum: Digest::parse_hex(&v.checksum).ok_or_else(|| StoreError("checksum".into()))?,
            bytes: v.bytes,
            objects: v.objects,
            trailer: v.trailer.into(),
        };
        Ok(Validated::Done(Some(ix), v.detail))
    }

    pub async fn init(&self, network: NetworkId, fresh: bool) -> Result<(), StoreError> {
        self.ok(Method::Post, &format!("/init?net={}&fresh={}", network.0.0, u8::from(fresh)), None).await.map(|_| ())
    }

    /// The whole ref set of the network's member chains, as their leaders gave it.
    /// Replaces the container's whole ref set with `refs`, one `PUT /refs` of at most a page of lines, the first
    /// page with `start=1` and the last with `done=1` (`containers/git/server`), so the edge's memory holds one page.
    pub async fn put_refs_page(
        &self,
        network: NetworkId,
        refs: &[(String, String)],
        start: bool,
        done: bool,
    ) -> Result<(), StoreError> {
        let body: String = refs.iter().map(|(r, o)| format!("{o} {r}\n")).collect();
        let path = format!("/refs?net={}&start={}&done={}", network.0.0, u8::from(start), u8::from(done));
        self.ok(Method::Put, &path, Some(body.into_bytes())).await.map(|_| ())
    }

    /// Applies ref changes (`crate::ingest::ref_changes`) to the container's refs: `POST /refs`, deletes then sets.
    /// An update to an object the shard does not hold is skipped by the container, as a rehydrate's is.
    pub async fn apply_refs(&self, network: NetworkId, changes: &[(String, Option<String>)]) -> Result<(), StoreError> {
        if changes.is_empty() {
            return Ok(());
        }
        let zero = "0".repeat(40);
        let body: String = changes.iter().map(|(r, o)| format!("{} {r}\n", o.as_deref().unwrap_or(&zero))).collect();
        self.ok(Method::Post, &format!("/refs?net={}", network.0.0), Some(body.into_bytes())).await.map(|_| ())
    }

    pub async fn fsck(&self, network: NetworkId) -> Result<serde_json::Value, StoreError> {
        let mut r = self.ok(Method::Post, &format!("/fsck?net={}", network.0.0), None).await?;
        r.json().await.map_err(worker_err)
    }

    /// One complete pack of every object the given ingested packs brought: the home shard's half of `Promoted`.
    pub async fn promote(&self, network: NetworkId, sums: &[Digest]) -> Result<Vec<u8>, StoreError> {
        let body: String = sums.iter().map(|s| format!("{}\n", s.hex())).collect();
        let mut r = self.ok(Method::Post, &format!("/promote?net={}", network.0.0), Some(body.into_bytes())).await?;
        r.bytes().await.map_err(worker_err)
    }

    /// The test merge of `head` into `base` (`GitCacheShard::merge` without the plan): the container's `/merge`, which
    /// runs merge-tree, commit-tree, and the tree-diff check in a quarantine and leaves the repository unchanged. With
    /// the answer, the CPU ms of the merge's own git processes (`cpuMs`), which TEST_MERGE_BUDGET is charged.
    pub async fn merge(
        &self,
        network: NetworkId,
        base: crate::refs::ObjectId,
        head: crate::refs::ObjectId,
    ) -> Result<(crate::prsync::MergeAnswer, Option<f32>), StoreError> {
        let path = format!("/merge?net={}&base={}&head={}", network.0.0, base.hex(), head.hex());
        let mut r = self.ok(Method::Post, &path, None).await?;
        let v: serde_json::Value = r.json().await.map_err(worker_err)?;
        let cpu = v["cpuMs"].as_f64().map(|c| c as f32);
        Ok((serde_json::from_value(v).map_err(|e| StoreError(format!("{path}: {e}")))?, cpu))
    }

    /// `GitCacheShard::merge` for a real merge or a merge-queue group: each head in turn on the previous result, from
    /// `base`, by `method` (the container's `/merge` with `method`; `crate::merge_cache`).
    pub async fn merge_chain(
        &self,
        network: NetworkId,
        base: crate::refs::ObjectId,
        heads: &[(crate::issues::IssueNumber, crate::refs::ObjectId)],
        method: crate::issues::MergeMethod,
    ) -> Result<Option<crate::merge_cache::ChainAnswer>, StoreError> {
        let q = crate::merge_cache::chain_query(base, heads, method);
        let path = format!("/merge?net={}&{q}", network.0.0);
        let mut r = self.call(Method::Post, &path, None, &[]).await?;
        match r.status_code() {
            // A commit of the chain, or the whole repository, is not here yet: the caller catches up and asks again.
            c if crate::merge_cache::needs_catch_up(c) => Ok(None),
            c if c >= 300 => Err(StoreError(format!("{path}: {c} {}", r.text().await.unwrap_or_default()))),
            _ => r.json().await.map(Some).map_err(worker_err),
        }
    }

    /// `GitCacheShard::merge` as SHARED names it: the `MergePlan` of one pull request onto its base's tip.
    pub async fn merge_plan(
        &self,
        network: NetworkId,
        req: &crate::merge::MergeRequest,
    ) -> Result<Result<crate::merge::MergePlan, crate::merge_cache::CacheError>, StoreError> {
        let a = self
            .merge_chain(network, req.base_old, &[(req.n, req.head)], req.method)
            .await?
            .ok_or_else(|| StoreError("a commit of the merge is not in the git cache".into()))?;
        Ok(a.merge_plan(req))
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

    /// `git archive` of one commit of `network` (`codeload-archives`).
    pub async fn archive(&self, network: NetworkId, query: &str) -> Result<Response, StoreError> {
        self.call(Method::Get, &format!("/archive?net={}&{query}", network.0.0), None, &[]).await
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
            // An evicted network's half-done rehydrate starts again from nothing.
            self.state.storage().delete(&Self::rehydrating_key(n)).await.map_err(worker_err)?;
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

    /// Catch-up before serving, and `rehydrate` when the disk has no cursor (`crate::catchup`): a page of the log at a
    /// time, stored packs streamed from a bucket to the container, and every page saved before the next.
    pub async fn catch_up(
        &self,
        network: NetworkId,
        chains: &[ChainId],
        buckets: &[&R2],
        log: &impl ChainLog,
    ) -> Result<Report, StoreError> {
        let io = Io { host: self, network, buckets };
        crate::catchup::catch_up(&io, &io, log, network, chains, &|| js_sys::Date::now() as u64).await
    }

    /// Whether the disk holds a cursor for `network`, so a catch-up reads from the log rather than rehydrating.
    pub async fn warm(&self, network: NetworkId) -> Result<bool, StoreError> {
        if !self.container()?.running() {
            return Ok(false);
        }
        let disk = self.health().await?.disk;
        Ok(self.cursor(network, &disk).await?.is_some())
    }

    fn rehydrating_key(network: NetworkId) -> String {
        format!("rehydrating:{}", network.0.0)
    }

    /// Ingests one pack's bytes: `Later` when the container answers 409.
    async fn ingest_body(&self, network: NetworkId, sum: Digest, body: JsBody) -> Result<Option<Ingested>, StoreError> {
        let path = format!("/ingest?net={}&sha256={}", network.0.0, sum.hex());
        let mut init = RequestInit::new();
        init.with_method(Method::Post).with_body(Some(body.0));
        let req = Request::new_with_init(&format!("http://container{path}"), &init).map_err(worker_err)?;
        let mut r = self.port()?.fetch_request(req).await.map_err(worker_err)?;
        match r.status_code() {
            200 => Ok(Some(Ingested::Done)),
            409 => Ok(Some(Ingested::Later)),
            // A copy whose bytes do not match the checksum: the caller tries the next bucket.
            422 if r.text().await.unwrap_or_default().starts_with("checksum") => Ok(None),
            c => Err(StoreError(format!("ingest {}: {c} {}", sum.hex(), r.text().await.unwrap_or_default()))),
        }
    }
}

/// A request body for the container: bytes, or a stream that never enters the isolate's memory.
struct JsBody(wasm_bindgen::JsValue);

/// `crate::catchup`'s container, buckets, and storage, over one shard object.
struct Io<'a, 'h> {
    host: &'a ShardHost<'h>,
    network: NetworkId,
    buckets: &'a [&'a R2],
}

impl ShardIo for Io<'_, '_> {
    async fn up(&self) -> Result<String, StoreError> {
        Ok(self.host.up().await?.health.disk)
    }
    async fn init(&self, fresh: bool) -> Result<(), StoreError> {
        self.host.init(self.network, fresh).await
    }
    async fn have(&self, sums: &[Digest]) -> Result<BTreeSet<Digest>, StoreError> {
        #[derive(Deserialize)]
        struct Have {
            have: Vec<String>,
        }
        let body: String = sums.iter().map(|s| format!("{}\n", s.hex())).collect();
        let mut r =
            self.host.ok(Method::Post, &format!("/have?net={}", self.network.0.0), Some(body.into_bytes())).await?;
        let h: Have = r.json().await.map_err(worker_err)?;
        Ok(h.have.iter().filter_map(|s| Digest::parse_hex(s)).collect())
    }
    async fn ingest(&self, sum: Digest, body: &[u8]) -> Result<Ingested, StoreError> {
        let b = JsBody(js_sys::Uint8Array::from(body).into());
        self.host
            .ingest_body(self.network, sum, b)
            .await?
            .ok_or_else(|| StoreError(format!("ingest {}: the bytes do not match their checksum", sum.hex())))
    }
    /// From each bucket in turn, its object's body streamed into the container's `/ingest`, which checks the
    /// checksum as it writes: a copy that does not match is refused, and the next bucket's is tried.
    async fn ingest_stored(&self, id: PackId, checksum: Digest) -> Result<Ingested, StoreError> {
        let key = R2Key::Pack { network: self.network, id }.to_string();
        for b in self.buckets {
            let Some(o) = b.0.get(&key).execute().await.map_err(err)? else { continue };
            let Some(body) = o.body() else { continue };
            let worker::ResponseBody::Stream(s) = body.response_body().map_err(worker_err)? else { continue };
            if let Some(r) = self.host.ingest_body(self.network, checksum, JsBody(s.into())).await? {
                return Ok(r);
            }
        }
        Err(StoreError(format!("{key}: no bucket holds a copy matching its checksum")))
    }
    async fn put_refs_page(&self, refs: &[(String, String)], start: bool, done: bool) -> Result<(), StoreError> {
        self.host.put_refs_page(self.network, refs, start, done).await
    }
    async fn apply_refs(&self, changes: &[(String, Option<String>)]) -> Result<(), StoreError> {
        self.host.apply_refs(self.network, changes).await
    }
    async fn make_room(&self, incoming: u64) -> Result<Vec<i64>, StoreError> {
        Ok(self.host.make_room(self.network, incoming, None).await?.evicted)
    }
    async fn markers(&self) -> Result<Vec<(PackId, Marker)>, StoreError> {
        let mut out = Vec::new();
        for b in self.buckets {
            out.extend(markers(*b, self.network).await?);
        }
        Ok(out)
    }
}

impl Saved for Io<'_, '_> {
    async fn cursor(&self) -> Result<Option<(String, IngestCursor)>, StoreError> {
        self.host.state.storage().get(&ShardHost::cursor_key(self.network)).await.map_err(worker_err)
    }
    async fn put_cursor(&self, disk: &str, c: &IngestCursor) -> Result<(), StoreError> {
        self.host.save_cursor(c, disk).await
    }
    async fn drop_cursor(&self) -> Result<(), StoreError> {
        self.host.state.storage().delete(&ShardHost::cursor_key(self.network)).await.map(|_| ()).map_err(worker_err)
    }
    async fn rehydrating(&self) -> Result<Option<(String, Rehydrating)>, StoreError> {
        // A value this release cannot read (an older release's) is a rehydrate started from nothing.
        Ok(self.host.state.storage().get(&ShardHost::rehydrating_key(self.network)).await.ok().flatten())
    }
    async fn put_rehydrating(&self, disk: &str, r: &Rehydrating) -> Result<(), StoreError> {
        self.host.state.storage().put(&ShardHost::rehydrating_key(self.network), (disk, r)).await.map_err(worker_err)
    }
    async fn drop_rehydrating(&self) -> Result<(), StoreError> {
        let s = self.host.state.storage();
        s.delete(&ShardHost::rehydrating_key(self.network)).await.map_err(worker_err)?;
        // An older release kept its resume point here.
        s.delete(&format!("rehydrate:{}", self.network.0.0)).await.map(|_| ()).map_err(worker_err)
    }
}

/// `setInactivityTimeout` on a container. The timeout belongs to the Durable Object instance that set it: an instance
/// that restarts while its container runs starts without one, and the runtime then stops the container shortly after
/// the object goes idle, so the shard object's constructor sets it again (`workers/site/src/shard.rs`).
pub async fn set_inactivity(c: &Container, ms: u64) -> Result<(), StoreError> {
    let f =
        js_sys::Reflect::get(c.as_ref(), &"setInactivityTimeout".into()).map_err(|_| StoreError("timeout".into()))?;
    let p =
        js_sys::Function::from(f).call1(c.as_ref(), &(ms as f64).into()).map_err(|e| StoreError(format!("{e:?}")))?;
    wasm_bindgen_futures_await(p).await
}

async fn wasm_bindgen_futures_await(p: wasm_bindgen::JsValue) -> Result<(), StoreError> {
    let p: js_sys::Promise = p.into();
    worker::wasm_bindgen_futures::JsFuture::from(p).await.map(|_| ()).map_err(|e| StoreError(format!("{e:?}")))
}
