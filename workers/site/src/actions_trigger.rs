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
//! - **`push`** (BF-78; `decisions/DECISIONS.md`, "`push-trigger`"), only where a site day set `GRAFTYARD_PUSH_TRIGGER`
//!   to `1`: each branch or tag an entry under an hour old moves to a commit (`graftyard_actions::trigger::push_events`)
//!   commits each matching workflow's `CreateRunPush` (format 0x15.12) at that commit, with the pusher, its number, and
//!   its `github.event`. The git cache gives the changed paths for `paths` filters (`/ui/diff`), the pushed commits
//!   (`/ui/commits`), and whether it was forced (`/ui/compare`); a head commit that says `[skip ci]` starts none.
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
    AgentRun, Carried, GateEvent, IN_FLIGHT_MAX, Item, PrEvent, PrTarget, PushEvent, RETRY_MS, Stream, Tried,
    agent_run, built, diff_paths, gate_events, merge_group_run, pr_events, pr_target, pull_request_run, push_base,
    push_events, push_run, skips_ci, two_dot,
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
    if !queued && git::queue::queue_bases(core).is_empty() && !pr_trigger(&h.env) && !push_trigger(&h.env) {
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
    let with_pushes = push_trigger(&h.env);
    let with_gates = crate::actions::secrets_wait(&h.env) && crate::actions::agent_ci(&h.env);
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
                if with_pushes {
                    items.extend(push_events(&e, now).into_iter().map(|p| item(Item::Push(p))));
                }
                if with_gates {
                    items.extend(gate_events(&e, now).into_iter().map(|g| item(Item::Gate(g))));
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

/// BF-73 (F120): the event's actor and its login, only where `GRAFTYARD_OIDC_CLAIMS` is on; else none, and the run's
/// call stays the one before 0x15.8. A login that does not answer fails the try, which is tried again.
async fn run_actor(env: &Env, actor: Option<AccountId>) -> std::result::Result<Option<(AccountId, String)>, String> {
    let Some(a) = actor.filter(|_| crate::actions::oidc_claims(env)) else { return Ok(None) };
    Ok(Some((a, crate::activity::login_of(env, a).await?)))
}

/// Whether a site day set `GRAFTYARD_REUSABLE` (`decisions/DECISIONS.md`, "`reusable`"): only then is a planned run's
/// 0x15.9 `CreateRunPlanned` sent. A site day sets it only once `FormatsEnable` enabled 0x15.9, after the day that
/// shipped its reader to every member.
fn reusable(env: &Env) -> bool {
    env.var("GRAFTYARD_REUSABLE").is_ok_and(|v| v.to_string() == "1")
}

/// Whether a site day set `GRAFTYARD_PUSH_TRIGGER` (BF-78), after the day that shipped format 0x15.12's reader.
fn push_trigger(env: &Env) -> bool {
    env.var("GRAFTYARD_PUSH_TRIGGER").is_ok_and(|v| v.to_string() == "1")
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
            let call = Call::Run { base: None, head: None, actor: None };
            // A queue ref is the queue's, never an agent's (`agent_ci_of`).
            let agent = (AgentRun::No, q.tip);
            match runs_on(&h.env, &site, &b, f.repo, f.owner, &f.record, q.tip, make, call, agent, &mut times).await {
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
        Item::Gate(g) => match gate_released(h, &b, f, g).await {
            Ok(released) => {
                let head = g.head.hex();
                console_log!("actions gate {} #{} {head} at {}: released {released}", f.repo.0.0, g.n.0, c.index.0);
                Tried::Done
            }
            Err(e) => {
                console_error!("actions gate {} #{} at {}: {e}", f.repo.0.0, g.n.0, c.index.0);
                Tried::Again
            }
        },
        Item::Push(p) => {
            let t0 = crate::now_ms();
            match push_runs(h, &site, &b, f, c, p).await {
                Ok(n) => {
                    console_log!(
                        "actions push {} {} at {}: {n} runs in {}ms",
                        f.repo.0.0,
                        p.r#ref.display(),
                        c.index.0,
                        crate::now_ms() - t0
                    );
                    Tried::Done
                }
                Err(e) => {
                    console_error!("actions push {} {} at {}: {e}", f.repo.0.0, p.r#ref.display(), c.index.0);
                    Tried::Again
                }
            }
        }
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
            let actor = run_actor(&h.env, p.actor).await?;
            let actor = actor.as_ref().map(|(a, l)| (*a, l.as_str()));
            let call = Call::Run { base: Some(&base), head: head.as_deref(), actor };
            // The jobs of a waiting run wait on the head this merge was computed from, the agent's commit it tests.
            let agent = (agent_ci_of(h, repo, p.n).await?.0, v.head);
            let n = runs_on(&h.env, site, b, repo, owner, record, merge, make, call, agent, &mut times).await?;
            if waits(&h.env, agent.0) {
                release_if_gated(h, b, repo, owner, record, agent.1).await?;
            }
            n
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

/// The changed paths a `push`'s `paths` filters read, GitHub's two-dot diff from `base` (`trigger::push_base`) to
/// `after` [doc: GHES 3.22, "Workflow syntax", "Git diff comparisons": "Pushes to existing branches: A two-dot diff
/// compares the head and base SHAs directly"] (F133). `/ui/diff` reads from the merge base of `base` and `after`, which
/// is the two-dot diff when `base` is an ancestor: an existing ref's fast-forward, and a new branch read from its merge
/// base with the default branch. A force push's two-dot diff is `trigger::two_dot` of the diffs from that merge base
/// to each tip. `None`, which runs every path-filtered workflow, without a base or with one that shares no history
/// with `after`, for a diff past the 3,000 files the cache lists, or more than 1,000 commits [doc: same section: "If a
/// push contains more than 1,000 commits, the workflow will always run"].
#[allow(clippy::too_many_arguments)]
async fn push_paths(
    env: &Env,
    site: &SiteEnv,
    repo: RepoId,
    record: &PlacementRecord,
    p: &PushEvent,
    base: Option<git::refs::ObjectId>,
    forced: bool,
    ahead: u64,
) -> std::result::Result<Option<Vec<String>>, String> {
    let Some(base) = base.filter(|_| ahead <= 1_000) else {
        return Ok(None);
    };
    let (base, after) = (base.hex(), p.after.hex());
    let d = ui(env, site, repo, record, "diff", &[("base", &base), ("head", &after)]).await?;
    let Some(to_after) = d.as_ref().and_then(diff_paths) else {
        return Ok(None);
    };
    if !forced {
        return Ok(Some(to_after.into_keys().collect()));
    }
    let d = ui(env, site, repo, record, "diff", &[("base", &after), ("head", &base)]).await?;
    Ok(d.as_ref().and_then(diff_paths).map(|to_before| two_dot(&to_after, &to_before)))
}

/// Push `p`'s runs (BF-78): whether the push was forced and the pushed commits from the git cache, nothing when its
/// head commit skips CI, then every matching workflow's `CreateRunPush` at `p.after`, with its `github.event` built as
/// the `push` webhook's (`graftyard_api::webhooks::actions_push`) and the pusher as the run's actor.
async fn push_runs(
    h: &Rc<Host<Repo>>,
    site: &SiteEnv,
    b: &Bindings<'_>,
    f: &Found,
    c: &Carried,
    p: &PushEvent,
) -> std::result::Result<usize, String> {
    let (env, repo, record) = (&h.env, f.repo, &f.record);
    let after = p.after.hex();
    let base = {
        let core = h.core.borrow();
        let fold = &core.as_ref().ok_or("not joined")?.m.group_fold().fold;
        let default = git::tiers::default_branch(&*fold.core);
        push_base(p, &default, fold.get(&default))
    };
    // Forced: the old tip is not an ancestor of the new one (`behind_by` > 0 from `before` to `after`). `shared`: the
    // base and `after` have a merge base, which the paths diff reads from.
    let (forced, ahead, shared) = match base {
        None => (false, 0, false),
        Some(b) => {
            let cmp = ui(env, site, repo, record, "compare", &[("base", &b.hex()), ("head", &after)]).await?;
            let cmp = cmp.ok_or("compare: no such commit")?;
            let behind = cmp["behind_by"].as_u64().unwrap_or(0);
            (!p.created() && behind > 0, cmp["ahead_by"].as_u64().unwrap_or(0), !cmp["merge_base"].is_null())
        }
    };
    // The pushed commits, oldest first: those `after` reaches and the base does not, which for a force push are the
    // new ones and for a new branch those the default branch does not hold. Without a base, the head commit alone.
    let mut raws: Vec<(git::refs::ObjectId, String)> = Vec::new();
    if let Some(b) = base {
        let v = ui(env, site, repo, record, "commits", &[("base", &b.hex()), ("head", &after)]).await?;
        for x in v.ok_or("commits: no such commit")?["commits"].as_array().into_iter().flatten() {
            let id = x["sha"].as_str().and_then(|s| git::refs::ObjectId::parse_hex(s.as_bytes()));
            if let (Some(id), Some(raw)) = (id, x["raw"].as_str()) {
                raws.push((id, raw.to_owned()));
            }
        }
    }
    let head_raw = match raws.iter().find(|(id, _)| *id == p.after) {
        Some((_, r)) => r.as_bytes().to_vec(),
        None => head_commit(env, site, repo, record, p.after).await?,
    };
    let mut dir = crate::hookseq::directory(h, repo, f.owner, record).await?;
    // The pusher's login: the directory holds only the logins the repository's rows name, so without it `pusher` and
    // `sender` read `ghost` (BF-78 r1). The run's actor is the same account, sent only where claims are on.
    let pusher = match p.pusher {
        Some(a) => Some((a, crate::activity::login_of(env, a).await?)),
        None => None,
    };
    if let Some((a, l)) = &pusher {
        dir.logins.entry(a.0.0).or_insert_with(|| l.clone());
    }
    let actor = pusher.filter(|_| crate::actions::oidc_claims(env));
    let event = {
        let host = graftyard_api::wire::Host::new(&crate::hookseq::api_host(env), false).map_err(|e| e.to_string())?;
        let core = h.core.borrow();
        let fold = &core.as_ref().ok_or("not joined")?.m.group_fold().fold;
        let at = dir.home.clone().ok_or("no repository")?;
        let view = graftyard_api::view::Repo { host: &host, at, core: &fold.core, meta: &fold.meta, dir: &dir };
        let head = graftyard_api::webhooks::push_commit(&view, p.after, &head_raw).ok_or("the head is not a commit")?;
        let mut commits: Vec<Value> = raws
            .iter()
            .filter_map(|(id, raw)| graftyard_api::webhooks::push_commit(&view, *id, raw.as_bytes()))
            .collect();
        if base.is_none() {
            commits.push(head.clone());
        }
        let at = graftyard_types::Timestamp(c.at);
        graftyard_api::webhooks::actions_push(&view, &p.r#ref, (p.before, p.after), p.pusher, at, forced, commits, head)
    };
    if skips_ci(event["head_commit"]["message"].as_str().unwrap_or("")) {
        console_log!("actions push {} {} at {}: skipped by its head commit", repo.0.0, p.r#ref.display(), c.index.0);
        return Ok(0);
    }
    let paths = push_paths(env, site, repo, record, p, base.filter(|_| shared), forced, ahead).await?;
    let paths: Option<Vec<&str>> = paths.as_ref().map(|v| v.iter().map(String::as_str).collect());
    let make = |path: &str, src: &str| push_run(repo, c.index, path, src, p, paths.as_deref());
    let actor = actor.as_ref().map(|(a, l)| (*a, l.as_str()));
    let mut times = Times::default();
    let agent = agent_ci_on(h, repo, &p.r#ref, p.after).await?;
    let call = Call::Push { actor, event: &event };
    let n = runs_on(env, site, b, repo, f.owner, record, p.after, make, call, agent, &mut times).await?;
    if waits(env, agent.0) {
        release_if_gated(h, b, repo, f.owner, record, p.after).await?;
    }
    Ok(n)
}

/// Commit `id`'s raw object from this site's git cache shard: a protocol v2 fetch of the commit alone
/// (`graftyard_git::commit_fetch`), as REST's `GET .../commits/{ref}` reads it on the edge.
async fn head_commit(
    env: &Env,
    site: &SiteEnv,
    repo: RepoId,
    record: &PlacementRecord,
    id: git::refs::ObjectId,
) -> std::result::Result<Vec<u8>, String> {
    let me = site.id;
    let region = site.sites.get(me).ok_or("this site is not in GRAFTYARD_SITES")?.region;
    let stub = env
        .durable_object(&format!("SHARD_{}", me.0))
        .and_then(|ns| {
            ns.get_by_name_with_location_hint(
                &git::first_shard_name(graftyard_types::NetworkId::of_root(repo), me),
                region.hint(),
            )
        })
        .map_err(|e| e.to_string())?;
    let rec = serde_json::to_string(record).map_err(|e| e.to_string())?;
    let url =
        format!("https://shard/upload-pack?repo={}&wait=30&record={}", repo.0.0, js_sys::encode_uri_component(&rec));
    let hd = Headers::new();
    hd.set("git-protocol", "version=2").map_err(|e| e.to_string())?;
    let mut init = RequestInit::new();
    let body = git::commit_fetch::request(id);
    init.with_method(Method::Post).with_headers(hd).with_body(Some(js_sys::Uint8Array::from(&body[..]).into()));
    let req = Request::new_with_init(&url, &init).map_err(|e| e.to_string())?;
    let mut r = stub.fetch_with_request(req).await.map_err(|e| e.to_string())?;
    if r.status_code() != 200 {
        return Err(format!("upload-pack {}: {}", r.status_code(), r.text().await.unwrap_or_default()));
    }
    git::commit_fetch::commit_of(&r.bytes().await.map_err(|e| e.to_string())?, id)
}

/// `agent-ci`: what the repository's `AgentPolicy.ci` does to a run on `branch` at `head`
/// (`graftyard_actions::trigger::agent_run`), from the leader's fold and the install's `AgentApp` rows, read once per
/// event, with `head`, the agent's commit an `AutoReadOnly` run's jobs wait on for `AgentGate` (0x15.14). `AgentRun::No`
/// unless a site day set `GRAFTYARD_AGENT_CI`, so every run is created as before until then.
async fn agent_ci_on(
    h: &Rc<Host<Repo>>,
    repo: RepoId,
    branch: &git::refs::RefName,
    head: git::refs::ObjectId,
) -> std::result::Result<(AgentRun, git::refs::ObjectId), String> {
    if !crate::actions::agent_ci(&h.env) {
        return Ok((AgentRun::No, head));
    }
    let (ci, gated) = {
        let core = h.core.borrow();
        let fold = &core.as_ref().ok_or("not joined")?.m.group_fold().fold;
        (git::tiers::agent_policy(&*fold.core).ci, git::merge::agent_gate_holds(&*fold.core, repo, branch, head))
    };
    let prefixes = agent_prefixes(&h.env).await?;
    Ok((agent_run(ci, branch, &prefixes, gated), head))
}

/// Whether an agent run is created waiting (`CreateRunWaiting`, 0x15.14): one under `AutoReadOnly` where a site day set
/// `GRAFTYARD_SECRETS_WAIT`. The flag decides only what this site sends.
fn waits(env: &Env, agent: AgentRun) -> bool {
    agent == AgentRun::ReadOnly && crate::actions::secrets_wait(env)
}

/// `secrets-wait` (0x15.14): `AgentGate` approval `g`, read again on this leader now. When the gate holds for `g.head`
/// on its pull request's head branch, `ReleaseSecrets` on the repository's Actions shard; whether it was sent. A
/// gate that does not hold yet (one approval of two) releases nothing, and a later approval's entry tries again.
async fn gate_released(
    h: &Rc<Host<Repo>>,
    b: &Bindings<'_>,
    f: &Found,
    g: &GateEvent,
) -> std::result::Result<bool, String> {
    let branch = {
        let core = h.core.borrow();
        let core = &*core.as_ref().ok_or("not joined")?.m.group_fold().fold.core;
        git::tiers::pull_head(core, g.n).filter(|ph| ph.head.repo == f.repo).map(|ph| ph.head.branch)
    };
    let Some(branch) = branch else { return Ok(false) };
    release_on(h, b, f.repo, f.owner, &f.record, &branch, g.head).await
}

/// After an agent run was created waiting on `head`: the gate read again, so an `AgentGate` that committed between
/// this item's first read (`agent_ci_on`) and the create, whose own item may have released nothing yet, is not lost.
/// Either the gate's entry applied before this read and this releases, or after it, and its item's release reaches the
/// shard after the create committed.
async fn release_if_gated(
    h: &Rc<Host<Repo>>,
    b: &Bindings<'_>,
    repo: RepoId,
    owner: AccountId,
    record: &PlacementRecord,
    head: git::refs::ObjectId,
) -> std::result::Result<(), String> {
    let branches: Vec<git::refs::RefName> = {
        let core = h.core.borrow();
        let core = &*core.as_ref().ok_or("not joined")?.m.group_fold().fold.core;
        git::tiers::pulls_with_head(core, repo, head)
    };
    for branch in branches {
        release_on(h, b, repo, owner, record, &branch, head).await?;
    }
    Ok(())
}

/// `ReleaseSecrets` for `head` when `AgentGate` holds there for a pull request from `branch`, read on this leader.
/// The shard cannot read the repository's fold (no `deps` reads a repository chain), so the gate is read here; a gate
/// that stops holding between this read and the commit is the accepted gap in `decisions/DECISIONS.md`,
/// "`secrets-wait`".
async fn release_on(
    h: &Rc<Host<Repo>>,
    b: &Bindings<'_>,
    repo: RepoId,
    owner: AccountId,
    record: &PlacementRecord,
    branch: &git::refs::RefName,
    head: git::refs::ObjectId,
) -> std::result::Result<bool, String> {
    let holds = {
        let core = h.core.borrow();
        let core = &*core.as_ref().ok_or("not joined")?.m.group_fold().fold.core;
        git::merge::agent_gate_holds(core, repo, branch, head)
    };
    if !holds {
        return Ok(false);
    }
    let chain = ChainId::Actions(owner, graftyard_actions::actions_shard(repo));
    let body = json!({ "op": "release_secrets", "repo": repo, "head": head.hex() });
    graftyard_lookup::create(b, chain, record, &json!({})).await?;
    let (v, _) = graftyard_lookup::call(b, chain, record, "/runsvc", &body).await?;
    match v["status"].as_u64() {
        Some(200) => Ok(true),
        _ => Err(format!("release_secrets answered {v}")),
    }
}

/// `agent_ci_on` for pull request `n`'s run: its head branch at its head commit, a same-repository pull request's
/// only, since `pr_target` starts no run for a fork's.
async fn agent_ci_of(
    h: &Rc<Host<Repo>>,
    repo: RepoId,
    n: git::issues::IssueNumber,
) -> std::result::Result<(AgentRun, git::refs::ObjectId), String> {
    let ph = {
        let core = h.core.borrow();
        git::tiers::pull_head(&*core.as_ref().ok_or("not joined")?.m.group_fold().fold.core, n)
    };
    match ph.filter(|ph| ph.head.repo == repo) {
        Some(ph) => agent_ci_on(h, repo, &ph.head.branch, ph.head_sha).await,
        None => Ok((AgentRun::No, git::refs::ObjectId::ZERO)),
    }
}

/// Every `AgentApp`'s `confine` on the install's own `Authz` chain, through `fresh`.
async fn agent_prefixes(env: &Env) -> std::result::Result<Vec<graftyard_identity::glob::RefPattern>, String> {
    let install = graftyard_identity::install_account();
    let chain = ChainId::Authz(install);
    let record = crate::member::owner_record(env, install).await?;
    let (v, _) = crate::lookup::call(env, chain, &record, "/read", &json!({ "q": { "dep": true } })).await?;
    let bytes = v.get("value").and_then(Value::as_str).and_then(crate::host::unhex).ok_or("no Authz fold")?;
    let f: graftyard_identity::AuthzFold = graftyard_raft::codec::from_bytes(&bytes).map_err(|e| e.to_string())?;
    Ok(f.more.more.agent_apps.values().filter_map(|r| r.confine.clone()).collect())
}

/// How `runs_on` calls `create_run`: as the pull request and merge group triggers do (`create_run_body`), or as a
/// `push` (`runsvc::push_run_call`).
enum Call<'a> {
    Run { base: Option<&'a str>, head: Option<&'a str>, actor: Option<(AccountId, &'a str)> },
    Push { actor: Option<(AccountId, &'a str)>, event: &'a Value },
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
    call: Call<'_>,
    (agent, agent_head): (AgentRun, git::refs::ObjectId),
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
        let (mut run, mut jobs, mut permissions) = match make(&path, src) {
            Ok(Some(x)) => x,
            Ok(None) => continue,
            Err(e) => {
                console_error!("actions trigger {} {path}: {e}", repo.0.0);
                continue;
            }
        };
        // `reusable`: a workflow that calls a reusable one, or whose jobs declare `outputs:`, is created with its plan
        // where a site day set `GRAFTYARD_REUSABLE` (0x15.9). Its called files are read as the edge's dispatch reads
        // them (`reusable::called_file`, `CallReads`), so a call into another repository runs from a push or a pull
        // request too (F132); a tree that cannot be read or breaks a limit is skipped, as a file that does not parse.
        let parsed = graftyard_actions::workflow::parse(&path, src);
        let origin = graftyard_actions::reusable::Origin {
            repo: None,
            path: path.clone(),
            r#ref: run.head_ref.to_string(),
            sha: tip.0,
        };
        let t = crate::now_ms();
        let reads = CallReads { env, site, b, repo, record, failed: RefCell::new(None) };
        let planned = graftyard_actions::reusable::plan_run(&parsed.workflow, &origin, reusable(env), |w| {
            graftyard_actions::reusable::called_file(&reads, w)
        })
        .await;
        times.reads += crate::now_ms().saturating_sub(t);
        // A read that failed (no leader, a shed, a tier still loading) is tried again with the item, not taken as a
        // tree that cannot run.
        if let Some(e) = reads.failed.take() {
            return Err(format!("{path}: {e}"));
        }
        let plan = match planned {
            Ok(Some(e)) => {
                (jobs, permissions) = (e.jobs, e.declared);
                e.plan
            }
            Ok(None) => None,
            Err(e) => {
                console_error!("actions trigger {} {path}: {e}", repo.0.0);
                continue;
            }
        };
        agent.mark(&mut run);
        let body = graftyard_actions::runsvc::with_plan(
            match &call {
                Call::Run { base, head, actor } => {
                    crate::actions::create_run_body(env, &run, &jobs, &permissions, *base, *head, *actor)
                }
                Call::Push { actor, event } => graftyard_actions::runsvc::push_run_call(
                    &run,
                    &jobs,
                    &permissions,
                    crate::actions::job_tokens(env),
                    *actor,
                    event,
                ),
            },
            plan.as_ref(),
        );
        // `agent-ci`: an agent's run under `ci: Approve` is created held for a writer's approval (0x15.13).
        let body = if agent == AgentRun::Held { graftyard_actions::runsvc::held(body) } else { body };
        // `secrets-wait`: an agent's run under `ci: AutoReadOnly` is created waiting, so its jobs that reference a
        // secret wait for `AgentGate` on the agent's own commit, not the pull request's test merge (0x15.14).
        let body = if waits(env, agent) { graftyard_actions::runsvc::waiting(body, &agent_head.0) } else { body };
        let t = crate::now_ms();
        let created = async {
            graftyard_lookup::create(b, chain, record, &json!({})).await?;
            graftyard_lookup::call(b, chain, record, "/runsvc", &body).await
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

/// `ActionsTrigger`'s reads for a plan (`graftyard_actions::reusable::called_file`, F132), as the edge's dispatch's:
/// `owner/repo` resolved by name on the install (the owner's login row, its `Placement` row, the name on its `Authz`
/// chain) and read as an anonymous caller from a narrow `tarball/{version}` read on its group, and each file from its
/// network's git cache shard on this site at the pinned commit.
struct CallReads<'a> {
    env: &'a Env,
    site: &'a SiteEnv,
    b: &'a Bindings<'a>,
    repo: RepoId,
    record: &'a PlacementRecord,
    /// The first read that failed, which `runs_on` retries rather than skipping the workflow.
    failed: RefCell<Option<String>>,
}

impl graftyard_actions::reusable::CallReads for CallReads<'_> {
    type At = PlacementRecord;
    async fn other(
        &self,
        owner: &str,
        name: &str,
        version: &str,
    ) -> std::result::Result<Option<graftyard_actions::reusable::Other<PlacementRecord>>, String> {
        self.note(self.other_rows(owner, name, version).await)
    }
    async fn blob(
        &self,
        repo: Option<&graftyard_actions::reusable::Other<PlacementRecord>>,
        sha: [u8; 20],
        path: &str,
    ) -> std::result::Result<Option<String>, String> {
        let (r, rec) = repo.map_or((self.repo, self.record), |o| (o.id, &o.at));
        let commit = git::refs::ObjectId(sha).hex();
        let v = self.note(ui(self.env, self.site, r, rec, "blob", &[("commit", &commit), ("path", path)]).await)?;
        Ok(v.map(|v| v["text"].as_str().unwrap_or_default().to_owned()))
    }
}

impl CallReads<'_> {
    fn note<T>(&self, r: std::result::Result<T, String>) -> std::result::Result<T, String> {
        if let Err(e) = &r {
            self.failed.borrow_mut().get_or_insert_with(|| e.clone());
        }
        r
    }

    async fn other_rows(
        &self,
        owner: &str,
        name: &str,
        version: &str,
    ) -> std::result::Result<Option<graftyard_actions::reusable::Other<PlacementRecord>>, String> {
        use graftyard_identity as id;
        let (Ok(login), Ok(rn)) = (id::Login::parse(owner), id::RepoName::parse(name)) else { return Ok(None) };
        let names = ChainId::Names(id::name_shard(&id::NameKey::Login(login.key())));
        let nrec = crate::release::system_record(self.env, names).await?;
        let (v, _) = graftyard_lookup::call(self.b, names, &nrec, "/read", &json!({ "q": { "login": owner } })).await?;
        let Ok(Some(id::LoginRow::Claimed { account, .. })) =
            serde_json::from_value::<Option<id::LoginRow>>(v.pointer("/value/login").cloned().unwrap_or_default())
        else {
            return Ok(None);
        };
        let record = crate::member::owner_record(self.env, account).await?;
        let q = json!({ "q": { "repo": rn.as_str() } });
        let Some((v, _, _)) = graftyard_lookup::read_spread(self.b, ChainId::Authz(account), &record, &q).await? else {
            return Ok(None);
        };
        let Some(other) = v.pointer("/value/repo/repo").and_then(Value::as_i64) else { return Ok(None) };
        let other = RepoId(graftyard_types::DbId(other));
        let route = graftyard_api::narrow::Route::Rest {
            tail: vec!["tarball".into(), version.into()],
            query: String::new(),
            path: format!("/repos/{owner}/{name}/tarball/{version}"),
            agent: false,
        };
        let at = graftyard_api::view::RepoRef { id: other, owner: owner.into(), owner_id: account, name: name.into() };
        let q = json!({ "q": { "rows": { "api": graftyard_api::narrow::Spec { at, route } } } });
        let chain = ChainId::Repo(other);
        // Created on first use, as the edge's reads create it (`first_use`).
        let v = match graftyard_lookup::read_spread(self.b, chain, &record, &q).await? {
            Some((v, _, _)) => v,
            None => {
                graftyard_lookup::create(self.b, chain, &record, &json!({ "owner": account })).await?;
                graftyard_lookup::read_spread(self.b, chain, &record, &q)
                    .await?
                    .ok_or_else(|| format!("{owner}/{name}: its group has no leader"))?
                    .0
            }
        };
        let rows =
            graftyard_api::hosted::TierRows::from_value(&v["value"]).map_err(|e| format!("{owner}/{name}: {e}"))?;
        // A member whose metadata tier is loading answers the access rows alone, without the refs: as on the edge,
        // that is not an answer yet.
        if !rows.complete || rows.meta.is_none() {
            return Err(format!("{owner}/{name}: the repository's tiers are loading"));
        }
        Ok(Some(graftyard_actions::reusable::Other {
            id: other,
            owner: account,
            core: graftyard_api::hosted::TierRows::store(&rows.core),
            at: record,
        }))
    }
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
        .and_then(|ns| {
            ns.get_by_name_with_location_hint(
                &git::first_shard_name(graftyard_types::NetworkId::of_root(repo), me),
                region.hint(),
            )
        })
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
