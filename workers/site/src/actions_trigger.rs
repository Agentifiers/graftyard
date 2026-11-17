//! `ActionsTrigger` for `on: merge_group` (BF-37; `design/SHARED.md`, "Event fan-out", Other consumers; the pure half
//! is `graftyard_actions::trigger`). It runs in band on a repository leader's alarm, as `Notify` and `Search` do
//! (`crate::activity`), with no queue hop.
//!
//! - **Scan.** From the committed `ActionsTrigger` cursor, or from where this leadership's scan reached, it reads at
//!   most `BATCH` committed entries from the leader's log and lists the queue refs each `GroupBuilt` built.
//! - **Runs.** For each one it lists `.github/workflows` at the ref's tip on this site's git cache shard (`/ui/tree`,
//!   `/ui/blob`), and commits each matching workflow's `CreateRun` (format 0x15.2) on the repository's Actions shard
//!   through `/runsvc`. The run key names the entry and the queue ref, so a batch run twice creates each run once.
//! - **Cursor.** After a batch that built a queue ref, and every `CURSOR_STRIDE` entries otherwise, it commits
//!   `ConsumerCursorCmd { ActionsTrigger }` through its own member, so a new leader rescans at most that much.
//! - **Off by default**, as `crate::activity` is: only where `GRAFTYARD_ACTIVITY` is `1`, since its cursor is 32.19.
//!   Its 0x15.2 `CreateRun` goes only to a shard whose members read it, which one site day gives all three sites.
//! - **`pull_request`** (B0-35; `decisions/DECISIONS.md`, "`pr-trigger`"), only where a later site day set
//!   `GRAFTYARD_PR_TRIGGER` to `1`: each `opened`, `synchronize`, and `reopened` in an entry under an hour old asks for
//!   the pull request's test merge (`crate::prsync::demand`) and commits each matching workflow's `CreateRun` (format
//!   0x15.4) at the merge commit, on `refs/pull/N/merge`. A conflicting merge starts no run; a shed one fails the round,
//!   which runs again.
//!
//! No request path calls the Cloudflare API: every call is a Durable Object binding.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use graftyard_actions::shard::{JobSpec, WorkflowRun};
use graftyard_actions::trigger::{
    Built, PrEvent, PrTarget, built, merge_group_run, pr_events, pr_target, pull_request_run,
};
use graftyard_git as git;
use graftyard_git::issues::Consumer;
use graftyard_git::watch::ConsumerCursorCmd;
use graftyard_lookup::Bindings;
use graftyard_raft::{Index, Role};
use graftyard_types::{AccountId, ChainId, PlacementRecord, RepoId, RequestId, Ulid};
use serde_json::{Value, json};
use worker::wasm_bindgen_futures::spawn_local;
use worker::*;

use crate::SiteEnv;
use crate::family::Repo;
use crate::host::Host;

/// Entries one round reads at most [guess].
const BATCH: u64 = 200;
/// With no queue ref built, the cursor is committed only this many entries apart, so it costs no entry per batch.
const CURSOR_STRIDE: u64 = 1_000;
/// After a failed round, the alarm comes back in this long.
const RETRY_MS: i64 = 5_000;
const WORKFLOWS: &str = ".github/workflows";

/// A leadership's (term, boot), how far its scan reached, and whether a round runs.
type Scan = ((u64, u64), Index, bool);

thread_local! {
    /// Per member object.
    static SCAN: RefCell<HashMap<String, Scan>> = RefCell::default();
}

/// A request id from the chain and the cursor's index: the tag `0x4154` ("AT") in the top 16 bits.
fn rid(repo: RepoId, through: Index) -> RequestId {
    RequestId(Ulid(
        (0x4154u128 << 112) | (u128::from(repo.0.0 as u64 & 0xffff_ffff_ffff) << 64) | u128::from(through.0),
    ))
}

pub async fn on_alarm(h: &Rc<Host<Repo>>) {
    // Its cursor is a 32.19 command, so it runs where `Notify` and `Search` do.
    if !crate::activity::enabled(&h.env) {
        return;
    }
    let found = {
        let core = h.core.borrow();
        let Some(c) = core.as_ref().filter(|c| c.m.role() == Role::Leader) else { return };
        let ChainId::Repo(repo) = c.join.chain else { return };
        let fold = &c.m.group_fold().fold;
        // Only a repository with a merge queue rule or a queue can build a queue ref, so no other one scans its log.
        let core = &*fold.core;
        let queued = graftyard_fold::Read::prefix(core, "rule", &[]).into_iter().any(|(k, _)| {
            git::tiers::get_rec::<git::issues::BranchRule>(core, "rule", &k)
                .is_some_and(|r| r.protection.is_some_and(|p| p.merge_queue.is_some()))
        });
        if !queued && git::queue::queue_bases(core).is_empty() && !pr_trigger(&h.env) {
            return;
        }
        let cursor = git::tiers::consumer_cursor(&*fold.core, Consumer::ActionsTrigger);
        let Some(info) = git::search::RepoInfo::of(repo, &*fold.core) else { return };
        let key = c.join.me.do_name.to_string();
        (key, (c.m.term().0, c.m.boot()), repo, info.owner, cursor, c.m.state().applied, c.join.record.clone())
    };
    let (key, leadership, repo, owner, cursor, applied, record) = found;
    let from = SCAN.with(|s| {
        let mut s = s.borrow_mut();
        let e = s.entry(key.clone()).or_insert((leadership, cursor, false));
        if e.0 != leadership {
            *e = (leadership, cursor, false);
        }
        if e.2 || e.1.max(cursor) >= applied {
            return None;
        }
        e.2 = true;
        Some(e.1.max(cursor))
    });
    let Some(from) = from else { return };
    let through = Index(applied.0.min(from.0 + BATCH));
    let w = Rc::downgrade(h);
    spawn_local(async move {
        let Some(h) = w.upgrade() else { return };
        let r = round(&h, repo, owner, &record, cursor, from, through).await;
        SCAN.with(|s| {
            if let Some(e) = s.borrow_mut().get_mut(&key).filter(|e| e.0 == leadership) {
                e.2 = false;
                if r.is_ok() {
                    e.1 = through;
                }
            }
        });
        match r {
            Ok(()) if through < applied => h.arm_in(0),
            Ok(()) => {}
            Err(e) => {
                console_error!("actions trigger {}: {e}", repo.0.0);
                h.arm_in(RETRY_MS);
            }
        }
    });
}

/// Whether a site day set `GRAFTYARD_PR_TRIGGER`, after the day that shipped format 0x15.4's reader.
fn pr_trigger(env: &Env) -> bool {
    env.var("GRAFTYARD_PR_TRIGGER").is_ok_and(|v| v.to_string() == "1")
}

/// Entries `from + 1 ..= through`: every queue ref they built and every pull request event gets its runs, then the
/// cursor.
async fn round(
    h: &Rc<Host<Repo>>,
    repo: RepoId,
    owner: AccountId,
    record: &PlacementRecord,
    committed: Index,
    from: Index,
    through: Index,
) -> std::result::Result<(), String> {
    let mut work: Vec<(Index, Built)> = Vec::new();
    let mut prs: Vec<(Index, u64, PrEvent)> = Vec::new();
    let with_prs = pr_trigger(&h.env);
    {
        let core = h.core.borrow();
        let c = core.as_ref().ok_or("not joined")?;
        let sql = h.state.storage().sql();
        let first = c.m.group_fold().fold.first_index();
        for i in from.0.max(first.0.saturating_sub(1)) + 1..=through.0 {
            let e = match c.m.entry_at(Index(i)) {
                Some(e) => e.clone(),
                None => crate::host::stored_entry::<Repo>(&sql, Index(i))
                    .ok_or_else(|| format!("entry {i} is in neither the member nor the stored log"))?,
            };
            work.extend(built(&e).into_iter().map(|b| (e.index, b)));
            if with_prs {
                let core = &c.m.group_fold().fold.core;
                prs.extend(pr_events(core, repo, &e, crate::now_ms()).into_iter().map(|p| (e.index, e.at.0, p)));
            }
        }
    }
    let site = SiteEnv::read(&h.env).map_err(|e| e.to_string())?;
    let b = Bindings { env: &h.env, sites: &site.sites };
    for (index, q) in &work {
        let t0 = crate::now_ms();
        let make = |path: &str, src: &str| merge_group_run(repo, *index, path, src, q);
        let n = runs_on(&h.env, &site, &b, repo, owner, record, q.tip, make).await?;
        console_log!(
            "actions trigger {} {} at {}: {n} runs in {}ms",
            repo.0.0,
            q.queue_ref.display(),
            index.0,
            crate::now_ms() - t0
        );
    }
    let dispatched = futures_util::future::join_all(
        prs.iter().map(|(index, at, p)| pull_request_runs(h, &site, &b, repo, owner, record, *index, *at, p)),
    )
    .await;
    for r in dispatched {
        r?;
    }
    if work.is_empty() && prs.is_empty() && through.0 < committed.0 + CURSOR_STRIDE {
        return Ok(());
    }
    let request = rid(repo, through);
    let cmd = ConsumerCursorCmd { consumer: Consumer::ActionsTrigger, through, request };
    crate::activity::commit_here(h, request, git::repo::RepoCmd::ConsumerCursor(cmd)).await.map_err(|e| e.to_string())
}

/// Pull request event `p` of entry `index` (stamped `at`): its test merge, asked for when missing, then every matching
/// workflow's run at it. Logs each event's dispatch time from `at`, and whether the head had moved on (`superseded`).
#[allow(clippy::too_many_arguments)]
async fn pull_request_runs(
    h: &Rc<Host<Repo>>,
    site: &SiteEnv,
    b: &Bindings<'_>,
    repo: RepoId,
    owner: AccountId,
    record: &PlacementRecord,
    index: Index,
    at: u64,
    p: &PrEvent,
) -> std::result::Result<(), String> {
    let view = || -> std::result::Result<git::prsync::PrView, String> {
        let core = h.core.borrow();
        let c = core.as_ref().ok_or("not joined")?;
        let (_, mut v) = git::prsync::views(&c.m.group_fold().fold.core, repo, &[p.n]);
        v.pop().ok_or_else(|| format!("pull request {} is not in the fold", p.n.0))
    };
    let mut v = view()?;
    if pr_target(&v) == PrTarget::Pending {
        crate::prsync::demand(&h.env, repo, p.n, git::prsync::MergeCause::Dispatch).await?;
        v = view()?;
    }
    let superseded = p.after.is_some_and(|a| a != v.head);
    let n = match pr_target(&v) {
        PrTarget::Merge(merge) => {
            let make = |path: &str, src: &str| pull_request_run(repo, index, path, src, p.n, p.action, &v.base, merge);
            runs_on(&h.env, site, b, repo, owner, record, merge, make).await?
        }
        PrTarget::Pending => return Err(format!("test merge of #{} shed", p.n.0)),
        PrTarget::Conflict | PrTarget::None => 0,
    };
    console_log!(
        "actions dispatch {} #{} {} at {}: {n} runs in {}ms{}{}",
        repo.0.0,
        p.n.0,
        p.action,
        index.0,
        crate::now_ms().saturating_sub(at),
        if superseded { ", superseded" } else { "" },
        if pr_target(&v) == PrTarget::Conflict { ", conflict" } else { "" }
    );
    Ok(())
}

/// Every matching workflow's run at `tip`, each built by `make` from its path and source; the count created or found
/// created.
#[allow(clippy::too_many_arguments)]
async fn runs_on(
    env: &Env,
    site: &SiteEnv,
    b: &Bindings<'_>,
    repo: RepoId,
    owner: AccountId,
    record: &PlacementRecord,
    tip: git::refs::ObjectId,
    make: impl Fn(&str, &str) -> std::result::Result<Option<(WorkflowRun, Vec<JobSpec>)>, String>,
) -> std::result::Result<usize, String> {
    let commit = tip.hex();
    let tree = match ui(env, site, repo, record, "tree", &[("commit", &commit), ("path", WORKFLOWS)]).await? {
        Some(t) => t,
        None => return Ok(0),
    };
    let files: Vec<String> = tree["entries"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|e| e["type"] == "file")
        .filter_map(|e| e["path"].as_str().map(String::from))
        .filter(|p| p.ends_with(".yml") || p.ends_with(".yaml"))
        .collect();
    let chain = ChainId::Actions(owner, graftyard_actions::actions_shard(repo));
    let mut n = 0;
    for path in files {
        let Some(blob) = ui(env, site, repo, record, "blob", &[("commit", &commit), ("path", &path)]).await? else {
            continue;
        };
        let src = blob["text"].as_str().unwrap_or_default();
        let (run, jobs) = match make(&path, src) {
            Ok(Some(x)) => x,
            Ok(None) => continue,
            Err(e) => {
                console_error!("actions trigger {} {path}: {e}", repo.0.0);
                continue;
            }
        };
        graftyard_lookup::create(b, chain, record, &json!({})).await?;
        let (v, _) = graftyard_lookup::call(
            b,
            chain,
            record,
            "/runsvc",
            &json!({ "op": "create_run", "run": run, "jobs": jobs }),
        )
        .await?;
        let message = v["body"]["message"].as_str().unwrap_or_default();
        match v["status"].as_u64() {
            Some(201) => n += 1,
            // A batch run again: the run key already holds this run.
            Some(422) if message.contains("Duplicate") => n += 1,
            // A workflow the shard refuses (too many jobs, bad needs) is skipped, as an unparsable one is.
            Some(422) => console_error!("actions trigger {} {path}: {message}", repo.0.0),
            _ => return Err(format!("create_run answered {v}")),
        }
    }
    Ok(n)
}

/// A read-only route of this site's git cache shard of the network, `None` for a 404.
async fn ui(
    env: &Env,
    site: &SiteEnv,
    repo: RepoId,
    record: &PlacementRecord,
    route: &str,
    q: &[(&str, &str)],
) -> std::result::Result<Option<Value>, String> {
    let me = site.id;
    let region = site.sites.get(me).ok_or("this site is not in GRAFTYARD_SITES")?.region;
    let stub = env
        .durable_object(&format!("SHARD_{}", me.0))
        .and_then(|ns| ns.get_by_name_with_location_hint(&format!("git#{}#net:{}", me.0, repo.0.0), region.hint()))
        .map_err(|e| e.to_string())?;
    let rec = serde_json::to_string(record).map_err(|e| e.to_string())?;
    let mut url = format!("https://shard/ui/{route}?repo={}&wait=30", repo.0.0);
    for (k, v) in q.iter().chain([("record", rec.as_str())].iter()) {
        url.push_str(&format!("&{k}={}", js_sys::encode_uri_component(v)));
    }
    let mut r = stub.fetch_with_str(&url).await.map_err(|e| e.to_string())?;
    let text = r.text().await.map_err(|e| e.to_string())?;
    match r.status_code() {
        200 => serde_json::from_str(&text).map(Some).map_err(|e| format!("shard {route}: {e}")),
        404 => Ok(None),
        s => Err(format!("shard {route}: {s} {}", text.trim())),
    }
}
