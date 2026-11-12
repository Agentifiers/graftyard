//! The `PrSync` consumer's pure half (`design/SHARED.md`, "Event fan-out" and "Issues and pull requests";
//! `design/DESIGN.md`, "Dominant access patterns", the webhook path: leader alarm, cursor, Queue message, consumer).
//!
//! - The cursor is the core-tier row `consumer_cursor`, written only by a committed `ConsumerCursor` event in the same
//!   entry as the work it covers. It is the group's state, so a new leader and a restored member read the same one.
//! - The leader's `Dispatcher` is memory only. It reads the log from the cursor, names the pull requests the entries
//!   touch, and hands out `SyncBatch`es. A batch always starts at the committed cursor. Its first attempt runs in
//!   band (`Delivery::First`): the leader runs the consumer's handler itself. Work the cursor has not passed
//!   `REDELIVER_AFTER_MS` after it was first handed out is sent again (`Delivery::Again`), through the site's queue, so
//!   a lost attempt, a dead consumer, or a new leader only delays. A chain whose cursor has not moved after
//!   `PARK_AFTER` batches is parked until it does.
//! - The consumer reads each pull request's state fresh (`views`), has the git cache compute missing test merges, and
//!   commits a `PrSync`. Its `decide` keeps only what still holds on the fold, so a batch delivered twice, or out of
//!   order with a later one, changes nothing the first did not.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

use graftyard_fold::Read;
use graftyard_raft::{Command, EntryBody, Format, LogEntry, Record, TierSet};
use graftyard_types::{ChainId, Index, NetworkId, RepoId, RequestId, Timestamp, Ulid};
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;

use crate::issues::{Consumer, IssueChange, IssueNumber, Mergeability, MergeabilityResult, PrState};
use crate::pack::PackRef;
use crate::refs::{ObjectId, RefName, RefUpdate};
use crate::repo::{RepoEvent, RepoFold, RepoScratch};
use crate::tiers::{consumer_cursor, get_rec, head_ref, merge_ref, mergeability, pull_head, pulls_from, ref_key};

/// The repository chain's records that carry `PrSync` and the `TestMerged` and `ConsumerCursor` events. A member
/// reads it only once a release enables it (`design/SHARED.md`, "Record formats and the two-release rule").
pub const REPO_FORMAT_V3: Format = Format { family: 0x20, version: 3 };

/// Work the cursor has not passed this long after it was first handed out is sent again, through the queue [guess].
/// The first attempt runs in band, so this covers an attempt that failed silently or never returned, a message lost
/// with its consumer, or a leader change; Queues retries a failed queue delivery itself.
pub const REDELIVER_AFTER_MS: u64 = 30_000;
/// Batches handed out, first attempts and re-sends, while the committed cursor stays at one index. Past it the
/// dispatcher parks the chain for its consumer (`design/SHARED.md`, "Event fan-out", Isolation: a handler that fails 10
/// times on one entry parks that chain), so a batch that fails the same way every time stops waking the network's git
/// cache shard every `REDELIVER_AFTER_MS` (`tests/am1/results/idle-shards.md`). A cursor that moves, because the work
/// committed or an operator replayed it, unparks the chain.
pub const PARK_AFTER: u32 = 10;
/// A stretch of entries that touch no pull request is still covered by a cursor entry this often [guess], so a new
/// leader never reads more than this many entries back.
pub const CURSOR_STRIDE: u64 = 1_000;
/// Inline merge-pack bytes per `PrSync` command, under `COMMAND_MAX` with room for the rest of the command. A test
/// merge whose own objects pack larger is not recorded: its merge ref is deleted and `mergeable` stays null, as DESIGN
/// allows past the test-merge budget ("Shape", `TestMerge`) [guess: a merge commit and its new trees are a few KB].
pub const MERGE_BYTES_PER_COMMAND: usize = 900_000;
/// Entries one leader alarm reads for the dispatcher [guess]; the alarm comes back for the rest.
pub const SCAN_MAX: u64 = 2_000;
/// How soon after the leader applies entries its alarm runs the dispatcher [guess]: BF-24's 10 s holds the queue's
/// delivery, the test merge, and a commit round as well.
pub const ALARM_MS: i64 = 50;
/// The leader's alarm while the cursor trails a sent batch, so an overdue one is sent again [guess].
pub const WAIT_MS: i64 = 5_000;

/// One Queue message: the pull requests the entries `(from, through]` touched. `from` is the committed cursor when
/// the leader sent it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncBatch<T = IssueNumber> {
    pub from: Index,
    pub through: Index,
    pub prs: Vec<T>,
}

/// The pull requests an entry asks `PrSync` to look at: a push to an open pull request's head branch, an open, or a
/// reopen. `PrSync`'s own events touch none of these, so its entries ask for nothing more. A base branch's move is
/// not here: DESIGN runs a test merge "on a head push, on Actions dispatch ..., and
/// on a single-PR GET" ("Shape", `TestMerge`), so a base move leaves `mergeable` visibly stale.
pub fn touched(core: &dyn Read, repo: RepoId, e: &LogEntry<RepoEvent>) -> Vec<IssueNumber> {
    let EntryBody::Events(evs) = &e.body else { return Vec::new() };
    let mut out = Vec::new();
    for ev in evs {
        match ev {
            RepoEvent::Refs { updates, .. } => {
                for u in updates.iter().filter(|u| u.new.is_some()) {
                    out.extend(pulls_from(core, repo, &u.name));
                }
            }
            RepoEvent::PrOpened { n, .. } => out.push(*n),
            RepoEvent::IssueChanged { n, change: IssueChange::State { open: true, .. }, .. } => out.push(*n),
            _ => {}
        }
    }
    out
}

/// How a batch goes out: its first attempt, which the leader runs in band, or a re-send of work the cursor has not
/// passed in `REDELIVER_AFTER_MS`, which goes through the queue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delivery {
    First,
    Again,
}

/// The leader's half of the cursor, in memory only: `scanned` is the last entry read, `pending` what each unhandled
/// entry touched (pull requests for `PrSync`, base branches for `MergeQueue`), `sent` the last batch's `through`,
/// `sent_at` when the work the cursor has not passed was first handed out, or last sent again, `running` when the
/// first attempt in flight started, `tries` the batches handed out since the committed cursor last moved, and
/// `tried_at` the cursor they were handed out from. Nothing here outlives the leadership; the committed cursor is
/// where a new one starts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dispatcher<T = IssueNumber> {
    scanned: Index,
    pending: BTreeMap<Index, Vec<T>>,
    sent: Index,
    sent_at: u64,
    running: Option<u64>,
    tries: u32,
    tried_at: Index,
}

impl<T> Default for Dispatcher<T> {
    fn default() -> Dispatcher<T> {
        Dispatcher {
            scanned: Index(0),
            pending: BTreeMap::new(),
            sent: Index(0),
            sent_at: 0,
            running: None,
            tries: 0,
            tried_at: Index(0),
        }
    }
}

impl<T: Ord + Clone> Dispatcher<T> {
    /// The entries to read before `batch`: `(scanned, applied]`. A dispatcher behind the committed cursor, a new
    /// leader's, starts at the cursor; entries the cursor passed are dropped.
    pub fn to_read(&mut self, cursor: Index, applied: Index) -> Option<(Index, Index)> {
        self.scanned = self.scanned.max(cursor);
        self.pending = self.pending.split_off(&Index(cursor.0 + 1));
        (applied > self.scanned).then_some((self.scanned, applied))
    }

    /// Records what entry `index` touched. Entries come in order, right after `to_read`.
    pub fn read(&mut self, index: Index, prs: Vec<T>) {
        debug_assert_eq!(index.0, self.scanned.0 + 1, "entries are read in order");
        self.scanned = index;
        if !prs.is_empty() {
            self.pending.insert(index, prs);
        }
    }

    /// The batch to send now, if any, and how. `Again` once work the cursor has not passed was handed out
    /// `REDELIVER_AFTER_MS` ago, however many batches went out since, so a first attempt that never returns is
    /// re-sent while pushes keep coming. Otherwise `First` when new entries touched a pull request, or when
    /// `CURSOR_STRIDE` entries passed the cursor. Only one first attempt runs at a time: new work waits for it and
    /// goes out as one batch, and a first attempt that has not `finished` `REDELIVER_AFTER_MS` after it started
    /// counts as lost, so it no longer holds the work behind it. A batch always starts at the
    /// committed cursor and names every pull request after it, so no entry between the cursor and `through` goes
    /// unhandled, whoever led before. After `PARK_AFTER` batches at one cursor the chain is parked until the cursor
    /// moves.
    pub fn batch(&mut self, cursor: Index, now: Timestamp) -> Option<(SyncBatch<T>, Delivery)> {
        if self.scanned <= cursor {
            return None;
        }
        if cursor != self.tried_at {
            self.tried_at = cursor;
            self.tries = 0;
        }
        if self.tries >= PARK_AFTER {
            return None;
        }
        let outstanding = self.sent > cursor;
        let fresh = self.scanned > self.sent;
        let overdue = outstanding && now.0 >= self.sent_at.saturating_add(REDELIVER_AFTER_MS);
        let stride = self.scanned.0 - cursor.0 >= CURSOR_STRIDE;
        let touched = self.pending.range(Index(self.sent.0 + 1)..).next().is_some();
        // A first attempt that has not finished in REDELIVER_AFTER_MS died with its isolate or its object: no longer
        // wait for it. Its own work is overdue by then, and goes out again below.
        if self.running.is_some_and(|t| now.0 >= t.saturating_add(REDELIVER_AFTER_MS)) {
            self.running = None;
        }
        let due = fresh && (touched || stride);
        let how = if overdue {
            Delivery::Again
        } else if due && self.running.is_none() {
            self.running = Some(now.0);
            Delivery::First
        } else {
            return None;
        };
        if how == Delivery::Again || !outstanding {
            self.sent_at = now.0;
        }
        self.sent = self.scanned;
        let prs: BTreeSet<T> = self.pending.values().flatten().cloned().collect();
        self.tries = self.tries.saturating_add(1);
        Some((SyncBatch { from: cursor, through: self.scanned, prs: prs.into_iter().collect() }, how))
    }

    /// Whether this consumer's chain is parked at `cursor` after `PARK_AFTER` batches without the cursor moving.
    pub fn parked(&self, cursor: Index) -> bool {
        self.tried_at == cursor && self.tries >= PARK_AFTER
    }

    /// Restore a park its member stored, so a new leadership does not spend another `PARK_AFTER` attempts.
    pub fn park(&mut self, cursor: Index) {
        self.tried_at = cursor;
        self.tries = PARK_AFTER;
    }

    /// Whether the cursor still trails work, so the leader's alarm should come back for it. A parked chain does not
    /// keep re-arming the alarm.
    pub fn waiting(&self, cursor: Index) -> bool {
        !self.parked(cursor) && (self.sent > cursor || !self.pending.is_empty())
    }

    /// The last batch did not reach the queue: its work is due again at once.
    pub fn unsent(&mut self) {
        self.sent_at = 0;
    }

    /// The first attempt in flight returned: the next one may start. One that failed leaves its work due again at
    /// once, through the queue.
    pub fn finished(&mut self, ok: bool) {
        self.running = None;
        if !ok {
            self.unsent();
        }
    }
}

/// The leader alarm's read: up to `SCAN_MAX` entries past what `d` has read, from the committed cursor on, each by
/// `entry`. Stops at an entry `entry` cannot give (`Err` names it), so nothing past a missing entry is read.
pub fn scan(
    d: &mut Dispatcher,
    core: &dyn Read,
    repo: RepoId,
    cursor: Index,
    applied: Index,
    mut entry: impl FnMut(Index) -> Option<LogEntry<RepoEvent>>,
) -> Result<(), Index> {
    let Some((from, to)) = d.to_read(cursor, applied) else { return Ok(()) };
    for i in from.0 + 1..=to.0.min(from.0 + SCAN_MAX) {
        let e = entry(Index(i)).ok_or(Index(i))?;
        d.read(Index(i), touched(core, repo, &e));
    }
    Ok(())
}

/// REST's `mergeable` (GraphQL `MERGEABLE`, `CONFLICTING`, or `UNKNOWN`): the recorded test merge, only while it is of
/// the pull request's current head and its base's current tip. Otherwise it is null until the next test merge, as
/// GHES 3.22 documents for one not yet computed.
pub fn mergeable(m: Option<&Mergeability>, head: ObjectId, base_tip: Option<ObjectId>) -> Option<bool> {
    let m = m.filter(|m| m.head == head && Some(m.base) == base_tip)?;
    Some(matches!(m.result, MergeabilityResult::Clean { .. }))
}

// ---- The test-merge budget (`design/DESIGN.md`, Shape, `TestMerge` and `TEST_MERGE_BUDGET`) ----

/// Of the merging shard's vCPU, the share test merges may use.
pub const TEST_MERGE_BUDGET: f32 = 0.25;
/// A single-PR GET by a writer or CI token computes at most once per PR per this long.
pub const GET_COALESCE_MS: u64 = 30_000;
/// A test merge's estimated vCPU-ms before it runs: `MERGE_VCPU_S` [measured, `tests/m0/results/m0-wp9.md`, B0-34:
/// torvalds/linux p50 5 to 45 ms; `scripts/cost.py` prices 0.05 s].
pub const MERGE_EST_MS: f32 = 50.0;
/// The share of a second's budget a GET may not take, kept for pushes and dispatch [guess].
pub const GET_RESERVE: f32 = 0.5;

/// Why a test merge would run. Pushes and Actions dispatch are served first, then GETs; list fields never compute.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum MergeCause {
    HeadPush,
    Dispatch,
    /// A single-PR GET by a writer or CI token.
    Get,
    /// A list field (`GET /pulls`, a connection). Never computes.
    List,
}

/// A shard's vCPU at a Containers instance type [doc: Containers instance types, `research/cloudflare.md`], or `None`
/// for a name that is not one, which the site refuses rather than guess a budget for.
pub fn instance_vcpu(instance: &str) -> Option<f32> {
    Some(match instance {
        "lite" => 0.0625,
        "basic" => 0.25,
        "standard-1" => 0.5,
        "standard-2" => 1.0,
        "standard-3" => 2.0,
        "standard-4" => 4.0,
        _ => return None,
    })
}

/// What one network's budget did on its shard since `since` (ms), B0-35's instruments: the CPU ms charged and the
/// merges' wall ms, merges computed and shed, GET asks and GET computes, asks from list fields, which never compute,
/// and merges computed for one, which must stay zero. `low_ms` is the lowest the bucket went and `max_ms` the largest
/// single charge, so `low_ms > -max_ms.max(MERGE_EST_MS)` shows the budget was never overrun beyond one merge. The
/// shard sends it with each test merge (`x-graft-budget`) and in `/status`; the leader that ran the consumer keeps the latest
/// in `/stats` as `testMerges`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BudgetStats {
    pub since: u64,
    /// The budget, in vCPU.
    pub vcpu: f32,
    pub spent_ms: f64,
    pub wall_ms: f64,
    pub computed: u32,
    pub shed: u32,
    pub get_asked: u32,
    pub get_computes: u32,
    pub list_asked: u32,
    pub list_computes: u32,
    pub low_ms: f32,
    pub max_ms: f32,
    /// The most merges at once that had passed the first look at the budget and were catching up, waiting for the
    /// network's turn, or running: above 1, merges arrived concurrently and the turn put them in line.
    pub in_flight_max: u32,
}

/// The test-merge budget of one network on one site: a bucket of vCPU-milliseconds that refills at TEST_MERGE_BUDGET
/// of the vCPU of the shard that runs the merges, the network's first shard on the leader's site, and holds one second
/// of it, or one merge's estimate when that is more. Pushes and dispatch may take the bucket below zero by one merge,
/// so they are never shed while GETs are; a GET runs only on a positive balance that covers its estimate. That bound
/// holds only if each merge is charged before the next is admitted, which `budgeted` ensures. The bucket is in that
/// shard object's memory (`Budgets`), and a restart starts it full: the object resets about 90 s after its last
/// request, by when the bucket had refilled anyway [measured, `tests/am1/results/merge-budget-2.md`]. Its counts
/// (`BudgetStats`) outlive the reset: the shard stores them and a restarted object goes on from them (`restored`).
#[derive(Clone, Debug, PartialEq)]
pub struct MergeBudget {
    rate: f32,
    ms: f32,
    at: u64,
    /// The last GET compute per PR, for GET_COALESCE_MS.
    got: BTreeMap<IssueNumber, u64>,
    /// Merges inside `asked` now, for `BudgetStats::in_flight_max`.
    in_flight: u32,
    stats: BudgetStats,
}

impl MergeBudget {
    /// The budget of a shard of `vcpu` (`instance_vcpu`), full at `now`.
    pub fn new(vcpu: f32, now: u64) -> MergeBudget {
        let rate = TEST_MERGE_BUDGET * vcpu * 1000.0;
        let stats = BudgetStats { since: now, vcpu: rate / 1000.0, ..BudgetStats::default() };
        MergeBudget { rate, ms: rate, at: now, got: BTreeMap::new(), in_flight: 0, stats }
    }

    /// A restarted shard's budget: full at `now`, as `new`, going on from the counts the shard stored, so `since` and
    /// every count cover the network's merges on this site across resets. `vcpu` is the shard's now.
    pub fn restored(vcpu: f32, now: u64, stats: BudgetStats) -> MergeBudget {
        let mut b = MergeBudget::new(vcpu, now);
        b.stats = BudgetStats { vcpu: b.rate / 1000.0, ..stats };
        b
    }

    fn refill(&mut self, now: u64, est_ms: f32) {
        let dt = now.saturating_sub(self.at) as f32 / 1000.0;
        self.ms = (self.ms + self.rate * dt).min(self.rate.max(est_ms));
        self.at = self.at.max(now);
    }

    /// Whether a test merge for `cause` of pull request `n`, estimated at `est_ms` of vCPU, may run now: `ask`, then
    /// `decide`. A `true` charges the estimate; `charge` corrects it once the merge's real cost is known. A `false`
    /// leaves `mergeable` null (GraphQL `UNKNOWN`) until a later compute.
    pub fn admit(&mut self, cause: MergeCause, n: IssueNumber, est_ms: f32, now: u64) -> bool {
        self.ask(cause, n, now) && self.decide(cause, n, est_ms, now)
    }

    /// The ask, before any budget is looked at: a list field's is counted and refused, and a GET's is counted and
    /// refused while the pull request's last GET ask is under GET_COALESCE_MS old. A GET that passes holds that slot,
    /// so the GETs that arrive while it waits for its turn do not queue behind it.
    pub fn ask(&mut self, cause: MergeCause, n: IssueNumber, now: u64) -> bool {
        match cause {
            MergeCause::HeadPush | MergeCause::Dispatch => true,
            MergeCause::List => {
                self.stats.list_asked += 1;
                false
            }
            MergeCause::Get => {
                self.stats.get_asked += 1;
                let fresh = self.got.get(&n).is_none_or(|t| now >= t + GET_COALESCE_MS);
                if fresh {
                    self.got.insert(n, now);
                }
                fresh
            }
        }
    }

    /// Whether the budget would admit a merge for `cause` now, without taking anything from it: pushes and dispatch
    /// from a positive balance, so with one merge in flight (`budgeted`) the bucket never goes lower than one merge's
    /// cost below zero, and a GET only while GET_RESERVE of a second's budget is left for pushes and dispatch, which
    /// arrive unannounced.
    pub fn would(&mut self, cause: MergeCause, est_ms: f32, now: u64) -> bool {
        self.refill(now, est_ms);
        match cause {
            MergeCause::List => false,
            MergeCause::HeadPush | MergeCause::Dispatch => self.ms > 0.0,
            MergeCause::Get => self.ms - est_ms >= GET_RESERVE * self.rate,
        }
    }

    /// The budget's answer to an ask that passed (`would`); a `true` takes the estimate. A `false` counts as shed, and
    /// a shed GET gives back its coalescing slot, since it computed nothing.
    pub fn decide(&mut self, cause: MergeCause, n: IssueNumber, est_ms: f32, now: u64) -> bool {
        let ok = self.would(cause, est_ms, now);
        if ok {
            self.ms -= est_ms;
        } else {
            self.stats.shed += 1;
            if cause == MergeCause::Get {
                self.got.remove(&n);
            }
        }
        ok
    }

    /// A merge for `cause` admitted at `est_ms` ran, used `cpu_ms` of CPU, and took `wall_ms`. The bucket is charged the
    /// CPU: hosted, a merge's wall time, which includes waiting for the container's vCPUs behind validates and
    /// catch-ups, was 1.9 to 4.4 times its CPU (`tests/am1/results/merge-budget-2.md`).
    pub fn charge(&mut self, cause: MergeCause, est_ms: f32, cpu_ms: f32, wall_ms: f32) {
        self.ms -= cpu_ms - est_ms;
        let s = &mut self.stats;
        s.low_ms = s.low_ms.min(self.ms);
        s.max_ms = s.max_ms.max(cpu_ms);
        s.spent_ms += f64::from(cpu_ms);
        s.wall_ms += f64::from(wall_ms);
        s.computed += 1;
        match cause {
            MergeCause::List => s.list_computes += 1,
            MergeCause::Get => s.get_computes += 1,
            MergeCause::HeadPush | MergeCause::Dispatch => {}
        }
    }

    /// The budget, in vCPU.
    pub fn vcpu(&self) -> f32 {
        self.rate / 1000.0
    }

    pub fn stats(&self) -> BudgetStats {
        self.stats.clone()
    }
}

/// Every network's `MergeBudget` in one shard object, by network: a hot-pool object serves several networks, and each
/// network's budget is its own (`design/DESIGN.md`, Shape, `MergeBudget`). `vcpu` is the shard's. Each network also
/// has one turn, so its merges run one at a time (`budgeted`).
#[derive(Clone)]
pub struct Budgets {
    vcpu: f32,
    of: BTreeMap<NetworkId, MergeBudget>,
    turns: BTreeMap<NetworkId, crate::merge_cache::CatchUps>,
}

impl Budgets {
    pub fn new(vcpu: f32) -> Budgets {
        Budgets { vcpu, of: BTreeMap::new(), turns: BTreeMap::new() }
    }

    /// `net`'s budget, full at `now` on first use.
    pub fn of(&mut self, net: NetworkId, now: u64) -> &mut MergeBudget {
        self.of.entry(net).or_insert_with(|| MergeBudget::new(self.vcpu, now))
    }

    pub fn stats(&self, net: NetworkId) -> Option<BudgetStats> {
        self.of.get(&net).map(MergeBudget::stats)
    }

    /// Puts back the counts a shard stored for `net` before it restarted (`MergeBudget::restored`), unless this object
    /// already budgets `net`, whose counts are newer.
    pub fn restore(&mut self, net: NetworkId, stats: BudgetStats, now: u64) {
        let vcpu = self.vcpu;
        self.of.entry(net).or_insert_with(|| MergeBudget::restored(vcpu, now, stats));
    }

    fn turn(&mut self, net: NetworkId) -> crate::merge_cache::CatchUps {
        self.turns.entry(net).or_default().clone()
    }
}

/// The shard's test merge under the budget: `MergeBudget::ask`, then `asked`.
pub async fn budgeted<T, E, P: Future<Output = Result<(), E>>, F: Future<Output = Result<(T, Option<f32>), E>>>(
    budgets: &RefCell<Budgets>,
    net: NetworkId,
    cause: MergeCause,
    n: IssueNumber,
    clock: impl Fn() -> u64,
    prepare: impl FnOnce() -> P,
    run: impl FnOnce() -> F,
) -> Result<Option<T>, E> {
    let now = clock();
    if !budgets.borrow_mut().of(net, now).ask(cause, n, now) {
        return Ok(None);
    }
    asked(budgets, net, cause, n, clock, prepare, run).await
}

/// A test merge whose ask passed. A merge the budget would not admit now is shed before anything runs. Otherwise
/// `prepare` (the shard's catch-up, not charged) runs, then the merge waits for `net`'s turn, so the merge before it
/// has been charged, and the budget decides again. Only when admitted does `run` (the merge) run; it answers with the
/// CPU ms of the merge's own git processes, which is charged, or `run`'s wall time on `clock` (ms) when the container
/// did not report it. Admitting at the estimate while earlier merges still ran let LM-1 round 5's shard spend 0.274
/// vCPU of 0.25: up to five batches' merges were in flight at once, each admitted before those ahead of it were charged
/// (`tests/am1/results/merge-budget-2.md`). `None` when the budget shed it, so `mergeable` stays null. A merge that
/// failed is charged for its time.
pub async fn asked<T, E, P: Future<Output = Result<(), E>>, F: Future<Output = Result<(T, Option<f32>), E>>>(
    budgets: &RefCell<Budgets>,
    net: NetworkId,
    cause: MergeCause,
    n: IssueNumber,
    clock: impl Fn() -> u64,
    prepare: impl FnOnce() -> P,
    run: impl FnOnce() -> F,
) -> Result<Option<T>, E> {
    {
        let now = clock();
        let mut all = budgets.borrow_mut();
        let b = all.of(net, now);
        if !b.would(cause, MERGE_EST_MS, now) {
            b.decide(cause, n, MERGE_EST_MS, now);
            return Ok(None);
        }
        b.in_flight += 1;
        b.stats.in_flight_max = b.stats.in_flight_max.max(b.in_flight);
    }
    /// Leaves `in_flight` however `asked` ends: charged, shed, failed, or dropped.
    struct InFlight<'a>(&'a RefCell<Budgets>, NetworkId);
    impl Drop for InFlight<'_> {
        fn drop(&mut self) {
            if let Some(b) = self.0.borrow_mut().of.get_mut(&self.1) {
                b.in_flight = b.in_flight.saturating_sub(1);
            }
        }
    }
    let _in_flight = InFlight(budgets, net);
    prepare().await?;
    let turn = budgets.borrow_mut().turn(net);
    let _turn = turn.turn().await;
    let now = clock();
    if !budgets.borrow_mut().of(net, now).decide(cause, n, MERGE_EST_MS, now) {
        return Ok(None);
    }
    let t0 = clock();
    let r = run().await;
    let wall = clock().saturating_sub(t0) as f32;
    let cpu = r.as_ref().ok().and_then(|(_, c)| *c).unwrap_or(wall);
    budgets.borrow_mut().of(net, t0).charge(cause, MERGE_EST_MS, cpu, wall);
    r.map(|(t, _)| Some(t))
}

/// An on-demand test merge's command, for a single-PR GET (the edge) or Actions dispatch (the site's
/// `crate::prsync::demand`, which M4's `ActionsTrigger` calls): the merge the shard answered for pull request `n` at
/// (base, head), recorded as `TestMerged`. It moves no head ref and no cursor, so the consumer's batches go on as
/// before, and `decide_sync` records it only while `head` is the pull request's head. `None` when the shard shed it or its pack is past MERGE_BYTES_PER_COMMAND, so
/// nothing is committed and `mergeable` stays null. Its request id is its content's, so the same merge asked twice
/// commits once.
pub fn on_demand(n: IssueNumber, base: ObjectId, head: ObjectId, answer: MergeAnswer) -> Option<PrSync> {
    let m = answer.test_merge(n, base, head)?;
    if matches!(&m.pack, Some(PackRef::Inline(b)) if b.len() > MERGE_BYTES_PER_COMMAND) {
        return None;
    }
    let mut c = PrSync {
        from: Index(0),
        through: None,
        heads: Vec::new(),
        merges: vec![m],
        unmerged: Vec::new(),
        request: RequestId(Ulid(0)),
    };
    c.request = request_id(&c);
    Some(c)
}

/// One test merge's answer, into the batch's `merges`, or its `shed` (`commands`), so `mergeable` reads null when the
/// shard ran no merge.
pub fn take(
    a: MergeAnswer,
    n: IssueNumber,
    base: ObjectId,
    head: ObjectId,
    merges: &mut Vec<TestMerge>,
    shed: &mut Vec<(IssueNumber, ObjectId)>,
) -> Result<(), &'static str> {
    match a {
        MergeAnswer::Shed { .. } => shed.push((n, head)),
        a => merges.push(a.test_merge(n, base, head).ok_or("the shard named no object id")?),
    }
    Ok(())
}

impl MergeCause {
    /// The shard's `cause` query value.
    pub fn as_str(self) -> &'static str {
        match self {
            MergeCause::HeadPush => "push",
            MergeCause::Dispatch => "dispatch",
            MergeCause::Get => "get",
            MergeCause::List => "list",
        }
    }

    pub fn parse(s: &str) -> Option<MergeCause> {
        [MergeCause::HeadPush, MergeCause::Dispatch, MergeCause::Get, MergeCause::List]
            .into_iter()
            .find(|c| c.as_str() == s)
    }
}

/// What the consumer reads of one pull request, through the leader's `fresh`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrView {
    pub n: IssueNumber,
    pub open: bool,
    /// The head is a branch of this repository. A fork's head is on the fork's chain, which `PrSync` does not read.
    pub same_repo: bool,
    pub head: ObjectId,
    pub base: RefName,
    pub base_tip: Option<ObjectId>,
    pub head_ref: Option<ObjectId>,
    pub merge: Option<Mergeability>,
}

impl PrView {
    pub fn mergeable(&self) -> Option<bool> {
        mergeable(self.merge.as_ref(), self.head, self.base_tip)
    }

    /// The test merge this pull request lacks: none, or one already recorded, for its (base tip, head).
    pub fn needs_merge(&self) -> Option<(ObjectId, ObjectId)> {
        let base = self.base_tip?;
        let current = self.merge.as_ref().is_some_and(|m| m.base == base && m.head == self.head);
        (self.open && self.same_repo && !current).then_some((base, self.head))
    }
}

/// The cursor and each pull request's view, from the core tier: the leader's answer to the consumer's read.
pub fn views(core: &dyn Read, repo: RepoId, prs: &[IssueNumber]) -> (Index, Vec<PrView>) {
    let views = prs
        .iter()
        .filter_map(|n| {
            let ph = pull_head(core, *n)?;
            Some(PrView {
                n: *n,
                open: matches!(ph.state, PrState::Open { .. }),
                same_repo: ph.head.repo == repo,
                head: ph.head_sha,
                base_tip: get_rec(core, "ref", &[ref_key(&ph.base)]),
                base: ph.base,
                head_ref: get_rec(core, "ref", &[ref_key(&head_ref(*n))]),
                merge: mergeability(core, *n),
            })
        })
        .collect();
    (consumer_cursor(core, Consumer::PrSync), views)
}

/// The leader's answer to the consumer's read (`{"q": {"prsync": [n..]}}`): the cursor, each pull request's view,
/// and the `mergeable` GraphQL renders for each. One type on both sides of the member route, so they cannot drift.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncRead {
    pub cursor: Index,
    pub views: Vec<PrView>,
    pub mergeable: Vec<Option<bool>>,
}

impl SyncRead {
    pub fn of(core: &dyn Read, repo: RepoId, prs: &[IssueNumber]) -> SyncRead {
        let (cursor, views) = views(core, repo, prs);
        let mergeable = views.iter().map(PrView::mergeable).collect();
        SyncRead { cursor, views, mergeable }
    }
}

/// One test merge the git cache computed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestMerge {
    pub n: IssueNumber,
    pub base: ObjectId,
    pub head: ObjectId,
    pub result: MergeabilityResult,
    /// The merge commit's own objects, for a clean merge.
    pub pack: Option<PackRef>,
}

/// The git cache's answer to a test merge (`POST /merge` on the shard's container): a clean merge's commit and a pack of
/// its new objects, or a conflict's paths. Only the clean answer's pack enters the log.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MergeAnswer {
    Clean {
        merge: String,
        #[serde(with = "crate::pack::b64")]
        pack: Vec<u8>,
    },
    Conflict {
        conflict: Vec<String>,
    },
    /// The shard's answer past TEST_MERGE_BUDGET, or to a list field: no merge ran, and `mergeable` stays null. The
    /// container never sends it.
    Shed {
        shed: bool,
    },
}

impl MergeAnswer {
    /// The test merge of pull request `n` at (base, head), or `None` when the answer names no object id or no merge
    /// ran (`Shed`). A conflict keeps its first four paths, as `MergeabilityResult::Conflict` holds.
    pub fn test_merge(self, n: IssueNumber, base: ObjectId, head: ObjectId) -> Option<TestMerge> {
        let (result, pack) = match self {
            MergeAnswer::Clean { merge, pack } => {
                let merge = ObjectId::parse_hex(merge.as_bytes())?;
                (MergeabilityResult::Clean { merge }, Some(PackRef::Inline(pack)))
            }
            MergeAnswer::Conflict { conflict } => {
                let paths = conflict.into_iter().take(4).map(Into::into).collect();
                (MergeabilityResult::Conflict { paths }, None)
            }
            MergeAnswer::Shed { .. } => return None,
        };
        Some(TestMerge { n, base, head, result, pack })
    }
}

/// The command the consumer commits. `heads` moves `refs/pull/N/head` to each head; `merges` records each test
/// merge as `TestMerged`; `unmerged` names heads whose test merge could not be recorded, whose old merge ref is
/// deleted rather than left on an older head; `through`, on the batch's last command, moves the cursor. `from` is the
/// batch's.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrSync {
    pub from: Index,
    pub through: Option<Index>,
    pub heads: Vec<(IssueNumber, ObjectId)>,
    pub merges: Vec<TestMerge>,
    pub unmerged: Vec<(IssueNumber, ObjectId)>,
    pub request: RequestId,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncOut {
    /// Events this command committed: 0 for a batch the cursor had already passed.
    pub events: u32,
    pub cursor: Index,
}

/// A command's request id, from its content: a redelivered batch planned on the same fold re-sends the same commands
/// under the same ids, which their `request` rows answer, and a retry planned on a fold its first attempt changed
/// gets new ids, so no command is answered from a row of a different one. The top 16 bits are a tag no edge-minted
/// id has before the year 10889, since those hold milliseconds there.
pub fn request_id(c: &PrSync) -> RequestId {
    use sha2::Digest as _;
    let mut zeroed = c.clone();
    zeroed.request = RequestId(Ulid(0));
    let mut b = Vec::new();
    graftyard_fold::canon::write(&mut b, &zeroed);
    let h = sha2::Sha256::digest(&b);
    let low = u128::from_be_bytes(h[..16].try_into().expect("16 bytes")) & ((1u128 << 112) - 1);
    RequestId(Ulid((0x5059u128 << 112) | low))
}

/// The commands for a batch: the head moves and the test merges, at most `MERGE_BYTES_PER_COMMAND` of merge packs
/// each, the last moving the cursor. A merge whose pack alone is past the bound goes to `unmerged`, and so does each
/// of `shed`, the merges `MergeBudget` did not admit, so `mergeable` reads null for them.
pub fn commands(
    batch: &SyncBatch,
    views: &[PrView],
    merges: Vec<TestMerge>,
    shed: Vec<(IssueNumber, ObjectId)>,
) -> Vec<PrSync> {
    let heads: Vec<(IssueNumber, ObjectId)> =
        views.iter().filter(|v| v.open && v.same_repo && v.head_ref != Some(v.head)).map(|v| (v.n, v.head)).collect();
    let bytes = |m: &TestMerge| match &m.pack {
        Some(PackRef::Inline(b)) => b.len(),
        _ => 0,
    };
    let mut unmerged = shed;
    let mut chunks: Vec<Vec<TestMerge>> = vec![Vec::new()];
    let mut used = 0;
    for m in merges {
        let n = bytes(&m);
        if n > MERGE_BYTES_PER_COMMAND {
            unmerged.push((m.n, m.head));
            continue;
        }
        if used + n > MERGE_BYTES_PER_COMMAND {
            chunks.push(Vec::new());
            used = 0;
        }
        used += n;
        chunks.last_mut().expect("one chunk").push(m);
    }
    let last = chunks.len() - 1;
    chunks
        .into_iter()
        .enumerate()
        .map(|(i, merges)| {
            let mut c = PrSync {
                from: batch.from,
                through: (i == last).then_some(batch.through),
                heads: if i == 0 { heads.clone() } else { Vec::new() },
                merges,
                unmerged: if i == 0 { unmerged.clone() } else { Vec::new() },
                request: RequestId(Ulid(0)),
            };
            c.request = request_id(&c);
            c
        })
        .collect()
}

impl PrSync {
    /// Pure: the events that still hold on the fold. A head move only to the pull request's current head; a test
    /// merge only of its current (base tip, head), and only once; the cursor only forward.
    pub fn decide_sync(&self, s: &RepoScratch<'_>) -> (SmallVec<[RepoEvent; 4]>, SyncOut) {
        let core = s.core();
        let cursor = consumer_cursor(core, Consumer::PrSync);
        if self.through.is_some_and(|t| t <= cursor) {
            return (SmallVec::new(), SyncOut { events: 0, cursor });
        }
        let open_head = |n: IssueNumber| {
            pull_head(core, n).filter(|ph| matches!(ph.state, PrState::Open { .. })).map(|ph| (ph.head_sha, ph.base))
        };
        let mut updates = Vec::new();
        for (n, new) in &self.heads {
            let name = head_ref(*n);
            let have = s.get(&name);
            if open_head(*n).is_some_and(|(h, _)| h == *new) && have != Some(*new) {
                updates.push(RefUpdate { name, expect: have, new: Some(*new) });
            }
        }
        for (n, head) in &self.unmerged {
            let name = merge_ref(*n);
            let have = s.get(&name);
            let recorded = mergeability(core, *n).is_some_and(|r| r.head == *head);
            if open_head(*n).is_some_and(|(h, _)| h == *head) && !recorded && have.is_some() {
                updates.push(RefUpdate { name, expect: have, new: None });
            }
        }
        let mut events: SmallVec<[RepoEvent; 4]> = SmallVec::new();
        if !updates.is_empty() {
            events.push(RepoEvent::Refs { push: self.request, actor: None, updates, pack: None });
        }
        // A merge of the pull request's current head is recorded even when its base has moved since: the row is keyed
        // by the SHA pair, so `mergeable` shows it stale rather than leaving the merge ref on an older head.
        for m in &self.merges {
            let Some((head, _)) = open_head(m.n) else { continue };
            let recorded = mergeability(core, m.n).is_some_and(|r| r.base == m.base && r.head == m.head);
            if head != m.head || recorded {
                continue;
            }
            events.push(RepoEvent::TestMerged {
                n: m.n,
                base: m.base,
                head: m.head,
                result: m.result.clone(),
                pack: m.pack.clone(),
            });
        }
        let mut next = cursor;
        if let Some(t) = self.through {
            events.push(RepoEvent::ConsumerCursor { consumer: Consumer::PrSync, through: t });
            next = t;
        }
        let n = events.len() as u32;
        (events, SyncOut { events: n, cursor: next })
    }
}

impl Record for PrSync {
    fn encode(&self, out: &mut Vec<u8>) {
        graftyard_fold::canon::write(out, self);
    }
}

/// `PrSync` alone as a command; a member hosts it inside `crate::repo::RepoCmd`.
impl Command for PrSync {
    type Fold = RepoFold;
    type Dep = graftyard_identity::IdentityDep;
    type Out = SyncOut;
    type Err = std::convert::Infallible;

    fn format(&self) -> Format {
        REPO_FORMAT_V3
    }
    fn tiers(&self) -> TierSet {
        TierSet::CORE
    }
    /// A consumer's command names no credential (`design/DESIGN.md`, Shape, `CredentialShard`).
    fn deps(&self, _fold: &RepoFold) -> SmallVec<[ChainId; 3]> {
        SmallVec::new()
    }
    fn decide(
        &self,
        s: &RepoScratch<'_>,
        _deps: &[(ChainId, Self::Dep)],
    ) -> Result<(SmallVec<[RepoEvent; 4]>, SyncOut), Self::Err> {
        Ok(self.decide_sync(s))
    }
    fn replay(&self, events: &[RepoEvent]) -> SyncOut {
        let cursor = events
            .iter()
            .find_map(|e| match e {
                RepoEvent::ConsumerCursor { through, .. } => Some(*through),
                _ => None,
            })
            .unwrap_or(self.from);
        SyncOut { events: events.len() as u32, cursor }
    }
}
