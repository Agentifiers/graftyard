//! The Workers host: `Bucket` over an R2 binding, and the git cache shard object's side of its container
//! (`GitCacheShard::validate`, `upload_pack`, `rehydrate`, and catch-up behind the `IngestCursor`). A site script's shard
//! class holds a `ShardHost` and calls these; the throwaway M1 scripts under `tests/m1/` do the same today.

use std::collections::{BTreeMap, BTreeSet};

use graftyard_types::{ChainId, Digest, NetworkId, Region, SiteId, Timestamp};
use serde::Deserialize;
use worker::{Conditional, Container, Fetcher, Headers, Include, Method, Request, RequestInit, Response, State};

use crate::bucket::{Bucket, ObjectInfo, PutNew, StoreError};
use crate::catchup::{Fetched, IdxRange, Ingested, Rehydrating, Report, Saved, ShardIo, StoredMs, idx_range};
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

    /// The source's body goes into the put through a `FixedLengthStream` of the expected length, so the runtime moves
    /// the bytes and none enter wasm memory; R2 checks the length as the stream ends and the SHA-256 before it stores.
    /// The pipe runs on its own: a put refused by its precondition may never read the stream.
    async fn copy_new(
        &self,
        from: &Self,
        key: &str,
        expect: Option<(u64, Digest)>,
    ) -> Result<Option<PutNew>, StoreError> {
        let Some(o) = from.0.get(key).execute().await.map_err(err)? else { return Ok(None) };
        let meta: Vec<(String, String)> = o.custom_metadata().unwrap_or_default().into_iter().collect();
        let len = expect.map_or(o.size(), |(n, _)| n);
        let Some(body) = o.body() else { return Ok(None) };
        let worker::ResponseBody::Stream(src) = body.response_body().map_err(worker_err)? else {
            return Err(StoreError(format!("{key}: the source's body is not a stream")));
        };
        let fixed = match u32::try_from(len) {
            Ok(n) => worker::worker_sys::FixedLengthStream::new(n),
            Err(_) => worker::worker_sys::FixedLengthStream::new_big_int(js_sys::BigInt::from(len)),
        }
        .map_err(|e| err(worker::Error::from(e)))?;
        let piped = src.pipe_to(&fixed.writable());
        worker::wasm_bindgen_futures::spawn_local(async move {
            let _ = worker::wasm_bindgen_futures::JsFuture::from(piped).await;
        });
        let mut opts: Vec<(&str, wasm_bindgen::JsValue)> = vec![
            ("onlyIf", object(&[("etagDoesNotMatch", "*".into())]).into()),
            ("customMetadata", meta_object(&meta)),
        ];
        if let Some((_, sum)) = expect {
            opts.push(("sha256", js_sys::Uint8Array::from(&sum.0[..]).buffer().into()));
        }
        let v = js(self.raw().put(key.to_owned(), fixed.readable().into(), object(&opts).into())).await?;
        if v.is_null() {
            return Ok(Some(PutNew::Exists(self.head(key).await?.and_then(|o| o.sha256))));
        }
        Ok(Some(PutNew::Created))
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

/// One part of a streamed pack, held as a JavaScript `Uint8Array` rather than in wasm memory, so every bucket's put
/// reads the one copy and the isolate's wasm heap holds none of the pack (`PackStore::place_streamed`).
pub struct JsPart {
    buf: js_sys::Uint8Array,
    len: u32,
}

impl crate::bucket::PartBuf for JsPart {
    fn with_capacity(n: usize) -> Self {
        JsPart { buf: js_sys::Uint8Array::new_with_length(n as u32), len: 0 }
    }
    fn push(&mut self, b: &[u8]) {
        self.buf.subarray(self.len, self.len + b.len() as u32).copy_from(b);
        self.len += b.len() as u32;
    }
    fn filled(&self) -> usize {
        self.len as usize
    }
    fn clear(&mut self) {
        self.len = 0;
    }
}

impl JsPart {
    /// The filled bytes as a view, not a copy. A put reads it before it resolves, and the tee refills the part only
    /// after every put of it resolved.
    fn view(&self) -> js_sys::Uint8Array {
        self.buf.subarray(0, self.len)
    }
}

impl R2 {
    fn raw(&self) -> &worker::worker_sys::R2Bucket {
        use wasm_bindgen::JsCast;
        AsRef::<wasm_bindgen::JsValue>::as_ref(&self.0).unchecked_ref()
    }
}

/// Awaits an R2 binding promise.
async fn js(p: Result<js_sys::Promise, wasm_bindgen::JsValue>) -> Result<wasm_bindgen::JsValue, StoreError> {
    let p = p.map_err(|e| err(worker::Error::from(e)))?;
    worker::wasm_bindgen_futures::JsFuture::from(p).await.map_err(|e| err(worker::Error::from(e)))
}

fn object(pairs: &[(&str, wasm_bindgen::JsValue)]) -> js_sys::Object {
    let o = js_sys::Object::new();
    for (k, v) in pairs {
        let _ = js_sys::Reflect::set(&o, &(*k).into(), v);
    }
    o
}

fn meta_object(meta: &[(String, String)]) -> wasm_bindgen::JsValue {
    let o = js_sys::Object::new();
    for (k, v) in meta {
        let _ = js_sys::Reflect::set(&o, &k.into(), &v.into());
    }
    o.into()
}

/// The streamed tee's parts go to the binding as views of their `JsPart`, which the runtime reads as bytes
/// (`R2PutValue`'s `kj::Array<kj::byte>`), so no part is copied back into wasm memory.
impl crate::bucket::Multipart for R2 {
    type Part = JsPart;

    async fn put_whole(
        &self,
        key: &str,
        part: &JsPart,
        sha256: Digest,
        meta: &[(String, String)],
    ) -> Result<PutNew, StoreError> {
        let sum = js_sys::Uint8Array::from(&sha256.0[..]).buffer();
        let opts = object(&[
            ("onlyIf", object(&[("etagDoesNotMatch", "*".into())]).into()),
            ("sha256", sum.into()),
            ("customMetadata", meta_object(meta)),
        ]);
        let v = js(self.raw().put(key.to_owned(), part.view().into(), opts.into())).await?;
        if v.is_null() {
            return Ok(PutNew::Exists(self.head(key).await?.and_then(|o| o.sha256)));
        }
        Ok(PutNew::Created)
    }

    async fn start_upload(
        &self,
        key: &str,
        journal: &str,
        meta: &[(String, String)],
    ) -> Result<Result<String, PutNew>, StoreError> {
        if let Some(o) = self.head(key).await? {
            return Ok(Err(PutNew::Exists(o.sha256)));
        }
        let m: std::collections::HashMap<String, String> = meta.iter().cloned().collect();
        let u = self.0.create_multipart_upload(key).custom_metadata(m).execute().await.map_err(err)?;
        let id = u.upload_id().await;
        let mut jm = std::collections::HashMap::new();
        jm.insert("upload".to_owned(), id.clone());
        self.0.put(journal, Vec::<u8>::new()).custom_metadata(jm).execute().await.map_err(err)?;
        Ok(Ok(id))
    }

    async fn put_part(&self, key: &str, upload_id: &str, n: u16, part: &JsPart) -> Result<String, StoreError> {
        use wasm_bindgen::JsCast;
        let u: worker::worker_sys::R2MultipartUpload = self
            .raw()
            .resume_multipart_upload(key.to_owned(), upload_id.to_owned())
            .map_err(|e| err(worker::Error::from(e)))?
            .unchecked_into();
        let p: worker::worker_sys::R2UploadedPart = js(u.upload_part(n, part.view().into())).await?.unchecked_into();
        p.etag().map_err(|e| err(worker::Error::from(e)))
    }

    async fn complete_upload(
        &self,
        key: &str,
        upload_id: &str,
        journal: &str,
        parts: &[(u16, String)],
    ) -> Result<(), StoreError> {
        let u = self.0.resume_multipart_upload(key, upload_id).map_err(err)?;
        u.complete(parts.iter().map(|(n, e)| worker::UploadedPart::new(*n, e.clone()))).await.map_err(err)?;
        self.0.delete(journal).await.map_err(err)
    }
}

/// A clone-cache hit: the object under `key` in `b` as a response whose body is R2's stream, with `headers` and the
/// object's length, so a 270 MB entry passes through the isolate without entering its memory. Read whole, every hit
/// on B0-34's monorepo answered 500 (LM-1 round 26). `None` when there is no object.
pub async fn cached_response(b: &R2, key: &str, headers: Headers) -> Result<Option<Response>, StoreError> {
    let Some(o) = b.0.get(key).execute().await.map_err(err)? else { return Ok(None) };
    headers.set("content-length", &o.size().to_string()).map_err(worker_err)?;
    let body = o.body().ok_or_else(|| StoreError(format!("{key}: no body")))?.response_body().map_err(worker_err)?;
    Ok(Some(Response::from_body(body).map_err(worker_err)?.with_headers(headers)))
}

/// The composed answer a manifest and tail describe (`crate::compose::Manifest`), streamed from the chain's packs in
/// the first of `buckets` that holds every one, every one checked first (`manifest_check`, `composed_answer`): the
/// edge's answer to a composed miss. `body` is the manifest answer's stream and `total` its length.
pub async fn manifest_answer(
    buckets: &[&R2],
    network: NetworkId,
    body: worker::web_sys::ReadableStream,
    total: u64,
    headers: Headers,
    sets: &impl crate::compose::SetStore,
) -> Result<crate::compose::ChainCheck<Response>, StoreError> {
    use crate::compose::ChainCheck;
    Ok(match manifest_check(buckets, network, body, total, sets).await? {
        ChainCheck::Checked(a) => ChainCheck::Checked(composed_answer(a, headers)?),
        ChainCheck::Declined => ChainCheck::Declined,
        ChainCheck::Late => ChainCheck::Late,
    })
}

/// A manifest answer whose chain packs `open_chain` checked, ready for `composed_answer`: the manifest, the bucket and
/// packs to stream, and the tail still unread in `body` after `first`, the rest of the chunk the line ended in.
pub struct ChainAnswer {
    bucket: worker::Bucket,
    m: crate::compose::Manifest,
    packs: Vec<CheckedPack>,
    first: js_sys::Uint8Array,
    body: wasm_streams::readable::IntoStream<'static>,
}

/// Reads a manifest answer's line from `body` (a manifest clone-cache entry, or a shard's `compose=only&tail=1`
/// answer, of `total` bytes) and checks its chain packs (`open_chain`). Only the line enters wasm memory, at most
/// `MANIFEST_MAX` bytes (`crate::compose::LineSplit`); the tail stays in `body` and streams on in `composed_answer`.
/// Read whole and copied once more, as main did, a tail held twice its size in the isolate: 16.8 MB for an 8 MiB tail
/// in the edgebed (`decisions/DECISIONS.md`, "`hit-gate`"). `Declined`, and nothing sent, when the line is not a manifest
/// of this network, the tail's length is not the manifest's, or no bucket holds every chain pack as the manifest
/// counted it; `Late` when the checks did not end within `CHAIN_CHECK_WITHIN` (`decisions/DECISIONS.md`,
/// "`chain-hold`" and "`chain-warm`").
pub async fn manifest_check(
    buckets: &[&R2],
    network: NetworkId,
    body: worker::web_sys::ReadableStream,
    total: u64,
    sets: &impl crate::compose::SetStore,
) -> Result<crate::compose::ChainCheck<ChainAnswer>, StoreError> {
    use crate::compose::{CHAIN_STALL, ChainCheck, LineSplit, LineStep, Manifest, ManifestLine, within};
    use wasm_bindgen::JsCast;
    let mut body = wasm_streams::ReadableStream::from_raw(body).into_stream();
    let mut split = LineSplit::default();
    let (line, first) = loop {
        let next = futures_util::StreamExt::next(&mut body);
        let v: js_sys::Uint8Array = match within(CHAIN_STALL, next, worker::Delay::from).await {
            None => return Err(StoreError(format!("manifest: no read in {} s", CHAIN_STALL.as_secs()))),
            Some(None) => return Ok(ChainCheck::Declined),
            Some(Some(c)) => c.map_err(|e| err(worker::Error::from(e)))?.unchecked_into(),
        };
        // Only the chunk's bytes up to its first newline enter wasm, and never more than `MANIFEST_MAX` of them.
        let nl = v.index_of(b'\n', 0);
        let end = if nl >= 0 { nl as u32 + 1 } else { v.length() };
        match split.push(&v.subarray(0, end.min(crate::compose::MANIFEST_MAX as u32)).to_vec()) {
            LineStep::More => {}
            LineStep::TooLong => return Ok(ChainCheck::Declined),
            LineStep::Line(line, at) => break (line, v.subarray(at as u32, v.length())),
        }
    };
    let ManifestLine::Read(m, _) = Manifest::read(&line, network) else { return Ok(ChainCheck::Declined) };
    if total.checked_sub(line.len() as u64) != Some(m.tail) || m.count().is_none() {
        return Ok(ChainCheck::Declined);
    }
    let opened = if sets.enabled() {
        open_chain_sets(buckets, network, &m, sets).await?
    } else {
        open_chain(buckets, &m).await?
    };
    Ok(match opened {
        ChainCheck::Checked((bucket, packs)) => {
            ChainCheck::Checked(ChainAnswer { bucket: bucket.0.clone(), m, packs, first, body })
        }
        ChainCheck::Declined => ChainCheck::Declined,
        ChainCheck::Late => ChainCheck::Late,
    })
}

/// One chain pack of a manifest, checked in its bucket: its key, the R2 etag its check read, and its body's length
/// (the object less header and trailer).
pub struct CheckedPack {
    key: String,
    etag: String,
    body: u64,
}

/// Every chain pack a manifest names, checked before the answer starts in the first of `buckets` that holds them all
/// (`crate::compose::check_in_time`): each one's first 12 bytes, read whole by a ranged `get`, against the manifest's
/// count. They are checked `CHAIN_OPEN_AT_ONCE` at a time in the
/// manifest's order (`crate::compose::open_in_order`, `decisions/DECISIONS.md`, "`chain-open`"), within
/// `CHAIN_CHECK_WITHIN`. No body stays open: one invocation that held eight R2 bodies got no ninth, and main, which
/// opened every pack's whole body here and held it to its turn, hung every chain of nine packs or more
/// (`decisions/DECISIONS.md`, "`chain-hold`"). `Declined` when every bucket lacks one, holds it short, or holds
/// another pack than the manifest counted, so the caller asks the shard instead and no client sees a cut answer.
pub async fn open_chain<'b>(
    buckets: &[&'b R2],
    m: &crate::compose::Manifest,
) -> Result<crate::compose::ChainCheck<(&'b R2, Vec<CheckedPack>)>, StoreError> {
    crate::compose::check_in_time(
        buckets.iter().copied(),
        |b| m.packs.iter().map(move |p| check_pack(b, &p.key, p.n)),
        worker::Delay::from,
    )
    .await
}

/// `open_chain` with header sets (`colo-gate`, `crate::compose::check_with_sets`): each bucket's `head` is read, and a
/// pack that head names comes from the set kept under the head object's version, so a set never outlives the head it
/// was checked under. Every pack still streams on the etag its check read (`composed_answer`).
pub async fn open_chain_sets<'b>(
    buckets: &[&'b R2],
    network: NetworkId,
    m: &crate::compose::Manifest,
    sets: &impl crate::compose::SetStore,
) -> Result<crate::compose::ChainCheck<(&'b R2, Vec<CheckedPack>)>, StoreError> {
    use crate::compose::{ChainCheck, HeadObject, Header};
    let head_key = R2Key::Compose { network, file: "head".into() }.to_string();
    let head = async |b: &R2| -> Result<Option<(String, HeadObject)>, StoreError> {
        let Some(o) = b.0.get(&head_key).execute().await.map_err(err)? else { return Ok(None) };
        let version = o.version();
        let Some(body) = o.body() else { return Ok(None) };
        let bytes = body.bytes().await.map_err(err)?;
        let Ok(h) = serde_json::from_slice::<HeadObject>(&bytes) else { return Ok(None) };
        Ok(h.chain.clone().checked().map(|chain| (version, HeadObject { chain, packs: h.packs })))
    };
    let check = async |b: &R2, key: String, n: u32| -> Result<Option<Header>, StoreError> {
        Ok(check_pack(b, &key, n).await?.map(|p| Header { n, etag: p.etag, body: p.body }))
    };
    let got =
        crate::compose::check_with_sets(buckets.iter().copied(), m, head, check, sets, worker::Delay::from).await?;
    Ok(match got {
        ChainCheck::Checked((b, packs)) => ChainCheck::Checked((
            b,
            packs.into_iter().map(|(key, h)| CheckedPack { key, etag: h.etag, body: h.body }).collect(),
        )),
        ChainCheck::Declined => ChainCheck::Declined,
        ChainCheck::Late => ChainCheck::Late,
    })
}

/// One chain pack's 12-byte header read from `bucket` and checked against `n`; `None` as `open_chain` says.
async fn check_pack(bucket: &R2, key: &str, n: u32) -> Result<Option<CheckedPack>, StoreError> {
    let Some(p) = pack_object(bucket, key).await? else { return Ok(None) };
    if p.2 != n {
        return Ok(None);
    }
    Ok(Some(CheckedPack { key: key.to_owned(), etag: p.0, body: p.1 - 32 }))
}

/// One chain pack's object in `bucket` as a head records it (`crate::compose::HeadPack`): its etag, size, and its
/// header's object count, from one 12-byte ranged read; `None` when it is missing or not a pack.
pub async fn pack_object(bucket: &R2, key: &str) -> Result<Option<crate::compose::HeadPack>, StoreError> {
    let range = worker::Range::OffsetWithLength { offset: 0, length: 12 };
    let Some(o) = bucket.0.get(key).range(range).execute().await.map_err(err)? else { return Ok(None) };
    if o.size() < 32 {
        return Ok(None);
    }
    let head = o.body().ok_or_else(|| StoreError(format!("{key}: no body")))?.bytes().await.map_err(err)?;
    Ok(crate::compose::pack_count(&head).map(|n| crate::compose::HeadPack(o.etag(), o.size(), n)))
}

/// The composed answer a manifest describes, from the chain packs `manifest_check` checked (`decisions/DECISIONS.md`,
/// `edge-compose`): the prefix, the pack header's packet, each pack's body, then the tail, framed in side-band packets
/// (`crate::compose::Framer`) and closed by the SHA-1 trailer of the pack and a flush, the same bytes as the
/// container's composed answer. The body goes through a `FixedLengthStream` of `answer_len`, so a read that fails
/// midway ends the answer short and the client's index-pack refuses it, never a pack that looks whole. Each pack's body
/// is one ranged `get` opened only when the pack before it has been sent and let go (`crate::compose::stream_in_turn`),
/// on the etag its check read, so a pack replaced since ends the answer short; a `get` or chunk that waits past
/// `CHAIN_STALL` does too. The tail streams last from the manifest answer's own body, counted against the manifest's
/// length, under the same `CHAIN_STALL`. R2's chunks and the tail's stay JavaScript values written as views: the
/// isolate holds one chunk, never a pack or the tail.
pub fn composed_answer(a: ChainAnswer, headers: Headers) -> Result<Response, StoreError> {
    use crate::compose::{CHAIN_STALL, Framer, Piece, answer_len, header_packet, within};
    use wasm_bindgen::JsCast;
    let ChainAnswer { bucket, m, packs, first, body: mut tail } = a;
    let count = m.count().ok_or_else(|| StoreError("manifest count past u32".into()))?;
    let prefix = m.prefix();
    let pack = 12 + packs.iter().map(|p| p.body).sum::<u64>() + m.tail + 20;
    let len = answer_len(prefix.len() as u64, pack);
    let fixed = worker::worker_sys::FixedLengthStream::new_big_int(js_sys::BigInt::from(len))
        .map_err(|e| err(worker::Error::from(e)))?;
    let readable = fixed.readable();
    let writer = fixed.writable().get_writer().map_err(|e| err(worker::Error::from(e)))?;
    let digest = worker::crypto::DigestStream::new(worker::crypto::DigestStreamAlgorithm::Sha1);
    let hasher = digest.raw().get_writer().map_err(|e| err(worker::Error::from(e)))?;
    headers.set("content-length", &len.to_string()).map_err(worker_err)?;
    worker::wasm_bindgen_futures::spawn_local(async move {
        let write = async |w: &worker::web_sys::WritableStreamDefaultWriter,
                           v: &js_sys::Uint8Array|
               -> Result<(), StoreError> { js(Ok(w.write_with_chunk(v))).await.map(|_| ()) };
        let run = async {
            write(&writer, &js_sys::Uint8Array::from(prefix.as_slice())).await?;
            let hp = header_packet(count);
            write(&writer, &js_sys::Uint8Array::from(&hp[..])).await?;
            write(&hasher, &js_sys::Uint8Array::from(&hp[5..])).await?;
            let mut framer = Framer::new(pack - 12);
            // One run of pack bytes as its packets to the client.
            let frame = async |framer: &mut Framer, v: &js_sys::Uint8Array| -> Result<(), StoreError> {
                let pieces = framer.pieces(v.length() as usize).ok_or_else(|| StoreError("past the pack".into()))?;
                for p in pieces {
                    match p {
                        Piece::Header(h) => write(&writer, &js_sys::Uint8Array::from(&h[..])).await?,
                        Piece::Data(a, b) => write(&writer, &v.subarray(a as u32, b as u32)).await?,
                    }
                }
                Ok(())
            };
            // Every run before the trailer also goes to the hash.
            let send = async |framer: &mut Framer, v: js_sys::Uint8Array| -> Result<(), StoreError> {
                frame(framer, &v).await?;
                write(&hasher, &v).await
            };
            let stalled = |what: &str| StoreError(format!("{what}: no R2 read in {} s", CHAIN_STALL.as_secs()));
            // One pack's body: a ranged `get` of bytes 12 to its length less 20, on the etag its check read.
            let open = async |p: CheckedPack| -> Result<(String, u64, wasm_streams::readable::IntoStream<'static>), StoreError> {
                let range = worker::Range::OffsetWithLength { offset: 12, length: p.body };
                let only = worker::Conditional { etag_matches: Some(p.etag.clone()), ..Default::default() };
                let get = bucket.get(&p.key).range(range).only_if(only).execute();
                let o = within(CHAIN_STALL, get, worker::Delay::from).await.ok_or_else(|| stalled(&p.key))?.map_err(err)?;
                let o = o.ok_or_else(|| StoreError(format!("{}: gone since its check", p.key)))?;
                let raw = o.body().ok_or_else(|| StoreError(format!("{}: changed since its check", p.key)))?;
                let worker::ResponseBody::Stream(raw) = raw.response_body().map_err(err)? else {
                    return Err(StoreError(format!("{}: no stream", p.key)));
                };
                Ok((p.key, p.body, wasm_streams::ReadableStream::from_raw(raw).into_stream()))
            };
            // Its bytes to the client; the read is let go when this returns, before the next pack's `get`.
            let each = async |(key, body, mut chunks): (String, u64, wasm_streams::readable::IntoStream<'static>)| {
                let mut left = body;
                while left > 0 {
                    let next = futures_util::StreamExt::next(&mut chunks);
                    let v: js_sys::Uint8Array = match within(CHAIN_STALL, next, worker::Delay::from).await {
                        None => return Err(stalled(&key)),
                        Some(Some(c)) => c.map_err(|e| err(worker::Error::from(e)))?.unchecked_into(),
                        Some(None) => return Err(StoreError(format!("{key}: {left} body bytes short"))),
                    };
                    let k = left.min(u64::from(v.length()));
                    send(&mut framer, v.subarray(0, k as u32)).await?;
                    left -= k;
                }
                Ok(())
            };
            crate::compose::stream_in_turn(packs, open, each).await?;
            // The tail: the rest of the line's chunk, then the body's chunks, exactly the manifest's length.
            let mut got = u64::from(first.length());
            if got > m.tail {
                return Err(StoreError("the tail is longer than the manifest's".into()));
            }
            send(&mut framer, first).await?;
            while got < m.tail {
                let next = futures_util::StreamExt::next(&mut tail);
                let v: js_sys::Uint8Array = match within(CHAIN_STALL, next, worker::Delay::from).await {
                    None => return Err(stalled("the tail")),
                    Some(Some(c)) => c.map_err(|e| err(worker::Error::from(e)))?.unchecked_into(),
                    Some(None) => return Err(StoreError(format!("the tail: {} bytes short", m.tail - got))),
                };
                got += u64::from(v.length());
                if got > m.tail {
                    return Err(StoreError("the tail is longer than the manifest's".into()));
                }
                send(&mut framer, v).await?;
            }
            js(Ok(hasher.close())).await?;
            let sum = digest.digest().await.map_err(err)?;
            frame(&mut framer, &sum).await?;
            if !framer.done() {
                return Err(StoreError("the pack ended short".into()));
            }
            write(&writer, &js_sys::Uint8Array::from(&b"0000"[..])).await?;
            js(Ok(writer.close())).await.map(|_| ())
        };
        if let Err(e) = run.await {
            worker::console_error!("edge-compose answer: {}", e.0);
            let _ = writer.abort_with_reason(&e.0.into());
        }
    });
    Response::from_body(worker::ResponseBody::Stream(readable)).map_err(worker_err).map(|r| r.with_headers(headers))
}

/// `r` with its body passed on chunk by chunk, calling `first` when the first chunk goes on, or dropping it unrun if
/// the body ends or is cancelled before one. The chunks stay JavaScript values, so a 270 MB answer never enters wasm
/// memory (`tests/composed/bed/run.sh`, `FWD=1`), and a cancel from the reader cancels `r`'s body. A body that is not
/// a stream is returned as it is, with `first` dropped.
pub fn on_first_chunk(r: Response, first: impl FnOnce() + 'static) -> Response {
    let worker::ResponseBody::Stream(raw) = r.body().clone() else { return r };
    let (status, headers) = (r.status_code(), r.headers().clone());
    let mut first = Some(first);
    let chunks = futures_util::StreamExt::map(wasm_streams::ReadableStream::from_raw(raw).into_stream(), move |c| {
        if let Some(f) = first.take() {
            f();
        }
        c
    });
    let body = wasm_streams::ReadableStream::from_stream(chunks).into_raw();
    worker::ResponseBuilder::new().with_status(status).with_headers(headers).stream(body)
}

/// A container response's body put to `b` under `key` through a `FixedLengthStream` of its `content-length`, so the
/// bytes never enter wasm memory and R2 stores the object only if the whole length arrived. `false` when the
/// response names no length or is not a stream.
async fn put_body(b: &R2, key: &str, r: Response) -> Result<bool, StoreError> {
    let Some(len) = r.headers().get("content-length").map_err(worker_err)?.and_then(|v| v.parse::<u64>().ok()) else {
        return Ok(false);
    };
    let worker::ResponseBody::Stream(src) = r.body().clone() else { return Ok(false) };
    let fixed = match u32::try_from(len) {
        Ok(n) => worker::worker_sys::FixedLengthStream::new(n),
        Err(_) => worker::worker_sys::FixedLengthStream::new_big_int(js_sys::BigInt::from(len)),
    }
    .map_err(|e| err(worker::Error::from(e)))?;
    let piped = src.pipe_to(&fixed.writable());
    worker::wasm_bindgen_futures::spawn_local(async move {
        let _ = worker::wasm_bindgen_futures::JsFuture::from(piped).await;
    });
    js(b.raw().put(key.to_owned(), fixed.readable().into(), js_sys::Object::new().into())).await?;
    Ok(true)
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
#[serde(rename_all = "camelCase")]
struct RepoOut {
    network: String,
    bytes: u64,
    used: u64,
    // A container on the previous release's image (`git_prev`) reports none of these.
    #[serde(default)]
    pack_bytes: u64,
    #[serde(default)]
    fetched: u64,
    #[serde(default)]
    pushed: u64,
    #[serde(default)]
    caught_up: u64,
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

/// `ShardHost::validate`'s answer: the pack's index (none for a detail-only call), each update's detail, and the
/// container's times (`ValidateMs`), or the
/// container's slots stayed full past an agent's wait (HTTP 429), to be retried after `retry_after_s`, or, checked
/// on the disk as it stands, a base or ref the disk does not hold yet: the container kept the request as `held`, to
/// be checked again once the shard caught up (`OnDisk::Held`).
#[derive(Debug)]
pub enum Validated {
    Done(Option<PackIndex>, Vec<crate::agent::RefDetail>, ValidateMs),
    Busy { retry_after_s: u32 },
    MissingBase { held: String },
}

/// Where a validate's time went in the container: the wait for the network lock shared, `index-pack` (0 for detail
/// only), and the connectivity walk and the detail after it. Zero from an image that does not report them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ValidateMs {
    pub lock_wait: u64,
    pub index: u64,
    pub walk: u64,
}

/// Which disk a validate checks against: one the shard just caught up, the disk as it stands (`disk=1`), whose
/// missing base is `Validated::MissingBase`, or a request the container kept on one, checked again after a
/// catch-up with no body (`held=`). Every failure but a missing base on `AsItStands` is the pack's.
#[derive(Clone, Copy, Debug)]
pub enum OnDisk<'a> {
    CaughtUp,
    AsItStands,
    Held(&'a str),
    /// A kept request checked again on the disk as it stands, between catch-up pieces: a base still missing keeps it
    /// again, under a new id (`MissingBase`), and every other failure is the pack's, as on `AsItStands`.
    HeldOnDisk(&'a str),
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
    #[serde(default)]
    lock_wait_ms: u64,
    #[serde(default)]
    index_ms: u64,
    #[serde(default)]
    walk_ms: u64,
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

    /// Bucket object `key`'s body streamed to the container's `path`, never in the isolate's memory: the container's
    /// answer, or `None` when the bucket has no such object.
    async fn put_stream(&self, b: &R2, key: &str, method: Method, path: &str) -> Result<Option<Response>, StoreError> {
        let Some(o) = b.0.get(key).execute().await.map_err(err)? else { return Ok(None) };
        let Some(body) = o.body() else { return Ok(None) };
        let worker::ResponseBody::Stream(st) = body.response_body().map_err(worker_err)? else { return Ok(None) };
        let mut init = RequestInit::new();
        init.with_method(method).with_body(Some(st.into()));
        let req = Request::new_with_init(&format!("http://container{path}"), &init).map_err(worker_err)?;
        Ok(Some(self.port()?.fetch_request(req).await.map_err(worker_err)?))
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

    /// The container's running git processes (`GET /procs`: command, nice, CPU seconds, RSS), most CPU first, and the
    /// network's pack counts (all and kept), as JSON for `/status`. A container that is not running lists none and is
    /// not started for it.
    pub async fn procs(&self, network: NetworkId) -> Result<serde_json::Value, StoreError> {
        if !self.container()?.running() {
            return Ok(serde_json::Value::Null);
        }
        let mut r = self.ok(Method::Get, &format!("/procs?net={}", network.0.0), None).await?;
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
        let image = self.image_value(&c)?;
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

    /// The image map's entry this shard starts. The caller already turned `git_prev` into `git` for a map without it.
    fn image_value(&self, c: &worker::Container) -> Result<wasm_bindgen::JsValue, StoreError> {
        let images = js_sys::Reflect::get(c.as_ref(), &"images".into()).map_err(|_| StoreError("images".into()))?;
        let get = |k: &str| js_sys::Reflect::get(&images, &k.into()).map_err(|_| StoreError("image".into()));
        get(self.image)
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

    /// Records a request and arms the renewal alarm (`renewal`), at most once per TOUCH_EVERY (`touch_due`). Returns
    /// whether it wrote. The shard's `alarm` handler calls `on_alarm`.
    pub async fn touch(&self, now: Timestamp) -> Result<bool, StoreError> {
        let s = self.state.storage();
        let last: Option<u64> = s.get("last_request").await.map_err(worker_err)?;
        if !crate::shard::touch_due(now, last.map(Timestamp)) {
            return Ok(false);
        }
        s.put("last_request", now.0).await.map_err(worker_err)?;
        if let Renewal::Renew { next } = renewal(now, now) {
            s.set_alarm(alarm_at(now, next)).await.map_err(worker_err)?;
        }
        Ok(true)
    }

    /// From the alarm of a shard whose container runs, once a day (`stats_due`): the `shard stats` line of every
    /// repository on its disk (`crate::shard::stats_line`), logged under the object's name. A container that is not
    /// running is not started for it, and a failed listing logs nothing and is tried at the next alarm.
    pub async fn log_stats(&self, now: Timestamp) -> Result<(), StoreError> {
        if !self.container()?.running() {
            return Ok(());
        }
        let s = self.state.storage();
        let last: Option<u64> = s.get("stats_at").await.map_err(worker_err)?;
        if !crate::shard::stats_due(now, last.map(Timestamp)) {
            return Ok(());
        }
        let (disk, repos) = self.repos().await?;
        s.put("stats_at", now.0).await.map_err(worker_err)?;
        let name = self.state.id().name().unwrap_or_else(|| self.state.id().to_string());
        worker::console_log!("shard stats {name}: {}", crate::shard::stats_line(disk, &repos, now));
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
    /// counts as derived work, outside `NetworkLoad`. An error carries the container's status when it answered
    /// one, `None` when the call to it failed (`crate::validate_status`); a 429 is never an error but `Busy`, and a
    /// 409 `missingBase` on `OnDisk::AsItStands` is `MissingBase`. `sums` are the edge's `headsum` and `packsum`,
    /// passed on so the container answers a body changed on the way 502, not the pack's 4xx (G35).
    #[allow(clippy::too_many_arguments)]
    pub async fn validate(
        &self,
        network: NetworkId,
        tipsn: usize,
        detailn: usize,
        agent: bool,
        derived: bool,
        disk: OnDisk<'_>,
        body: Option<wasm_bindgen::JsValue>,
        sums: &[(&str, String)],
    ) -> Result<Validated, (Option<u16>, StoreError)> {
        let none = |e: StoreError| (None, e);
        let mut path = format!("/validate?net={}&tipsn={tipsn}", network.0.0);
        for (k, v) in sums {
            path.push_str(&format!("&{k}={v}"));
        }
        match disk {
            OnDisk::CaughtUp => {}
            OnDisk::AsItStands => path.push_str("&disk=1"),
            OnDisk::Held(id) => path.push_str(&format!("&held={id}")),
            OnDisk::HeldOnDisk(id) => path.push_str(&format!("&held={id}&disk=1")),
        }
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
        let req =
            Request::new_with_init(&format!("http://container{path}"), &init).map_err(worker_err).map_err(none)?;
        let mut r = self.port().map_err(none)?.fetch_request(req).await.map_err(worker_err).map_err(none)?;
        if r.status_code() == 429 {
            let retry = r.headers().get("retry-after").ok().flatten().and_then(|v| v.trim().parse().ok());
            return Ok(Validated::Busy { retry_after_s: retry.unwrap_or(1).max(1) });
        }
        if r.status_code() == 409 && matches!(disk, OnDisk::AsItStands | OnDisk::HeldOnDisk(_)) {
            #[derive(Deserialize)]
            struct Held {
                #[serde(rename = "missingBase")]
                missing_base: bool,
                held: String,
            }
            let t = r.text().await.unwrap_or_default();
            return match serde_json::from_str::<Held>(&t) {
                Ok(h) if h.missing_base => Ok(Validated::MissingBase { held: h.held }),
                _ => Err((Some(409), StoreError(format!("validate: 409 {t}")))),
            };
        }
        if r.status_code() >= 300 {
            let t = r.text().await.unwrap_or_default();
            return Err((Some(r.status_code()), StoreError(format!("validate: {} {t}", r.status_code()))));
        }
        let v: ValidateOut = r.json().await.map_err(worker_err).map_err(none)?;
        let ms = ValidateMs { lock_wait: v.lock_wait_ms, index: v.index_ms, walk: v.walk_ms };
        if v.checksum.is_empty() {
            return Ok(Validated::Done(None, v.detail, ms));
        }
        let ix = PackIndex {
            checksum: Digest::parse_hex(&v.checksum).ok_or_else(|| none(StoreError("checksum".into())))?,
            bytes: v.bytes,
            objects: v.objects,
            trailer: v.trailer.into(),
        };
        Ok(Validated::Done(Some(ix), v.detail, ms))
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
        repack: bool,
        head: Option<&str>,
    ) -> Result<(), StoreError> {
        let body: String = refs.iter().map(|(r, o)| format!("{o} {r}\n")).collect();
        let path = format!(
            "/refs?net={}&start={}&done={}&repack={}{}",
            network.0.0,
            u8::from(start),
            u8::from(done),
            u8::from(repack),
            crate::catchup::head_param(head)
        );
        self.ok(Method::Put, &path, Some(body.into_bytes())).await.map(|_| ())
    }

    /// Applies ref changes (`crate::ingest::ref_changes`) to the container's refs: `POST /refs`, deletes then sets.
    /// An update to an object the shard does not hold is skipped by the container, as a rehydrate's is.
    /// With `head`, HEAD points at it too (the default branch, G26), and with no changes that is all the call does.
    /// Answers the container's wait for the network lock alone, 0 when it reported none.
    pub async fn apply_refs(
        &self,
        network: NetworkId,
        changes: &[(String, Option<String>)],
        head: Option<&str>,
    ) -> Result<u64, StoreError> {
        #[derive(Default, Deserialize)]
        struct Out {
            #[serde(default)]
            lock_wait_ms: u64,
        }
        if changes.is_empty() && head.is_none() {
            return Ok(0);
        }
        let zero = "0".repeat(40);
        let body: String = changes.iter().map(|(r, o)| format!("{} {r}\n", o.as_deref().unwrap_or(&zero))).collect();
        let path = format!("/refs?net={}{}", network.0.0, crate::catchup::head_param(head));
        let mut r = self.ok(Method::Post, &path, Some(body.into_bytes())).await?;
        Ok(r.json::<Out>().await.unwrap_or_default().lock_wait_ms)
    }

    pub async fn fsck(&self, network: NetworkId) -> Result<serde_json::Value, StoreError> {
        let mut r = self.ok(Method::Post, &format!("/fsck?net={}", network.0.0), None).await?;
        r.json().await.map_err(worker_err)
    }

    /// The `.idx` of the pack `/validate` kept aside under `sum`, or `None`, also from an image without `/idx`.
    pub async fn validated_idx(&self, network: NetworkId, sum: Digest) -> Result<Option<Vec<u8>>, StoreError> {
        let mut r =
            self.call(Method::Get, &format!("/idx?net={}&sha256={}", network.0.0, sum.hex()), None, &[]).await?;
        match r.status_code() {
            200 => Ok(Some(r.bytes().await.map_err(worker_err)?)),
            _ => Ok(None),
        }
    }

    /// One complete pack of every object the given ingested packs brought: the home shard's half of `Promoted`. The
    /// container's answer, its body unread, so the pack streams on to the caller and is never held here.
    pub async fn promote(&self, network: NetworkId, sums: &[Digest]) -> Result<Response, StoreError> {
        let body: String = sums.iter().map(|s| format!("{}\n", s.hex())).collect();
        self.ok(Method::Post, &format!("/promote?net={}", network.0.0), Some(body.into_bytes())).await
    }

    /// One complete pack of every object of the given stored promoted packs, oldest first: the home shard's half of
    /// `Compacted` (`decisions/DECISIONS.md`, `roll-up`), streamed on as `promote`'s is.
    pub async fn roll(&self, network: NetworkId, sums: &[Digest]) -> Result<Response, StoreError> {
        let body: String = sums.iter().map(|s| format!("{}\n", s.hex())).collect();
        self.ok(Method::Post, &format!("/roll?net={}", network.0.0), Some(body.into_bytes())).await
    }

    /// The test merge of `head` into `base` (`GitCacheShard::merge` without the plan): the container's `/merge`, which
    /// runs merge-tree, commit-tree, and the tree-diff check in a quarantine and leaves the repository unchanged. With
    /// the answer, the CPU ms of the merge's own git processes (`cpuMs`), which TEST_MERGE_BUDGET is charged. `None`
    /// when base or head, or the whole repository, is not on the disk (`crate::merge_cache::needs_catch_up`).
    pub async fn merge(
        &self,
        network: NetworkId,
        base: crate::refs::ObjectId,
        head: crate::refs::ObjectId,
    ) -> Result<(Option<crate::prsync::MergeAnswer>, Option<f32>), StoreError> {
        let path = format!("/merge?net={}&base={}&head={}", network.0.0, base.hex(), head.hex());
        let mut r = self.call(Method::Post, &path, None, &[]).await?;
        match r.status_code() {
            c if crate::merge_cache::needs_catch_up(c) => return Ok((None, None)),
            c if c >= 300 => return Err(StoreError(format!("{path}: {c} {}", r.text().await.unwrap_or_default()))),
            _ => {}
        }
        let v: serde_json::Value = r.json().await.map_err(worker_err)?;
        let cpu = v["cpuMs"].as_f64().map(|c| c as f32);
        Ok((Some(serde_json::from_value(v).map_err(|e| StoreError(format!("{path}: {e}")))?), cpu))
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

    /// A depth-1 fill of the clone cache (`spool=<key>`): the container answers the client as `upload_pack` does and
    /// writes the whole answer to its disk, to its end whoever reads it; `spooled` then reads it.
    /// With `compose_only`, the container answers only from the composed-miss chain, and 409 when it would serve stock.
    pub async fn upload_pack_spooled(
        &self,
        network: NetworkId,
        body: Vec<u8>,
        git_protocol: &str,
        content_encoding: &str,
        spool: Option<Digest>,
        compose_only: bool,
    ) -> Result<Response, StoreError> {
        let mut h = vec![("git-protocol", git_protocol)];
        if !content_encoding.is_empty() {
            h.push(("content-encoding", content_encoding));
        }
        let mut path = format!("/upload-pack?net={}", network.0.0);
        if let Some(s) = spool {
            path.push_str(&format!("&spool={}", s.hex()));
        }
        if compose_only {
            path.push_str("&compose=only");
        }
        self.call(Method::Post, &path, Some(body), &h).await
    }

    /// A composable fetch's chain as a manifest and its tail (`compose=only&tail=1`, `edge-compose`): the container's
    /// answer, 200 with `x-graft-compose: tail` and the bytes whole, or the same answers `compose=only` gives (409 with
    /// its reason; a composed pack, spooled under `spool`, when the tail passes the container's cap or the container is
    /// on an image without it).
    pub async fn upload_pack_tail(
        &self,
        network: NetworkId,
        body: Vec<u8>,
        git_protocol: &str,
        spool: Option<Digest>,
    ) -> Result<Response, StoreError> {
        let mut path = format!("/upload-pack?net={}&compose=only&tail=1", network.0.0);
        if let Some(s) = spool {
            path.push_str(&format!("&spool={}", s.hex()));
        }
        self.call(Method::Post, &path, Some(body), &[("git-protocol", git_protocol)]).await
    }

    /// The whole answer a spooled upload-pack wrote, put to each of `to` under `key` without passing through the
    /// isolate's memory, then deleted from the container. `Ok(false)` when the container holds no whole answer: the
    /// upload-pack failed, or another fill of the key spooled it.
    pub async fn put_spooled(&self, spool: Digest, to: &[&R2], key: &str) -> Result<bool, StoreError> {
        let path = format!("/spooled?id={}", spool.hex());
        let mut put = false;
        for b in to {
            let r = self.call(Method::Get, &path, None, &[]).await?;
            if r.status_code() != 200 {
                break;
            }
            put |= put_body(b, key, r).await?;
        }
        let _ = self.call(Method::Delete, &path, None, &[]).await;
        Ok(put)
    }

    /// The container's composed-miss chain (`/compose-head`), `None` when it holds none.
    pub async fn chain(&self, network: NetworkId) -> Result<Option<crate::compose::ChainHead>, StoreError> {
        let mut r = self.ok(Method::Get, &format!("/compose-head?net={}", network.0.0), None).await?;
        let h: Option<crate::compose::ChainHead> = r.json().await.map_err(worker_err)?;
        Ok(h.and_then(crate::compose::ChainHead::checked))
    }

    /// The chain's next step on the container, to the repository's HEAD (`/compose?start=1`): a base when there is no
    /// chain, a part, or a fold. Answers the head after it and the files it built.
    pub async fn compose_step(
        &self,
        network: NetworkId,
    ) -> Result<(Option<crate::compose::ChainHead>, Vec<String>), StoreError> {
        #[derive(Deserialize)]
        struct Out {
            head: Option<crate::compose::ChainHead>,
            #[serde(default)]
            built: Vec<String>,
        }
        let path = format!("/compose?net={}&start=1", network.0.0);
        let mut r = self.ok(Method::Post, &path, None).await?;
        let o: Out = r.json().await.map_err(worker_err)?;
        Ok((o.head, o.built))
    }

    /// One chain file from the container to `b` under its `R2Key::Compose`.
    pub async fn store_chain_file(&self, network: NetworkId, b: &R2, file: &str) -> Result<(), StoreError> {
        let r = self.ok(Method::Get, &format!("/compose-file?net={}&name={file}", network.0.0), None).await?;
        put_body(b, &R2Key::Compose { network, file: file.to_owned() }.to_string(), r).await.map(|_| ())
    }

    /// One chain file from the first of `buckets` that holds it, streamed to the container's staging; `false` when
    /// none does.
    pub async fn fetch_chain_file(&self, network: NetworkId, buckets: &[&R2], file: &str) -> Result<bool, StoreError> {
        let key = R2Key::Compose { network, file: file.to_owned() }.to_string();
        let path = format!("/compose-file?net={}&name={file}", network.0.0);
        for b in buckets {
            if let Some(r) = self.put_stream(b, &key, Method::Put, &path).await?
                && r.status_code() == 200
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Readies the network's composed-miss chain (`crate::compose`). The builder, the home site's first shard, takes
    /// the container's next step to HEAD (a base when it holds no chain, at nice 19 in the container), then brings each of `buckets`, the warm sites',
    /// up to its chain: the files that bucket's head lacks, then the head, and after a fold it deletes the files the
    /// new head does not name; `synced`, the head it last put to the buckets, skips that when nothing moved. Any other
    /// shard installs the chain the first of `buckets` names, fetching only the files its container lacks; one missing
    /// file leaves its chain as it was, so its misses go to stock upload-pack. Answers the chain the container holds.
    pub async fn ready_chain(
        &self,
        network: NetworkId,
        buckets: &[&R2],
        builder: bool,
        synced: Option<&crate::compose::ChainHead>,
    ) -> Result<Option<crate::compose::ChainHead>, StoreError> {
        use crate::compose::HeadObject;
        let head_key = R2Key::Compose { network, file: "head".into() }.to_string();
        let read_head = async |b: &R2| -> Result<Option<HeadObject>, StoreError> {
            let v = Bucket::get(b, &head_key).await?;
            let Some(h) = v.and_then(|v| serde_json::from_slice::<HeadObject>(&v).ok()) else { return Ok(None) };
            Ok(h.chain.clone().checked().map(|chain| HeadObject { chain, packs: h.packs }))
        };
        if builder {
            let (Some(head), _) = self.compose_step(network).await? else { return Ok(None) };
            if synced == Some(&head) {
                return Ok(Some(head));
            }
            for b in buckets {
                let old = read_head(b).await?;
                let Some(fetch) = HeadObject::plan(old.as_ref(), &head) else { continue };
                for f in &fetch {
                    self.store_chain_file(network, b, f).await?;
                }
                // `head-etags`: each pack's object as this bucket holds it after the puts, so an edge takes its header
                // from the head and checks none. A pack this step did not put keeps the old head's record, which named
                // the same object; any other is read back once. One that cannot be read leaves the head without packs,
                // and the edge checks every pack as before.
                let mut packs = Vec::new();
                for (p, kept) in HeadObject::carried(old.as_ref(), &head, &fetch) {
                    let key = R2Key::Compose { network, file: format!("{p}.pack") }.to_string();
                    let Some(k) = (match kept {
                        Some(k) => Some(k),
                        None => pack_object(b, &key).await?,
                    }) else {
                        break;
                    };
                    packs.push(k);
                }
                let object = HeadObject::new(head.clone(), packs);
                b.0.put(&head_key, serde_json::to_vec(&object).map_err(err)?).execute().await.map_err(err)?;
                if old.is_some_and(|o| o.chain.base != head.base) {
                    let keep = head.files();
                    let stale: Vec<String> = Bucket::list(*b, &format!("compose/{}/", network.0.0))
                        .await?
                        .into_iter()
                        .map(|o| o.key)
                        .filter(|k| k != &head_key && !keep.iter().any(|f| k.ends_with(&format!("/{f}"))))
                        .collect();
                    Bucket::delete(*b, &stale).await?;
                }
            }
            return Ok(Some(head));
        }
        let mut head = None;
        for b in buckets {
            if let Some(h) = read_head(b).await? {
                head = Some(h.chain);
                break;
            }
        }
        let Some(head) = head else { return Ok(None) };
        let had = self.chain(network).await?;
        if had.as_ref() == Some(&head) {
            return Ok(had);
        }
        for f in head.to_fetch(had.as_ref()) {
            if !self.fetch_chain_file(network, buckets, &f).await? {
                return Ok(had);
            }
        }
        Ok(if self.install_chain(network, &head).await? { Some(head) } else { had })
    }

    /// Installs `head` on the container once its files are staged; `false` when the container still lacks one.
    pub async fn install_chain(
        &self,
        network: NetworkId,
        head: &crate::compose::ChainHead,
    ) -> Result<bool, StoreError> {
        let body = serde_json::to_vec(head).map_err(err)?;
        let r = self.call(Method::Put, &format!("/compose-head?net={}", network.0.0), Some(body), &[]).await?;
        Ok(r.status_code() == 200)
    }

    /// `git archive` of one commit of `network` (`codeload-archives`).
    pub async fn archive(&self, network: NetworkId, query: &str) -> Result<Response, StoreError> {
        self.call(Method::Get, &format!("/archive?net={}&{query}", network.0.0), None, &[]).await
    }

    /// One of the web app's read-only routes (`/ui/resolve`, `/ui/tree`, `/ui/blame`, `/ui/diff`, `/ui/compare`, `/ui/commits`,
    /// `/ui/readme`; `containers/git/server/ui.go`) on `network`, with the caller's query less `repo`. The container's
    /// status and JSON pass through unchanged.
    pub async fn ui(&self, network: NetworkId, route: &str, query: &str) -> Result<Response, StoreError> {
        self.call(Method::Get, &format!("/ui/{route}?net={}&{query}", network.0.0), None, &[]).await
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
                let at = |t: u64| (t > 0).then_some(Timestamp(t));
                Some(DiskRepo {
                    network: NetworkId(graftyard_types::DbId(o.network.parse().ok()?)),
                    bytes: o.bytes,
                    used: Timestamp(o.used),
                    pack_bytes: o.pack_bytes,
                    fetched: at(o.fetched),
                    pushed: at(o.pushed),
                    caught_up: at(o.caught_up),
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
    /// time, stored packs streamed from a bucket to the container, and every page saved before the next. One call is
    /// at most `piece` (`crate::catchup::PIECE` on the site, `WHOLE` for one call to the head); `Report::behind` says
    /// another follows, toward `until`.
    ///
    /// `buckets` is read in the order given for a stored pack (`crate::catchup::own_first`: the shard's own site's
    /// first), and all of them for the markers.
    pub async fn catch_up(
        &self,
        network: NetworkId,
        chains: &[ChainId],
        buckets: &[(SiteId, &R2)],
        log: &impl ChainLog,
        piece: crate::catchup::Piece,
        until: Option<&crate::shard::IngestCursor>,
    ) -> Result<Report, StoreError> {
        let io = Io { host: self, network, buckets, stored: Default::default(), direct: Default::default() };
        let mut r =
            crate::catchup::catch_up(&io, &io, log, network, chains, piece, until, &|| js_sys::Date::now() as u64)
                .await?;
        r.stored = io.stored.take();
        Ok(r)
    }

    /// Whether the disk holds a cursor for `network`, so a catch-up reads from the log rather than rehydrating.
    pub async fn warm(&self, network: NetworkId) -> Result<bool, StoreError> {
        if !self.container()?.running() {
            return Ok(false);
        }
        let disk = self.health().await?.disk;
        Ok(self.cursor(network, &disk).await?.is_some())
    }

    /// Whether a rehydrate of `network` is under way or was cut off on this shard: its saved place is kept from its
    /// start until its last catch-up reaches the head, or until the catch-up that finishes a cut-off one does
    /// (`crate::catchup`). One storage read; the container is not asked.
    pub async fn rehydrating(&self, network: NetworkId) -> bool {
        type Saved = (String, serde_json::Value);
        self.state.storage().get::<Saved>(&Self::rehydrating_key(network)).await.ok().flatten().is_some()
    }

    fn rehydrating_key(network: NetworkId) -> String {
        format!("rehydrating:{}", network.0.0)
    }

    /// Ingests one pack's bytes: `Later` when the container answers 409, `None` for bytes that do not match `sum`.
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
    buckets: &'a [(SiteId, &'a R2)],
    stored: std::cell::RefCell<StoredMs>,
    /// Stored packs a container on the previous release's image (`git_prev`, no `/spool`) takes from the bucket at
    /// their turn instead, by checksum.
    direct: std::cell::RefCell<BTreeMap<Digest, PackId>>,
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
    /// One `/ingest-batch` call: per pack a `<sha256> <bytes>` line and its bytes, written straight into one JS array,
    /// so the isolate holds no second copy of the batch in its own memory.
    async fn ingest(&self, packs: &[(Digest, &[u8])]) -> Result<Vec<Ingested>, StoreError> {
        #[derive(Deserialize)]
        struct Out {
            results: Vec<String>,
            #[serde(default)]
            lock_wait_ms: u64,
            #[serde(default)]
            midx_ms: u64,
        }
        let heads: Vec<String> = packs.iter().map(|(s, b)| format!("{} {}\n", s.hex(), b.len())).collect();
        let total: usize = packs.iter().zip(&heads).map(|((_, b), h)| h.len() + b.len()).sum();
        let body = js_sys::Uint8Array::new_with_length(total as u32);
        let mut at = 0u32;
        for ((_, b), h) in packs.iter().zip(&heads) {
            for part in [h.as_bytes(), b] {
                body.subarray(at, at + part.len() as u32).copy_from(part);
                at += part.len() as u32;
            }
        }
        let path = format!("/ingest-batch?net={}", self.network.0.0);
        let mut init = RequestInit::new();
        init.with_method(Method::Post).with_body(Some(body.into()));
        let req = Request::new_with_init(&format!("http://container{path}"), &init).map_err(worker_err)?;
        let mut r = self.host.port()?.fetch_request(req).await.map_err(worker_err)?;
        // A container on the previous release's image (`git_prev`) has no `/ingest-batch`: one `/ingest` a pack.
        if r.status_code() == 404 {
            let mut out = Vec::with_capacity(packs.len());
            for (sum, b) in packs {
                let b = JsBody(js_sys::Uint8Array::from(*b).into());
                out.push(self.host.ingest_body(self.network, *sum, b).await?.ok_or_else(|| {
                    StoreError(format!("ingest {}: the bytes do not match their checksum", sum.hex()))
                })?);
            }
            return Ok(out);
        }
        if r.status_code() != 200 {
            return Err(StoreError(format!("{path}: {} {}", r.status_code(), r.text().await.unwrap_or_default())));
        }
        let o: Out = r.json().await.map_err(worker_err)?;
        {
            // The batch takes the lock alone.
            let mut st = self.stored.borrow_mut();
            st.lock_wait_ms += o.lock_wait_ms;
            st.lock_excl_wait_ms += o.lock_wait_ms;
            st.midx_ms += o.midx_ms;
        }
        o.results
            .iter()
            .map(|a| match a.as_str() {
                "done" => Ok(Ingested::Done),
                "later" => Ok(Ingested::Later),
                a => Err(StoreError(format!("{path}: answered {a:?}"))),
            })
            .collect()
    }
    /// From each bucket in turn, in the order the caller gave (its own site's first), its object's body streamed into
    /// the container's `/spool`, which checks the checksum as it writes: a copy that does not match is refused, and the
    /// next bucket's is tried. With `idx`, the stored `.idx` from the same bucket goes to `/spool-idx`; a missing one
    /// leaves the ingest to `index-pack`. Each pack's source and spool time go into `stored`.
    async fn fetch_stored(&self, id: PackId, checksum: Digest, idx: bool) -> Result<Fetched, StoreError> {
        #[derive(Deserialize)]
        struct Spool {
            #[serde(default)]
            have: bool,
            #[serde(default)]
            bytes: u64,
            #[serde(default)]
            spool_ms: u64,
        }
        let key = R2Key::Pack { network: self.network, id }.to_string();
        let idx_key = R2Key::Idx { network: self.network, id }.to_string();
        let net = self.network.0.0;
        let start = js_sys::Date::now() as u64;
        let got = crate::catchup::first_copy(self.buckets, |b: &&R2| {
            let (b, key, idx_key): (&R2, String, String) = (b, key.clone(), idx_key.clone());
            async move {
                let path = format!("/spool?net={net}&sha256={}", checksum.hex());
                let Some(mut r) = self.host.put_stream(b, &key, Method::Post, &path).await? else { return Ok(None) };
                let s: Spool = match r.status_code() {
                    200 => r.json().await.map_err(worker_err)?,
                    // The previous release's image: the pack is read from the bucket again at its turn.
                    404 | 405 => {
                        self.direct.borrow_mut().insert(checksum, id);
                        return Ok(Some(Spool { have: false, bytes: 0, spool_ms: 0 }));
                    }
                    422 if r.text().await.unwrap_or_default().starts_with("checksum") => return Ok(None),
                    c => return Err(StoreError(format!("{path}: {c} {}", r.text().await.unwrap_or_default()))),
                };
                if idx && !s.have {
                    // Streamed as the pack is, so `FETCH_AT_ONCE` fetches hold none of their bytes in the isolate:
                    // from this bucket or, when its pack put finished after the placement returned, from the first
                    // other that holds one. Derived: without it the container runs `index-pack`, so a failed read or
                    // put is not an error.
                    let path = format!("/spool-idx?net={net}&sha256={}", checksum.hex());
                    for o in std::iter::once(b).chain(self.buckets.iter().map(|(_, o)| *o)) {
                        if self.host.put_stream(o, &idx_key, Method::Put, &path).await.unwrap_or(None).is_some() {
                            break;
                        }
                    }
                }
                Ok(Some(s))
            }
        })
        .await?;
        let Some((site, s)) = got else {
            return Err(StoreError(format!("{key}: no bucket holds a copy matching its checksum")));
        };
        let mut st = self.stored.borrow_mut();
        *st.from_site.entry(site.0).or_default() += 1;
        st.spool_ms += s.spool_ms;
        st.bytes += s.bytes;
        if !s.have && st.spools.len() < crate::catchup::SPOOLS_MAX {
            st.spools.push([start, js_sys::Date::now() as u64, s.bytes, u64::from(site.0)]);
        }
        Ok(if s.have { Fetched::Have } else { Fetched::Spooled })
    }
    /// `/ingest&spooled=1`: the container's answer, with its times and whether it used the stored `.idx`.
    async fn ingest_spooled(&self, checksum: Digest) -> Result<Ingested, StoreError> {
        #[derive(Default, Deserialize)]
        struct Out {
            #[serde(default)]
            index_ms: u64,
            #[serde(default)]
            lock_ms: u64,
            #[serde(default)]
            midx_ms: u64,
            #[serde(default)]
            lock_wait_ms: u64,
            #[serde(default)]
            lock_excl_wait_ms: u64,
            #[serde(default)]
            idx: String,
        }
        let t0 = js_sys::Date::now() as u64;
        let direct = self.direct.borrow().get(&checksum).copied();
        if let Some(id) = direct {
            let key = R2Key::Pack { network: self.network, id }.to_string();
            let got = crate::catchup::first_copy(self.buckets, |b: &&R2| {
                let (b, key): (&R2, String) = (b, key.clone());
                async move {
                    let Some(o) = b.0.get(&key).execute().await.map_err(err)? else { return Ok(None) };
                    let Some(body) = o.body() else { return Ok(None) };
                    let worker::ResponseBody::Stream(st) = body.response_body().map_err(worker_err)? else {
                        return Ok(None);
                    };
                    self.host.ingest_body(self.network, checksum, JsBody(st.into())).await
                }
            })
            .await?;
            let (_, a) =
                got.ok_or_else(|| StoreError(format!("{key}: no bucket holds a copy matching its checksum")))?;
            if a == Ingested::Done {
                self.direct.borrow_mut().remove(&checksum);
                self.stored.borrow_mut().packs += 1;
            }
            return Ok(a);
        }
        let path = format!("/ingest?net={}&sha256={}&spooled=1", self.network.0.0, checksum.hex());
        let mut r = self.host.call(Method::Post, &path, None, &[]).await?;
        let (a, o) = match r.status_code() {
            200 => (Ingested::Done, r.json::<Out>().await.unwrap_or_default()),
            409 => (Ingested::Later, Out::default()),
            c => return Err(StoreError(format!("{path}: {c} {}", r.text().await.unwrap_or_default()))),
        };
        let mut st = self.stored.borrow_mut();
        if a == Ingested::Done {
            st.packs += 1;
        }
        st.total_ms += js_sys::Date::now() as u64 - t0;
        st.index_ms += o.index_ms;
        st.lock_ms += o.lock_ms;
        st.midx_ms += o.midx_ms;
        st.lock_wait_ms += o.lock_wait_ms;
        st.lock_excl_wait_ms += o.lock_excl_wait_ms;
        match o.idx.as_str() {
            "used" => st.idx_used += 1,
            "rejected" => st.idx_rejected += 1,
            _ => {}
        }
        Ok(a)
    }
    /// None from a container on the previous release's image (`git_prev`), which has no `/connected` to check a
    /// rehydrate that skipped packs, so it loads every one as before.
    async fn idx_ranges(&self) -> Result<BTreeMap<PackId, Option<IdxRange>>, StoreError> {
        let mut out = BTreeMap::new();
        if self.host.image != "git" {
            return Ok(out);
        }
        for (_, b) in self.buckets {
            for o in b.list(&R2Key::pack_prefix(self.network)).await? {
                if let Ok(R2Key::Idx { id, .. }) = o.key.parse::<R2Key>() {
                    let r = idx_range(&o.meta);
                    let e = out.entry(id).or_insert(r);
                    if e.is_none() {
                        *e = r;
                    }
                }
            }
        }
        Ok(out)
    }
    async fn connected(&self, repack: bool) -> Result<Result<(), String>, StoreError> {
        #[derive(Deserialize)]
        struct Out {
            connected: bool,
            #[serde(default)]
            error: String,
        }
        let mut r = self
            .host
            .ok(
                Method::Post,
                &format!("/connected?net={}&objects=tips&repack={}", self.network.0.0, u8::from(repack)),
                None,
            )
            .await?;
        let o: Out = r.json().await.map_err(worker_err)?;
        Ok(if o.connected { Ok(()) } else { Err(o.error) })
    }
    async fn put_refs_page(
        &self,
        refs: &[(String, String)],
        start: bool,
        done: bool,
        repack: bool,
        head: Option<&str>,
    ) -> Result<(), StoreError> {
        self.host.put_refs_page(self.network, refs, start, done, repack, head).await
    }
    async fn apply_refs(&self, changes: &[(String, Option<String>)], head: Option<&str>) -> Result<(), StoreError> {
        let w = self.host.apply_refs(self.network, changes, head).await?;
        self.stored.borrow_mut().refs_lock_wait_ms += w;
        Ok(())
    }
    async fn make_room(&self, incoming: u64) -> Result<Vec<i64>, StoreError> {
        Ok(self.host.make_room(self.network, incoming, None).await?.evicted)
    }
    async fn markers(&self) -> Result<Vec<(PackId, Marker)>, StoreError> {
        let mut out = Vec::new();
        for (_, b) in self.buckets {
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
