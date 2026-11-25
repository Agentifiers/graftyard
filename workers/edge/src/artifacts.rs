//! `actions-artifacts` on the edge (`decisions/DECISIONS.md`, "`actions-artifacts`"; `graftyard_actions::artifacts`):
//! legacy pipelines artifacts for `upload-artifact@v3.2.2` and `download-artifact@v3`, as R2 objects in the owner's
//! home bucket with no fold rows.
//!
//! - **`run/_apis/pipelines/workflows/{run}/artifacts`** (the runner's `ACTIONS_RUNTIME_URL` is the run service): `POST`
//!   creates the artifact and answers its container URL, `PATCH ?artifactName=` finalizes it by writing its manifest,
//!   and `GET` lists the run's finalized artifacts.
//! - **`artifacts/<owner>/<shard>/<repo>/<run>?itemPath=`**: `PUT` stores one chunk of one file at its `Content-Range`
//!   start, streamed through a `FixedLengthStream` of exactly that length, under the upload the file's first chunk
//!   started (`artifacts::upload_for`); the chunk that ends the file lists that upload's chunks and, once they tile
//!   the file, writes its record, so the last upload to finish is the one a download reads, whole, and two uploads'
//!   chunks never mix. The replaced upload's chunks are deleted after. `GET` with the artifact's name lists its files, and with a file's
//!   path streams the chunks its record names, with the client's own `content-encoding: gzip` and `encodeBody:
//!   "manual"`, so the stored bytes reach it as sent.
//! - **Who.** Every call carries the job token. The job's run and that run's repository come from the shard's fold,
//!   and a path naming any other run, repository, or shard is 403 (`artifacts::authorize`).
//! - **Off** unless the edge's `GRAFTYARD_ARTIFACTS` is `1`: every route answers 404 then, as on a GHES without
//!   artifact storage.
//!
//! Retention is the bucket's lifecycle rule on `actions/artifacts/` (install configuration, never a request path);
//! a repository delete drops its prefix with its cache (F125). Nothing here calls the Cloudflare API.

use futures_util::{StreamExt as _, TryStreamExt as _, stream};
use graftyard_actions::artifacts::{self as art, Chunk, Container, FileRecord, Refused};
use graftyard_actions::runsvc::{Grant, Urls};
use graftyard_actions::{JobId, RunId};
use graftyard_types::{ChainId, DbId, RepoId};
use serde_json::{Value, json};
use worker::*;

use crate::Ctx;

const PIPELINES: &str = "run/_apis/pipelines/workflows/";
const CONTAINERS: &str = "artifacts/";

/// Whether this edge serves artifacts: only where its `GRAFTYARD_ARTIFACTS` is `1`.
pub fn served(env: &Env) -> bool {
    env.var("GRAFTYARD_ARTIFACTS").is_ok_and(|v| v.to_string() == "1")
}

/// Whether `rest` (after `/_services/`) is an artifact route.
pub fn claims(rest: &str) -> bool {
    rest.starts_with(PIPELINES) || rest.starts_with(CONTAINERS)
}

fn reply(status: u16, v: &Value) -> Result<Response> {
    let r = if v.is_null() { Response::empty()? } else { Response::from_json(v)? };
    Ok(r.with_status(status))
}

fn error(status: u16, message: &str) -> Result<Response> {
    reply(status, &json!({ "message": message, "statusCode": status }))
}

fn refusal(r: Refused) -> Result<Response> {
    match r {
        Refused::Forbidden => error(403, "The job may not reach this artifact container"),
        Refused::NotFound => error(404, "Not Found"),
        Refused::Bad(m) => error(400, m),
    }
}

/// The job's run and the run's repository, from the shard's fold.
async fn job_run(c: &Ctx<'_>, chain: ChainId, job: JobId) -> std::result::Result<(RunId, RepoId), String> {
    let row = crate::actions::job_row(c, chain, job).await.ok_or("no such job")?;
    let v = crate::actions::shard_read(c, chain, json!({ "run": row.run.0.0 })).await?;
    let run: graftyard_actions::shard::WorkflowRun =
        serde_json::from_value(v["run"]["run"].clone()).map_err(|e| e.to_string())?;
    Ok((row.run, run.repo))
}

async fn home_bucket(c: &Ctx<'_>, chain: ChainId) -> std::result::Result<Bucket, String> {
    let record = crate::actions::shard_record(c, chain).await?;
    c.env.bucket(&format!("BUCKET_{}", record.home.0)).map_err(|e| e.to_string())
}

/// Every key under `prefix` with its size and custom metadata, all pages.
async fn listing(b: &Bucket, prefix: &str) -> std::result::Result<Vec<(String, u64)>, String> {
    let (mut out, mut cursor) = (Vec::new(), None);
    loop {
        let mut l = b.list().prefix(prefix.to_owned());
        if let Some(k) = cursor.take() {
            l = l.cursor(k);
        }
        let page = l.execute().await.map_err(|e| e.to_string())?;
        out.extend(page.objects().iter().map(|o| (o.key(), o.size())));
        match page.cursor().filter(|_| page.truncated()) {
            Some(k) => cursor = Some(k),
            None => return Ok(out),
        }
    }
}

fn query(req: &Request, k: &str) -> Result<Option<String>> {
    Ok(req.url()?.query_pairs().find(|(n, _)| n == k).map(|(_, v)| v.into_owned()))
}

/// A request under `/_services/` that `claims` took, with the bearer token's grant.
pub async fn serve(c: &Ctx<'_>, req: Request, rest: &str, grant: Option<(ChainId, Grant)>) -> Result<Response> {
    if !served(c.env) {
        return error(404, "Not Found");
    }
    let Some((chain, Grant::Job { job, .. })) = grant else { return error(401, "Unauthorized") };
    let (run, repo) = match job_run(c, chain, job).await {
        Ok(x) => x,
        Err(e) => {
            console_error!("artifacts {chain} job {}: {e}", job.0.0);
            return error(503, "the run did not read");
        }
    };
    let urls = Urls::new(&format!("https://{}", c.cfg.host));
    if let Some(tail) = rest.strip_prefix(PIPELINES) {
        // `{run}/artifacts`: the run the client names must be its own.
        let Some(named) = tail.strip_suffix("/artifacts").and_then(|r| r.parse::<i64>().ok()) else {
            return error(404, "Not Found");
        };
        let ct = Container { chain, repo, run: RunId(DbId(named)) };
        if let Err(r) = art::authorize(&ct, chain, run, repo) {
            return refusal(r);
        }
        return pipelines(c, req, &ct, &urls.services).await;
    }
    let Some(ct) = rest.strip_prefix(CONTAINERS).and_then(Container::parse) else { return error(404, "Not Found") };
    if let Err(r) = art::authorize(&ct, chain, run, repo) {
        return refusal(r);
    }
    let Some(path) = query(&req, "itemPath")? else { return error(400, "itemPath") };
    match req.method() {
        Method::Put => upload(c, req, &ct, job, &path).await,
        Method::Get => match art::check_item_path(&path) {
            // A file's path: its bytes.
            Ok(_) => download(c, &ct, &path).await,
            // The artifact's name alone: its files.
            Err(_) => items(c, &ct, &path, &urls.services).await,
        },
        _ => error(405, "Method Not Allowed"),
    }
}

/// `POST` create, `PATCH ?artifactName=` finalize, `GET` list.
async fn pipelines(c: &Ctx<'_>, mut req: Request, ct: &Container, services: &str) -> Result<Response> {
    let Ok(bucket) = home_bucket(c, ct.chain).await else { return error(503, "the artifact store is unavailable") };
    match req.method() {
        Method::Post => {
            let b: Value = serde_json::from_str(&req.text().await.unwrap_or_default()).unwrap_or(Value::Null);
            let Some(name) = b["Name"].as_str() else { return error(400, "Name") };
            if let Err(r) = art::check_name(name) {
                return refusal(r);
            }
            // An artifact uploaded to again in the same run keeps its files: v3's container is a file share.
            reply(201, &art::artifact_json(ct, services, name, -1))
        }
        Method::Patch => {
            let Some(name) = query(&req, "artifactName")? else { return error(400, "artifactName") };
            if let Err(r) = art::check_name(&name) {
                return refusal(r);
            }
            let b: Value = serde_json::from_str(&req.text().await.unwrap_or_default()).unwrap_or(Value::Null);
            let size = b["Size"].as_i64().unwrap_or(0);
            let manifest = json!({ "name": name, "size": size }).to_string();
            if let Err(e) = bucket.put(ct.manifest_key(&name), manifest.into_bytes()).execute().await {
                console_error!("artifacts {} manifest {name}: {e}", ct.prefix());
                return error(503, "the artifact store is unavailable");
            }
            reply(200, &art::artifact_json(ct, services, &name, size))
        }
        Method::Get => {
            let Ok(names) = listing(&bucket, &ct.manifests()).await else {
                return error(503, "the artifact store is unavailable");
            };
            let p = ct.manifests();
            let mut value = Vec::new();
            for (key, _) in names {
                let Some(name) = key.strip_prefix(&p) else { continue };
                let size = match bucket.get(&key).execute().await {
                    Ok(Some(o)) => match o.body() {
                        Some(b) => {
                            serde_json::from_slice::<Value>(&b.bytes().await?).ok().and_then(|v| v["size"].as_i64())
                        }
                        None => None,
                    },
                    _ => None,
                };
                value.push(art::artifact_json(ct, services, name, size.unwrap_or(0)));
            }
            reply(200, &json!({ "count": value.len(), "value": value }))
        }
        _ => error(405, "Method Not Allowed"),
    }
}

/// One chunk of one file. The chunk that reaches the file's end writes the record once this job's chunks tile it.
async fn upload(c: &Ctx<'_>, mut req: Request, ct: &Container, job: JobId, path: &str) -> Result<Response> {
    if let Err(r) = art::check_item_path(path) {
        return refusal(r);
    }
    let range = req.headers().get("content-range")?;
    let Some(chunk) = range.as_deref().and_then(art::chunk) else { return error(400, "Content-Range") };
    let gzip = req.headers().get("content-encoding")?.is_some_and(|v| v.eq_ignore_ascii_case("gzip"));
    let length = if gzip {
        match req.headers().get("x-tfs-filelength")?.and_then(|v| v.parse::<u64>().ok()) {
            Some(n) => n,
            None => return error(400, "x-tfs-filelength"),
        }
    } else {
        match chunk {
            Chunk::Bytes { total, .. } => total,
            Chunk::Empty => 0,
        }
    };
    let Ok(bucket) = home_bucket(c, ct.chain).await else { return error(503, "the artifact store is unavailable") };
    let (start, end, total) = match chunk {
        Chunk::Empty => {
            let r = FileRecord { job, upload: 0, size: 0, length, gzip };
            return finish(&bucket, ct, path, r).await;
        }
        Chunk::Bytes { start, end, total } => (start, end, total),
    };
    let pointer = match bucket.get(ct.upload_key(job, path)).execute().await? {
        Some(o) => match o.body() {
            Some(b) => b.text().await?.parse::<u64>().ok(),
            None => None,
        },
        None => None,
    };
    let fresh = u64::from_be_bytes(crate::rand10()[..8].try_into().expect("8 bytes")) >> 1;
    let Some(upload) = art::upload_for(start, pointer, fresh) else {
        return error(409, "no upload of this file was started at its first chunk");
    };
    if start == 0 && bucket.put(ct.upload_key(job, path), upload.to_string().into_bytes()).execute().await.is_err() {
        return error(503, "the artifact store is unavailable");
    }
    let body = FixedLengthStream::wrap(req.stream()?, end - start + 1);
    if let Err(e) = bucket.put(ct.chunk_key(job, upload, path, start), body).execute().await {
        console_error!("artifacts chunk {} {path}: {e}", ct.prefix());
        return error(400, "the chunk's body is not its Content-Range length");
    }
    if end + 1 < total {
        return reply(200, &json!({}));
    }
    let prefix = ct.chunk_prefix(job, upload, path);
    let Ok(list) = listing(&bucket, &prefix).await else { return error(503, "the artifact store is unavailable") };
    let parts: Vec<(u64, u64)> =
        list.iter().filter_map(|(k, n)| art::chunk_start(&prefix, k).map(|s| (s, *n))).collect();
    match art::file_done(job, upload, &parts, total, length, gzip) {
        Ok(r) => finish(&bucket, ct, path, r).await,
        // Chunks still arriving out of order: the one that completes the tiling writes the record. The client sends
        // one file's chunks in order, so this is a retried or missing chunk; the file stays unlisted.
        Err(_) => reply(200, &json!({})),
    }
}

async fn finish(bucket: &Bucket, ct: &Container, path: &str, r: FileRecord) -> Result<Response> {
    let before = file_record(bucket, &ct.record_key(path)).await.ok().flatten();
    let bytes = serde_json::to_vec(&r)?;
    if let Err(e) = bucket.put(ct.record_key(path), bytes).execute().await {
        console_error!("artifacts record {} {path}: {e}", ct.prefix());
        return error(503, "the artifact store is unavailable");
    }
    // The upload this one replaced, which no record names now. A delete lost here leaves bytes for the lifecycle rule.
    if let Some(old) = before.filter(|o| (o.job, o.upload) != (r.job, r.upload) && o.size > 0) {
        let prefix = ct.chunk_prefix(old.job, old.upload, path);
        if let Ok(keys) = listing(bucket, &prefix).await {
            let keys: Vec<String> =
                keys.into_iter().map(|(k, _)| k).filter(|k| art::chunk_start(&prefix, k).is_some()).collect();
            for c in keys.chunks(1000) {
                let _ = bucket.delete_multiple(c.to_vec()).await;
            }
        }
    }
    reply(200, &json!({}))
}

async fn file_record(bucket: &Bucket, key: &str) -> Result<Option<FileRecord>> {
    let Some(o) = bucket.get(key).execute().await? else { return Ok(None) };
    let Some(b) = o.body() else { return Ok(None) };
    Ok(serde_json::from_slice(&b.bytes().await?).ok())
}

/// The finalized artifact `name`'s files, as `QueryArtifactResponse`.
async fn items(c: &Ctx<'_>, ct: &Container, name: &str, services: &str) -> Result<Response> {
    if art::check_name(name).is_err() {
        return error(400, "itemPath");
    }
    let Ok(bucket) = home_bucket(c, ct.chain).await else { return error(503, "the artifact store is unavailable") };
    if bucket.head(ct.manifest_key(name)).await?.is_none() {
        return error(404, "Not Found");
    }
    let prefix = ct.records(name);
    let Ok(keys) = listing(&bucket, &prefix).await else { return error(503, "the artifact store is unavailable") };
    let base = ct.record_key("");
    let mut value = Vec::new();
    for (key, _) in keys {
        let Some(path) = key.strip_prefix(&base) else { continue };
        if let Some(r) = file_record(&bucket, &key).await? {
            value.push(art::entry_json(ct, services, path, &r));
        }
    }
    reply(200, &json!({ "count": value.len(), "value": value }))
}

/// A file's bytes as stored: the chunks its record names, in order, with the client's gzip encoding kept.
async fn download(c: &Ctx<'_>, ct: &Container, path: &str) -> Result<Response> {
    let Ok(bucket) = home_bucket(c, ct.chain).await else { return error(503, "the artifact store is unavailable") };
    let name = path.split('/').next().unwrap_or_default();
    if bucket.head(ct.manifest_key(name)).await?.is_none() {
        return error(404, "Not Found");
    }
    let Some(r) = file_record(&bucket, &ct.record_key(path)).await? else { return error(404, "Not Found") };
    let prefix = ct.chunk_prefix(r.job, r.upload, path);
    let Ok(list) = listing(&bucket, &prefix).await else { return error(503, "the artifact store is unavailable") };
    let mut parts: Vec<(u64, u64, String)> =
        list.into_iter().filter_map(|(k, n)| art::chunk_start(&prefix, &k).map(|s| (s, n, k))).collect();
    parts.sort();
    let tiled: Vec<(u64, u64)> = parts.iter().map(|(s, n, _)| (*s, *n)).collect();
    if graftyard_actions::cache::tiles(&tiled, r.size).is_err() {
        return error(404, "Not Found");
    }
    stored(bucket, parts.into_iter().map(|(_, _, k)| k).collect(), r.size, r.gzip)
}

/// `keys`' objects in order as one body of `len` bytes. A gzip body keeps `content-encoding: gzip` and is sent with
/// `encodeBody: "manual"`, which hands the bytes through as stored: with the default, workerd gzips them again
/// (`decisions/DECISIONS.md`, "`actions-artifacts`", 307,964 B stored answered as 308,077 B, measured in local
/// workerd). A plain body keeps the default.
pub fn stored(bucket: Bucket, keys: Vec<String>, len: u64, gzip: bool) -> Result<Response> {
    let h = Headers::new();
    h.set("content-type", "application/octet-stream")?;
    h.set("content-length", &len.to_string())?;
    if gzip {
        h.set("content-encoding", "gzip")?;
    }
    if len == 0 {
        return Ok(Response::empty()?.with_headers(h));
    }
    let body = stream::iter(keys)
        .then(move |key| {
            let bucket = bucket.clone();
            async move {
                let gone = || Error::RustError("an artifact chunk is gone".into());
                let o = bucket.get(key).execute().await?.ok_or_else(gone)?;
                o.body().ok_or_else(gone)?.stream()
            }
        })
        .try_flatten();
    let fixed: worker_sys::FixedLengthStream = FixedLengthStream::wrap(body, len).into();
    let r = Response::from_body(ResponseBody::Stream(fixed.readable()))?.with_headers(h);
    Ok(if gzip { r.with_encode_body(EncodeBody::Manual) } else { r })
}

/// A test install's check that stored gzip bytes leave the edge as stored (tests/edgeboot): `PUT /_bed/gzip` stores
/// the body with its `content-encoding` in a bucket bound as `BED_ARTIFACTS`, and `GET` answers it through `stored`.
#[cfg(feature = "test-faults")]
pub async fn bed(c: &Ctx<'_>, req: &mut Request) -> Result<Response> {
    let bucket = c.env.bucket("BED_ARTIFACTS")?;
    const KEY: &str = "bed/gzip";
    if req.method() == Method::Put {
        let gzip = req.headers().get("content-encoding")?.is_some_and(|v| v == "gzip");
        let bytes = req.bytes().await?;
        let mut meta = std::collections::HashMap::new();
        meta.insert("gzip".to_owned(), gzip.to_string());
        bucket.put(KEY, bytes).custom_metadata(meta).execute().await?;
        return reply(204, &Value::Null);
    }
    let Some(o) = bucket.head(KEY).await? else { return error(404, "Not Found") };
    let gzip = o.custom_metadata()?.get("gzip").is_some_and(|v| v == "true");
    stored(bucket, vec![KEY.to_owned()], o.size(), gzip)
}
