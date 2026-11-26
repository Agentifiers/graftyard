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
use std::cell::RefCell;
use std::rc::Rc;

use futures_util::stream::LocalBoxStream;
use graftyard_actions::artifacts::{self as art, ArtifactRow, Chunk, Container, FileRecord, Refused};
use graftyard_actions::runsvc::{Grant, Urls};
use graftyard_actions::shard::WorkflowRun;
use graftyard_actions::zip::Composer;
use graftyard_actions::{JobId, RunId, actions_shard};
use graftyard_identity::{Actor, Credential, Decision, Principal};
use graftyard_types::{ChainId, DbId, RepoId};
use graftyard_wire::Reply;
use serde_json::{Value, json};
use worker::*;

use crate::Ctx;
use crate::api::Read;

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

/// The job's run, from the shard's fold: its id is the job row's, and its repository the run's.
pub(crate) async fn job_run(c: &Ctx<'_>, chain: ChainId, job: JobId) -> std::result::Result<WorkflowRun, String> {
    #[cfg(feature = "test-faults")]
    if bed_twirp(c.env) {
        return Ok(bed_run());
    }
    let row = crate::actions::job_row(c, chain, job).await.ok_or("no such job")?;
    let v = crate::actions::shard_read(c, chain, json!({ "run": row.run.0.0 })).await?;
    let run: WorkflowRun = serde_json::from_value(v["run"]["run"].clone()).map_err(|e| e.to_string())?;
    if run.id != row.run {
        return Err(format!("run {} answered for the job's run {}", run.id.0.0, row.run.0.0));
    }
    Ok(run)
}

pub(crate) async fn home_bucket(c: &Ctx<'_>, chain: ChainId) -> std::result::Result<Bucket, String> {
    #[cfg(feature = "test-faults")]
    if bed_twirp(c.env) {
        return c.env.bucket("BED_ARTIFACTS").map_err(|e| e.to_string());
    }
    let record = crate::actions::shard_record(c, chain).await?;
    c.env.bucket(&format!("BUCKET_{}", record.home.0)).map_err(|e| e.to_string())
}

/// The run service's base URL for the URLs an artifact call answers: the install's, or on a test build with
/// `GRAFTYARD_BED_TWIRP`, the request's own origin, so a local client reaches them.
pub(crate) fn services(c: &Ctx<'_>, req: &Request) -> Result<String> {
    #[cfg(feature = "test-faults")]
    if bed_twirp(c.env) {
        let u = req.url()?;
        return Ok(format!("{}/_services/", u.origin().ascii_serialization()));
    }
    let _ = req;
    Ok(Urls::new(&format!("https://{}", c.cfg.host)).services)
}

/// A test build's v4 bed (tests/edgeboot): every artifact route reads the bucket `BED_ARTIFACTS` and every job token's
/// run is `bed_run`, so the real Twirp, block, and zip routes answer with no shard bound. The token is still verified.
#[cfg(feature = "test-faults")]
fn bed_twirp(env: &Env) -> bool {
    env.var("GRAFTYARD_BED_TWIRP").is_ok_and(|v| v.to_string() == "1")
}

#[cfg(feature = "test-faults")]
fn bed_run() -> WorkflowRun {
    let repo = RepoId(DbId(65_000));
    WorkflowRun {
        id: RunId(DbId(3)),
        repo,
        workflow: "ci.yml".into(),
        event: "push".into(),
        head_sha: [0xab; 20],
        head_ref: "refs/heads/main".into(),
        trigger: (ChainId::Repo(repo), graftyard_types::Index(1)),
        attempt: 1,
        concurrency: None,
        fork: false,
    }
}

/// Every key under `prefix` with its size and custom metadata, all pages.
pub(crate) async fn listing(b: &Bucket, prefix: &str) -> std::result::Result<Vec<(String, u64)>, String> {
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
    let wf = match job_run(c, chain, job).await {
        Ok(x) => x,
        Err(e) => {
            console_error!("artifacts {chain} job {}: {e}", job.0.0);
            return error(503, "the run did not read");
        }
    };
    let (run, repo) = (wf.id, wf.repo);
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
        return pipelines(c, req, &ct, &wf, &urls.services).await;
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
async fn pipelines(
    c: &Ctx<'_>,
    mut req: Request,
    ct: &Container,
    run: &WorkflowRun,
    services: &str,
) -> Result<Response> {
    let Ok(bucket) = home_bucket(c, ct.chain).await else { return error(503, "the artifact store is unavailable") };
    match req.method() {
        Method::Post => {
            let b: Value = serde_json::from_str(&req.text().await.unwrap_or_default()).unwrap_or(Value::Null);
            let Some(name) = b["Name"].as_str() else { return error(400, "Name") };
            if let Err(r) = art::check_name(name) {
                return refusal(r);
            }
            // The retention and creation time the finalize's row takes (F131). The first create of a name in a run
            // keeps them: matrix legs that upload to one artifact each create it.
            if bucket.head(ct.create_key(name)).await?.is_none() {
                let rec = art::create_record(b["RetentionDays"].as_i64(), crate::now().0);
                if bucket.put(ct.create_key(name), rec.to_string().into_bytes()).execute().await.is_err() {
                    return error(503, "the artifact store is unavailable");
                }
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
            // The REST row (F131), after the manifest: a finalize the client retries writes both again, and the row
            // names the same id, so REST never lists an artifact the job's own listing lacks.
            let created = match bucket.get(ct.create_key(&name)).execute().await? {
                Some(o) => match o.body() {
                    Some(b) => serde_json::from_slice::<Value>(&b.bytes().await?).ok(),
                    None => None,
                },
                None => None,
            };
            let row = art::finalized_row(ct, &name, size, created.as_ref(), run, crate::now().0);
            if let Err(e) = put_row(&bucket, ct.chain, ct.repo, &row).await {
                console_error!("artifacts {} row {name}: {e}", ct.prefix());
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
    if req.url()?.path() == "/_bed/artifacts" {
        return bed_rest(req, bucket).await;
    }
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

// ---- REST: `/repos/{o}/{r}/actions/artifacts` (F131) ----
//
// The rows are R2 objects too, `actions/artifacts/<chain>/<repo>/a/<id>`, one per finalized artifact, its JSON in the
// object's custom metadata so a repository's list is one paged R2 list (`decisions/DECISIONS.md`, "`actions-artifacts`").

const DOC: &str = "rest/actions/artifacts";

pub(crate) async fn put_row(
    bucket: &Bucket,
    chain: ChainId,
    repo: RepoId,
    row: &ArtifactRow,
) -> std::result::Result<(), String> {
    let json = serde_json::to_string(row).map_err(|e| e.to_string())?;
    let meta = std::collections::HashMap::from([("row".to_owned(), json.clone())]);
    let key = art::index_key(chain, repo, row.id);
    bucket.put(key, json.into_bytes()).custom_metadata(meta).execute().await.map_err(|e| e.to_string())?;
    Ok(())
}

/// Every row of the repository's artifacts, from one paged list with their custom metadata.
pub(crate) async fn rows(
    bucket: &Bucket,
    chain: ChainId,
    repo: RepoId,
) -> std::result::Result<Vec<ArtifactRow>, String> {
    let (mut out, mut cursor) = (Vec::new(), None);
    loop {
        let mut l = bucket.list().prefix(art::index_prefix(chain, repo)).include(vec![Include::CustomMetadata]);
        if let Some(k) = cursor.take() {
            l = l.cursor(k);
        }
        let page = l.execute().await.map_err(|e| e.to_string())?;
        for o in page.objects() {
            let meta = o.custom_metadata().map_err(|e| e.to_string())?;
            if let Some(row) = meta.get("row").and_then(|r| serde_json::from_str::<ArtifactRow>(r).ok()) {
                out.push(row);
            }
        }
        match page.cursor().filter(|_| page.truncated()) {
            Some(k) => cursor = Some(k),
            None => return Ok(out),
        }
    }
}

pub(crate) async fn row(bucket: &Bucket, chain: ChainId, repo: RepoId, id: i64) -> Result<Option<ArtifactRow>> {
    let Some(o) = bucket.get(art::index_key(chain, repo, id)).execute().await? else { return Ok(None) };
    let Some(b) = o.body() else { return Ok(None) };
    Ok(serde_json::from_slice(&b.bytes().await?).ok())
}

/// Whether `who` may read (`Access::Read`) or delete (`Access::Write`) the repository's artifacts, past the read gate
/// the request already passed: an App token (a job's `GITHUB_TOKEN` among them) needs `actions` at that level, a
/// fine-grained token the same, and a user needs write access to delete, as GHES documents [doc: GHES 3.22 REST,
/// "Artifacts"]. `Some` is the refusal.
fn refused(rd: &Read, c: &Ctx<'_>, who: Option<&Principal>, level: graftyard_identity::Access) -> Option<Reply> {
    let access = graftyard_git::issue_ops::Access { repo: rd.repo.id, owner: rd.repo.owner, core: &rd.core };
    let folds: Vec<&dyn graftyard_identity::AuthzRead> =
        rd.dir.authz.iter().map(|a| a as &dyn graftyard_identity::AuthzRead).collect();
    let needs = [("actions", level)];
    let deny = |d: Decision, who: &str| match d {
        Decision::Allow => None,
        Decision::Forbidden => Some(Reply::error(403, &format!("Resource not accessible by {who}"), DOC)),
        Decision::Hidden => Some(Reply::not_found()),
    };
    if let Some(r) = deny(
        graftyard_identity::token_allows(c.token_fields.borrow().as_ref(), &needs, &access),
        "personal access token",
    ) {
        return Some(r);
    }
    let write = level >= graftyard_identity::Access::Write;
    match who {
        Some(Principal { actor: Actor::App { .. }, credential: Credential::Stateless(claims), .. }) => {
            deny(graftyard_identity::authorize_app(claims, &needs, &folds, &access), "integration")
        }
        Some(p) if write => (graftyard_identity::permission(p, &folds, &access)
            < Some(graftyard_identity::RepoPermission::Write))
        .then(Reply::not_found),
        None if write => Some(Reply::error(401, "Requires authentication", DOC)),
        _ => None,
    }
}

/// The REST artifact routes, or `None` for another tail or while artifacts are off.
pub async fn repo_route(
    c: &Ctx<'_>,
    rd: &Read,
    who: Option<&Principal>,
    method: &Method,
    tail: &[&str],
    query: &str,
    host: &graftyard_wire::Host,
) -> Result<Option<Reply>> {
    let id = |s: &str| s.parse::<i64>().ok().filter(|n| *n > 0);
    let route = match (method, tail) {
        (Method::Get, ["actions", "artifacts"]) => Rest::List(None),
        (Method::Get, ["actions", "runs", run, "artifacts"]) => match id(run) {
            Some(r) => Rest::List(Some(RunId(DbId(r)))),
            None => return Ok(Some(Reply::not_found())),
        },
        (Method::Get, ["actions", "artifacts", a]) => Rest::Get(id(a)),
        (Method::Get, ["actions", "artifacts", a, "zip"]) => Rest::Zip(id(a)),
        (Method::Delete, ["actions", "artifacts", a]) => Rest::Delete(id(a)),
        _ => return Ok(None),
    };
    if !served(c.env) {
        return Ok(None);
    }
    let write = matches!(route, Rest::Delete(_));
    let level = if write { graftyard_identity::Access::Write } else { graftyard_identity::Access::Read };
    if let Some(r) = refused(rd, c, who, level) {
        return Ok(Some(r));
    }
    let (repo, chain) = (rd.repo.id, ChainId::Actions(rd.repo.owner, actions_shard(rd.repo.id)));
    let bucket = c.env.bucket(&format!("BUCKET_{}", rd.repo.record.home.0))?;
    let api = host.api();
    let full_name = format!("{}/{}", rd.at.owner, rd.at.name);
    let x = art::RestCtx { api: &api, full_name: &full_name, repo };
    let now = crate::now().0;
    let store = |e: &str| crate::api::unavailable(&format!("the artifact store: {e}"));
    let one = async |a: Option<i64>| -> Result<std::result::Result<ArtifactRow, Reply>> {
        let Some(a) = a else { return Ok(Err(Reply::not_found())) };
        Ok(row(&bucket, chain, repo, a).await?.ok_or_else(Reply::not_found))
    };
    Ok(Some(match route {
        Rest::List(run) => {
            let q: Vec<(String, String)> =
                url::form_urlencoded::parse(query.as_bytes()).map(|(k, v)| (k.into_owned(), v.into_owned())).collect();
            let name = q.iter().find(|(k, _)| k == "name").map(|(_, v)| v.as_str());
            let page = graftyard_wire::Page::parse(query);
            let all = match rows(&bucket, chain, repo).await {
                Ok(r) => r,
                Err(e) => return Ok(Some(store(&e))),
            };
            let (total, got, more) = art::page_rows(all, name, run, page);
            let artifacts: Vec<Value> = got.iter().map(|a| art::rest_json(&x, a, now)).collect();
            let path = match run {
                Some(r) => format!("/actions/runs/{}/artifacts", r.0.0),
                None => "/actions/artifacts".to_owned(),
            };
            let named: Vec<(&str, &str)> = name.map(|n| ("name", n)).into_iter().collect();
            let last = total.div_ceil(page.per_page).max(1);
            let reply = Reply::json(200, json!({ "total_count": total, "artifacts": artifacts }));
            match graftyard_wire::link(&api, repo.0.0, &path, &named, page, more, Some(last)) {
                Some(l) => reply.with("link", l),
                None => reply,
            }
        }
        Rest::Get(a) => match one(a).await? {
            Ok(a) => Reply::json(200, art::rest_json(&x, &a, now)),
            Err(r) => r,
        },
        // GHES answers 302 to a URL that works for one minute, and 410 once the artifact expired.
        Rest::Zip(a) => match one(a).await? {
            Ok(a) if a.expired(now) => Reply::error(410, "Artifact has expired", DOC),
            Ok(a) => match zip_ticket(
                &Urls::new(&format!("https://{}", c.cfg.host)).services,
                &bucket,
                chain,
                repo,
                a.id,
                now,
            )
            .await
            {
                Ok(url) => Reply::empty(302).with("location", url),
                Err(e) => store(&e),
            },
            Err(r) => r,
        },
        Rest::Delete(a) => match one(a).await? {
            Ok(a) => match delete(&bucket, chain, repo, &a).await {
                Ok(()) => Reply::empty(204),
                Err(e) => store(&e),
            },
            Err(r) => r,
        },
    }))
}

/// A fresh 128-bit name: a zip or upload ticket's.
pub(crate) fn nonce() -> String {
    let mut b = [0u8; 16];
    b[..10].copy_from_slice(&crate::rand10());
    b[10..].copy_from_slice(&crate::rand10()[..6]);
    art::ticket_name(&b)
}

/// A zip URL for artifact `id` that works for one minute: its ticket written, its URL answered.
pub(crate) async fn zip_ticket(
    services: &str,
    bucket: &Bucket,
    chain: ChainId,
    repo: RepoId,
    id: i64,
    now: u64,
) -> std::result::Result<String, String> {
    let nonce = nonce();
    let ticket = json!({ "id": id, "exp": now / 1000 + art::ZIP_URL_LIFE_S }).to_string();
    bucket.put(art::ticket_key(chain, repo, &nonce), ticket.into_bytes()).execute().await.map_err(|e| e.to_string())?;
    Ok(art::zip_url(services, chain, repo, id, &nonce))
}

enum Rest {
    List(Option<RunId>),
    Get(Option<i64>),
    Zip(Option<i64>),
    Delete(Option<i64>),
}

/// Every object of artifact `a` in its run (manifest, create record, records, upload pointers, chunks), then its row,
/// so a delete that stops part way leaves a row that the next delete finishes, and never bytes no row names.
pub(crate) async fn delete(
    bucket: &Bucket,
    chain: ChainId,
    repo: RepoId,
    a: &ArtifactRow,
) -> std::result::Result<(), String> {
    let ct = Container { chain, repo, run: a.run };
    let prefix = ct.prefix();
    let keys: Vec<String> = listing(bucket, &prefix)
        .await?
        .into_iter()
        .map(|(k, _)| k)
        .filter(|k| k.strip_prefix(&prefix).and_then(art::of_artifact) == Some(a.name.as_str()))
        .collect();
    for c in keys.chunks(1000) {
        bucket.delete_multiple(c.to_vec()).await.map_err(|e| e.to_string())?;
    }
    bucket.delete(art::index_key(chain, repo, a.id)).await.map_err(|e| e.to_string())
}

/// `/_services/artifact-zip/<owner>/<shard>/<repo>/<id>/<ticket>`: the artifact's files as one zip, streamed from
/// their chunks (`graftyard_actions::zip::Composer`), for as long as the ticket the REST `zip` route wrote admits it.
/// No bearer token: the ticket, 128 random bits in the URL, is the capability, as GHES's blob URL is.
pub async fn zip(c: &Ctx<'_>, req: &Request, rest: &str) -> Result<Response> {
    if !served(c.env) {
        return error(404, "Not Found");
    }
    if req.method() != Method::Get {
        return error(405, "Method Not Allowed");
    }
    let Some((chain, repo, id, nonce)) = art::parse_zip_path(rest) else { return error(404, "Not Found") };
    let Ok(bucket) = home_bucket(c, chain).await else { return error(503, "the artifact store is unavailable") };
    let ticket = match bucket.get(art::ticket_key(chain, repo, nonce)).execute().await? {
        Some(o) => match o.body() {
            Some(b) => serde_json::from_slice::<Value>(&b.bytes().await?).unwrap_or(Value::Null),
            None => Value::Null,
        },
        None => Value::Null,
    };
    if !art::ticket_admits(&ticket, id, crate::now()) {
        return error(403, "This URL has expired");
    }
    let Some(a) = row(&bucket, chain, repo, id).await? else { return error(404, "Not Found") };
    if a.expired(crate::now().0) {
        return error(410, "Artifact has expired");
    }
    let ct = Container { chain, repo, run: a.run };
    let filename = format!("attachment; filename=\"{}.zip\"", a.name.replace('"', ""));
    // A v4 artifact is the zip its client uploaded: its committed blocks, in order, as stored.
    if let Some(blob) = committed(&bucket, &ct, &a.name).await? {
        let keys = blob.blocks.iter().map(|(p, _)| ct.block_key(&a.name, blob.upload, p)).collect();
        let mut r = stored(bucket, keys, blob.size, false)?;
        r.headers_mut().set("content-type", "application/zip")?;
        r.headers_mut().set("content-disposition", &filename)?;
        return Ok(r);
    }
    let Ok(steps) = plan(&bucket, &ct, &a.name).await else { return error(503, "the artifact store is unavailable") };
    let Some(steps) = steps else { return error(404, "Not Found") };
    let h = Headers::new();
    h.set("content-type", "application/zip")?;
    h.set("content-disposition", &filename)?;
    Ok(Response::from_stream(compose(bucket, steps))?.with_headers(h))
}

/// A v4 artifact's committed blob (`graftyard_actions::twirp::Committed`), if its client committed one.
pub(crate) async fn committed(
    bucket: &Bucket,
    ct: &Container,
    name: &str,
) -> Result<Option<graftyard_actions::twirp::Committed>> {
    let Some(o) = bucket.get(ct.blob_key(name)).execute().await? else { return Ok(None) };
    let Some(b) = o.body() else { return Ok(None) };
    Ok(serde_json::from_slice(&b.bytes().await?).ok())
}

/// One step of a zip: a file's header, one of its chunks, its end, or the zip's end.
enum Step {
    Begin { name: String, gzip: bool, length: u64, size: u64 },
    Chunk(String),
    End,
    Finish,
}

/// The zip's steps, every file's chunks listed and checked to tile it before the first byte; `None` when the artifact
/// is not finalized or a file's chunks do not tile it.
async fn plan(bucket: &Bucket, ct: &Container, name: &str) -> std::result::Result<Option<Vec<Step>>, String> {
    if bucket.head(ct.manifest_key(name)).await.map_err(|e| e.to_string())?.is_none() {
        return Ok(None);
    }
    let base = ct.record_key("");
    let mut paths: Vec<String> = listing(bucket, &ct.records(name))
        .await?
        .into_iter()
        .filter_map(|(k, _)| k.strip_prefix(&base).map(str::to_owned))
        .collect();
    paths.sort();
    let mut steps = Vec::new();
    for path in paths {
        let Some(inside) = art::zip_path(&path, name) else { continue };
        let Some(r) = file_record(bucket, &ct.record_key(&path)).await.map_err(|e| e.to_string())? else { continue };
        steps.push(Step::Begin { name: inside.to_owned(), gzip: r.gzip && r.size > 0, length: r.length, size: r.size });
        if r.size > 0 {
            let prefix = ct.chunk_prefix(r.job, r.upload, &path);
            let mut parts: Vec<(u64, u64, String)> = listing(bucket, &prefix)
                .await?
                .into_iter()
                .filter_map(|(k, n)| art::chunk_start(&prefix, &k).map(|s| (s, n, k)))
                .collect();
            parts.sort();
            let tiled: Vec<(u64, u64)> = parts.iter().map(|(s, n, _)| (*s, *n)).collect();
            if graftyard_actions::cache::tiles(&tiled, r.size).is_err() {
                return Ok(None);
            }
            steps.extend(parts.into_iter().map(|(_, _, k)| Step::Chunk(k)));
        }
        steps.push(Step::End);
    }
    steps.push(Step::Finish);
    Ok(Some(steps))
}

/// The steps as zip bytes: one chunk object open at a time, each passed through the composer as it arrives. A failure
/// ends the body short, which an unzip refuses.
fn compose(bucket: Bucket, steps: Vec<Step>) -> impl futures_util::Stream<Item = Result<Vec<u8>>> + 'static {
    let z = Rc::new(RefCell::new(Some(Composer::new())));
    let zerr = |e: graftyard_actions::zip::ZipError| Error::RustError(format!("zip: {e}"));
    futures_util::stream::iter(steps)
        .then(move |step| {
            let (bucket, z) = (bucket.clone(), z.clone());
            async move {
                type Piece = LocalBoxStream<'static, Result<Vec<u8>>>;
                let once = |b: Vec<u8>| -> Piece { stream::once(async { Ok(b) }).boxed_local() };
                let finished = || Error::RustError("zip: finished".into());
                let key = match step {
                    Step::Chunk(key) => key,
                    other => {
                        let mut comp = z.borrow_mut();
                        let cz = comp.as_mut().ok_or_else(finished)?;
                        return Ok::<Piece, Error>(match other {
                            Step::Begin { name, gzip, length, size } => {
                                once(cz.begin(&name, gzip, length, size).map_err(zerr)?)
                            }
                            Step::End => once(cz.end().map_err(zerr)?),
                            _ => once(comp.take().ok_or_else(finished)?.finish().map_err(zerr)?),
                        });
                    }
                };
                let gone = || Error::RustError("an artifact chunk is gone".into());
                let o = bucket.get(key).execute().await?.ok_or_else(gone)?;
                Ok(o.body()
                    .ok_or_else(gone)?
                    .stream()?
                    .map(move |b| {
                        let b = b?;
                        let mut comp = z.borrow_mut();
                        comp.as_mut().ok_or_else(finished)?.push(&b).map_err(zerr)
                    })
                    .boxed_local())
            }
        })
        .try_flatten()
}

/// A test install's check of the REST half against a local bucket (tests/edgeboot), in a fixed container of the bed
/// bucket `BED_ARTIFACTS`: `PUT ?path=` stores the body as one finalized file of its artifact (in 100,000 B chunks, its
/// `content-encoding: gzip` kept, `x-file-length` its length) with the artifact's manifest and row; `GET` lists the
/// rows, `GET ?zip=<id>` answers the artifact's zip, `GET ?keys=1` lists every object, and `DELETE ?id=` deletes one.
#[cfg(feature = "test-faults")]
pub async fn bed_rest(req: &mut Request, bucket: Bucket) -> Result<Response> {
    use graftyard_types::AccountId;
    let chain = ChainId::Actions(AccountId(DbId(7100001)), 11);
    let ct = Container { chain, repo: RepoId(DbId(65_000)), run: RunId(DbId(41)) };
    let run = WorkflowRun {
        id: ct.run,
        repo: ct.repo,
        workflow: "ci.yml".into(),
        event: "push".into(),
        head_sha: [0xab; 20],
        head_ref: "refs/heads/main".into(),
        trigger: (ChainId::Repo(ct.repo), graftyard_types::Index(1)),
        attempt: 1,
        concurrency: None,
        fork: false,
    };
    let url = req.url()?;
    let q = |k: &str| -> Result<Option<String>> {
        Ok(url.query_pairs().find(|(n, _)| n == k).map(|(_, v)| v.into_owned()))
    };
    match req.method() {
        Method::Put => {
            let Some(path) = q("path")? else { return error(400, "path") };
            let Ok(name) = art::check_item_path(&path) else { return error(400, "path") };
            let gzip = req.headers().get("content-encoding")?.is_some_and(|v| v == "gzip");
            let length: u64 = req.headers().get("x-file-length")?.and_then(|v| v.parse().ok()).unwrap_or(0);
            let bytes = req.bytes().await?;
            let job = JobId(DbId(9));
            for (i, c) in bytes.chunks(100_000).enumerate() {
                bucket.put(ct.chunk_key(job, 1, &path, i as u64 * 100_000), c.to_vec()).execute().await?;
            }
            let r = FileRecord { job, upload: 1, size: bytes.len() as u64, length, gzip };
            bucket.put(ct.record_key(&path), serde_json::to_vec(&r)?).execute().await?;
            bucket.put(ct.manifest_key(name), b"{}".to_vec()).execute().await?;
            let created = art::create_record(q("days")?.and_then(|d| d.parse().ok()), crate::now().0);
            bucket.put(ct.create_key(name), created.to_string().into_bytes()).execute().await?;
            let row = art::finalized_row(&ct, name, length as i64, Some(&created), &run, crate::now().0);
            put_row(&bucket, chain, ct.repo, &row).await.map_err(Error::RustError)?;
            reply(200, &json!({ "id": row.id }))
        }
        Method::Delete => {
            let Some(id) = q("id")?.and_then(|v| v.parse().ok()) else { return error(400, "id") };
            let Some(a) = row(&bucket, chain, ct.repo, id).await? else { return error(404, "Not Found") };
            delete(&bucket, chain, ct.repo, &a).await.map_err(Error::RustError)?;
            reply(204, &Value::Null)
        }
        _ if q("keys")?.is_some() => {
            let keys: Vec<String> = listing(&bucket, art::ARTIFACTS_ROOT)
                .await
                .map_err(Error::RustError)?
                .into_iter()
                .map(|(k, _)| k)
                .collect();
            reply(200, &json!(keys))
        }
        _ => match q("zip")?.and_then(|v| v.parse::<i64>().ok()) {
            Some(id) => {
                let Some(a) = row(&bucket, chain, ct.repo, id).await? else { return error(404, "Not Found") };
                let Some(steps) = plan(&bucket, &ct, &a.name).await.map_err(Error::RustError)? else {
                    return error(404, "Not Found");
                };
                Response::from_stream(compose(bucket, steps))
            }
            None => {
                let all = rows(&bucket, chain, ct.repo).await.map_err(Error::RustError)?;
                reply(200, &serde_json::to_value(all)?)
            }
        },
    }
}
