//! `actions-cache` on the edge (`decisions/DECISIONS.md`, "`actions-cache`"; `graftyard_actions::cache`): cache v1 at
//! `/_services/cache/_apis/artifactcache/*`, which `CacheServerUrl` names, and the downloads its `archiveLocation`
//! names, `/_services/cache/download/<owner>/<shard>/<id>?n=&e=&s=`.
//!
//! - **The API** takes the job token (`Grant::Job`) and sends each call to the job's shard leader (`/runsvc`,
//!   `op: "cache_*"`), which answers from its fold or commits. A lookup and a reserve name the repository's default
//!   branch, read here from the repository's `ls-refs` HEAD, which learners and followers serve.
//! - **A `PATCH`** asks the leader whether the job may upload to the entry (`cache_chunk`), then streams the body
//!   through a `FixedLengthStream` of its `Content-Range` length into the owner's home bucket at
//!   `actions/cache/<chain>/<id>/<start>`, so no chunk enters wasm memory. A body of another length fails the put.
//! - **A commit** lists the entry's chunks and sends their `(start, len)`; the leader commits only when they tile the
//!   reserved size.
//! - **A download** carries no token: its query is a capability the leader signed for one entry, its shard and size
//!   (`cache::verify_download`). The edge lists the chunks, checks that they tile that size, and streams them in order,
//!   or the `Range` asked for with 206, with each chunk's slice a ranged `get`. A chunk that does not arrive whole ends
//!   the body short, which the toolkit's length check refuses.
//!
//! - **Off** until the edge's `FORMATS_ENABLED` holds 21.6 (0x15.6): every route then answers 404, as before the cache.
//!
//! Nothing here calls the Cloudflare API.

use futures_util::{StreamExt as _, TryStreamExt as _, stream};
use graftyard_actions::cache::{self, CacheId};
use graftyard_actions::runsvc::Grant;
use graftyard_types::{AccountId, ChainId, DbId, RepoId};
use serde_json::{Value, json};
use worker::*;

use crate::Ctx;

const API: &str = "_apis/artifactcache/";

fn reply(status: u16, v: &Value) -> Result<Response> {
    let r = if v.is_null() { Response::empty()? } else { Response::from_json(v)? };
    Ok(r.with_status(status))
}

/// A refusal with the `message` the toolkit logs (`ReserveCacheError`, and a 400's "over the data cap").
fn error(status: u16, message: &str) -> Result<Response> {
    reply(status, &json!({ "message": message, "statusCode": status }))
}

/// The repository's default branch as a full ref, from its `ls-refs` HEAD on a learner or follower, else on the
/// leader: `refs/heads/main` for a repository with no settings row, as the shard's HEAD is.
async fn default_branch(c: &Ctx<'_>, repo: RepoId) -> std::result::Result<String, String> {
    let r = crate::repo_by_id(c, repo).await?;
    let q = json!({ "q": { "lsRefs": { "reads": [], "head": true } } });
    let b = c.bindings();
    let v = match graftyard_lookup::read_off_leader(&b, ChainId::Repo(repo), &r.record, &q).await? {
        Some((v, _, _)) => v,
        None => graftyard_lookup::call(&b, ChainId::Repo(repo), &r.record, "/read", &q).await?.0,
    };
    let read: graftyard_git::ls_refs::LsRefsRead =
        serde_json::from_value(v["value"].clone()).map_err(|e| e.to_string())?;
    let head = read.head.ok_or("the repository's HEAD did not read")?;
    String::from_utf8(head.target).map_err(|e| e.to_string())
}

/// The repository of the job's run, from the shard.
async fn run_repo(c: &Ctx<'_>, chain: ChainId, job: graftyard_actions::JobId) -> std::result::Result<RepoId, String> {
    let row = crate::actions::job_row(c, chain, job).await.ok_or("no such job")?;
    let v = crate::actions::shard_read(c, chain, json!({ "run": row.run.0.0 })).await?;
    let run: graftyard_actions::shard::WorkflowRun =
        serde_json::from_value(v["run"]["run"].clone()).map_err(|e| e.to_string())?;
    Ok(run.repo)
}

/// One `cache_*` call on the shard's leader: its status and body.
async fn call(c: &Ctx<'_>, chain: ChainId, grant: Grant, op: &str, mut b: Value) -> (u16, Value) {
    b["op"] = json!(op);
    b["grant"] = json!(grant);
    match crate::actions::runsvc_call(c, chain, &b).await {
        Ok((s, v)) => (s, v["body"].clone()),
        Err(e) => {
            console_error!("cache {op} {chain}: {e}");
            (503, json!({ "message": "the cache service is unavailable", "statusCode": 503 }))
        }
    }
}

/// The owner's home bucket, where its job logs and cache chunks live.
async fn home_bucket(c: &Ctx<'_>, chain: ChainId) -> std::result::Result<Bucket, String> {
    let record = crate::actions::shard_record(c, chain).await?;
    c.env.bucket(&format!("BUCKET_{}", record.home.0)).map_err(|e| e.to_string())
}

/// An entry's chunks as `(start, len, key)`, in start order, from every page of its prefix.
async fn chunks(b: &Bucket, prefix: &str) -> std::result::Result<Vec<(u64, u64, String)>, String> {
    let (mut out, mut cursor) = (Vec::new(), None);
    loop {
        let mut l = b.list().prefix(prefix.to_owned());
        if let Some(k) = cursor.take() {
            l = l.cursor(k);
        }
        let listed = l.execute().await.map_err(|e| e.to_string())?;
        out.extend(listed.objects().iter().filter_map(|o| {
            let key = o.key();
            cache::chunk_start(prefix, &key).map(|s| (s, o.size(), key))
        }));
        match listed.cursor().filter(|_| listed.truncated()) {
            Some(k) => cursor = Some(k),
            None => break,
        }
    }
    out.sort();
    Ok(out)
}

/// Whether this edge serves the cache at all: only once its `FORMATS_ENABLED` holds 0x15.6 (21.6). Before that every
/// route, downloads included, answers 404, as a GHES without the cache does, so the edge var is a kill switch that needs
/// no site day.
fn served(enabled: &[graftyard_raft::Format]) -> bool {
    enabled.contains(&graftyard_actions::shard::ACTIONS_FORMAT_CACHE)
}

/// A `CacheId` from the path's `caches/<id>`.
fn path_id(rest: &str) -> Option<CacheId> {
    rest.strip_prefix("caches/")?.parse().ok().map(CacheId)
}

/// `/_services/cache/<tail>`, with the bearer token's grant: the cache API for a job token, or a download.
pub async fn serve(
    c: &Ctx<'_>,
    mut req: Request,
    tail: &str,
    grant: Option<(ChainId, Grant)>,
    public: &[u8; 32],
) -> Result<Response> {
    if !served(&c.cfg.enabled) {
        return error(404, "Not Found");
    }
    if let Some(d) = tail.strip_prefix("download/") {
        return download(c, &req, d, public).await;
    }
    let Some(rest) = tail.strip_prefix(API) else { return error(404, "Not Found") };
    let Some((chain, g @ Grant::Job { job, .. })) = grant else { return error(401, "Unauthorized") };
    let q: Vec<(String, String)> = req.url()?.query_pairs().map(|(k, v)| (k.into_owned(), v.into_owned())).collect();
    let get = |k: &str| q.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone()).unwrap_or_default();
    let (status, body) = match (req.method(), rest) {
        (Method::Get, "cache") => {
            let keys = get("keys");
            let keys: Vec<&str> = keys.split(',').filter(|k| !k.is_empty()).collect();
            if keys.is_empty() || keys.len() > 10 {
                return error(400, "keys");
            }
            let Ok(branch) = branch_of(c, chain, job).await else { return error(503, "the repository did not read") };
            let b = json!({ "keys": keys, "version": get("version"), "default_branch": branch });
            call(c, chain, g, "cache_lookup", b).await
        }
        (Method::Get, "caches") => call(c, chain, g, "cache_list", json!({ "key": get("key") })).await,
        (Method::Post, "caches") => {
            let body: Value = serde_json::from_str(&req.text().await.unwrap_or_default()).unwrap_or(Value::Null);
            let (Some(key), Some(version)) = (body["key"].as_str(), body["version"].as_str()) else {
                return error(400, "key and version are required");
            };
            let Ok(branch) = branch_of(c, chain, job).await else { return error(503, "the repository did not read") };
            // The toolkit skips its own size check on GHES (`isGhes()`), so the leader checks `cacheSize` here.
            let size = body["cacheSize"].as_u64().unwrap_or(0);
            let b = json!({ "key": key, "version": version, "size": size, "default_branch": branch });
            call(c, chain, g, "cache_reserve", b).await
        }
        (Method::Patch, r) => {
            let Some(id) = path_id(r) else { return error(404, "Not Found") };
            return upload(c, &mut req, chain, g, id).await;
        }
        (Method::Post, r) => {
            let Some(id) = path_id(r) else { return error(404, "Not Found") };
            let body: Value = serde_json::from_str(&req.text().await.unwrap_or_default()).unwrap_or(Value::Null);
            let Some(size) = body["size"].as_u64() else { return error(400, "size") };
            let Ok(bucket) = home_bucket(c, chain).await else { return error(503, "the cache store is unavailable") };
            let Ok(list) = chunks(&bucket, &cache::chunk_prefix(chain, id)).await else {
                return error(503, "the cache store is unavailable");
            };
            let parts: Vec<(u64, u64)> = list.iter().map(|(s, n, _)| (*s, *n)).collect();
            let b = json!({ "id": id.0, "size": size, "chunks": parts, "limit": limit(c) });
            call(c, chain, g, "cache_commit", b).await
        }
        _ => return error(404, "Not Found"),
    };
    reply(status, &body)
}

async fn branch_of(c: &Ctx<'_>, chain: ChainId, job: graftyard_actions::JobId) -> std::result::Result<String, String> {
    default_branch(c, run_repo(c, chain, job).await?).await
}

/// `PATCH caches/<id>`: the leader says whether this job may upload to the entry, then the body streams to
/// `chunk_key` through a stream of exactly its `Content-Range` length, so a short or long body fails the put.
async fn upload(c: &Ctx<'_>, req: &mut Request, chain: ChainId, g: Grant, id: CacheId) -> Result<Response> {
    let range = req.headers().get("content-range")?.as_deref().and_then(cache::content_range);
    let Some((s, e)) = range else { return error(400, "Content-Range") };
    let (st, v) = call(c, chain, g, "cache_chunk", json!({ "id": id.0 })).await;
    if st != 200 {
        return reply(st, &v);
    }
    if e >= v["size"].as_u64().unwrap_or(0) {
        return error(400, "the chunk ends past the reserved size");
    }
    let Ok(bucket) = home_bucket(c, chain).await else { return error(503, "the cache store is unavailable") };
    let body = FixedLengthStream::wrap(req.stream()?, e - s + 1);
    match bucket.put(cache::chunk_key(chain, id, s), body).execute().await {
        Ok(_) => reply(204, &Value::Null),
        Err(err) => {
            console_error!("cache chunk {chain} {}: {err}", id.0);
            error(400, "the chunk's body is not its Content-Range length")
        }
    }
}

/// The repository's cache bytes: `CACHE_REPO_MAX`, or below it a test install's `GRAFTYARD_CACHE_LIMIT`, which
/// BF-69's eviction case lowers.
fn limit(_c: &Ctx<'_>) -> u64 {
    #[cfg(feature = "test-faults")]
    if let Some(n) = _c.env.var("GRAFTYARD_CACHE_LIMIT").ok().and_then(|v| v.to_string().parse::<u64>().ok()) {
        return n.min(cache::CACHE_REPO_MAX);
    }
    cache::CACHE_REPO_MAX
}

/// The status and inclusive byte span a download answers: 200 for the whole entry, 206 for one `Range`, `None` for
/// 416.
fn span(range: Option<&str>, size: u64) -> Option<(u16, u64, u64)> {
    match cache::byte_range(range, size) {
        Err(_) if size == 0 && range.is_none() => Some((200, 0, 0)),
        Err(_) => None,
        Ok(None) => Some((200, 0, size.saturating_sub(1))),
        Ok(Some((s, e))) => Some((206, s, e)),
    }
}

/// `download/<owner>/<shard>/<id>?n=&e=&s=`: the entry's bytes, or the `Range` asked for.
async fn download(c: &Ctx<'_>, req: &Request, path: &str, public: &[u8; 32]) -> Result<Response> {
    if !matches!(req.method(), Method::Get | Method::Head) {
        return error(405, "Method Not Allowed");
    }
    let parts: Vec<&str> = path.split('/').collect();
    let [o, s, id] = parts.as_slice() else { return error(404, "Not Found") };
    let (Ok(o), Ok(s), Ok(id)) = (o.parse::<i64>(), s.parse::<u8>(), id.parse::<u64>()) else {
        return error(404, "Not Found");
    };
    let (chain, id) = (ChainId::Actions(AccountId(DbId(o)), s), CacheId(id));
    let q = req.url()?.query().unwrap_or_default().to_owned();
    let Some(size) = cache::verify_download(&q, public, chain, id, crate::now()) else {
        return error(403, "Forbidden");
    };
    let Ok(bucket) = home_bucket(c, chain).await else { return error(503, "the cache store is unavailable") };
    let Ok(list) = chunks(&bucket, &cache::chunk_prefix(chain, id)).await else {
        return error(503, "the cache store is unavailable");
    };
    let tiled: Vec<(u64, u64)> = list.iter().map(|(s, n, _)| (*s, *n)).collect();
    if cache::tiles(&tiled, size).is_err() {
        return error(404, "Cache not found");
    }
    let h = Headers::new();
    h.set("accept-ranges", "bytes")?;
    let Some((status, start, end)) = span(req.headers().get("range")?.as_deref(), size) else {
        h.set("content-range", &format!("bytes */{size}"))?;
        return Ok(Response::empty()?.with_status(416).with_headers(h));
    };
    let len = if size == 0 { 0 } else { end - start + 1 };
    h.set("content-type", "application/octet-stream")?;
    h.set("content-length", &len.to_string())?;
    if status == 206 {
        h.set("content-range", &format!("bytes {start}-{end}/{size}"))?;
    }
    if req.method() == Method::Head || len == 0 {
        return Ok(Response::empty()?.with_status(status).with_headers(h));
    }
    // One ranged `get` per slice, each opened after the one before it ends, so the isolate holds no chunk. A slice
    // that fails ends the body short of its length, which the toolkit's length check refuses.
    let reads: Vec<(String, u64, u64)> =
        cache::pieces(&tiled, start, end).into_iter().map(|(i, off, n)| (list[i].2.clone(), off, n)).collect();
    let body = stream::iter(reads)
        .then(move |(key, offset, length)| {
            let bucket = bucket.clone();
            async move {
                let got = bucket.get(key).range(Range::OffsetWithLength { offset, length }).execute().await?;
                let gone = || Error::RustError("a cache chunk is gone".into());
                let o = got.ok_or_else(gone)?;
                o.body().ok_or_else(gone)?.stream()
            }
        })
        .try_flatten();
    let fixed: worker_sys::FixedLengthStream = FixedLengthStream::wrap(body, len).into();
    Ok(Response::from_body(ResponseBody::Stream(fixed.readable()))?.with_status(status).with_headers(h))
}

#[cfg(test)]
mod tests {
    use super::{served, span};
    use graftyard_raft::Format;

    #[test]
    fn the_cache_is_served_only_once_21_6_is_enabled() {
        assert!(!served(&[]));
        // 0x15.5 and a format of another family with version 6 are not 0x15.6.
        let near = [Format { family: 0x15, version: 5 }, Format { family: 0x14, version: 6 }];
        assert!(!served(&near));
        assert!(served(&[near[0], Format { family: 0x15, version: 6 }, near[1]]));
    }

    #[test]
    fn a_download_answers_the_whole_entry_one_range_or_416() {
        assert_eq!(span(None, 10), Some((200, 0, 9)));
        assert_eq!(span(Some("bytes=3-12"), 10), Some((206, 3, 9)));
        assert_eq!(span(Some("bytes=2-4"), 10), Some((206, 2, 4)), "a range that ends inside the entry");
        assert_eq!(span(Some("bytes=-4"), 10), Some((206, 6, 9)));
        assert_eq!(span(Some("bytes=10-"), 10), None);
        assert_eq!(span(None, 0), Some((200, 0, 0)));
    }
}
