//! The export endpoints under `/api/graftyard/v1/export` (`design/DESIGN.md`, Shape, "Lifeboat feed";
//! `decisions/DECISIONS.md`, `export-host`). The rules are `graftyard_git::export`'s (`Route::parse`, `in_scope`,
//! `serve`); this module reads what they serve.
//!
//! - **Auth.** An `ExportClaims` bearer token, verified against the customer key of the `Release` view's `Keys` (the
//!   trust anchor's before any `Keys` entry) and refused once its `jti`'s `JtiRevoked` row is read, through the same
//!   READ_CACHE as `ghs_` (`ident::jti_revoked`); or a registered lifeboat's signed tail, snapshot, or pack pull
//!   (`lifeboat_claims`), which is install-scoped. Anything else is 401.
//! - **Scope.** `in_scope` runs before any read: an org token's repository chain or pack is checked against the
//!   repository's owner from `RepoOwner`, and the feed needs an install token.
//! - **Reads.** Batches list the `EXPORT` bucket after the cursor. Entries come from a learner or follower
//!   (`graftyard_lookup::log_rows`, never the leader), or, below the member's stored log, from the archive object the
//!   member names, read as it streams (`export::ArchiveRows`). Snapshots and packs stream from the site buckets, so
//!   the edge holds no whole object; a pack only once a `Committed` marker names it.
//!
//! Every future here is boxed where it nests: the edge's `serve` future is already deep (B0-34's stack overflow).

use futures_util::StreamExt;
use graftyard_git::export::{self, ArchiveRows, ExportError, ExportReply, Prefetched, Route, StoredLog};
use graftyard_git::host::R2;
use graftyard_git::{Bucket, R2Key};
use graftyard_identity::export_token::{self, ExportClaims};
use graftyard_identity::lifeboat;
use graftyard_types::{AccountId, ChainId, Index, PlacementRecord, RepoId};
use graftyard_wire::Reply;
use sha2::{Digest as _, Sha256};
use worker::*;

use crate::Ctx;

/// The route for a path under `EXPORT_PREFIX`, or `None` for another path.
pub async fn route(c: &Ctx<'_>, req: &mut Request, path: &str) -> Result<Option<Response>> {
    if !path.starts_with(export::EXPORT_PREFIX) {
        return Ok(None);
    }
    let url = req.url()?;
    let query: Vec<(String, String)> = url.query_pairs().map(|(k, v)| (k.into_owned(), v.into_owned())).collect();
    let route = match Route::parse(req.method().as_ref(), path, &query) {
        Ok(r) => r,
        Err(e) => return refused(c, &e).map(Some),
    };
    let signed = req.headers().has(lifeboat::HOST_HEADER)?;
    let claims = if signed {
        Ok(lifeboat_claims(c, req, &url, &route).await?)
    } else {
        Box::pin(claims(c, req, !matches!(route, Route::RevokeToken))).await?
    };
    let claims = match claims {
        Ok(Some(claims)) => claims,
        Ok(None) => return crate::agents::respond(c, Reply::error(401, "Bad credentials", "rest")).map(Some),
        // The token verified and its `jti` row did not answer: retry, as for any read whose group is down.
        Err(why) => return crate::agents::respond(c, crate::api::unavailable(&why)).map(Some),
    };
    if matches!(route, Route::RevokeToken) {
        let r = Box::pin(crate::ident::revoke_jti(c, claims.jti(), export_token::revoke_command(&claims))).await?;
        return crate::agents::respond(c, r).map(Some);
    }
    let root = format!("https://{}", url.host_str().unwrap_or_default());
    let answer = match Box::pin(read(c, &route, &claims, &root)).await {
        Ok(a) => a,
        Err(Fail::Export(e)) => return refused(c, &e).map(Some),
        Err(Fail::Unavailable(why)) => return crate::agents::respond(c, crate::api::unavailable(&why)).map(Some),
    };
    Ok(Some(answer))
}

/// A lifeboat's signed tail, snapshot, or pack pull (`graftyard_identity::lifeboat`): an install-scoped claim once its
/// signature verifies under the key the `Release` view registered for its host, and `None` for any other route or a
/// request that does not verify. Nothing to revoke: a removed lifeboat leaves the view.
async fn lifeboat_claims(c: &Ctx<'_>, req: &Request, url: &Url, route: &Route) -> Result<Option<ExportClaims>> {
    if !route.lifeboat_may() {
        return Ok(None);
    }
    let Some(anchor) = c.anchor else { return Ok(None) };
    Box::pin(graftyard_lookup::view::ensure(&c.bindings(), &anchor, Some(c.ctx))).await;
    let h = req.headers();
    let (host, at, sig) = (h.get(lifeboat::HOST_HEADER)?, h.get(lifeboat::AT_HEADER)?, h.get(lifeboat::SIG_HEADER)?);
    let path = match url.query() {
        Some(q) => format!("{}?{q}", url.path()),
        None => url.path().to_owned(),
    };
    let now = crate::now();
    let registered = |host: &str| graftyard_lookup::view::with(|r| r.fold.lifeboats.get(host).copied());
    let ok = lifeboat::verify(host.as_deref(), at.as_deref(), sig.as_deref(), "GET", &path, now, registered);
    let Ok(host) = ok else { return Ok(None) };
    Ok(Some(ExportClaims {
        install: anchor.install,
        scope: graftyard_release::Scope::Install,
        holder: format!("lifeboat:{host}").into(),
        jti: graftyard_types::Ulid(0),
        iat: now,
        exp: now,
    }))
}

/// The token's claims once it verifies and its `jti` is not revoked; `None` otherwise, and `Err` when the `jti` row did
/// not answer. `read_class`: a read may take a not-revoked answer up to READ_CACHE old, so a revocation is honored
/// within READ_CACHE; the `DELETE` reads it fresh.
async fn claims(
    c: &Ctx<'_>,
    req: &Request,
    read_class: bool,
) -> Result<std::result::Result<Option<ExportClaims>, String>> {
    let Some(text) = crate::token_of(req)? else { return Ok(Ok(None)) };
    let Some(anchor) = c.anchor else { return Ok(Ok(None)) };
    Box::pin(graftyard_lookup::view::ensure(&c.bindings(), &anchor, Some(c.ctx))).await;
    let customer =
        graftyard_lookup::view::with(|r| r.fold.keys.as_ref().map(|k| k.customer)).unwrap_or(anchor.customer);
    let Ok(claims) = export_token::verify(&text, anchor.install, &customer, crate::now()) else {
        return Ok(Ok(None));
    };
    c.read_class.set(read_class);
    Ok(match crate::ident::jti_revoked(c, claims.jti(), crate::now(), read_class).await {
        Ok(revoked) => Ok((revoked == Some(false)).then_some(claims)),
        Err(e) => Err(format!("the export token's revocation row did not answer; retry ({e})")),
    })
}

enum Fail {
    Export(ExportError),
    Unavailable(String),
}

impl From<ExportError> for Fail {
    fn from(e: ExportError) -> Fail {
        Fail::Export(e)
    }
}

fn unavailable(e: impl std::fmt::Display) -> Fail {
    Fail::Unavailable(e.to_string())
}

fn refused(c: &Ctx<'_>, e: &ExportError) -> Result<Response> {
    crate::agents::respond(c, Reply::error(e.status(), &e.message(), "graftyard"))
}

/// The repository whose owner an org token's scope is checked against: a repository chain's, or a network's root
/// repository's (a network's id is its root repository's).
fn owned_repo(chain: Option<ChainId>) -> Option<RepoId> {
    match chain? {
        ChainId::Repo(r) | ChainId::RepoMeta(r) => Some(r),
        ChainId::Network(n) => Some(RepoId(n.0)),
        _ => None,
    }
}

async fn read(c: &Ctx<'_>, route: &Route, claims: &ExportClaims, root: &str) -> std::result::Result<Response, Fail> {
    // An org token's repository chain or pack needs its owner: a repository with no `RepoOwner` row is not found.
    let mut owner: Option<AccountId> = None;
    if claims.scope != graftyard_release::Scope::Install
        && let Some(r) = owned_repo(route.chain())
    {
        owner = Some(Box::pin(crate::repo_by_id(c, r)).await.map_err(|_| ExportError::NotFound)?.owner);
    }
    let covers = |chain: ChainId| export_token::covers(claims.scope, chain, owner);
    export::in_scope(route, &covers)?;
    let now = crate::now().0;
    let src = match route {
        Route::Pack { network, id } => return Box::pin(pack(c, *network, *id)).await,
        Route::Snapshots { chain, at } => return Box::pin(snapshot(c, *chain, *at)).await,
        Route::Batches { after } => {
            Prefetched { objects: Box::pin(objects_after(c, *after)).await?, ..Default::default() }
        }
        Route::Entries { chain, from, limit } => {
            Prefetched { log: Some(Box::pin(log(c, *chain, *from, *limit)).await?), ..Default::default() }
        }
        Route::RevokeToken => return Err(ExportError::Method.into()),
    };
    reply(c, export::serve(route, &covers, &src, now, root)?).map_err(unavailable)
}

fn reply(c: &Ctx<'_>, r: ExportReply) -> Result<Response> {
    let h = crate::headers(c.rid)?;
    h.set("content-type", "application/octet-stream")?;
    if let Some(next) = &r.next {
        h.set("link", next)?;
    }
    Ok(Response::from_bytes(r.body)?.with_headers(h))
}

/// The feed's objects after `after`, in order, as `export::batches_take` bounds a page.
async fn objects_after(c: &Ctx<'_>, after: export::FeedPosition) -> std::result::Result<Vec<Vec<u8>>, Fail> {
    let bucket = c.env.bucket("EXPORT").map_err(|_| unavailable("this install has no export bucket bound"))?;
    let mut l = bucket.list().prefix("export/").limit(export::BATCHES_PAGE as u32);
    if after != export::FeedPosition::default() {
        l = l.start_after(after.key().to_string());
    }
    let listed = l.execute().await.map_err(unavailable)?;
    let (mut out, mut bytes) = (Vec::new(), 0usize);
    for o in listed.objects() {
        if !export::batches_take(out.len(), bytes) {
            break;
        }
        let Some(got) = R2(bucket.clone()).get(&o.key()).await.map_err(|e| unavailable(e.0))? else { continue };
        bytes += got.len();
        out.push(got);
    }
    Ok(out)
}

/// The record a chain's members are found under: a system chain's from the `Release` view, an owner's chain's from
/// its owner's `Placement`. A network has no group.
async fn record(c: &Ctx<'_>, chain: ChainId) -> std::result::Result<PlacementRecord, Fail> {
    match chain {
        ChainId::Release | ChainId::Names(_) | ChainId::Tokens(_) | ChainId::Ids => {
            c.system_record(chain).await.map_err(unavailable)
        }
        ChainId::Repo(r) | ChainId::RepoMeta(r) => {
            Ok(Box::pin(crate::repo_by_id(c, r)).await.map_err(|_| ExportError::NotFound)?.record)
        }
        ChainId::Authz(a) | ChainId::Identity(a) | ChainId::Actions(a, _) => {
            Box::pin(crate::placement(c, a)).await.map_err(unavailable)?.ok_or(ExportError::NotFound.into())
        }
        ChainId::Network(_) => Err(ExportError::NotFound.into()),
    }
}

/// A chain's stored rows from `from`: one member's page, or below its stored log the archive object it names.
async fn log(c: &Ctx<'_>, chain: ChainId, from: Index, limit: u64) -> std::result::Result<StoredLog, Fail> {
    let record = Box::pin(record(c, chain)).await?;
    let to = from.0.saturating_add(limit - 1);
    let (_, bytes) = Box::pin(graftyard_lookup::log_rows(&c.bindings(), chain, &record, from.0, to))
        .await
        .ok_or_else(|| unavailable(format!("no learner or follower of {chain} answered; retry")))?;
    let mut page = export::decode_log_page(&bytes).map_err(unavailable)?;
    // A member that installed a snapshot names no archive below its install: its `Archived` entries were never applied
    // there. The objects are still in the record's buckets, so the range comes from the one that holds `from`.
    if page.archive.is_none() && from < page.first {
        page.archive = Box::pin(listed_archive(c, &record, chain, from)).await?;
    }
    let mut rows: Vec<(Index, Vec<u8>)> = Vec::new();
    let archived = page.archive.is_some();
    if let Some((first, last, key)) = &page.archive
        && from < page.first
    {
        rows = Box::pin(archive_rows(c, &record, key, *first, from, Index(to.min(last.0)), limit)).await?;
    }
    let next = rows.last().map_or(from, |(i, _)| Index(i.0 + 1));
    if rows.is_empty() || next == page.first {
        let first = page.first;
        rows.extend(page.rows.into_iter().enumerate().map(|(n, r)| (Index(first.0 + n as u64), r)));
    }
    Ok(StoredLog { first: page.first, archived, to: page.commit, rows })
}

/// The archive object of `chain` whose range holds `from`, listed in the record's first two buckets, which hold every
/// archive object (`export::archive_holding`).
async fn listed_archive(
    c: &Ctx<'_>,
    record: &PlacementRecord,
    chain: ChainId,
    from: Index,
) -> std::result::Result<Option<(Index, Index, String)>, Fail> {
    let prefix = format!("archive/{chain}/");
    for s in record.sites.iter().take(2) {
        let Ok(bucket) = c.env.bucket(&format!("BUCKET_{}", s.0)) else { continue };
        let (mut keys, mut cursor) = (Vec::new(), None);
        loop {
            let mut l = bucket.list().prefix(prefix.clone());
            if let Some(k) = cursor.take() {
                l = l.cursor(k);
            }
            let Ok(listed) = l.execute().await else { break };
            keys.extend(listed.objects().iter().filter_map(|o| o.key().parse::<R2Key>().ok()));
            match listed.cursor().filter(|_| listed.truncated()) {
                Some(k) => cursor = Some(k),
                None => break,
            }
        }
        if let Some((f, t, k)) = export::archive_holding(&keys, chain, from) {
            return Ok(Some((f, t, k.to_string())));
        }
    }
    Ok(None)
}

/// The rows `from..=to` of the archive object `key` (whose range starts at `first`), read as it streams from the
/// record's first two buckets, which hold every archive object.
async fn archive_rows(
    c: &Ctx<'_>,
    record: &PlacementRecord,
    key: &str,
    first: Index,
    from: Index,
    to: Index,
    limit: u64,
) -> std::result::Result<Vec<(Index, Vec<u8>)>, Fail> {
    for s in record.sites.iter().take(2) {
        let Ok(bucket) = c.env.bucket(&format!("BUCKET_{}", s.0)) else { continue };
        let Ok(Some(o)) = bucket.get(key).execute().await else { continue };
        let Some(body) = o.body() else { continue };
        let mut stream = body.stream().map_err(unavailable)?;
        let mut rows = ArchiveRows::new(first, from, to, limit);
        while let Some(chunk) = stream.next().await {
            if rows.push(&chunk.map_err(unavailable)?).map_err(unavailable)? {
                break;
            }
        }
        return Ok(rows.rows);
    }
    Err(ExportError::Gone(format!("the archive object {key} is in no bucket")).into())
}

/// The newest snapshot at or below `at`, streamed as frames: per object its key, then a 4-byte length, the bytes as
/// they come from the bucket, and their SHA-256.
async fn snapshot(c: &Ctx<'_>, chain: ChainId, at: Index) -> std::result::Result<Response, Fail> {
    let record = Box::pin(record(c, chain)).await?;
    let prefix = format!("snapshot/{chain}/");
    let mut found = None;
    for s in record.sites.iter() {
        let Ok(bucket) = c.env.bucket(&format!("BUCKET_{}", s.0)) else { continue };
        let Ok(listed) = R2(bucket.clone()).list(&prefix).await else { continue };
        let keys: Vec<R2Key> = listed.iter().filter_map(|o| o.key.parse().ok()).collect();
        if let Some(pick) = export::snapshot_at(&keys, chain, at) {
            let sizes = pick.iter().map(|k| listed.iter().find(|o| o.key == k.to_string()).map_or(0, |o| o.size));
            found = Some((bucket, pick.iter().cloned().zip(sizes).collect::<Vec<_>>()));
            break;
        }
    }
    let (bucket, objects) = found.ok_or(ExportError::NotFound)?;
    enum St {
        Next(usize),
        Body(usize, ByteStream, Sha256),
        Done,
    }
    let body = futures_util::stream::unfold(St::Next(0), move |st| {
        let (bucket, objects) = (bucket.clone(), objects.clone());
        async move {
            match st {
                St::Next(i) if i == objects.len() => None,
                St::Next(i) => {
                    let (key, size) = &objects[i];
                    let opened = async {
                        let o = bucket.get(key.to_string()).execute().await?;
                        let gone = || Error::RustError(format!("{key} is gone"));
                        let o = o.ok_or_else(gone)?;
                        o.body().ok_or_else(gone)?.stream()
                    };
                    match opened.await {
                        Ok(s) => {
                            let mut head = Vec::new();
                            export::frame(&mut head, key.to_string().as_bytes());
                            head.extend_from_slice(&(*size as u32).to_be_bytes());
                            Some((Ok::<Vec<u8>, Error>(head), St::Body(i, s, Sha256::new())))
                        }
                        Err(e) => Some((Err(e), St::Done)),
                    }
                }
                St::Body(i, mut s, mut h) => match s.next().await {
                    Some(Ok(chunk)) => {
                        h.update(&chunk);
                        Some((Ok(chunk), St::Body(i, s, h)))
                    }
                    Some(Err(e)) => Some((Err(e), St::Done)),
                    None => Some((Ok(h.finalize().to_vec()), St::Next(i + 1))),
                },
                St::Done => None,
            }
        }
    });
    let h = crate::headers(c.rid).map_err(unavailable)?;
    h.set("content-type", "application/octet-stream").map_err(unavailable)?;
    Ok(Response::from_stream(body).map_err(unavailable)?.with_headers(h))
}

/// A pack's bytes, streamed from the first bucket that holds it, once a `Committed` marker names it, with the
/// marker's checksum in `X-Graftyard-Checksum`.
async fn pack(
    c: &Ctx<'_>,
    network: graftyard_types::NetworkId,
    id: graftyard_git::PackId,
) -> std::result::Result<Response, Fail> {
    let marker_key = R2Key::committed(R2Key::Pack { network, id }).expect("pack marker").to_string();
    let pack_key = R2Key::Pack { network, id }.to_string();
    let buckets = crate::buckets(c.env, c.sites).map_err(unavailable)?;
    let mut sum = None;
    for (_, b) in &buckets {
        if let Ok(Some(info)) = b.head(&marker_key).await
            && let Some(m) = graftyard_git::store::marker_from_meta(&info.meta)
        {
            sum = Some(m.checksum);
            break;
        }
    }
    let sum = sum.ok_or(ExportError::NotFound)?;
    for (_, b) in &buckets {
        let Ok(Some(o)) = b.0.get(&pack_key).execute().await else { continue };
        let Some(body) = o.body() else { continue };
        let ResponseBody::Stream(s) = body.response_body().map_err(unavailable)? else { continue };
        let h = crate::headers(c.rid).map_err(unavailable)?;
        h.set("content-type", "application/octet-stream").map_err(unavailable)?;
        h.set("x-graftyard-checksum", &export::checksum_header(&sum)).map_err(unavailable)?;
        return Ok(Response::from_body(ResponseBody::Stream(s)).map_err(unavailable)?.with_headers(h));
    }
    Err(ExportError::Gone(format!("pack {id} is marked committed and in no bucket")).into())
}
