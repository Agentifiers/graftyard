//! `ActionsTrigger` for `on: merge_group` (BF-37; `design/SHARED.md`, "Event fan-out", Other consumers; the pure half
//! is `graftyard_actions::trigger`). It runs in band on a repository leader's alarm, as `Notify` and `Search` do
//! (`crate::activity`), with no queue hop.
//!
//! - **Stream** (`decisions/DECISIONS.md`, "`trigger-stream`"; the state is `graftyard_actions::trigger::Stream`).
//!   From the committed `ActionsTrigger` cursor it reads at most `BATCH` committed entries at a time from the leader's
//!   log, and lists the queue refs each `GroupBuilt` built and the pull request events each entry holds. Each item
//!   starts as soon as a slot is free, up to `IN_FLIGHT_MAX` at once, and ends on its own. The next entries are read
//!   once the items read have all started, so no item waits for another's merge, as items in rounds did.
//! - **Runs.** For each queue ref it lists `.github/workflows` at the ref's tip on this site's git cache shard
//!   (`/ui/tree`, `/ui/blob`), and commits each matching workflow's `CreateRun` (format 0x15.2) on the repository's
//!   Actions shard through `/runsvc`. The run key names the entry and the queue ref, so an item run twice creates each
//!   run once.
//! - **Cursor.** Just below the oldest item not done (`graftyard_actions::trigger::low_water`), or at the last entry
//!   read when every item is done, so a new leader rescans every item not done and never skips one. It commits as
//!   `ConsumerCursorCmd { ActionsTrigger }` through its own member, at most every `CURSOR_EVERY_MS` while items end,
//!   and every `CURSOR_STRIDE` entries otherwise.
//! - **Off by default**, as `crate::activity` is: only where `GRAFTYARD_ACTIVITY` is `1`, since its cursor is 32.19.
//!   Its 0x15.2 `CreateRun` goes only to a shard whose members read it, which one site day gives all three sites.
//! - **`pull_request`** (B0-35; `decisions/DECISIONS.md`, "`pr-trigger`"), only where a later site day set
//!   `GRAFTYARD_PR_TRIGGER` to `1`: each `opened`, `synchronize`, and `reopened` in an entry under an hour old asks for
//!   the pull request's test merge (`crate::prsync::demand`) and commits each matching workflow's `CreateRun` (format
//!   0x15.4) at the merge commit, on `refs/pull/N/merge`. A conflicting merge starts no run.
//! - **Tried again** (F84): an item that could not finish (its merge still shed after `DEMAND_TRIES` asks, a read or
//!   a `CreateRun` that failed) starts again `RETRY_MS` later, alone. An event still not done an hour after its entry is
//!   given up, `unmerged`.
//! - **Timing.** Each event's log line splits its time into the wait before its try started, the demand, the workflow
//!   reads, and the `CreateRun`s.
//!
//! No request path calls the Cloudflare API: every call is a Durable Object binding.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use graftyard_actions::shard::{JobSpec, WorkflowRun};
use graftyard_actions::specs::Declared;
use graftyard_actions::trigger::{
    Carried, IN_FLIGHT_MAX, Item, PrEvent, PrTarget, RETRY_MS, Stream, Tried, built, merge_group_run, pr_events,
    pr_target, pull_request_run,
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

/// Entries one scan reads at most [guess].
const BATCH: u64 = 200;
/// With no queue ref built, the cursor is committed only this many entries apart, so it costs no entry per batch.
const CURSOR_STRIDE: u64 = 1_000;
/// Batches one call of `pump` reads at most.
const READS_PER_PUMP: usize = 4;
/// The cursor commits at most this often while items end [guess: rounds committed it about every 42 s on B0-35 m54].
const CURSOR_EVERY_MS: u64 = 5_000;
/// A pull request event asks for its test merge at most this many times a try, DEMAND_WAIT_MS apart, and is then
/// tried again RETRY_MS later [guess: the counts].
const DEMAND_TRIES: u32 = 4;
const DEMAND_WAIT_MS: u64 = 2_000;
const WORKFLOWS: &str = ".github/workflows";

/// A leadership's stream (`graftyard_actions::trigger::Stream`): its (term, boot), the items read, running, and to be
/// tried again, whether a cursor commit runs, the index last committed, and whether a commit is owed (the last failed).
struct Scan {
    leadership: (u64, u64),
    stream: Stream,
    committing: bool,
    committed: Index,
    owed: bool,
    /// An item ended since the last cursor commit, and when that commit started.
    worked: bool,
    last_commit: u64,
}

impl Scan {
    fn new(leadership: (u64, u64), cursor: Index) -> Scan {
        Scan {
            leadership,
            stream: Stream::new(cursor, IN_FLIGHT_MAX),
            committing: false,
            committed: cursor,
            owed: false,
            worked: false,
            last_commit: 0,
        }
    }
}

thread_local! {
    /// Per member object.
    static SCAN: RefCell<HashMap<String, Scan>> = RefCell::default();
}

fn rid(repo: RepoId, through: Index) -> RequestId {
    RequestId(Ulid(
        (0x4154u128 << 112) | (u128::from(repo.0.0 as u64 & 0xffff_ffff_ffff) << 64) | u128::from(through.0),
    ))
}

/// What a leader's alarm hands the stream: the member's key, its leadership, the repository and its owner and record,
/// the committed cursor, and the applied index.
struct Found {
    key: String,
    leadership: (u64, u64),
    repo: RepoId,
    owner: AccountId,
    cursor: Index,
    applied: Index,
    record: PlacementRecord,
}

pub async fn on_alarm(h: &Rc<Host<Repo>>) {
    // Its cursor is a 32.19 command, so it runs where `Notify` and `Search` do.
    if crate::activity::enabled(&h.env) {
        pump(h);
    }
}

/// This member's stream inputs now, or `None` when it does not lead or this repository has nothing to scan for.
fn found(h: &Rc<Host<Repo>>) -> Option<Found> {
    let core = h.core.borrow();
    let c = core.as_ref().filter(|c| c.m.role() == Role::Leader)?;
    let ChainId::Repo(repo) = c.join.chain else { return None };
    let fold = &c.m.group_fold().fold;
    // Only a repository with a merge queue rule or a queue can build a queue ref, so no other one scans its log.
    let core = &*fold.core;
    let queued = graftyard_fold::Read::prefix(core, "rule", &[]).into_iter().any(|(k, _)| {
        git::tiers::get_rec::<git::issues::BranchRule>(core, "rule", &k)
            .is_some_and(|r| r.protection.is_some_and(|p| p.merge_queue.is_some()))
    });
    if !queued && git::queue::queue_bases(core).is_empty() && !pr_trigger(&h.env) {
        return None;
    }
    let info = git::search::RepoInfo::of(repo, core)?;
    Some(Found {
        key: c.join.me.do_name.to_string(),
        leadership: (c.m.term().0, c.m.boot()),
        repo,
        owner: info.owner,
        cursor: git::tiers::consumer_cursor(core, Consumer::ActionsTrigger),
        applied: c.m.state().applied,
        record: c.join.record.clone(),
    })
}

/// Reads what entries the stream has room for, starts every item it lets start, and commits the cursor when it is
/// due. Called by the alarm and again as each item ends, so an entry's items start as soon as it is applied and a slot
/// is free, and none waits for another item's merge (`decisions/DECISIONS.md`, "`trigger-stream`").
fn pump(h: &Rc<Host<Repo>>) {
    let Some(f) = found(h) else { return };
    let f = Rc::new(f);
    let now = crate::now_ms();
    let with_prs = pr_trigger(&h.env);
    let (started, commit) = {
        let read = |from: Index| -> std::result::Result<(Vec<Carried>, Index), String> {
            let through = Index(f.applied.0.min(from.0 + BATCH).max(from.0));
            let core = h.core.borrow();
            let c = core.as_ref().ok_or("not joined")?;
            let sql = h.state.storage().sql();
            let first = c.m.group_fold().fold.first_index();
            let mut items = Vec::new();
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
                    items.extend(pr_events(core, f.repo, &e, now).into_iter().map(|p| item(Item::Pr(p))));
                }
            }
            Ok((items, through))
        };
        SCAN.with(|s| {
            let mut s = s.borrow_mut();
            let e = s.entry(f.key.clone()).or_insert_with(|| Scan::new(f.leadership, f.cursor));
            if e.leadership != f.leadership {
                *e = Scan::new(f.leadership, f.cursor);
            }
            e.committed = e.committed.max(f.cursor);
            // Read the next entries only while a slot is free and every item read has started, so the items waiting
            // for a slot are at most one batch's, and a stream far behind the log reads READS_PER_PUMP batches and
            // comes back on the next alarm rather than reading the whole log in one invocation.
            if e.stream.queued() == 0 && e.stream.running() < IN_FLIGHT_MAX && e.stream.reached() < f.applied {
                let core = h.core.borrow();
                if let Some(c) = core.as_ref() {
                    let reached = e.stream.reached();
                    let first = c.m.group_fold().fold.first_index();
                    crate::alarm_steps::note_scan(
                        &h.state.storage().sql(),
                        &f.key,
                        "actions_trigger",
                        reached.0.max(first.0.saturating_sub(1)),
                        f.applied.0.min(reached.0 + BATCH * READS_PER_PUMP as u64),
                        Some(c.m.log_base().index.0),
                    );
                }
            }
            let mut started = e.stream.start(now);
            for _ in 0..READS_PER_PUMP {
                if e.stream.queued() > 0 || e.stream.running() >= IN_FLIGHT_MAX || e.stream.reached() >= f.applied {
                    break;
                }
                match read(e.stream.reached()) {
                    Ok((items, through)) => e.stream.read(items, through),
                    Err(err) => {
                        console_error!("actions trigger {}: {err}", f.repo.0.0);
                        h.arm_in(RETRY_MS as i64);
                        break;
                    }
                }
                started.extend(e.stream.start(now));
            }
            if e.stream.queued() == 0 && e.stream.running() < IN_FLIGHT_MAX && e.stream.reached() < f.applied {
                h.arm_in(0);
            }
            // The cursor commits once items ended since the last commit, at most every CURSOR_EVERY_MS, so a stream
            // of events costs one cursor entry per interval, not one per event, and the cursor's own entry, which
            // holds no item, commits nothing again. A failed commit is owed; CURSOR_STRIDE entries with no item
            // commit too, so a new leader rescans at most that many.
            let cursor = e.stream.cursor();
            let gap = now.saturating_sub(e.last_commit);
            let due = cursor > e.committed
                && (e.owed || cursor.0 >= e.committed.0 + CURSOR_STRIDE || (e.worked && gap >= CURSOR_EVERY_MS));
            let commit = (!e.committing && due).then(|| {
                e.committing = true;
                e.worked = false;
                e.last_commit = now;
                cursor
            });
            if !e.committing && e.worked && cursor > e.committed {
                h.arm_in(CURSOR_EVERY_MS.saturating_sub(gap) as i64);
            }
            if let Some(t) = e.stream.next_retry() {
                h.arm_in(t.saturating_sub(now) as i64);
            }
            (started, commit)
        })
    };
    for (id, c) in started {
        let (w, f) = (Rc::downgrade(h), f.clone());
        spawn_local(async move {
            let Some(h) = w.upgrade() else { return };
            let t = run_item(&h, &f, &c).await;
            let now = crate::now_ms();
            let dropped = SCAN.with(|s| {
                s.borrow_mut().get_mut(&f.key).filter(|e| e.leadership == f.leadership).and_then(|e| {
                    e.worked = true;
                    e.stream.finish(id, t, now)
                })
            });
            if let Some(c) = dropped
                && let Item::Pr(p) = &c.item
            {
                console_log!(
                    "actions dispatch {} #{} {} at {}: 0 runs in {}ms, 0 asks, unmerged",
                    f.repo.0.0,
                    p.n.0,
                    p.action,
                    c.index.0,
                    now.saturating_sub(c.at)
                );
            }
            pump(&h);
        });
    }
    if let Some(cursor) = commit {
        let (w, f) = (Rc::downgrade(h), f.clone());
        spawn_local(async move {
            let Some(h) = w.upgrade() else { return };
            let request = rid(f.repo, cursor);
            let cmd = ConsumerCursorCmd { consumer: Consumer::ActionsTrigger, through: cursor, request };
            let r = crate::activity::commit_here(&h, request, git::repo::RepoCmd::ConsumerCursor(cmd)).await;
            if let Err(e) = &r {
                console_error!("actions trigger {} cursor {}: {e}", f.repo.0.0, cursor.0);
            }
            let again = SCAN.with(|s| {
                let mut s = s.borrow_mut();
                let Some(e) = s.get_mut(&f.key).filter(|e| e.leadership == f.leadership) else { return false };
                e.committing = false;
                e.owed = r.is_err();
                if r.is_ok() {
                    e.committed = e.committed.max(cursor);
                }
                e.owed
            });
            // Items that ended while it committed are not covered: look again, so the cursor's last move is not left
            // to an unrelated alarm.
            if again { h.arm_in(RETRY_MS as i64) } else { pump(&h) }
        });
    }
}

/// Whether a site day set `GRAFTYARD_PR_TRIGGER`, after the day that shipped format 0x15.4's reader.
fn pr_trigger(env: &Env) -> bool {
    env.var("GRAFTYARD_PR_TRIGGER").is_ok_and(|v| v.to_string() == "1")
}

/// One item: a queue ref's runs at its tip, or a pull request event's runs at its test merge. `Again` when a read or a
/// `CreateRun` failed, or the event's merge is still shed.
async fn run_item(h: &Rc<Host<Repo>>, f: &Found, c: &Carried) -> Tried {
    let site = match SiteEnv::read(&h.env) {
        Ok(s) => s,
        Err(e) => {
            console_error!("actions trigger {}: {e}", f.repo.0.0);
            return Tried::Again;
        }
    };
    let b = Bindings { env: &h.env, sites: &site.sites };
    match &c.item {
        Item::Queue(q) => {
            let t0 = crate::now_ms();
            let make = |path: &str, src: &str| merge_group_run(f.repo, c.index, path, src, q);
            let mut times = Times::default();
            match runs_on(&h.env, &site, &b, f.repo, f.owner, &f.record, q.tip, make, None, None, &mut times).await {
                Ok(n) => {
                    let ms = crate::now_ms() - t0;
                    console_log!(
                        "actions trigger {} {} at {}: {n} runs in {ms}ms",
                        f.repo.0.0,
                        q.queue_ref.display(),
                        c.index.0
                    );
                    Tried::Done
                }
                Err(e) => {
                    console_error!("actions trigger {} {} at {}: {e}", f.repo.0.0, q.queue_ref.display(), c.index.0);
                    Tried::Again
                }
            }
        }
        Item::Pr(p) => pull_request_runs(h, &site, &b, f.repo, f.owner, &f.record, c.index, c.at, p).await,
    }
}

/// Where one event's dispatch time went, in ms: its test merge's asks (`demand`, with the waits between them), the
/// workflow reads on the git cache shard (`tree`, `blob`), and the `CreateRun` calls (`decisions/DECISIONS.md`,
/// "`trigger-stream`"). The rest of the logged time is the wait from the entry's `at` to the item's start.
#[derive(Default)]
struct Times {
    demand: u64,
    reads: u64,
    create: u64,
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
    let view = || -> std::result::Result<(git::prsync::PrView, Option<git::refs::RefName>), String> {
        let core = h.core.borrow();
        let c = core.as_ref().ok_or("not joined")?;
        let (_, mut v) = git::prsync::views(&c.m.group_fold().fold.core, repo, &[p.n]);
        let head = graftyard_actions::trigger::pr_head(
            &*c.m.group_fold().fold.core,
            repo,
            p.n,
            crate::actions::oidc_head_ref(&h.env),
        );
        Ok((v.pop().ok_or_else(|| format!("pull request {} is not in the fold", p.n.0))?, head))
    };
    let started = crate::now_ms();
    let mut times = Times::default();
    let (mut v, mut head) = view()?;
    let mut tries = 0;
    while pr_target(&v) == PrTarget::Pending && tries < DEMAND_TRIES {
        if tries > 0 {
            worker::Delay::from(std::time::Duration::from_millis(DEMAND_WAIT_MS)).await;
        }
        tries += 1;
        if let Err(e) = crate::prsync::demand(&h.env, repo, p.n, git::prsync::MergeCause::Dispatch).await {
            console_error!("actions dispatch {} #{} demand {tries}: {e}", repo.0.0, p.n.0);
        }
        (v, head) = view()?;
    }
    times.demand = crate::now_ms().saturating_sub(started);
    let superseded = p.after.is_some_and(|a| a != v.head);
    let n = match pr_target(&v) {
        PrTarget::Merge(merge) => {
            let make = |path: &str, src: &str| pull_request_run(repo, index, path, src, p.n, p.action, &v.base, merge);
            let base = v.base.display();
            let head = head.as_ref().map(|h| h.display());
            runs_on(&h.env, site, b, repo, owner, record, merge, make, Some(&base), head.as_deref(), &mut times).await?
        }
        PrTarget::Pending | PrTarget::Conflict | PrTarget::None => 0,
    };
    // The split of the time logged: the wait from the entry to this try's start, then the try's demand, workflow
    // reads, and `CreateRun`s (`Times`).
    console_log!(
        "actions dispatch {} #{} {} at {}: {n} runs in {}ms, {tries} asks{}{}; wait {}ms demand {}ms reads {}ms create {}ms",
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
        },
        started.saturating_sub(at),
        times.demand,
        times.reads,
        times.create
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
    base: Option<&str>,
    head: Option<&str>,
    times: &mut Times,
) -> std::result::Result<usize, String> {
    let commit = tip.hex();
    let t = crate::now_ms();
    let tree = ui(env, site, repo, record, "tree", &[("commit", &commit), ("path", WORKFLOWS)]).await;
    times.reads += crate::now_ms().saturating_sub(t);
    let tree = match tree? {
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
        let t = crate::now_ms();
        let blob = ui(env, site, repo, record, "blob", &[("commit", &commit), ("path", &path)]).await;
        times.reads += crate::now_ms().saturating_sub(t);
        let Some(blob) = blob? else {
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
        let t = crate::now_ms();
        let created = async {
            graftyard_lookup::create(b, chain, record, &json!({})).await?;
            graftyard_lookup::call(
                b,
                chain,
                record,
                "/runsvc",
                &crate::actions::create_run_body(env, &run, &jobs, &permissions, base, head),
            )
            .await
        }
        .await;
        times.create += crate::now_ms().saturating_sub(t);
        let (v, _) = created?;
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
