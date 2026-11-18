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
//!   0x15.4) at the merge commit, on `refs/pull/N/merge`. A conflicting merge starts no run.
//! - **Carried items** (F84): a queue ref or event a round could not finish (its merge still shed, a read or a
//!   `CreateRun` failed) is kept in this leadership's scan and tried again by the next round, `RETRY_MS` later, while
//!   the rest of its batch counts as done. The cursor commits just below the oldest carried entry
//!   (`graftyard_actions::trigger::settle`). An event still not done an hour after its entry is given up, `unmerged`.
//!
//! No request path calls the Cloudflare API: every call is a Durable Object binding.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use graftyard_actions::shard::{JobSpec, WorkflowRun};
use graftyard_actions::specs::Declared;
use graftyard_actions::trigger::{
    Carried, Item, PrEvent, PrTarget, Tried, built, merge_group_run, pr_events, pr_target, pull_request_run, settle,
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
/// After a failed round, or one that carried items, the alarm comes back in this long.
const RETRY_MS: i64 = 5_000;
/// A pull request event asks for its test merge at most this many times a round, DEMAND_WAIT_MS apart, and is then
/// carried to the next round [guess: the counts].
const DEMAND_TRIES: u32 = 4;
const DEMAND_WAIT_MS: u64 = 2_000;
const WORKFLOWS: &str = ".github/workflows";

/// A leadership's scan: its (term, boot), how far it read, whether a round runs, the items carried to the next round,
/// and whether a cursor commit is owed (the last one failed).
struct Scan {
    leadership: (u64, u64),
    reached: Index,
    running: bool,
    carried: Vec<Carried>,
    owed: bool,
}

impl Scan {
    fn new(leadership: (u64, u64), reached: Index) -> Scan {
        Scan { leadership, reached, running: false, carried: Vec::new(), owed: false }
    }
}

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
    let start = SCAN.with(|s| {
        let mut s = s.borrow_mut();
        let e = s.entry(key.clone()).or_insert_with(|| Scan::new(leadership, cursor));
        if e.leadership != leadership {
            *e = Scan::new(leadership, cursor);
        }
        let from = e.reached.max(cursor);
        if e.running || (from >= applied && e.carried.is_empty() && !e.owed) {
            return None;
        }
        e.running = true;
        Some((from, std::mem::take(&mut e.carried), e.owed))
    });
    let Some((from, carried, owed)) = start else { return };
    let through = Index(applied.0.min(from.0 + BATCH).max(from.0));
    let w = Rc::downgrade(h);
    spawn_local(async move {
        let Some(h) = w.upgrade() else { return };
        let r = round(&h, repo, owner, &record, cursor, from, through, carried.clone(), owed).await;
        let again = SCAN.with(|s| {
            let mut s = s.borrow_mut();
            let Some(e) = s.get_mut(&key).filter(|e| e.leadership == leadership) else { return false };
            e.running = false;
            match &r {
                Ok((left, committed)) => {
                    e.reached = through;
                    e.carried = left.clone();
                    e.owed = !committed;
                }
                // Read nothing new: try the same items again.
                Err(_) => e.carried = carried,
            }
            !e.carried.is_empty() || e.owed
        });
        match r {
            Ok(_) if through < applied => h.arm_in(0),
            Ok(_) if again => h.arm_in(RETRY_MS),
            Ok(_) => {}
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

/// The items `carried` from the last round, then every queue ref entries `from + 1 ..= through` built and every pull
/// request event they hold: each is tried once, then the cursor. `Err` only when the entries could not be read;
/// otherwise the items left for the next round and whether the cursor commit, when one was due, went through.
#[allow(clippy::too_many_arguments)]
async fn round(
    h: &Rc<Host<Repo>>,
    repo: RepoId,
    owner: AccountId,
    record: &PlacementRecord,
    committed: Index,
    from: Index,
    through: Index,
    carried: Vec<Carried>,
    owed: bool,
) -> std::result::Result<(Vec<Carried>, bool), String> {
    let mut items = carried;
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
            let item = |item| Carried { index: e.index, at: e.at.0, item };
            items.extend(built(&e).into_iter().map(|b| item(Item::Queue(b))));
            if with_prs {
                let core = &c.m.group_fold().fold.core;
                items.extend(pr_events(core, repo, &e, crate::now_ms()).into_iter().map(|p| item(Item::Pr(p))));
            }
        }
    }
    let site = SiteEnv::read(&h.env).map_err(|e| e.to_string())?;
    let b = Bindings { env: &h.env, sites: &site.sites };
    let mut tried = Vec::with_capacity(items.len());
    for (c, q) in items.iter().filter_map(|c| match &c.item {
        Item::Queue(q) => Some((c, q)),
        Item::Pr(_) => None,
    }) {
        let t0 = crate::now_ms();
        let make = |path: &str, src: &str| merge_group_run(repo, c.index, path, src, q);
        let t = match runs_on(&h.env, &site, &b, repo, owner, record, q.tip, make).await {
            Ok(n) => {
                let ms = crate::now_ms() - t0;
                console_log!(
                    "actions trigger {} {} at {}: {n} runs in {ms}ms",
                    repo.0.0,
                    q.queue_ref.display(),
                    c.index.0
                );
                Tried::Done
            }
            Err(e) => {
                console_error!("actions trigger {} {} at {}: {e}", repo.0.0, q.queue_ref.display(), c.index.0);
                Tried::Again
            }
        };
        tried.push((c.clone(), t));
    }
    let prs: Vec<(&Carried, &PrEvent)> = items
        .iter()
        .filter_map(|c| match &c.item {
            Item::Pr(p) => Some((c, p)),
            Item::Queue(_) => None,
        })
        .collect();
    let dispatched = futures_util::future::join_all(
        prs.iter().map(|(c, p)| pull_request_runs(h, &site, &b, repo, owner, record, c.index, c.at, p)),
    )
    .await;
    tried.extend(prs.into_iter().map(|(c, _)| c.clone()).zip(dispatched));
    let any = !tried.is_empty();
    let (left, dropped, cursor) = settle(tried, through, crate::now_ms());
    for c in &dropped {
        let Item::Pr(p) = &c.item else { continue };
        let ms = crate::now_ms().saturating_sub(c.at);
        console_log!(
            "actions dispatch {} #{} {} at {}: 0 runs in {ms}ms, 0 asks, unmerged",
            repo.0.0,
            p.n.0,
            p.action,
            c.index.0
        );
    }
    if cursor <= committed || !(any || owed || cursor.0 >= committed.0 + CURSOR_STRIDE) {
        return Ok((left, true));
    }
    let request = rid(repo, cursor);
    let cmd = ConsumerCursorCmd { consumer: Consumer::ActionsTrigger, through: cursor, request };
    let r = crate::activity::commit_here(h, request, git::repo::RepoCmd::ConsumerCursor(cmd)).await;
    if let Err(e) = &r {
        console_error!("actions trigger {} cursor {}: {e}", repo.0.0, cursor.0);
    }
    Ok((left, r.is_ok()))
}

/// Pull request event `p` of entry `index` (stamped `at`): its test merge, asked for when missing, then every matching
/// workflow's run at it. Logs each event's dispatch time from `at`, and whether the head had moved on (`superseded`).
/// `Again` while its merge is still shed (logged `pending`) or a read or `CreateRun` failed.
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
) -> Tried {
    match pull_request_try(h, site, b, repo, owner, record, index, at, p).await {
        Ok(t) => t,
        Err(e) => {
            console_error!("actions dispatch {} #{} at {}: {e}", repo.0.0, p.n.0, index.0);
            Tried::Again
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn pull_request_try(
    h: &Rc<Host<Repo>>,
    site: &SiteEnv,
    b: &Bindings<'_>,
    repo: RepoId,
    owner: AccountId,
    record: &PlacementRecord,
    index: Index,
    at: u64,
    p: &PrEvent,
) -> std::result::Result<Tried, String> {
    let view = || -> std::result::Result<git::prsync::PrView, String> {
        let core = h.core.borrow();
        let c = core.as_ref().ok_or("not joined")?;
        let (_, mut v) = git::prsync::views(&c.m.group_fold().fold.core, repo, &[p.n]);
        v.pop().ok_or_else(|| format!("pull request {} is not in the fold", p.n.0))
    };
    let mut v = view()?;
    let mut tries = 0;
    while pr_target(&v) == PrTarget::Pending && tries < DEMAND_TRIES {
        if tries > 0 {
            worker::Delay::from(std::time::Duration::from_millis(DEMAND_WAIT_MS)).await;
        }
        tries += 1;
        if let Err(e) = crate::prsync::demand(&h.env, repo, p.n, git::prsync::MergeCause::Dispatch).await {
            console_error!("actions dispatch {} #{} demand {tries}: {e}", repo.0.0, p.n.0);
        }
        v = view()?;
    }
    let superseded = p.after.is_some_and(|a| a != v.head);
    let n = match pr_target(&v) {
        PrTarget::Merge(merge) => {
            let make = |path: &str, src: &str| pull_request_run(repo, index, path, src, p.n, p.action, &v.base, merge);
            runs_on(&h.env, site, b, repo, owner, record, merge, make).await?
        }
        PrTarget::Pending | PrTarget::Conflict | PrTarget::None => 0,
    };
    console_log!(
        "actions dispatch {} #{} {} at {}: {n} runs in {}ms, {tries} asks{}{}",
        repo.0.0,
        p.n.0,
        p.action,
        index.0,
        crate::now_ms().saturating_sub(at),
        if superseded { ", superseded" } else { "" },
        match pr_target(&v) {
            PrTarget::Conflict => ", conflict",
            PrTarget::Pending => ", pending",
            _ => "",
        }
    );
    Ok(if pr_target(&v) == PrTarget::Pending { Tried::Again } else { Tried::Done })
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
    make: impl Fn(&str, &str) -> std::result::Result<Option<(WorkflowRun, Vec<JobSpec>, Declared)>, String>,
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
        let (run, jobs, permissions) = match make(&path, src) {
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
            &crate::actions::create_run_body(env, &run, &jobs, &permissions),
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
