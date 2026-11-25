//! `actions-cache` on an Actions shard's leader (`decisions/DECISIONS.md`, "`actions-cache`";
//! `graftyard_actions::cache`). The edge authenticates the job token and sends each call to `/runsvc` with
//! `op: "cache_*"`; this module answers them from the fold or commits the 0x15.6 command, and only where a site day set
//! `GRAFTYARD_ACTIONS_CACHE` (`crate::actions::actions_cache`), so no member sends 0x15.6 before `FormatsEnable`.
//!
//! - **`cache_lookup`** `{keys, version, default_branch}`: the entry `ActionsFold::cache_find` restores, with its
//!   signed download query (`cache::sign_download`), and `used: true` when its last use is over `CACHE_USED_EVERY_MS`
//!   old, which sends a `CacheUse` after the answer. Nothing is committed for a miss.
//! - **`cache_list`** `{key}`: the toolkit's diagnostic list, the repository's committed entries with that key.
//! - **`cache_reserve`** `{key, version, size, default_branch}`, **`cache_chunk`** `{id}` (whether the job may upload
//!   to a reserved entry, and its chunk prefix), and **`cache_commit`** `{id, size, chunks, limit}` commit or check.
//! - **Chunks of removed entries.** After any commit that removed entries (LRU, the cap, a stale reservation, the
//!   sweep), the leader deletes their chunk prefixes from the owner's home bucket, `BUCKET_<home>`, the one the edge
//!   writes chunks to; once a day per shard and isolate, `cleanup` deletes any prefix no live entry names.
//! - **The sweep** in the leader's alarm commits `CacheSweep` once the earliest entry is due, and arms the alarm for
//!   the next one.
//! - **A repository delete** (F125): the `Audit` finisher's delete, once the chain holds its tombstone, reads whether
//!   the repository still holds entries here or artifacts in the home bucket (`repo_held`), and drops them
//!   (`drop_repo`): `cache_drop_repo` on the shard's leader commits `CacheDropRepo`, whose `CacheEvicted` names every
//!   entry of that repository and no other's, and deletes their chunks after it as any eviction does; the finisher then
//!   deletes the repository's artifact prefix. A leader built before it answers the unknown op 401, so the batch fails
//!   and the finisher runs it again after `RETRY_MS`.

use std::rc::Rc;

use graftyard_actions::JobId;
use graftyard_actions::cache::{self, CacheId, CacheState};
use graftyard_actions::runsvc::{ServiceKey, Urls};
use graftyard_actions::shard::{ActionsActor, ActionsErr, ActionsOp, ActionsOut};
use graftyard_raft::Role;
use graftyard_types::{ChainId, Timestamp};
use serde_json::{Value, json};

use crate::actions::{Actions, actions_cache, commit, fresh_rid, refused, rid};
use crate::host::Host;

thread_local! {
    /// When this isolate last ran `cleanup` for each shard.
    static CLEANED: std::cell::RefCell<std::collections::BTreeMap<ChainId, u64>> = Default::default();
}

fn entry_json(id: CacheId, e: &cache::CacheEntry) -> Value {
    json!({
        "cacheKey": e.key,
        "scope": e.scope,
        "cacheVersion": e.version,
        "creationTime": graftyard_wire::time::iso(e.created),
        "id": id.0,
        "size": e.size,
    })
}

fn cache_refusal(e: ActionsErr) -> Value {
    match e {
        ActionsErr::CacheExists => refused(409, "Cache already exists or is being created"),
        ActionsErr::CacheRefused(m) => refused(400, m),
        ActionsErr::NotFound => refused(404, "Cache not found"),
        ActionsErr::Forbidden => refused(403, "The job may not write this cache"),
        e => refused(422, e),
    }
}

/// One `cache_*` call from the edge for job `job`.
#[allow(clippy::too_many_arguments)]
pub async fn route(
    h: &Rc<Host<Actions>>,
    op: &str,
    job: JobId,
    b: &Value,
    key: &ServiceKey,
    urls: &Urls,
    chain: ChainId,
    now: u64,
) -> Value {
    if !actions_cache(&h.env) {
        return refused(404, "actions-cache is not enabled on this install");
    }
    let s = |k: &str| b[k].as_str().unwrap_or_default().to_owned();
    match op {
        "cache_lookup" => {
            let keys: Vec<String> = serde_json::from_value(b["keys"].clone()).unwrap_or_default();
            let keys: Vec<&str> = keys.iter().map(String::as_str).collect();
            let (version, default_branch) = (s("version"), s("default_branch"));
            let found = {
                let core = h.core.borrow();
                let Some(c) = core.as_ref() else { return json!({ "error": { "notLeader": {} } }) };
                let f = &c.m.group_fold().fold;
                match f.cache_find(job, &keys, &version, &default_branch) {
                    Ok(Some((id, e))) => Ok(Some((id, e.clone()))),
                    Ok(None) => Ok(None),
                    Err(e) => Err(e),
                }
            };
            match found {
                Err(e) => cache_refusal(e),
                Ok(None) => json!({ "status": 204, "body": null }),
                Ok(Some((id, e))) => {
                    let exp = now / 1000 + cache::CACHE_URL_LIFE_S;
                    let q = cache::sign_download(key, chain, id, e.size, exp);
                    let ChainId::Actions(owner, shard) = chain else { return refused(400, "not an Actions shard") };
                    let mut body = entry_json(id, &e);
                    body["archiveLocation"] =
                        json!(format!("{}cache/download/{}/{shard}/{}?{q}", urls.services, owner.0.0, id.0));
                    let used = now.saturating_sub(e.last_used.0) >= cache::CACHE_USED_EVERY_MS;
                    if used {
                        // After the answer: a lost `CacheUse` only moves the entry's last use an hour later.
                        let h = h.clone();
                        worker::wasm_bindgen_futures::spawn_local(async move {
                            let rid = rid(&format!("{chain}/cache-use/{}/{}", id.0, now / cache::CACHE_USED_EVERY_MS));
                            if let Err(e) = commit(&h, rid, ActionsActor::Job(job), ActionsOp::CacheUse { id }).await {
                                worker::console_error!("{chain} CacheUse {}: {e}", id.0);
                            }
                        });
                    }
                    json!({ "status": 200, "body": body })
                }
            }
        }
        "cache_list" => {
            let want = s("key");
            let core = h.core.borrow();
            let Some(c) = core.as_ref() else { return json!({ "error": { "notLeader": {} } }) };
            let f = &c.m.group_fold().fold;
            let run = match f.cache_run(job) {
                Ok(r) => r,
                Err(e) => return cache_refusal(e),
            };
            let caches: Vec<Value> = f
                .more
                .more
                .entries
                .iter()
                .filter(|(_, e)| e.repo == run.repo && e.committed() && *e.key == *want)
                .map(|(id, e)| entry_json(*id, e))
                .collect();
            json!({ "status": 200, "body": { "totalCount": caches.len(), "artifactCaches": caches } })
        }
        "cache_reserve" => {
            let op = ActionsOp::CacheReserve {
                job,
                key: s("key").into(),
                version: s("version").into(),
                size: b["size"].as_u64().unwrap_or(0),
                default_branch: s("default_branch").into(),
            };
            match commit_then_delete(h, chain, fresh_rid(), ActionsActor::Job(job), op).await {
                Ok(Ok(ActionsOut::Cache(id))) => json!({ "status": 201, "body": { "cacheId": id.0 } }),
                Ok(Ok(o)) => refused(500, o),
                Ok(Err(e)) => cache_refusal(e),
                Err(e) => refused(503, e),
            }
        }
        // Whether this job may upload a chunk to entry `id`, and where: the edge asks before it streams the body.
        "cache_chunk" => {
            let Some(id) = b["id"].as_u64().map(CacheId) else { return refused(400, "id") };
            let core = h.core.borrow();
            let Some(c) = core.as_ref() else { return json!({ "error": { "notLeader": {} } }) };
            let f = &c.m.group_fold().fold;
            if let Err(e) = f.cache_run(job) {
                return cache_refusal(e);
            }
            match f.more.more.entries.get(&id) {
                Some(e) if e.state == (CacheState::Reserved { job }) && e.holds(Timestamp(now)) => {
                    json!({ "status": 200, "body": { "prefix": cache::chunk_prefix(chain, id), "size": e.size } })
                }
                Some(_) => refused(409, "Cache is not reserved by this job"),
                None => refused(404, "Cache not found"),
            }
        }
        "cache_commit" => {
            let Some(id) = b["id"].as_u64().map(CacheId) else { return refused(400, "id") };
            let chunks: Vec<(u64, u64)> = serde_json::from_value(b["chunks"].clone()).unwrap_or_default();
            let limit = b["limit"].as_u64().unwrap_or(cache::CACHE_REPO_MAX);
            let size = b["size"].as_u64().unwrap_or(u64::MAX);
            let op = ActionsOp::CacheCommit { job, id, size, chunks, limit };
            match commit_then_delete(h, chain, fresh_rid(), ActionsActor::Job(job), op).await {
                Ok(Ok(_)) => json!({ "status": 204, "body": null }),
                Ok(Err(e)) => cache_refusal(e),
                Err(e) => refused(503, e),
            }
        }
        _ => refused(404, "no such cache call"),
    }
}

/// The live entry ids and the highest id assigned, on the leader.
fn live(h: &Rc<Host<Actions>>) -> Option<(std::collections::BTreeSet<CacheId>, u64)> {
    let core = h.core.borrow();
    let c = core.as_ref().filter(|c| c.m.role() == Role::Leader)?;
    let rows = &c.m.group_fold().fold.more.more;
    Some((rows.entries.keys().copied().collect(), rows.next))
}

/// Commits `op` and deletes, after it, the chunks of every entry its entry removed (live before, gone after). Ids only
/// grow (`CacheRows::next`), so a removed id never returns and deleting its prefix never touches a live entry. A
/// delete lost with the isolate is left to `cleanup`.
async fn commit_then_delete(
    h: &Rc<Host<Actions>>,
    chain: ChainId,
    rid: graftyard_types::RequestId,
    actor: ActionsActor,
    op: ActionsOp,
) -> Result<Result<ActionsOut, ActionsErr>, String> {
    let before = live(h).map(|(ids, _)| ids).unwrap_or_default();
    let r = commit(h, rid, actor, op).await;
    if matches!(r, Ok(Ok(_))) {
        let after = live(h).map(|(ids, _)| ids).unwrap_or_default();
        let gone: Vec<CacheId> = before.difference(&after).copied().collect();
        if !gone.is_empty() {
            let h = h.clone();
            worker::wasm_bindgen_futures::spawn_local(async move {
                let Some(bucket) = home_bucket(&h) else { return };
                for id in gone {
                    if let Err(e) = delete_prefix(&bucket, &cache::chunk_prefix(chain, id)).await {
                        worker::console_error!("{chain} cache {} chunks: {e}", id.0);
                    }
                }
            });
        }
    }
    r
}

fn home_bucket(h: &Rc<Host<Actions>>) -> Option<worker::Bucket> {
    let home = h.core.borrow().as_ref().map(|c| c.join.record.home)?;
    h.env.bucket(&format!("BUCKET_{}", home.0)).ok()
}

/// Once a day per shard and isolate: every chunk prefix under the shard's that no live entry names and whose id was
/// assigned, deleted, so a delete lost with an isolate (or a reservation's chunks the sweep removed elsewhere) is not
/// kept past a day.
async fn cleanup(h: &Rc<Host<Actions>>, chain: ChainId, now: u64) {
    const EVERY_MS: u64 = 24 * 3_600_000;
    if CLEANED.with(|c| c.borrow().get(&chain).is_some_and(|t| now.saturating_sub(*t) < EVERY_MS)) {
        return;
    }
    CLEANED.with(|c| c.borrow_mut().insert(chain, now));
    let (Some((ids, next)), Some(bucket)) = (live(h), home_bucket(h)) else { return };
    use graftyard_git::Bucket as _;
    let r2 = graftyard_git::host::R2(bucket.clone());
    let root = format!("actions/cache/{chain}/");
    let segs = match r2.segments(&root).await {
        Ok(s) => s,
        Err(e) => return worker::console_error!("{chain} cache cleanup: {}", e.0),
    };
    for seg in segs {
        let Ok(id) = seg.parse::<u64>().map(CacheId) else { continue };
        if id.0 <= next
            && !ids.contains(&id)
            && let Err(e) = delete_prefix(&bucket, &cache::chunk_prefix(chain, id)).await
        {
            worker::console_error!("{chain} cache cleanup {}: {e}", id.0);
        }
    }
}

/// `cache_drop_repo` `{repo}` on the shard's leader (F125): every entry of the repository evicted, their chunks deleted
/// after the commit. A retry commits nothing, since `decide` finds no entry left.
pub async fn drop_route(h: &Rc<Host<Actions>>, chain: ChainId, b: &Value) -> Value {
    if !actions_cache(&h.env) {
        return refused(404, "actions-cache is not enabled on this install");
    }
    let Some(repo) = b["repo"].as_i64().map(|r| graftyard_types::RepoId(graftyard_types::DbId(r))) else {
        return refused(400, "repo");
    };
    match commit_then_delete(h, chain, fresh_rid(), ActionsActor::Operator, ActionsOp::CacheDropRepo { repo }).await {
        Ok(Ok(_)) => json!({ "status": 204, "body": null }),
        Ok(Err(e)) => refused(422, e),
        Err(e) => refused(503, e),
    }
}

/// The repository's Actions shard under its owner, and the owner's home bucket.
fn actions_of(
    env: &worker::Env,
    owner: graftyard_types::AccountId,
    record: &graftyard_types::PlacementRecord,
    repo: graftyard_types::RepoId,
) -> Result<(ChainId, worker::Bucket), String> {
    let chain = ChainId::Actions(owner, graftyard_actions::actions_shard(repo));
    let bucket = env.bucket(&format!("BUCKET_{}", record.home.0)).map_err(|e| e.to_string())?;
    Ok((chain, bucket))
}

/// Whether a deleted repository still holds cache entries on its owner's shard or artifacts in its home bucket (F125).
/// A shard with no group holds none; a leader that does not answer `repoCaches` is an error, so the finisher waits for
/// one that does rather than skip the drop.
pub async fn repo_held(
    env: &worker::Env,
    owner: graftyard_types::AccountId,
    record: &graftyard_types::PlacementRecord,
    repo: graftyard_types::RepoId,
) -> Result<bool, String> {
    let (chain, bucket) = actions_of(env, owner, record, repo)?;
    if actions_cache(env) {
        let site = crate::SiteEnv::read(env).map_err(|e| e.to_string())?;
        let b = graftyard_lookup::Bindings { env, sites: &site.sites };
        let q = json!({ "q": { "repoCaches": repo.0.0 } }).to_string();
        if let Some((v, _, _)) = graftyard_lookup::call_opt(&b, chain, record, "/read", q.as_bytes()).await? {
            let n = v["value"]["repoCaches"].as_u64().ok_or_else(|| format!("{chain}: no repoCaches in {v}"))?;
            if n > 0 {
                return Ok(true);
            }
        }
    }
    let l = bucket
        .list()
        .prefix(graftyard_actions::artifacts::repo_prefix(chain, repo))
        .limit(1)
        .execute()
        .await
        .map_err(|e| e.to_string())?;
    Ok(!l.objects().is_empty())
}

/// The drop itself (F125): the shard leader's `cache_drop_repo`, then the artifact prefix. `Err` on any refusal, so
/// the finisher's batch runs again.
pub async fn drop_repo(
    env: &worker::Env,
    b: &graftyard_lookup::Bindings<'_>,
    owner: graftyard_types::AccountId,
    record: &graftyard_types::PlacementRecord,
    repo: graftyard_types::RepoId,
) -> Result<(), String> {
    let (chain, bucket) = actions_of(env, owner, record, repo)?;
    if actions_cache(env) {
        let body = json!({ "op": "cache_drop_repo", "repo": repo.0.0 }).to_string();
        if let Some((v, _, _)) = graftyard_lookup::call_opt(b, chain, record, "/runsvc", body.as_bytes()).await?
            && v["status"].as_u64() != Some(204)
        {
            return Err(format!("{chain} cache_drop_repo {}: {v}", repo.0.0));
        }
    }
    delete_prefix(&bucket, &graftyard_actions::artifacts::repo_prefix(chain, repo)).await
}

async fn delete_prefix(b: &worker::Bucket, prefix: &str) -> Result<(), String> {
    use graftyard_git::Bucket as _;
    let r2 = graftyard_git::host::R2(b.clone());
    let keys: Vec<String> = r2.list(prefix).await.map_err(|e| e.0)?.into_iter().map(|o| o.key).collect();
    for c in keys.chunks(1000) {
        r2.delete(c).await.map_err(|e| e.0)?;
    }
    Ok(())
}

/// The leader's alarm: `CacheSweep` once the earliest entry is due, then an alarm at the next one.
pub async fn sweep(h: &Rc<Host<Actions>>) {
    if !actions_cache(&h.env) {
        return;
    }
    let now = crate::now_ms();
    let found = {
        let core = h.core.borrow();
        let Some(c) = core.as_ref().filter(|c| c.m.role() == Role::Leader) else { return };
        (c.join.chain, c.m.group_fold().fold.more.more.due())
    };
    let (chain, due) = found;
    cleanup(h, chain, now).await;
    let Some(Timestamp(due)) = due else { return };
    if due > now {
        h.arm_in((due - now).min(i64::MAX as u64) as i64);
        return;
    }
    let rid = rid(&format!("{chain}/cache-sweep/{due}"));
    match commit_then_delete(h, chain, rid, ActionsActor::Operator, ActionsOp::CacheSweep).await {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => worker::console_error!("{chain} CacheSweep: {e:?}"),
        Err(e) => {
            worker::console_error!("{chain} CacheSweep: {e}");
            h.arm_in(5_000);
        }
    }
}
