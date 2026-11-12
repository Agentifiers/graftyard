//! The `MergeQueue` consumer's pure half (`design/DESIGN.md`, Shape, "Merge queue"; `design/SHARED.md`, "Event
//! fan-out"): which entries ask it to look at a base branch, what it reads of that base, and `step`, a function from
//! that read and the git cache's answers to the commands it submits.
//!
//! It keeps no state of its own. Everything it decides on is the base's queue as the core tier holds it, read fresh
//! (`QueueView`), so a new leader, a redelivered batch, or a second consumer run plans the same commands, and a
//! command planned on a view that has since changed is refused by its `decide` and costs only that refusal. The host
//! (M2-WP5's cursor, Queue, and consumer Worker) runs it as:
//!
//! 1. For a batch of committed entries, `touched` names the bases to look at. A base with a queue is also looked at
//!    on a timer, so a check timeout or a `min_wait_minutes` fires with no new entry.
//! 2. Read `QueueView::of` from the leader's fresh core tier.
//! 3. `step(view, now, answers)` gives `Step::Ask` for each merge it needs from the git cache, or `Step::Submit`
//!    with the commands. The host runs each `CacheAsk` on `GitCacheShard::merge`, places the pack, and calls `step`
//!    again on a fresh view with the answers. An answer for an older view is ignored.
//! 4. Commit each command in order. A `QueueMerge` refused with `(n, err)` goes to `on_refused`, which ejects `n`.

use graftyard_fold::Read;
use graftyard_raft::{EntryBody, LogEntry};
use graftyard_types::Timestamp;
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;

use crate::issue_ops::queue_base;
use crate::issues::{BranchRule, Grouping, IssueNumber, MergeMethod, MergeQueueSettings, PrState};
use crate::merge::{MergeCmd, MergeError, MergePlan, RequiredCheckView, RuleViolation, check_state, rules_over};
use crate::pack::DurablePack;
use crate::queue::{
    EjectReason, MergeQueueEntry, QueueAction, QueueEvent, QueueMerge, QueueOp, queue_of, queue_settings, queued,
};
use crate::refs::{ObjectId, RefName};
use crate::repo::RepoEvent;
use crate::tiers::{get_rec, pull_head, pulls_from, ref_key};

const MINUTE_MS: u64 = 60_000;

/// The bases an entry asks `MergeQueue` to look at: a `Queue` event's, a moved base or queued head, a required
/// conclusion on a queue ref, a closed or merged queued pull request, and every queue on a rule change.
pub fn touched(core: &dyn Read, repo: graftyard_types::RepoId, e: &LogEntry<RepoEvent>) -> Vec<RefName> {
    let EntryBody::Events(evs) = &e.body else { return Vec::new() };
    let mut out: Vec<RefName> = Vec::new();
    let mut add = |b: RefName| {
        if !out.contains(&b) {
            out.push(b);
        }
    };
    for ev in evs {
        match ev {
            RepoEvent::Queue(QueueEvent::Enqueued { base, .. } | QueueEvent::GroupBuilt { base, .. }) => {
                add(base.clone())
            }
            RepoEvent::Queue(QueueEvent::Landed { base, .. }) => add(base.clone()),
            RepoEvent::Queue(QueueEvent::Ejected { refs, .. }) => {
                refs.iter().filter_map(|u| queue_base(&u.name)).for_each(&mut add)
            }
            RepoEvent::Refs { updates, .. } => {
                for u in updates {
                    if !queue_of(core, &u.name).is_empty() {
                        add(u.name.clone());
                    }
                    for n in pulls_from(core, repo, &u.name) {
                        if let Some((b, _)) = queued(core, n) {
                            add(b);
                        }
                    }
                }
            }
            RepoEvent::RequiredCheck { sha, .. } => {
                for b in crate::queue::queue_bases(core) {
                    let built =
                        queue_of(core, &b).iter().any(|(_, r)| r.built.is_some_and(|(e, _)| e.group_tip == *sha));
                    if built {
                        add(b);
                    }
                }
            }
            RepoEvent::PrClosed { n, .. } | RepoEvent::Merged { n, .. } => {
                if let Some((b, _)) = queued(core, *n) {
                    add(b);
                }
            }
            RepoEvent::Rule(_) => crate::queue::queue_bases(core).into_iter().for_each(&mut add),
            _ => {}
        }
    }
    out
}

/// One queued pull request as the consumer reads it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntryView {
    pub n: IssueNumber,
    pub pinned: ObjectId,
    /// The pull request's head now, `None` once it is closed or merged.
    pub head: Option<ObjectId>,
    pub enqueued: Timestamp,
    pub built: Option<BuiltView>,
}

/// A built entry, when its `GroupBuilt` committed, and the required conclusions on its `group_tip`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuiltView {
    pub entry: MergeQueueEntry,
    pub at: Timestamp,
    pub checks: Vec<RequiredCheckView>,
}

/// What the consumer reads of one base branch, through the leader's `fresh`. One type on both sides of the member
/// route, so they cannot drift.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueView {
    pub base: RefName,
    pub tip: Option<ObjectId>,
    pub settings: Option<MergeQueueSettings>,
    pub rules: Vec<BranchRule>,
    pub entries: Vec<EntryView>,
}

impl QueueView {
    pub fn of(core: &dyn Read, base: &RefName) -> QueueView {
        let rules = rules_over(core, base);
        let entries = queue_of(core, base)
            .into_iter()
            .map(|(_, r)| {
                let head = pull_head(core, r.n).filter(|p| matches!(p.state, PrState::Open { .. })).map(|p| p.head_sha);
                let built = r.built.map(|(entry, at)| BuiltView {
                    entry,
                    at,
                    checks: crate::merge::required_on(core, &entry.group_tip),
                });
                EntryView { n: r.n, pinned: r.head, head, enqueued: r.enqueued, built }
            })
            .collect();
        QueueView {
            base: base.clone(),
            tip: get_rec(core, "ref", &[ref_key(base)]),
            settings: queue_settings(&rules),
            rules,
            entries,
        }
    }

    /// How many entries from the head are built and chain from the base tip. A base move that no `QueueMerge` made
    /// leaves none, so the queue is rebuilt from the new tip.
    pub fn valid_built(&self) -> usize {
        let mut tip = self.tip;
        self.entries
            .iter()
            .take_while(|e| match &e.built {
                Some(b) if Some(b.entry.parent_tip) == tip => {
                    tip = Some(b.entry.group_tip);
                    true
                }
                _ => false,
            })
            .count()
    }
}

/// What the consumer asks the git cache for. `Build` merges each pull request in order on the previous result,
/// starting at `on`; `Plan` re-plans built entries for landing, each `head` on its `parent_tip`, which must give its
/// `group_tip` again.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CacheAsk {
    Build { base: RefName, on: ObjectId, prs: Vec<(IssueNumber, ObjectId)>, method: MergeMethod },
    Plan { base: RefName, entries: Vec<(MergeQueueEntry, ObjectId)>, method: MergeMethod },
}

/// The git cache's answer to a `CacheAsk`.
#[allow(clippy::large_enum_variant)] // short-lived, one per planning round
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CacheAnswer {
    /// The entries built from `on`, in order, and one pack of all their objects. `conflict` names the first pull
    /// request that did not merge; the entries before it are still good.
    Built {
        ask: CacheAsk,
        entries: SmallVec<[MergeQueueEntry; 8]>,
        pack: Option<DurablePack>,
        conflict: Option<IssueNumber>,
    },
    Planned {
        ask: CacheAsk,
        plans: Vec<(IssueNumber, MergePlan)>,
    },
}

impl CacheAnswer {
    fn ask(&self) -> &CacheAsk {
        match self {
            CacheAnswer::Built { ask, .. } | CacheAnswer::Planned { ask, .. } => ask,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// Nothing to do until a new entry or `at`, when a check times out or the wait ends.
    Idle {
        at: Option<Timestamp>,
    },
    Ask(CacheAsk),
    Submit(Vec<MergeCmd>),
}

/// A consumer command's request id, from its content: a redelivered batch planned on the same view re-sends the same
/// commands under the same ids, which their `request` rows answer, and one planned on a changed view gets new ids. The
/// top 16 bits are a tag no edge-minted id has, as `PrSync`'s are on M2-WP5's branch [inference: the same rule].
pub fn request_id(cmd: &MergeCmd, planned_on: &QueueView) -> graftyard_types::RequestId {
    use graftyard_raft::Record as _;
    use sha2::Digest as _;
    let mut b = Vec::new();
    cmd.encode(&mut b);
    graftyard_fold::canon::write(&mut b, planned_on);
    let h = sha2::Sha256::digest(&b);
    let low = u128::from_be_bytes(h[..16].try_into().expect("16 bytes")) & ((1u128 << 112) - 1);
    graftyard_types::RequestId(graftyard_types::Ulid((0x4d51u128 << 112) | low))
}

fn eject(n: IssueNumber, reason: EjectReason) -> MergeCmd {
    MergeCmd::Queue(QueueOp { op: QueueAction::Eject { n, reason }, actor: None })
}

fn checks_of(e: &EntryView) -> Vec<RequiredCheckView> {
    e.built.as_ref().map_or_else(Vec::new, |b| b.checks.clone())
}

/// Every required check of every rule over the base.
fn required_names(v: &QueueView) -> Vec<(Box<str>, Option<crate::issues::AppId>)> {
    v.rules
        .iter()
        .filter_map(|r| r.protection.as_ref()?.checks.as_ref())
        .flat_map(|c| c.checks.iter().cloned())
        .collect()
}

/// A built entry's checks: `Ok(true)` all pass, `Ok(false)` some still pending or expected, `Err` a failure.
fn entry_checks(v: &QueueView, e: &EntryView) -> Result<bool, Box<str>> {
    let have = checks_of(e);
    let mut done = true;
    for (name, app) in required_names(v) {
        match check_state(&name, app, &have) {
            Ok(()) => {}
            Err(RuleViolation::CheckFailing { name }) => return Err(name),
            Err(_) => done = false,
        }
    }
    Ok(done)
}

/// Whether a built entry's own ref has a pending or failed conclusion for a required check.
fn blocked(v: &QueueView, e: &EntryView) -> bool {
    let have = checks_of(e);
    required_names(v).iter().any(|(n, a)| {
        matches!(
            check_state(n, *a, &have),
            Err(RuleViolation::CheckPending { .. } | RuleViolation::CheckFailing { .. })
        )
    })
}

/// The first required check that has no passing conclusion yet.
fn missing(v: &QueueView, e: &EntryView) -> Box<str> {
    let have = checks_of(e);
    required_names(v).into_iter().find(|(n, a)| check_state(n, *a, &have).is_err()).map_or("".into(), |(n, _)| n)
}

/// Pure: the next thing to do on base `v.base` at `now`, given the cache answers the host has for this view.
/// Ejections first, then a landing, then a build.
pub fn step(v: &QueueView, now: Timestamp, answers: &[CacheAnswer]) -> Step {
    let Some(s) = &v.settings else {
        // The queue lost its rule: every queued pull request leaves it.
        let all = v.entries.iter().map(|e| eject(e.n, EjectReason::Protection(RuleViolation::MergeQueueRequired)));
        let all: Vec<MergeCmd> = all.collect();
        return if all.is_empty() { Step::Idle { at: None } } else { Step::Submit(all) };
    };
    let Some(tip) = v.tip else { return Step::Idle { at: None } };
    let valid = v.valid_built();
    let timeout = u64::from(s.check_timeout_minutes) * MINUTE_MS;
    let mut wake: Option<Timestamp> = None;
    let mut ejects = Vec::new();
    for e in &v.entries {
        if e.head.is_none() {
            ejects.push(eject(e.n, EjectReason::Closed));
        } else if e.head != Some(e.pinned) {
            ejects.push(eject(e.n, EjectReason::HeadMoved));
        }
    }
    // Only the first built entry without a passing result is judged: every later queue ref holds its changes, so a
    // later failure may be its. A failure ejects it, which unbuilds the later entries; no result within the timeout
    // ejects it too.
    let gone = |n: IssueNumber| {
        ejects
            .iter()
            .any(|c| matches!(c, MergeCmd::Queue(QueueOp { op: QueueAction::Eject { n: m, .. }, .. }) if *m == n))
    };
    if let Some(e) = v.entries[..valid].iter().find(|e| entry_checks(v, e) != Ok(true))
        && !gone(e.n)
    {
        let built_at = e.built.as_ref().expect("a valid built entry").at;
        let due = Timestamp(built_at.0 + timeout);
        match entry_checks(v, e) {
            Err(name) => ejects.push(eject(e.n, EjectReason::CheckFailed { name })),
            Ok(_) if now >= due => ejects.push(eject(e.n, EjectReason::CheckTimeout { name: missing(v, e) })),
            Ok(_) => wake = Some(due),
        }
    }
    if !ejects.is_empty() {
        return Step::Submit(ejects);
    }
    // Land the longest run from the head whose checks allow it, at most `max_merge`.
    let max = usize::from(s.max_merge.max(1)).min(valid);
    let pass: Vec<bool> = v.entries[..max].iter().map(|e| entry_checks(v, e) == Ok(true)).collect();
    let k = match s.grouping {
        Grouping::AllGreen => pass.iter().take_while(|p| **p).count(),
        // The head's checks decide, but never past an entry whose own ref has a pending or failed conclusion
        // (`QueueMerge::decide` refuses that too). An entry with no conclusion at all does not hold the group.
        Grouping::HeadGreen => {
            let open = v.entries[..max].iter().take_while(|e| !blocked(v, e)).count();
            pass[..open].iter().rposition(|p| *p).map_or(0, |i| i + 1)
        }
    };
    if k > 0 {
        let entries: Vec<(MergeQueueEntry, ObjectId)> =
            v.entries[..k].iter().map(|e| (e.built.as_ref().expect("built").entry, e.pinned)).collect();
        let ask = CacheAsk::Plan { base: v.base.clone(), entries, method: s.method };
        return match answers.iter().find(|a| *a.ask() == ask) {
            Some(CacheAnswer::Planned { plans, .. }) => {
                let entry = v.entries[k - 1].built.as_ref().expect("built").entry;
                Step::Submit(vec![MergeCmd::Land(QueueMerge { plans: plans.clone(), entry })])
            }
            _ => Step::Ask(ask),
        };
    }
    // Build after the valid entries, up to `max_build`, once `min_merge` wait or the oldest has waited
    // `min_wait_minutes`.
    let waiting = &v.entries[valid..];
    let room = usize::from(s.max_build.max(1)).saturating_sub(valid);
    if waiting.is_empty() || room == 0 {
        return Step::Idle { at: wake };
    }
    let ready_at = Timestamp(waiting[0].enqueued.0 + u64::from(s.min_wait_minutes) * MINUTE_MS);
    if valid == 0 && waiting.len() < usize::from(s.min_merge) && now < ready_at {
        return Step::Idle { at: Some(wake.map_or(ready_at, |w| w.min(ready_at))) };
    }
    let on = v.entries[..valid].last().map_or(tip, |e| e.built.as_ref().expect("built").entry.group_tip);
    let prs = waiting.iter().take(room).map(|e| (e.n, e.pinned)).collect();
    let ask = CacheAsk::Build { base: v.base.clone(), on, prs, method: s.method };
    match answers.iter().find(|a| *a.ask() == ask) {
        Some(CacheAnswer::Built { entries, pack, conflict, .. }) => {
            let mut cmds = Vec::new();
            if let (Some(pack), false) = (pack, entries.is_empty()) {
                let build = QueueAction::Build { base: v.base.clone(), entries: entries.clone(), pack: pack.clone() };
                cmds.push(MergeCmd::Queue(QueueOp { op: build, actor: None }));
            }
            if let Some(n) = conflict {
                cmds.push(eject(*n, EjectReason::Conflict));
            }
            if cmds.is_empty() { Step::Idle { at: wake } } else { Step::Submit(cmds) }
        }
        _ => Step::Ask(ask),
    }
}

/// A refused command's follow-up: a refused landing ejects the pull request it names, unless the refusal only says
/// the queue moved under it, which the next `step` rebuilds, or that a check is running again, which it waits for.
pub fn on_refused(cmd: &MergeCmd, n: IssueNumber, err: &MergeError) -> Option<MergeCmd> {
    if !matches!(cmd, MergeCmd::Land(_)) {
        return None;
    }
    let reason = match err {
        MergeError::Protection(RuleViolation::CheckFailing { name }) => EjectReason::CheckFailed { name: name.clone() },
        // A re-requested check: the landing waits for the new run (BF-23).
        MergeError::Protection(RuleViolation::CheckPending { .. } | RuleViolation::CheckExpected { .. }) => {
            return None;
        }
        MergeError::Protection(v) => EjectReason::Protection(v.clone()),
        MergeError::HeadModified => EjectReason::HeadMoved,
        MergeError::NotOpen | MergeError::NotFound | MergeError::Draft => EjectReason::Closed,
        _ => return None,
    };
    Some(eject(n, reason))
}

// ---- the host's half: the cursor, the read, and the git cache ----

/// The leader alarm's read for `MergeQueue`, as `crate::prsync::scan` is `PrSync`'s: up to `SCAN_MAX` entries and
/// `SCAN_BYTES` of stored rows past what `d` has read, each by `entry`, recording the bases it touched. `Ok(true)` when
/// entries are left for the next alarm.
pub fn scan(
    d: &mut crate::prsync::Dispatcher<RefName>,
    core: &dyn Read,
    repo: graftyard_types::RepoId,
    cursor: graftyard_types::Index,
    applied: graftyard_types::Index,
    entry: impl FnMut(graftyard_types::Index) -> Option<(LogEntry<RepoEvent>, usize)>,
) -> Result<bool, graftyard_types::Index> {
    crate::prsync::scan_with(d, cursor, applied, entry, |e| touched(core, repo, e))
}

/// The leader's answer to the consumer's read (`{"q": {"mergequeue": base}}`): the committed `MergeQueue` cursor and
/// the base's `QueueView`, at one index. One type on both sides of the member route.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueRead {
    pub cursor: graftyard_types::Index,
    pub view: QueueView,
}

impl QueueRead {
    pub fn of(core: &dyn Read, base: &RefName) -> QueueRead {
        QueueRead {
            cursor: crate::tiers::consumer_cursor(core, crate::issues::Consumer::MergeQueue),
            view: QueueView::of(core, base),
        }
    }
}
