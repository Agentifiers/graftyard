//! The Workers host for `pass`: the install's edge over HTTPS (`GRAFTYARD_URL`) with the export token
//! (`EXPORT_TOKEN`, a secret `graftyard vault init` writes), and the `VAULT` bucket.
//!
//! - **Copies stream.** A pack or snapshot object goes into a multipart upload in `PART_BYTES` parts as it arrives,
//!   checked as it goes (`PackCheck`, `SnapshotReader`), and the upload completes only once the check passed; a
//!   failed check aborts it, so a bad copy never becomes an object the lock would keep, and no object is held whole.
//! - **Create-only.** Every put but the state's carries `If-None-Match: *`, and a copy first HEADs its key, so a
//!   pass never overwrites what an earlier one stored (and the lock would refuse it anyway). The state is written on
//!   its etag.
//! - **Triggers.** The `* * * * *` cron runs passes for up to `POLL_FOR_MS`, one every `POLL_EVERY_MS` while the last
//!   found nothing left, back to back while it did. Each pass logs one line (`Report::line`), as an error when the
//!   feed lags past `LAG_ALARM_MS` or a copy was missed. The Worker serves no route.

use futures_util::StreamExt;
use graftyard_types::{ChainId, Digest, Index, NetworkId, Timestamp};
use worker::*;

use crate::{Copied, Got, Host, PART_BYTES, PackCheck, SnapPart, SnapshotReader};
use graftyard_git::pack::PackId;

/// Every export request names this agent: the front refuses a request with none (`tests/m5/bf49/pull.py`).
const AGENT: &str = "graftyard-vault";

struct Live {
    bucket: Bucket,
    base: String,
    token: String,
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

impl Live {
    fn load(env: &Env) -> std::result::Result<Live, String> {
        let base = env.var("GRAFTYARD_URL").map_err(|_| "GRAFTYARD_URL is not set")?.to_string();
        let token = env.secret("EXPORT_TOKEN").map_err(|_| "EXPORT_TOKEN is not set")?.to_string();
        let bucket = env.bucket("VAULT").map_err(|_| "no VAULT bucket bound")?;
        Ok(Live { bucket, base: base.trim_end_matches('/').to_string(), token })
    }

    async fn fetch(&self, path: &str) -> std::result::Result<Response, String> {
        let h = Headers::new();
        h.set("authorization", &format!("Bearer {}", self.token)).map_err(err)?;
        h.set("user-agent", AGENT).map_err(err)?;
        let mut init = RequestInit::new();
        init.with_headers(h);
        let url = format!("{}{}/{path}", self.base, graftyard_git::export::EXPORT_PREFIX);
        let req = Request::new_with_init(&url, &init).map_err(err)?;
        Fetch::Request(req).send().await.map_err(|e| format!("GET {path}: {e}"))
    }

    async fn exists(&self, key: &str) -> std::result::Result<bool, String> {
        Ok(self.bucket.head(key).await.map_err(err)?.is_some())
    }
}

/// One object written as it streams: parts of `PART_BYTES` into a multipart upload, completed by `finish`.
struct Upload {
    up: MultipartUpload,
    parts: Vec<UploadedPart>,
    buf: Vec<u8>,
}

impl Upload {
    async fn start(b: &Bucket, key: &str) -> std::result::Result<Upload, String> {
        let up = b.create_multipart_upload(key).execute().await.map_err(err)?;
        Ok(Upload { up, parts: Vec::new(), buf: Vec::with_capacity(PART_BYTES) })
    }

    async fn push(&mut self, mut chunk: &[u8]) -> std::result::Result<(), String> {
        while !chunk.is_empty() {
            let n = (PART_BYTES - self.buf.len()).min(chunk.len());
            self.buf.extend_from_slice(&chunk[..n]);
            chunk = &chunk[n..];
            if self.buf.len() == PART_BYTES {
                self.flush().await?;
            }
        }
        Ok(())
    }

    async fn flush(&mut self) -> std::result::Result<(), String> {
        let n = self.parts.len() as u16 + 1;
        let part = std::mem::replace(&mut self.buf, Vec::with_capacity(PART_BYTES));
        self.parts.push(self.up.upload_part(n, part).await.map_err(err)?);
        Ok(())
    }

    async fn finish(mut self) -> std::result::Result<(), String> {
        if !self.buf.is_empty() || self.parts.is_empty() {
            self.flush().await?;
        }
        self.up.complete(self.parts).await.map_err(err)?;
        Ok(())
    }

    async fn abort(self) {
        let _ = self.up.abort().await;
    }
}

impl Host for Live {
    fn now(&self) -> Timestamp {
        Timestamp(Date::now().as_millis())
    }

    async fn get(&self, path: &str) -> std::result::Result<Got, String> {
        let mut r = self.fetch(path).await?;
        if r.status_code() != 200 {
            return Ok(Got::Status(r.status_code()));
        }
        let more = r.headers().get("link").map_err(err)?.is_some_and(|l| l.contains("rel=\"next\""));
        Ok(Got::Ok { body: r.bytes().await.map_err(err)?, more })
    }

    async fn copy_pack(
        &self,
        network: NetworkId,
        id: PackId,
        checksum: Digest,
        bytes: u64,
    ) -> std::result::Result<Copied, String> {
        let key = crate::pack_key(network, id);
        if self.exists(&key).await? {
            return Ok(Copied::Exists);
        }
        let mut r = self.fetch(&format!("packs/{id}?network={}", network.0.0)).await?;
        if r.status_code() != 200 {
            return Ok(Copied::Status(r.status_code()));
        }
        let mut check = PackCheck::new(checksum, bytes);
        let mut up = Upload::start(&self.bucket, &key).await?;
        let mut body = r.stream().map_err(err)?;
        while let Some(chunk) = body.next().await {
            let chunk = match chunk {
                Ok(c) => c,
                Err(e) => {
                    up.abort().await;
                    return Err(err(e));
                }
            };
            if !check.push(&chunk) {
                break;
            }
            if let Err(e) = up.push(&chunk).await {
                up.abort().await;
                return Err(e);
            }
        }
        if let Err(e) = check.done() {
            up.abort().await;
            return Ok(Copied::Corrupt(format!("{key}: {e}")));
        }
        up.finish().await?;
        Ok(Copied::Stored)
    }

    async fn copy_snapshot(&self, chain: ChainId, at: Index) -> std::result::Result<Copied, String> {
        let mut r = self.fetch(&format!("chains/{chain}/snapshots?at={}", at.0)).await?;
        if r.status_code() != 200 {
            return Ok(Copied::Status(r.status_code()));
        }
        let mut rd = SnapshotReader::default();
        let mut out = Copied::Exists;
        // The object being written: `None` while skipping one the vault already holds.
        let mut up: Option<Upload> = None;
        let mut body = r.stream().map_err(err)?;
        while let Some(chunk) = body.next().await {
            let parts = match chunk.map_err(err).and_then(|c| rd.push(&c)) {
                Ok(p) => p,
                Err(e) => {
                    if let Some(u) = up.take() {
                        u.abort().await;
                    }
                    return Err(e);
                }
            };
            for p in parts {
                match p {
                    SnapPart::Start { key, .. } => {
                        if !crate::snapshot_of(&key, chain) {
                            return Ok(Copied::Corrupt(format!("{key} is not a snapshot of {chain}")));
                        }
                        up = if self.exists(&key).await? {
                            None
                        } else {
                            Some(Upload::start(&self.bucket, &key).await?)
                        };
                    }
                    SnapPart::Bytes(b) => {
                        if let Some(u) = up.as_mut() {
                            u.push(&b).await?;
                        }
                    }
                    SnapPart::End { ok } => match (up.take(), ok) {
                        (Some(u), true) => {
                            u.finish().await?;
                            out = Copied::Stored;
                        }
                        (Some(u), false) => {
                            u.abort().await;
                            return Ok(Copied::Corrupt(format!(
                                "a snapshot object of {chain} at {} fails its digest",
                                at.0
                            )));
                        }
                        (None, _) => {}
                    },
                }
            }
        }
        if !rd.finished() {
            if let Some(u) = up.take() {
                u.abort().await;
            }
            return Err(format!("the snapshot of {chain} at {} ended mid-object", at.0));
        }
        Ok(out)
    }

    async fn pack_status(&self, network: NetworkId, id: PackId) -> std::result::Result<u16, String> {
        // The body is never read: only the status says whether the install still serves the pack.
        Ok(self.fetch(&format!("packs/{id}?network={}", network.0.0)).await?.status_code())
    }

    async fn put_new(&self, key: &str, body: &[u8]) -> std::result::Result<bool, String> {
        let put = self
            .bucket
            .put(key, body.to_vec())
            .only_if(Conditional { etag_does_not_match: Some("*".into()), ..Default::default() })
            .execute()
            .await
            .map_err(err)?;
        Ok(put.is_some())
    }

    async fn state(&self) -> std::result::Result<Option<(Vec<u8>, String)>, String> {
        let Some(o) = self.bucket.get(crate::STATE_KEY).execute().await.map_err(err)? else { return Ok(None) };
        let etag = o.etag();
        let body = match o.body() {
            Some(b) => b.bytes().await.map_err(err)?,
            None => Vec::new(),
        };
        Ok(Some((body, etag)))
    }

    async fn put_state(&self, body: &[u8], etag: Option<&str>) -> std::result::Result<Option<String>, String> {
        let cond = match etag {
            Some(e) => Conditional { etag_matches: Some(e.into()), ..Default::default() },
            None => Conditional { etag_does_not_match: Some("*".into()), ..Default::default() },
        };
        let put = self.bucket.put(crate::STATE_KEY, body.to_vec()).only_if(cond).execute().await.map_err(err)?;
        Ok(put.map(|o| o.etag()))
    }

    async fn list(&self, prefix: &str) -> std::result::Result<Vec<String>, String> {
        let mut out = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let mut l = self.bucket.list().prefix(prefix);
            if let Some(c) = cursor.take() {
                l = l.cursor(c);
            }
            let page = l.execute().await.map_err(err)?;
            out.extend(page.objects().iter().map(|o| o.key()));
            match page.cursor().filter(|_| page.truncated()) {
                Some(c) => cursor = Some(c),
                None => return Ok(out),
            }
        }
    }

    async fn delete(&self, key: &str) -> std::result::Result<bool, String> {
        // A lock rule refuses the delete; the binding reports it as an error, which the next day's pass retries.
        Ok(self.bucket.delete(key).await.is_ok())
    }
}

async fn passes(env: &Env) {
    let live = match Live::load(env) {
        Ok(l) => l,
        Err(e) => return console_error!("vault: {e}"),
    };
    let start = Date::now().as_millis();
    loop {
        match crate::pass(&live).await {
            Ok((r, st)) => {
                let line = r.line(&st);
                if r.lag_ms.is_some_and(|l| l > crate::LAG_ALARM_MS) || st.missed > 0 || st.undecoded > 0 {
                    console_error!("{line}");
                } else {
                    console_log!("{line}");
                }
                if r.lost {
                    return;
                }
                if r.more {
                    continue;
                }
            }
            Err(e) => return console_error!("vault pass failed: {e}"),
        }
        if Date::now().as_millis() + crate::POLL_EVERY_MS > start + crate::POLL_FOR_MS {
            return;
        }
        Delay::from(std::time::Duration::from_millis(crate::POLL_EVERY_MS)).await;
    }
}

#[event(scheduled)]
async fn scheduled(_event: ScheduledEvent, env: Env, _ctx: ScheduleContext) {
    passes(&env).await;
}
