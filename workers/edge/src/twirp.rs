//! `actions-artifacts` v4 on the edge (`design/SHARED.md`, "Actions", Artifacts, T1; `graftyard_actions::twirp`): the
//! Twirp `ArtifactService` the `graftyard-actions` forks of `upload-artifact@v4` and `download-artifact@v4` call, and the
//! Azure block dialect their uploads speak, both as R2 objects in the owner's home bucket beside v3's.
//!
//! - **`/twirp/github.actions.results.api.v1.ArtifactService/<Method>`** at the install's root, with the job token:
//!   `CreateArtifact` answers a signed upload URL, `FinalizeArtifact` writes the manifest and the REST row,
//!   `ListArtifacts` reads the run's rows, `GetSignedArtifactURL` answers BF-75's one-minute zip URL, and
//!   `DeleteArtifact` is BF-75's delete. Every call's run backend id must be the token's own run.
//! - **`/_services/artifact-blob/<owner>/<shard>/<repo>/<ticket>?comp=`**: `block` stores one block, streamed through a
//!   `FixedLengthStream` of its `Content-Length`, and `blocklist` commits the listed blocks in order once each is a
//!   block of this upload, then deletes the blocks no record names. The ticket is the capability, as the zip's is.
//! - **Off** unless the edge's `GRAFTYARD_ARTIFACTS` is `1`, as the v3 routes are.

use graftyard_actions::artifacts::{self as art, Container};
use graftyard_actions::runsvc::{self, Grant};
use graftyard_actions::twirp::{self as tw, UploadTicket};
use serde_json::{Value, json};
use worker::*;

use crate::Ctx;
use crate::artifacts::{committed, delete, home_bucket, job_run, listing, put_row, row, rows, served, zip_ticket};

fn reply(status: u16, v: &Value) -> Result<Response> {
    Ok(Response::from_json(v)?.with_status(status))
}

fn twirp_error(status: u16, msg: &str) -> Result<Response> {
    let (s, v) = tw::error(status, msg);
    reply(s, &v)
}

/// Azure's error shape for the block calls: the client reads only the status.
fn blob_error(status: u16, msg: &str) -> Result<Response> {
    let mut r =
        Response::ok(format!("<?xml version=\"1.0\" encoding=\"utf-8\"?><Error><Message>{msg}</Message></Error>"))?
            .with_status(status);
    r.headers_mut().set("content-type", "application/xml")?;
    Ok(r)
}

async fn read_json<T: serde::de::DeserializeOwned>(b: &Bucket, key: &str) -> Result<Option<T>> {
    let Some(o) = b.get(key).execute().await? else { return Ok(None) };
    let Some(body) = o.body() else { return Ok(None) };
    Ok(serde_json::from_slice(&body.bytes().await?).ok())
}

/// One Twirp call, `method` the path after `PREFIX`.
pub async fn serve(c: &Ctx<'_>, mut req: Request, method: &str) -> Result<Response> {
    if !served(c.env) {
        return twirp_error(404, "Not Found");
    }
    if req.method() != Method::Post {
        return twirp_error(404, "Not Found");
    }
    let Some(public) = crate::actions::public_key(c) else { return twirp_error(503, "the run service has no key") };
    let grant = crate::actions::bearer(&req).and_then(|t| runsvc::verify_token(&t, &public, crate::now()));
    let Some((chain, Grant::Job { job, .. })) = grant else { return twirp_error(401, "Unauthorized") };
    let body: Value = serde_json::from_str(&req.text().await.unwrap_or_default()).unwrap_or(Value::Null);
    let wf = match job_run(c, chain, job).await {
        Ok(x) => x,
        Err(e) => {
            console_error!("twirp {chain} job {}: {e}", job.0.0);
            return twirp_error(503, "the run did not read");
        }
    };
    if !tw::own_run(&body, wf.id) {
        return twirp_error(403, "the job may not reach this run's artifacts");
    }
    let ct = Container { chain, repo: wf.repo, run: wf.id };
    let Ok(bucket) = home_bucket(c, chain).await else { return twirp_error(503, "the artifact store is unavailable") };
    let now = crate::now().0;
    let services = crate::artifacts::services(c, &req)?;
    let name = body["name"].as_str().unwrap_or_default().to_owned();
    let named = |m: &str| -> std::result::Result<(), Result<Response>> {
        art::check_name(&name).map_err(|_| twirp_error(400, &format!("{m}: the artifact name is not valid")))
    };
    match method {
        "CreateArtifact" => {
            if let Err(r) = named("CreateArtifact") {
                return r;
            }
            // v4's artifacts are immutable: a finalized name is taken for the run (the client's `overwrite` deletes
            // it first).
            if bucket.head(ct.manifest_key(&name)).await?.is_some() {
                return twirp_error(409, "an artifact with this name already exists on the workflow run");
            }
            let asked = tw::retention_asked(body["expires_at"].as_str(), now);
            let rec = art::create_record(asked, now);
            let fresh = u64::from_be_bytes(crate::rand10()[..8].try_into().expect("8 bytes")) >> 1;
            let ticket = UploadTicket {
                run: wf.id,
                job,
                name: name.clone(),
                upload: fresh,
                exp: now / 1000 + runsvc::JOB_TOKEN_TTL_S,
            };
            let nonce = crate::artifacts::nonce();
            let puts = async {
                bucket.put(ct.create_key(&name), rec.to_string().into_bytes()).execute().await?;
                let t = serde_json::to_vec(&ticket)?;
                bucket.put(tw::upload_ticket_key(chain, wf.repo, &nonce), t).execute().await?;
                Ok::<(), Error>(())
            };
            if let Err(e) = puts.await {
                console_error!("twirp create {} {name}: {e}", ct.prefix());
                return twirp_error(503, "the artifact store is unavailable");
            }
            reply(200, &json!({ "ok": true, "signed_upload_url": tw::upload_url(&services, chain, wf.repo, &nonce) }))
        }
        "FinalizeArtifact" => {
            if let Err(r) = named("FinalizeArtifact") {
                return r;
            }
            let Some(blob) = committed(&bucket, &ct, &name).await? else {
                return twirp_error(404, "no upload of this artifact was committed");
            };
            let size = match &body["size"] {
                Value::String(s) => s.parse::<u64>().ok(),
                v => v.as_u64(),
            };
            if size.is_some_and(|n| n != blob.size) {
                return twirp_error(400, "the size is not the committed upload's");
            }
            let created: Option<Value> = read_json(&bucket, &ct.create_key(&name)).await?;
            let mut r = art::finalized_row(&ct, &name, blob.size as i64, created.as_ref(), &wf, now);
            r.digest = body["hash"].as_str().filter(|h| h.starts_with("sha256:")).map(str::to_owned);
            let manifest = json!({ "name": name, "size": blob.size, "v": 4 }).to_string();
            if let Err(e) = bucket.put(ct.manifest_key(&name), manifest.into_bytes()).execute().await {
                console_error!("twirp manifest {} {name}: {e}", ct.prefix());
                return twirp_error(503, "the artifact store is unavailable");
            }
            if let Err(e) = put_row(&bucket, chain, wf.repo, &r).await {
                console_error!("twirp row {} {name}: {e}", ct.prefix());
                return twirp_error(503, "the artifact store is unavailable");
            }
            reply(200, &json!({ "ok": true, "artifact_id": r.id.to_string() }))
        }
        "ListArtifacts" => {
            let all = match rows(&bucket, chain, wf.repo).await {
                Ok(r) => r,
                Err(e) => return twirp_error(503, &format!("the artifact store: {e}")),
            };
            let got: Vec<Value> = tw::list_rows(all, wf.id, &body, now).iter().map(|r| tw::list_json(r, job)).collect();
            reply(200, &json!({ "artifacts": got }))
        }
        "GetSignedArtifactURL" | "DeleteArtifact" => {
            if let Err(r) = named(method) {
                return r;
            }
            let id = art::artifact_id(chain, wf.repo, wf.id, &name);
            let Some(a) = row(&bucket, chain, wf.repo, id).await?.filter(|a| !a.expired(now)) else {
                return twirp_error(404, "Artifact not found");
            };
            if method == "DeleteArtifact" {
                return match delete(&bucket, chain, wf.repo, &a).await {
                    Ok(()) => reply(200, &json!({ "ok": true, "artifact_id": a.id.to_string() })),
                    Err(e) => twirp_error(503, &format!("the artifact store: {e}")),
                };
            }
            match zip_ticket(&services, &bucket, chain, wf.repo, a.id, now).await {
                Ok(url) => reply(200, &json!({ "signed_url": url })),
                Err(e) => twirp_error(503, &format!("the artifact store: {e}")),
            }
        }
        _ => twirp_error(404, "Not Found"),
    }
}

/// `PUT artifact-blob/...?comp=block&blockid=` or `?comp=blocklist`: the Azure calls `uploadStream` makes.
pub async fn blob(c: &Ctx<'_>, mut req: Request, rest: &str) -> Result<Response> {
    if !served(c.env) {
        return blob_error(404, "Not Found");
    }
    if req.method() != Method::Put {
        return blob_error(405, "Method Not Allowed");
    }
    let Some((chain, repo, nonce)) = tw::parse_upload_path(rest) else { return blob_error(404, "Not Found") };
    let Ok(bucket) = home_bucket(c, chain).await else { return blob_error(503, "the artifact store is unavailable") };
    let ticket: Option<UploadTicket> = read_json(&bucket, &tw::upload_ticket_key(chain, repo, nonce)).await?;
    let Some(t) = ticket.filter(|t| t.admits(crate::now())) else {
        return blob_error(403, "This URL has expired");
    };
    let ct = Container { chain, repo, run: t.run };
    let url = req.url()?;
    let q = |k: &str| url.query_pairs().find(|(n, _)| n == k).map(|(_, v)| v.into_owned());
    let created = |r: Response| -> Result<Response> {
        let mut r = r.with_status(201);
        r.headers_mut().set("x-ms-request-id", &crate::artifacts::nonce())?;
        r.headers_mut().set("x-ms-version", "2023-11-03")?;
        Ok(r)
    };
    match q("comp").as_deref() {
        Some("block") => {
            let Some(part) = q("blockid").as_deref().and_then(tw::block_part) else {
                return blob_error(400, "InvalidQueryParameterValue");
            };
            let Some(len) = req.headers().get("content-length")?.and_then(|v| v.parse::<u64>().ok()) else {
                return blob_error(411, "MissingContentLengthHeader");
            };
            if len == 0 || len > art::CHUNK_MAX {
                return blob_error(413, "RequestBodyTooLarge");
            }
            let body = FixedLengthStream::wrap(req.stream()?, len);
            if let Err(e) = bucket.put(ct.block_key(&t.name, t.upload, &part), body).execute().await {
                console_error!("twirp block {} {}: {e}", ct.prefix(), t.name);
                return blob_error(400, "the block's body is not its Content-Length");
            }
            created(Response::empty()?)
        }
        Some("blocklist") => {
            let xml = req.text().await?;
            let Some(list) = tw::block_list(&xml) else { return blob_error(400, "InvalidXmlDocument") };
            let prefix = ct.block_prefix(&t.name, t.upload);
            let Ok(staged) = listing(&bucket, &prefix).await else {
                return blob_error(503, "the artifact store is unavailable");
            };
            let staged: Vec<(String, u64)> =
                staged.into_iter().filter_map(|(k, n)| k.strip_prefix(&prefix).map(|p| (p.to_owned(), n))).collect();
            let blob = match tw::commit(t.upload, &staged, &list) {
                Ok(b) => b,
                Err(e) => return blob_error(400, &format!("InvalidBlockList: {e}")),
            };
            let before = committed(&bucket, &ct, &t.name).await.ok().flatten();
            if let Err(e) = bucket.put(ct.blob_key(&t.name), serde_json::to_vec(&blob)?).execute().await {
                console_error!("twirp blocklist {} {}: {e}", ct.prefix(), t.name);
                return blob_error(503, "the artifact store is unavailable");
            }
            // The blocks no record names now: this upload's unlisted ones and a replaced upload's. A delete lost here
            // leaves bytes for the lifecycle rule.
            let keep: Vec<String> = blob.blocks.iter().map(|(p, _)| ct.block_key(&t.name, t.upload, p)).collect();
            let mut drop: Vec<String> =
                staged.iter().map(|(p, _)| ct.block_key(&t.name, t.upload, p)).filter(|k| !keep.contains(k)).collect();
            if let Some(old) = before.filter(|o| o.upload != t.upload) {
                drop.extend(old.blocks.iter().map(|(p, _)| ct.block_key(&t.name, old.upload, p)));
            }
            for k in drop.chunks(1000) {
                let _ = bucket.delete_multiple(k.to_vec()).await;
            }
            created(Response::empty()?)
        }
        _ => blob_error(400, "InvalidQueryParameterValue"),
    }
}
