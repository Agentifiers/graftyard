//! `lfs` over HTTPS (`design/SHARED.md`, "Surfaces, tiers, and oracles"): the Batch API, the basic transfer, the
//! `verify` action, and locks, exactly as git-lfs 3.8.0 sent them in the capture (`lfs/requests.json`). Replies are
//! checked against git-lfs's own JSON schemas, vendored in `lfs/schemas/`.
//!
//! An upload is three requests. The batch answers `upload` and `verify` actions for each object no `LfsObject` row
//! holds. The `PUT` to the transfer href places the bytes with `BlobStore::place`, which checks their SHA-256 against
//! the oid and returns only once two buckets hold them. `verify` rebuilds that `DurableBlob` with
//! `BlobStore::confirm`, commits `IssueOp::LfsObject`, and answers 200 from the command's `&Acked`, so `verify` never
//! succeeds while one bucket holds the object (BF-25). A download batch answers `download` only for an object with an
//! `LfsObject` row; the transfer `GET` reads it with `BlobStore::read`, which skips an unreadable bucket.
//!
//! Transfer hrefs point at the edge (`Host::objects`), not at presigned bucket URLs [inference: see `Host::objects`].

use graftyard_git::blob::{BLOB_MAX, BlobError, DurableBlob};
use graftyard_git::issue_ops::{IssueErr, IssueOp, IssueOut};
use graftyard_git::issues::LockId;
use graftyard_git::refs::RefName;
use graftyard_git::tiers::{LfsRow, LockRow};
use graftyard_raft::Acked;
use graftyard_types::{AccountId, DbId, Digest};
use serde_json::{Value, json};

use crate::view::Repo;
use crate::wire::{Reply, iso};

pub const MEDIA: &str = "application/vnd.git-lfs+json";

/// The transfer hrefs' lifetime in seconds [guess: GitHub's batch answers carry `expires_in` of 3600 in the git-lfs
/// test fixtures; the href here is the edge itself, so it stays valid as long as the token in its header].
const EXPIRES_IN: u64 = 3600;

fn lfs(status: u16, v: Value) -> Reply {
    Reply::json(status, v).with("content-type", MEDIA)
}

fn message(status: u16, m: &str) -> Reply {
    lfs(
        status,
        json!({ "message": m, "documentation_url": "https://docs.github.com/enterprise-server@3.22/repositories/working-with-files/managing-large-files" }),
    )
}

/// git-lfs's first request carries no credential; a 401 with a Basic challenge makes it ask the credential helper and
/// retry, as the capture shows. The edge answers it for every LFS route.
pub fn challenge() -> Reply {
    message(401, "Requires authentication").with("www-authenticate", "Basic realm=\"GitHub\"")
}

/// The headers a transfer action carries: the request's own credential, so the edge authenticates the transfer as the
/// batch.
fn action(href: String, authorization: &str) -> Value {
    json!({ "href": href, "header": { "Authorization": authorization }, "expires_in": EXPIRES_IN })
}

fn oid_of(v: &Value) -> Option<Digest> {
    v.get("oid").and_then(Value::as_str).and_then(Digest::parse_hex)
}

/// Where a transfer `PUT` or `GET` goes: `{objects}/{repo id}/{oid}`.
pub fn transfer_href(r: &Repo<'_>, oid: &Digest) -> String {
    format!("{}/{}/{}", r.host.objects(), r.at.id.0.0, oid.hex())
}

/// `POST {repo}.git/info/lfs/objects/batch`. `can_write` is whether the caller may push, which an upload needs.
/// `authorization` is the request's own `Authorization` header, echoed into each action.
pub fn batch(r: &Repo<'_>, body: &Value, can_write: bool, authorization: &str) -> Reply {
    let op = body.get("operation").and_then(Value::as_str);
    let upload = match op {
        Some("upload") => true,
        Some("download") => false,
        _ => return message(422, "operation must be upload or download"),
    };
    if upload && !can_write {
        return message(403, "You must have push access to upload");
    }
    let transfers: Vec<&str> = body
        .get("transfers")
        .and_then(Value::as_array)
        .map_or(vec!["basic"], |a| a.iter().filter_map(Value::as_str).collect());
    if !transfers.contains(&"basic") {
        return message(422, "only the basic transfer is supported");
    }
    if body.get("hash_algo").and_then(Value::as_str).is_some_and(|h| h != "sha256") {
        return message(409, "only sha256 is supported");
    }
    let Some(objects) = body.get("objects").and_then(Value::as_array) else { return message(422, "objects") };
    let verify = format!("{}/{}/{}.git/info/lfs/objects/verify", r.host.web(), r.at.owner, r.at.name);
    let out: Vec<Value> = objects
        .iter()
        .map(|o| {
            let size = o.get("size").and_then(Value::as_u64);
            let raw = o.get("oid").and_then(Value::as_str).unwrap_or("");
            let (Some(oid), Some(size)) = (oid_of(o), size) else {
                return json!({ "oid": raw, "size": size.unwrap_or(0),
                    "error": { "code": 422, "message": "the oid is not a SHA-256 or the size is missing" } });
            };
            let have = r.lfs_object(&oid).filter(|row| row.blob.bytes == size);
            match (upload, have) {
                (true, Some(_)) => json!({ "oid": raw, "size": size, "authenticated": true }),
                (true, None) if size > BLOB_MAX => json!({ "oid": raw, "size": size,
                    "error": { "code": 422, "message": format!("over the {BLOB_MAX}-byte object limit") } }),
                (true, None) => json!({ "oid": raw, "size": size, "authenticated": true, "actions": {
                    "upload": action(transfer_href(r, &oid), authorization),
                    "verify": json!({ "href": verify, "header": { "Authorization": authorization } }),
                } }),
                (false, Some(_)) => json!({ "oid": raw, "size": size, "authenticated": true, "actions": {
                    "download": action(transfer_href(r, &oid), authorization),
                } }),
                (false, None) => json!({ "oid": raw, "size": size,
                    "error": { "code": 404, "message": "Object does not exist on the server" } }),
            }
        })
        .collect();
    lfs(200, json!({ "transfer": "basic", "objects": out, "hash_algo": "sha256" }))
}

/// The transfer path `{repo id}/{oid}`, which `Endpoint::Objects` hands over.
pub fn transfer_path(path: &str) -> Option<(i64, Digest)> {
    let (repo, oid) = path.split_once('/')?;
    Some((repo.parse().ok()?, Digest::parse_hex(oid)?))
}

/// The `GET` of a transfer: the row the bytes come from, which the edge reads with `BlobStore::read`.
pub fn download(r: &Repo<'_>, oid: &Digest) -> Result<LfsRow, Reply> {
    r.lfs_object(oid).ok_or_else(|| message(404, "Object does not exist on the server"))
}

pub fn download_reply(bytes: Vec<u8>) -> Reply {
    Reply::bytes(200, "application/octet-stream", bytes)
}

/// The `PUT` when `BlobStore::place` refused the bytes or did not reach two buckets: nothing is acknowledged, and
/// git-lfs retries the transfer.
pub fn upload_failed(e: &BlobError) -> Reply {
    match e {
        BlobError::Mismatch => message(422, "the bytes do not match the oid and size"),
        BlobError::TooLarge => message(413, "object too large"),
        BlobError::Place(_) => message(503, "Service Unavailable"),
    }
}

/// The `PUT` of a transfer, after `BlobStore::place` held the bytes in two buckets: 200, with nothing committed yet.
/// `verify` commits.
pub fn uploaded(_placed: &DurableBlob) -> Reply {
    Reply::empty(200)
}

/// `POST {repo}.git/info/lfs/objects/verify`: the oid and size whose `DurableBlob` the edge then rebuilds with
/// `BlobStore::confirm`.
pub fn verify_request(body: &Value) -> Result<(Digest, u64), Reply> {
    match (oid_of(body), body.get("size").and_then(Value::as_u64)) {
        (Some(o), Some(s)) => Ok((o, s)),
        _ => Err(message(422, "oid and size are required")),
    }
}

/// The command `verify` commits once `BlobStore::confirm` returned the `DurableBlob`.
pub fn plan_verify(blob: DurableBlob) -> IssueOp {
    IssueOp::LfsObject(blob)
}

/// `verify`'s 200, only from the `LfsObject` command's `&Acked`.
pub fn verified(_acked: &Acked<IssueOut>) -> Reply {
    lfs(200, json!({}))
}

/// `verify` when `BlobStore::confirm` found fewer than two buckets holding the bytes, or other bytes: the object is
/// not on the server, so git-lfs retries the upload.
pub fn not_verified() -> Reply {
    message(404, "Object does not exist on the server")
}

// ---- locks ----

fn lock_json(r: &Repo<'_>, id: LockId, l: &LockRow) -> Value {
    json!({
        "id": id.0.0.to_string(),
        "path": &*l.path,
        "locked_at": iso(l.at),
        "owner": { "name": r.dir.login(l.owner).unwrap_or_else(|| "ghost".into()) },
    })
}

fn query_value<'a>(query: &'a str, k: &str) -> Option<&'a str> {
    query.split('&').find_map(|p| p.split_once('=').filter(|(a, _)| *a == k).map(|(_, v)| v))
}

/// `GET {repo}.git/info/lfs/locks?path=&id=&refspec=&cursor=&limit=`. Locks are repository-wide here, so `refspec`
/// filters nothing [inference: a lock created with a ref is listed for every ref, as one lock per path is enforced].
pub fn list_locks(r: &Repo<'_>, query: &str) -> Reply {
    let path = query_value(query, "path").map(|p| p.replace("%2F", "/"));
    let id = query_value(query, "id");
    let locks: Vec<Value> = r
        .locks()
        .iter()
        .filter(|(i, l)| path.as_deref().is_none_or(|p| *l.path == *p) && id.is_none_or(|x| x == i.0.0.to_string()))
        .map(|(i, l)| lock_json(r, *i, l))
        .collect();
    lfs(200, json!({ "locks": locks }))
}

/// `POST {repo}.git/info/lfs/locks/verify`: the caller's locks and everyone else's. `me` is the caller.
pub fn verify_locks(r: &Repo<'_>, me: Option<AccountId>, can_write: bool) -> Reply {
    if !can_write {
        return message(403, "You must have push access to verify locks");
    }
    let (mut ours, mut theirs) = (Vec::new(), Vec::new());
    for (i, l) in r.locks() {
        if Some(l.owner) == me { ours.push(lock_json(r, i, &l)) } else { theirs.push(lock_json(r, i, &l)) }
    }
    lfs(200, json!({ "ours": ours, "theirs": theirs }))
}

/// `POST {repo}.git/info/lfs/locks` with `{path, ref: {name}}`. A path someone holds answers 409 with that lock, as the
/// spec says, without a commit.
pub fn plan_lock(r: &Repo<'_>, body: &Value) -> Result<IssueOp, Reply> {
    let Some(path) = body.get("path").and_then(Value::as_str).filter(|p| !p.is_empty()) else {
        return Err(message(422, "path is required"));
    };
    if let Some((i, l)) = r.locks().into_iter().find(|(_, l)| *l.path == *path) {
        return Err(lfs(409, json!({ "lock": lock_json(r, i, &l), "message": "already created lock" })));
    }
    let refspec = body.pointer("/ref/name").and_then(Value::as_str).and_then(|n| RefName::parse(n.as_bytes()).ok());
    Ok(IssueOp::LockLfs { path: path.into(), refspec })
}

/// `POST {repo}.git/info/lfs/locks/{id}/unlock` with `{force, ref}`.
pub fn plan_unlock(r: &Repo<'_>, id: &str, body: &Value) -> Result<(IssueOp, Value), Reply> {
    let Some(id) = id.parse().ok().map(|i| LockId(DbId(i))) else { return Err(message(404, "Lock not found")) };
    let Some((_, row)) = r.locks().into_iter().find(|(i, _)| *i == id) else {
        return Err(message(404, "Lock not found"));
    };
    let force = body.get("force").and_then(Value::as_bool).unwrap_or(false);
    // The deleted lock is the reply, so it is rendered before the commit removes the row.
    Ok((IssueOp::UnlockLfs { id, force }, lock_json(r, id, &row)))
}

/// A lock command's reply from its `&Acked`: 201 with the new lock, or 200 with the deleted one.
pub fn locked(acked: &Acked<IssueOut>, after: &Repo<'_>) -> Reply {
    let IssueOut::Id(id) = acked.out else { return message(500, "Internal Server Error") };
    match after.locks().into_iter().find(|(i, _)| i.0 == id) {
        Some((i, l)) => lfs(201, json!({ "lock": lock_json(after, i, &l) })),
        None => message(500, "Internal Server Error"),
    }
}

pub fn unlocked(_acked: &Acked<IssueOut>, lock: Value) -> Reply {
    lfs(200, json!({ "lock": lock }))
}

pub fn refused(e: &IssueErr) -> Reply {
    match e {
        IssueErr::Unauthorized(_) => challenge(),
        IssueErr::Forbidden => message(403, "You must have push access to do that"),
        IssueErr::NotFound => message(404, "Lock not found"),
        IssueErr::Exists => message(409, "already created lock"),
        IssueErr::NeedIdBlock | IssueErr::Unavailable => message(503, "Service Unavailable"),
        _ => message(422, "Validation Failed"),
    }
}
