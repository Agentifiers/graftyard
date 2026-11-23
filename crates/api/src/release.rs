//! `rest:releases`, `uploads-release-assets`, and `codeload-archives` (`design/SHARED.md`, "Surfaces, tiers, and
//! oracles"): the routes gh 2.102.1's `gh release` commands sent in the capture (`lfs/requests.json`, `lfs/README.md`).
//!
//! A read renders from one fresh view. A write is two calls around a commit, as `graphql::plan` and `graphql::render`
//! are: `plan_*` turns the request into an `IssueOp` or answers it, and `created`, `updated`, and `deleted` render the
//! committed command's reply from its `&Acked` and a fresh read after it. An upload's `IssueOp::PutAsset` takes a
//! `DurableBlob`, which only `BlobStore::place` builds, so its 201 comes after two buckets hold the bytes (BF-25).

use graftyard_git::blob::{ArchiveFormat, BlobError, DurableBlob};
use graftyard_git::issue_ops::{IssueErr, IssueOp, IssueOut};
use graftyard_git::issues::{AssetId, RepoReleaseId};
use graftyard_git::refs::{ObjectId, RefName};
use graftyard_git::tiers::{AssetRow, ReleaseRow};
use graftyard_raft::Acked;
use graftyard_types::DbId;
use serde_json::{Value, json};

use crate::view::Repo;
use crate::wire::{FieldError, NodeType, Reply, iso, scoped, simple_user};

const DOC: &str = "rest/releases/releases";
const ASSET_DOC: &str = "rest/releases/assets";

fn tag_of(r: &ReleaseRow) -> String {
    let s = r.rec.tag.display();
    s.strip_prefix("refs/tags/").map(str::to_owned).unwrap_or(s)
}

/// The pinned `release-asset` schema's object.
pub fn asset_json(r: &Repo<'_>, id: AssetId, a: &AssetRow) -> Value {
    let api = r.host.api();
    let (o, n) = (&r.at.owner, &r.at.name);
    let tag = r.release(a.release).map(|rel| tag_of(&rel)).unwrap_or_default();
    json!({
        "url": format!("{api}/repos/{o}/{n}/releases/assets/{}", id.0.0),
        "browser_download_url": format!("{}/{o}/{n}/releases/download/{tag}/{}", r.host.web(), a.rec.name),
        "id": id.0.0,
        "node_id": scoped(NodeType::ReleaseAsset, r, id.0.0),
        "name": &*a.rec.name,
        "label": a.rec.label.as_deref(),
        "state": "uploaded",
        "content_type": &*a.rec.content_type,
        "size": a.rec.blob.bytes,
        "digest": format!("sha256:{}", a.rec.blob.oid.hex()),
        "download_count": 0,
        "created_at": iso(a.created),
        "updated_at": iso(a.updated),
        "uploader": simple_user(r.host, r.dir, a.uploader),
    })
}

/// The pinned `release` schema's object.
pub fn release_json(r: &Repo<'_>, id: RepoReleaseId, rel: &ReleaseRow) -> Value {
    let api = r.host.api();
    let (o, n) = (&r.at.owner, &r.at.name);
    let tag = tag_of(rel);
    let assets: Vec<Value> = r.assets_of(id).iter().map(|(aid, a)| asset_json(r, *aid, a)).collect();
    json!({
        "url": format!("{api}/repos/{o}/{n}/releases/{}", id.0.0),
        "html_url": format!("{}/{o}/{n}/releases/tag/{tag}", r.host.web()),
        "assets_url": format!("{api}/repos/{o}/{n}/releases/{}/assets", id.0.0),
        "upload_url": format!("{}/repos/{o}/{n}/releases/{}/assets{{?name,label}}", r.host.uploads(), id.0.0),
        "tarball_url": (!rel.rec.draft).then(|| format!("{api}/repos/{o}/{n}/tarball/{tag}")),
        "zipball_url": (!rel.rec.draft).then(|| format!("{api}/repos/{o}/{n}/zipball/{tag}")),
        "id": id.0.0,
        "node_id": scoped(NodeType::Release, r, id.0.0),
        "tag_name": tag,
        "target_commitish": rel.rec.target.hex(),
        "name": (!rel.rec.name.is_empty()).then_some(&*rel.rec.name),
        "body": (!rel.rec.body.is_empty()).then_some(&*rel.rec.body),
        "draft": rel.rec.draft,
        "prerelease": rel.rec.prerelease,
        "immutable": false,
        "created_at": iso(rel.created),
        "published_at": rel.published.map(iso),
        "author": simple_user(r.host, r.dir, rel.rec.author),
        "assets": assets,
    })
}

/// `GET` and `HEAD /repos/{o}/{r}/releases/tags/{tag}`: published releases only, as GitHub hides a draft from the tag
/// lookup. gh sends a `HEAD` before a create to see whether the tag has one.
pub fn get_by_tag(r: &Repo<'_>, tag: &str) -> Reply {
    match r.release_by_tag(tag).filter(|(_, rel)| !rel.rec.draft) {
        Some((id, rel)) => Reply::json(200, release_json(r, id, &rel)),
        None => Reply::not_found(),
    }
}

/// `GET /repos/{o}/{r}/releases/{id}`.
pub fn get(r: &Repo<'_>, id: i64) -> Reply {
    let id = RepoReleaseId(DbId(id));
    match r.release(id) {
        Some(rel) => Reply::json(200, release_json(r, id, &rel)),
        None => Reply::not_found(),
    }
}

/// What `GET /repos/{o}/{r}/releases/assets/{id}` answers.
pub enum AssetGet {
    Reply(Reply),
    /// `Accept: application/octet-stream`: the asset's bytes, which the edge reads with `BlobStore::read` and renders
    /// with `asset_bytes`.
    Bytes(Box<AssetRow>),
}

pub fn get_asset(r: &Repo<'_>, id: i64, accept: Option<&str>) -> AssetGet {
    let id = AssetId(DbId(id));
    let Some(a) = r.asset(id) else { return AssetGet::Reply(Reply::not_found()) };
    if accept.is_some_and(|h| h.split(',').any(|m| m.trim() == "application/octet-stream")) {
        return AssetGet::Bytes(Box::new(a));
    }
    AssetGet::Reply(Reply::json(200, asset_json(r, id, &a)))
}

/// The asset's bytes, once the edge read them from a bucket.
pub fn asset_bytes(a: &AssetRow, bytes: Vec<u8>) -> Reply {
    Reply::bytes(200, &a.rec.content_type, bytes)
        .with("content-disposition", format!("attachment; filename={}", a.rec.name))
}

fn invalid(field: &'static str, code: &'static str) -> Reply {
    let e = FieldError { resource: "Release", field, code };
    Reply::json(422, crate::wire::validation_error("Validation Failed", DOC, &[e]))
}

fn str_field<'a>(b: &'a Value, k: &str) -> Option<&'a str> {
    b.get(k).and_then(Value::as_str)
}

fn bool_field(b: &Value, k: &str) -> Option<bool> {
    b.get(k).and_then(Value::as_bool)
}

/// A ref `target_commitish` names: a full SHA, or a branch or tag name.
fn commitish(r: &Repo<'_>, s: &str) -> Option<ObjectId> {
    if let Some(o) = ObjectId::parse_hex(s.as_bytes()) {
        return Some(o);
    }
    ["refs/heads/", "refs/tags/", ""]
        .iter()
        .filter_map(|p| RefName::parse(format!("{p}{s}").as_bytes()).ok())
        .find_map(|n| r.ref_value(&n))
}

/// The default branch's tip, which a release without `target_commitish` targets.
fn default_tip(r: &Repo<'_>) -> Option<ObjectId> {
    r.ref_value(&r.settings()?.default_branch)
}

/// `generate_release_notes: true` with no `name` or `body`: GitHub writes notes from the merged pull requests between
/// this tag and the previous release. This answers with the tag as the name and an empty body [inference: the notes
/// need the git cache's commit walk, which no fold row holds; gh shows what the server returns].
fn generated(tag: &str) -> (String, String) {
    (tag.to_owned(), String::new())
}

/// `POST /repos/{o}/{r}/releases`. `Err` is the reply when nothing commits.
pub fn plan_create(r: &Repo<'_>, body: &Value) -> Result<IssueOp, Reply> {
    let Some(tag) = str_field(body, "tag_name").filter(|t| !t.is_empty()) else {
        return Err(invalid("tag_name", "missing_field"));
    };
    let Ok(tag_ref) = RefName::parse(format!("refs/tags/{tag}").as_bytes()) else {
        return Err(invalid("tag_name", "invalid"));
    };
    let target = match str_field(body, "target_commitish") {
        Some(t) => commitish(r, t).ok_or_else(|| invalid("target_commitish", "invalid"))?,
        None => {
            r.ref_value(&tag_ref).or_else(|| default_tip(r)).ok_or_else(|| invalid("target_commitish", "invalid"))?
        }
    };
    let (mut name, mut text) =
        (str_field(body, "name").unwrap_or("").to_owned(), str_field(body, "body").unwrap_or("").to_owned());
    if bool_field(body, "generate_release_notes") == Some(true) {
        let (n, b) = generated(tag);
        if name.is_empty() {
            name = n;
        }
        if text.is_empty() {
            text = b;
        }
    }
    Ok(IssueOp::PutRelease {
        id: None,
        tag: tag_ref,
        target,
        name: name.into(),
        body: text.into(),
        draft: bool_field(body, "draft").unwrap_or(false),
        prerelease: bool_field(body, "prerelease").unwrap_or(false),
        latest: str_field(body, "make_latest") != Some("false"),
    })
}

/// `PATCH /repos/{o}/{r}/releases/{id}`: every field the body leaves out keeps its value. gh publishes a draft with
/// `{draft: false}` after its uploads, and `gh release edit` sends `{name, tag_name}`.
pub fn plan_update(r: &Repo<'_>, id: i64, body: &Value) -> Result<IssueOp, Reply> {
    let rid = RepoReleaseId(DbId(id));
    let Some(old) = r.release(rid) else { return Err(Reply::not_found()) };
    let tag = match str_field(body, "tag_name") {
        Some(t) => RefName::parse(format!("refs/tags/{t}").as_bytes()).map_err(|_| invalid("tag_name", "invalid"))?,
        None => old.rec.tag.clone(),
    };
    let target = match str_field(body, "target_commitish") {
        Some(t) => commitish(r, t).ok_or_else(|| invalid("target_commitish", "invalid"))?,
        None => old.rec.target,
    };
    let keep = |k: &str, v: &str| str_field(body, k).unwrap_or(v).into();
    Ok(IssueOp::PutRelease {
        id: Some(rid),
        tag,
        target,
        name: keep("name", &old.rec.name),
        body: keep("body", &old.rec.body),
        draft: bool_field(body, "draft").unwrap_or(old.rec.draft),
        prerelease: bool_field(body, "prerelease").unwrap_or(old.rec.prerelease),
        latest: str_field(body, "make_latest").map_or(old.rec.latest, |m| m != "false"),
    })
}

/// `DELETE /repos/{o}/{r}/releases/{id}`. The tag stays; `gh release delete --cleanup-tag` deletes it next with `DELETE
/// /git/refs/tags/{tag}`, a push (`rest:git`).
pub fn plan_delete(r: &Repo<'_>, id: i64) -> Result<IssueOp, Reply> {
    let rid = RepoReleaseId(DbId(id));
    r.release(rid).ok_or_else(Reply::not_found)?;
    Ok(IssueOp::DeleteRelease { id: rid })
}

/// `DELETE /repos/{o}/{r}/releases/assets/{id}`, which `--clobber` sends before it uploads the same name again.
pub fn plan_delete_asset(r: &Repo<'_>, id: i64) -> Result<IssueOp, Reply> {
    let aid = AssetId(DbId(id));
    r.asset(aid).ok_or_else(Reply::not_found)?;
    Ok(IssueOp::DeleteAsset { id: aid })
}

/// What an upload names, before the edge places the bytes: `POST /api/uploads/repos/{o}/{r}/releases/{id}/assets
/// ?name=&label=` with the bytes as the body and their media type as `Content-Type`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Upload {
    pub release: RepoReleaseId,
    pub name: String,
    pub label: Option<String>,
    pub content_type: String,
}

/// Checks an upload before the edge streams its bytes to the buckets, so a refused one places nothing.
pub fn check_upload(r: &Repo<'_>, release: i64, query: &str, content_type: Option<&str>) -> Result<Upload, Reply> {
    let rid = RepoReleaseId(DbId(release));
    r.release(rid).ok_or_else(Reply::not_found)?;
    let q = |k: &str| query.split('&').find_map(|p| p.split_once('=').filter(|(a, _)| *a == k).map(|(_, v)| decode(v)));
    let Some(name) = q("name").filter(|n| !n.is_empty()) else {
        let e = FieldError { resource: "ReleaseAsset", field: "name", code: "missing_field" };
        return Err(Reply::json(422, crate::wire::validation_error("Validation Failed", ASSET_DOC, &[e])));
    };
    if r.assets_of(rid).iter().any(|(_, a)| *a.rec.name == *name) {
        let e = FieldError { resource: "ReleaseAsset", field: "name", code: "already_exists" };
        return Err(Reply::json(422, crate::wire::validation_error("Validation Failed", ASSET_DOC, &[e])));
    }
    Ok(Upload {
        release: rid,
        name,
        label: q("label").filter(|l| !l.is_empty()),
        content_type: content_type.unwrap_or("application/octet-stream").to_owned(),
    })
}

/// The reply when `BlobStore::place` did not reach two buckets: nothing commits and the client retries.
pub fn upload_failed(e: &BlobError) -> Reply {
    match e {
        BlobError::TooLarge => {
            let e = FieldError { resource: "ReleaseAsset", field: "size", code: "invalid" };
            Reply::json(422, crate::wire::validation_error("Validation Failed", ASSET_DOC, &[e]))
        }
        BlobError::Mismatch | BlobError::Place(_) => Reply::error(503, "Service Unavailable", ASSET_DOC),
    }
}

/// The command an upload commits once its bytes are a `DurableBlob`.
pub fn plan_upload(u: Upload, blob: DurableBlob) -> IssueOp {
    IssueOp::PutAsset {
        release: u.release,
        name: u.name.into(),
        label: u.label.map(Into::into),
        content_type: u.content_type.into(),
        blob,
    }
}

/// `%XX` and `+` in a query value.
pub(crate) fn decode(v: &str) -> String {
    let b = v.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = b.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(std::str::from_utf8(h).ok()?, 16).ok());
        match (b[i], hex) {
            (b'%', Some(x)) => {
                out.push(x);
                i += 2;
            }
            (b'+', _) => out.push(b' '),
            (c, _) => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The reply to a committed create or upload: 201 with the object, from its `&Acked` and a fresh read after it.
pub fn created(acked: &Acked<IssueOut>, after: &Repo<'_>) -> Reply {
    let IssueOut::Id(id) = acked.out else { return Reply::error(500, "Internal Server Error", "rest") };
    if let Some(rel) = after.release(RepoReleaseId(id)) {
        let reply = Reply::json(201, release_json(after, RepoReleaseId(id), &rel));
        let loc = format!("{}/repos/{}/{}/releases/{}", after.host.api(), after.at.owner, after.at.name, id.0);
        return reply.with("location", loc);
    }
    match after.asset(AssetId(id)) {
        Some(a) => Reply::json(201, asset_json(after, AssetId(id), &a)),
        None => Reply::error(500, "Internal Server Error", "rest"),
    }
}

/// The reply to a committed `PATCH`: 200 with the release.
pub fn updated(acked: &Acked<IssueOut>, after: &Repo<'_>) -> Reply {
    let IssueOut::Id(id) = acked.out else { return Reply::error(500, "Internal Server Error", "rest") };
    match after.release(RepoReleaseId(id)) {
        Some(rel) => Reply::json(200, release_json(after, RepoReleaseId(id), &rel)),
        None => Reply::error(500, "Internal Server Error", "rest"),
    }
}

/// The reply to a committed delete: 204.
pub fn deleted<T>(_acked: &Acked<T>) -> Reply {
    Reply::empty(204)
}

/// A refused release command's reply.
pub fn refused(e: &IssueErr) -> Reply {
    match e {
        IssueErr::Unauthorized(_) => Reply::error(401, "Bad credentials", "rest"),
        IssueErr::Forbidden => Reply::error(403, "Resource not accessible by integration", DOC),
        IssueErr::Agent(a) => Reply::error(403, &a.text, DOC),
        IssueErr::NotFound => Reply::not_found(),
        IssueErr::Exists => invalid("tag_name", "already_exists"),
        IssueErr::Invalid(_) => invalid("tag_name", "invalid"),
        IssueErr::NeedIdBlock | IssueErr::Unavailable => Reply::error(503, "Service Unavailable", "rest"),
        _ => Reply::error(422, "Unprocessable Entity", DOC),
    }
}

// ---- codeload-archives ----

/// `GET /repos/{o}/{r}/tarball/{ref}` and `zipball/{ref}`: a 302 to codeload, as GitHub answers
/// (`research/wire-census.md` #24). gh follows it.
pub fn archive_redirect(r: &Repo<'_>, format: ArchiveFormat, reference: &str) -> Reply {
    if resolve(r, reference).is_none() {
        return Reply::not_found();
    }
    let path = if reference.starts_with("refs/") || ObjectId::parse_hex(reference.as_bytes()).is_some() {
        reference.to_owned()
    } else if r.ref_value(&tag_name(reference)).is_some() {
        format!("refs/tags/{reference}")
    } else {
        format!("refs/heads/{reference}")
    };
    let loc = format!("{}/legacy.{}/{path}", r.host.codeload(&r.at.owner, &r.at.name), format.ext());
    Reply::empty(302).with("location", loc)
}

fn tag_name(t: &str) -> RefName {
    RefName::parse(format!("refs/tags/{t}").as_bytes()).unwrap_or_else(|_| RefName::parse(b"refs/tags/-").unwrap())
}

/// The commit a codeload ref names: a full SHA, a full ref, or a short tag or branch name.
pub fn resolve(r: &Repo<'_>, reference: &str) -> Option<ObjectId> {
    if let Some(o) = ObjectId::parse_hex(reference.as_bytes()) {
        return Some(o);
    }
    ["", "refs/tags/", "refs/heads/"]
        .iter()
        .filter_map(|p| RefName::parse(format!("{p}{reference}").as_bytes()).ok())
        .find_map(|n| r.ref_value(&n))
}

/// A codeload request: `legacy.tar.gz/{ref}` or `legacy.zip/{ref}`, resolved to the commit and the archive's top
/// directory `{repo}-{ref}/`, with `/` in the ref as `-`, as GitHub names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Codeload {
    pub commit: ObjectId,
    pub format: ArchiveFormat,
    pub prefix: String,
    pub filename: String,
}

pub fn codeload(r: &Repo<'_>, rest: &str) -> Result<Codeload, Reply> {
    let (fmt, reference) = rest.split_once('/').ok_or_else(Reply::not_found)?;
    let format = match fmt {
        "legacy.tar.gz" | "tar.gz" => ArchiveFormat::TarGz,
        "legacy.zip" | "zip" => ArchiveFormat::Zip,
        _ => return Err(Reply::not_found()),
    };
    let commit = resolve(r, reference).ok_or_else(Reply::not_found)?;
    let short =
        reference.strip_prefix("refs/tags/").or_else(|| reference.strip_prefix("refs/heads/")).unwrap_or(reference);
    let label = short.replace('/', "-");
    let prefix = format!("{}-{label}/", r.at.name);
    let filename = format!("{}-{}-{label}.{}", r.at.owner, r.at.name, format.ext());
    Ok(Codeload { commit, format, prefix, filename })
}

/// The archive's reply, once `graftyard_git::blob::archive` returned its bytes.
pub fn archive_reply(c: &Codeload, bytes: Vec<u8>) -> Reply {
    Reply::bytes(200, c.format.content_type(), bytes)
        .with("content-disposition", format!("attachment; filename={}", c.filename))
}
