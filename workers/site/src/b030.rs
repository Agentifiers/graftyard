//! Test installs: B0-30's storage readings on a repository member (`build/BUILD-PLAN.md`, B0-30;
//! `tests/am1/results/b0-30.md`), all under `/bench_seed {"b030": {..}}`:
//!
//! - `{"size": true}`: the member's SQLite size as `ctx.storage.sql.databaseSize` reports it, read as an `f64`
//!   (workers-rs 0.8.7's `database_size()` is `as usize`, which saturates at 4 GiB on wasm32), the stored log's rows
//!   and bytes, the `request` rows, and the archive ranges this member applied.
//! - `{"archives": {"after": i, "n": k}}`: the `archive_keys` rows with `from_idx > after`, in order.
//! - `{"verify": {"key": k}}`: the archive object `k` read whole from each of the record's first two buckets: size,
//!   SHA-256 against the key's digest, header, contiguous indexes from the key's `from` to its `to`, and each entry's
//!   `prev` against the digest of the one before it. Answers the first entry's `prev` and the last entry's digest, so
//!   a reader chains ranges across objects.
//! - `{"at": t}`: the `RestoredTo` target for time `t` (`design/SHARED.md`, `repo-restore`: "the last index whose
//!   `at` is at or before `t`"), read from the archive objects alone.

use std::rc::Rc;

use graftyard_fold::{ARCHIVE_HEADER, check_archive_header, decode_archive_some};
use graftyard_git as git;
use graftyard_raft::{self as raft, codec};
use graftyard_types::{Digest, SiteId};
use serde_json::{Value, json};
use sha2::Digest as _;
use worker::*;

use crate::family::Repo;
use crate::host::{Host, err, i, int, rows, text};

type Entry = raft::LogEntry<git::repo::RepoEvent>;

pub async fn route(h: &Rc<Host<Repo>>, b: &Value) -> Result<Value> {
    if b.get("size").is_some() {
        return size(h);
    }
    if let Some(a) = b.get("archives") {
        let sql = h.state.storage().sql();
        let q = "SELECT from_idx, to_idx, to_at, key FROM archive_keys WHERE from_idx > ? ORDER BY from_idx LIMIT ?";
        let out = rows(&sql, q, vec![i(a["after"].as_u64().unwrap_or(0)), i(a["n"].as_u64().unwrap_or(1000))])?
            .iter()
            .map(|r| Ok(json!({ "from": int(&r[0])?, "to": int(&r[1])?, "toAt": int(&r[2])?, "key": text(&r[3])? })))
            .collect::<Result<Vec<_>>>()?;
        return Ok(json!({ "archives": out }));
    }
    if let Some(v) = b.get("verify") {
        let key = v["key"].as_str().ok_or_else(|| err("key"))?;
        return verify(h, key).await;
    }
    if let Some(t) = b.get("at").and_then(Value::as_u64) {
        return at(h, t).await;
    }
    Err(err("b030: size, archives, verify, or at"))
}

/// `ctx.storage.sql.databaseSize` as the `f64` the runtime returns.
pub fn db_bytes(h: &Host<Repo>) -> f64 {
    crate::split::db_bytes(h)
}

fn size(h: &Rc<Host<Repo>>) -> Result<Value> {
    let sql = h.state.storage().sql();
    let one = |q: &str| -> Result<i64> { rows(&sql, q, vec![])?.first().map_or(Ok(0), |r| int(&r[0])) };
    let (role, applied, commit, promoted, split) = {
        let core = h.core.borrow();
        let c = core.as_ref().ok_or_else(|| err("not joined"))?;
        let f = &c.m.group_fold().fold;
        let promoted = f.promoted().map_or(0, |p| p.to.0);
        // The metadata split (`crate::split`): its state, the parent's slim cursor, and a child's restore.
        let split = json!({
            "state": git::split::SplitState::of(&*f.core),
            "slim": f.meta.loaded().then(|| git::split::slim_cursor(&f.meta)).flatten(),
            "child": f.child,
            "childLoaded": f.child && crate::split::child_loaded(f),
        });
        (format!("{:?}", c.m.role()), c.m.state().applied.0, c.m.state().commit.0, promoted, split)
    };
    let above = rows(&sql, "SELECT COALESCE(SUM(length(row)), 0) FROM log WHERE idx > ?", vec![i(promoted)])?
        .first()
        .map_or(Ok(0), |r| int(&r[0]))?;
    Ok(json!({
        "dbBytes": db_bytes(h),
        "split": split,
        "role": role,
        "applied": applied,
        "commit": commit,
        "logRows": one("SELECT COUNT(*) FROM log")?,
        "logBytes": one("SELECT COALESCE(SUM(length(row)), 0) FROM log")?,
        "logFirst": one("SELECT COALESCE(MIN(idx), 0) FROM log")?,
        "requestRows": one("SELECT COUNT(*) FROM request")?,
        "requestBytes": one("SELECT COALESCE(SUM(length(events)), 0) FROM request")?,
        "archiveRanges": one("SELECT COUNT(*) FROM archive_keys")?,
        "archivedTo": one("SELECT COALESCE(MAX(to_idx), 0) FROM archive_keys")?,
        // hot-shards: the log above the last `Promoted`, what a rehydrate's tail reads.
        "promotedTo": promoted,
        "unpromotedBytes": above,
        "sqliteFull": h.stats.borrow().sqlite_full,
        "archiveSteps": h.stats.borrow().archive_steps,
        "keepLed": h.stats.borrow().keep_led,
        "truncatedTo": h.stats.borrow().truncated_to,
        "alarms": h.stats.borrow().alarms.iter().rev().take(5).cloned().collect::<Vec<_>>(),
    }))
}

fn sites(h: &Rc<Host<Repo>>) -> Result<Vec<SiteId>> {
    let core = h.core.borrow();
    let c = core.as_ref().ok_or_else(|| err("not joined"))?;
    Ok(c.join.record.sites.iter().take(2).copied().collect())
}

async fn get(h: &Rc<Host<Repo>>, s: SiteId, key: &str) -> Result<Vec<u8>> {
    let o = h.env.bucket(&format!("BUCKET_{}", s.0))?.get(key).execute().await?;
    let o = o.ok_or_else(|| err(format!("{key} is not in bucket {}", s.0)))?;
    o.body().ok_or_else(|| err("no body"))?.bytes().await
}

/// The entries of an archive object, after its header.
fn entries(b: &[u8]) -> std::result::Result<Vec<Entry>, String> {
    check_archive_header(b).map_err(|e| e.to_string())?;
    let (rs, used, _) = decode_archive_some(&b[ARCHIVE_HEADER..]);
    if ARCHIVE_HEADER + used != b.len() {
        return Err(format!("{} trailing bytes", b.len() - ARCHIVE_HEADER - used));
    }
    rs.into_iter().map(|r| codec::from_bytes::<Entry>(r).map_err(|e| e.to_string())).collect()
}

async fn verify(h: &Rc<Host<Repo>>, key: &str) -> Result<Value> {
    let Ok(git::R2Key::Archive { from, to, digest, .. }) = key.parse::<git::R2Key>() else {
        return Err(err(format!("{key} is not an archive key")));
    };
    let mut copies = Vec::new();
    for s in sites(h)? {
        let t0 = crate::now_ms();
        let b = match get(h, s, key).await {
            Ok(b) => b,
            Err(e) => {
                copies.push(json!({ "site": s.0, "ok": false, "why": e.to_string() }));
                continue;
            }
        };
        let ms = crate::now_ms() - t0;
        let sha = Digest(sha2::Sha256::digest(&b).into());
        let mut why = Vec::new();
        if sha != digest {
            why.push(format!("sha256 {} against the key's {}", sha.hex(), digest.hex()));
        }
        let (mut first_prev, mut last, mut n) = (None, None, 0);
        match entries(&b) {
            Err(e) => why.push(e),
            Ok(es) => {
                n = es.len();
                let mut prev: Option<&Entry> = None;
                for (k, e) in es.iter().enumerate() {
                    if e.index.0 != from.0 + k as u64 {
                        why.push(format!("entry {k} is index {}, not {}", e.index.0, from.0 + k as u64));
                        break;
                    }
                    match prev {
                        None => first_prev = Some(e.prev),
                        Some(p) if raft::digest(p) != e.prev => {
                            why.push(format!("entry {}'s prev is not the digest of {}", e.index.0, p.index.0));
                            break;
                        }
                        Some(_) => {}
                    }
                    prev = Some(e);
                }
                if es.last().map(|e| e.index) != Some(to) {
                    why.push(format!("the object ends at {:?}, the key at {}", es.last().map(|e| e.index.0), to.0));
                }
                last = es.last().map(raft::digest);
            }
        }
        copies.push(json!({
            "site": s.0, "ok": why.is_empty(), "why": why, "bytes": b.len(), "ms": ms, "entries": n,
            "firstPrev": first_prev.map(|d| d.hex()), "lastDigest": last.map(|d| d.hex()),
        }));
    }
    let ok = copies.len() == 2 && copies.iter().all(|c| c["ok"] == true);
    Ok(json!({ "key": key, "from": from.0, "to": to.0, "ok": ok, "copies": copies }))
}

/// The last archived entry whose `at` is at or before `t`: the first range whose `to_at` reaches `t`, read from a
/// bucket; when that range's first entry is already past `t`, the entry is the previous range's last.
async fn at(h: &Rc<Host<Repo>>, t: u64) -> Result<Value> {
    let sql = h.state.storage().sql();
    let q = "SELECT from_idx, to_idx, key FROM archive_keys WHERE to_at >= ? ORDER BY from_idx LIMIT 1";
    let Some(r) = rows(&sql, q, vec![i(t)])?.into_iter().next() else {
        return Ok(json!({ "at": t, "found": null, "why": "no archived range reaches t" }));
    };
    let key = text(&r[2])?.to_owned();
    let s = sites(h)?[0];
    let es = entries(&get(h, s, &key).await?).map_err(err)?;
    match es.iter().rev().find(|e| e.at.0 <= t) {
        Some(e) => Ok(json!({
            "at": t, "key": key, "index": e.index.0, "entryAt": e.at.0, "digest": raft::digest(e).hex(),
            "nextAt": es.iter().find(|x| x.index.0 == e.index.0 + 1).map(|x| x.at.0),
        })),
        None => Ok(json!({ "at": t, "key": key, "index": int(&r[0])? - 1, "why": "before this range's first entry" })),
    }
}
