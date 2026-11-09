//! The merge queue (`design/DESIGN.md`, Shape, "Merge queue" and "Invariants"; `design/SHARED.md`, "Issues and pull
//! requests", merge queue): `MergeQueueEntry` with its queue ref and the `parent_tip` rule, `QueueEvent`,
//! `EjectReason`, `QueueOp`, and `QueueMerge`, whose `decide` lands a whole group or nothing.
//!
//! The queue of each base branch is core-tier rows in queue order: `queue` keyed by (base, seq), and `queue_pr` from a
//! pull request to its row. A row is built once a `GroupBuilt` names it, and built rows are always a prefix of the
//! queue whose `parent_tip`s chain from the base tip, because `decide` refuses any `Build` that does not extend the
//! last built entry or start over at the base tip, and any `QueueMerge` that does not start at the base tip.

use graftyard_fold::{Read, Rows, Val};
use graftyard_identity::{IdentityDep, Principal, RepoPermission, UserId};
use graftyard_types::{ChainId, Timestamp};
use serde::{Deserialize, Serialize};
use smallvec::{SmallVec, smallvec};

use crate::issue_ops::RepoView;
use crate::issues::{BranchRule, IssueNumber, MergeQueueSettings};
use crate::merge::{
    MergeError, MergePlan, PrView, RuleViolation, Via, authorize, check_state, head_deletion, open_pull, protect,
    required_on, rules_over,
};
use crate::pack::{DurablePack, PackRef};
use crate::refs::{ObjectId, RefName, RefUpdate};
use crate::repo::RepoEvent;
use crate::tiers::{get_rec, int, n_key, put_rec, ref_key};

/// One pull request's place in a base branch's queue. `group_tip` is the base plus the changes of this entry and every
/// earlier one; `parent_tip` is the previous entry's `group_tip`, or the base tip for the first entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeQueueEntry {
    pub pr: IssueNumber,
    pub group_tip: ObjectId,
    pub parent_tip: ObjectId,
}

/// A built entry's queue ref, `refs/heads/gh-readonly-queue/{base}/pr-{n}-{parent_tip}`, at `group_tip`
/// [unverified which SHA GitHub names]. `None` when the base is so long that the queue ref would pass
/// `REF_NAME_MAX`; `Enqueue` refuses such a base, so every built row has `Some`.
pub fn queue_ref(base: &RefName, e: &MergeQueueEntry) -> Option<RefName> {
    let b = base.display();
    let b = b.strip_prefix("refs/heads/").unwrap_or(&b);
    let name = format!("refs/heads/gh-readonly-queue/{b}/pr-{}-{}", e.pr.0, e.parent_tip.hex());
    RefName::parse(name.as_bytes()).ok()
}

/// Whether every queue ref of `base` is a ref name: the widest pull request number with any `parent_tip`.
fn queue_refs_fit(base: &RefName) -> bool {
    let widest = MergeQueueEntry { pr: IssueNumber(u32::MAX), group_tip: ObjectId::ZERO, parent_tip: ObjectId::ZERO };
    queue_ref(base, &widest).is_some()
}

/// Stored in `Ejected`: add variants at the end only.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EjectReason {
    CheckFailed { name: Box<str> },
    CheckTimeout { name: Box<str> },
    HeadMoved,
    Conflict,
    Protection(RuleViolation),
    Closed,
    Dequeued,
}

/// SHARED's `RepoEvent::Queue`, core tier, on the base repository's chain. `Enqueued` pins the pull request's head.
/// `GroupBuilt` appends entries after the last built one, or replaces every built entry when its first `parent_tip` is
/// the base tip, and `refs` creates and deletes the queue refs in the same entry. `Ejected` removes one pull request
/// and unbuilds every later entry, deleting their refs. `Landed` rides in the `QueueMerge` entry and removes the queue
/// through `through`, deleting the landed entries' refs. Add variants and fields at the end only.
#[allow(clippy::large_enum_variant)] // the stored shape is DESIGN's, with a group's entries inline
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum QueueEvent {
    Enqueued { n: IssueNumber, base: RefName, head: ObjectId, actor: UserId },
    GroupBuilt { base: RefName, entries: SmallVec<[MergeQueueEntry; 8]>, refs: Vec<RefUpdate>, pack: Option<PackRef> },
    Ejected { n: IssueNumber, reason: EjectReason, refs: Vec<RefUpdate> },
    Landed { base: RefName, through: IssueNumber, refs: Vec<RefUpdate> },
}

impl QueueEvent {
    pub fn refs(&self) -> &[RefUpdate] {
        match self {
            QueueEvent::Enqueued { .. } => &[],
            QueueEvent::GroupBuilt { refs, .. }
            | QueueEvent::Ejected { refs, .. }
            | QueueEvent::Landed { refs, .. } => refs,
        }
    }
}

// ---- the core rows ----

/// The core `queue` row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueRow {
    pub n: IssueNumber,
    /// The head `Enqueued` pinned.
    pub head: ObjectId,
    /// The enqueuing user, whom each `Merged` names.
    pub by: UserId,
    pub enqueued: Timestamp,
    /// The entry and when its `GroupBuilt` committed.
    pub built: Option<(MergeQueueEntry, Timestamp)>,
}

fn base_key(base: &RefName) -> Val {
    ref_key(base)
}

/// Base `base`'s queue in order: (seq, row).
pub fn queue_of(core: &(impl Read + ?Sized), base: &RefName) -> Vec<(i64, QueueRow)> {
    core.prefix("queue", &[base_key(base)])
        .into_iter()
        .filter_map(|(k, _)| match k.get(1) {
            Some(Val::Int(seq)) => Some((*seq, get_rec(core, "queue", &k)?)),
            _ => None,
        })
        .collect()
}

/// Where pull request `n` is queued: its base and seq.
pub fn queued(core: &(impl Read + ?Sized), n: IssueNumber) -> Option<(RefName, i64)> {
    get_rec(core, "queue_pr", &[n_key(n)])
}

/// Every base branch with a queue.
pub fn queue_bases(core: &(impl Read + ?Sized)) -> Vec<RefName> {
    let mut out: Vec<RefName> = Vec::new();
    for (k, _) in core.prefix("queue", &[]) {
        if let Some(Val::Blob(b)) = k.first()
            && let Ok(name) = RefName::parse(b)
            && out.last() != Some(&name)
        {
            out.push(name);
        }
    }
    out
}

/// The merge-queue settings of the first rule over the base that has them.
pub fn queue_settings(rules: &[BranchRule]) -> Option<MergeQueueSettings> {
    rules.iter().find_map(|r| r.protection.as_ref()?.merge_queue.clone())
}

/// Writes a `Queue` event's rows. The caller has applied its `refs`.
pub(crate) fn write(w: &mut dyn Rows, ev: &QueueEvent, at: Timestamp) {
    let put = |w: &mut dyn Rows, base: &RefName, seq: i64, row: &QueueRow| {
        put_rec(w, "queue", vec![base_key(base), int(seq)], row)
    };
    match ev {
        QueueEvent::Enqueued { n, base, head, actor } => {
            let seq = queue_of(w, base).last().map_or(1, |(s, _)| s + 1);
            put(w, base, seq, &QueueRow { n: *n, head: *head, by: *actor, enqueued: at, built: None });
            put_rec(w, "queue_pr", vec![n_key(*n)], &(base.clone(), seq));
        }
        QueueEvent::GroupBuilt { base, entries, .. } => {
            let tip: Option<ObjectId> = get_rec(w, "ref", &[ref_key(base)]);
            if entries.first().map(|e| e.parent_tip) == tip {
                for (seq, mut row) in queue_of(w, base) {
                    if row.built.take().is_some() {
                        put(w, base, seq, &row);
                    }
                }
            }
            for e in entries {
                let Some((_, seq)) = queued(w, e.pr) else { continue };
                if let Some(mut row) = get_rec::<QueueRow>(w, "queue", &[base_key(base), int(seq)]) {
                    row.built = Some((*e, at));
                    put(w, base, seq, &row);
                }
            }
        }
        QueueEvent::Ejected { n, .. } => {
            let Some((base, at_seq)) = queued(w, *n) else { return };
            w.delete("queue", &[base_key(&base), int(at_seq)]);
            w.delete("queue_pr", &[n_key(*n)]);
            for (seq, mut row) in queue_of(w, &base) {
                if seq > at_seq && row.built.take().is_some() {
                    put(w, &base, seq, &row);
                }
            }
        }
        QueueEvent::Landed { base, through, .. } => {
            for (seq, row) in queue_of(w, base) {
                w.delete("queue", &[base_key(base), int(seq)]);
                w.delete("queue_pr", &[n_key(row.n)]);
                if row.n == *through {
                    break;
                }
            }
        }
    }
}

/// The deletes of the queue refs of every built row from `from_seq` on.
fn unbuild_refs(rows: &[(i64, QueueRow)], base: &RefName, from_seq: i64) -> Vec<RefUpdate> {
    rows.iter()
        .filter(|(s, _)| *s >= from_seq)
        .filter_map(|(_, r)| r.built)
        .filter_map(|(e, _)| Some(RefUpdate { name: queue_ref(base, &e)?, expect: Some(e.group_tip), new: None }))
        .collect()
}

// ---- QueueOp ----

#[allow(clippy::large_enum_variant)] // DESIGN's shape; one per command
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QueueAction {
    Enqueue {
        n: IssueNumber,
        head: ObjectId,
    },
    Dequeue {
        n: IssueNumber,
    },
    /// `pack` holds the objects of every `group_tip`.
    Build {
        base: RefName,
        entries: SmallVec<[MergeQueueEntry; 8]>,
        pack: DurablePack,
    },
    Eject {
        n: IssueNumber,
        reason: EjectReason,
    },
}

#[derive(Serialize)]
pub(crate) enum ActionKey<'a> {
    Enqueue(IssueNumber, ObjectId),
    Dequeue(IssueNumber),
    Build(&'a RefName, &'a [MergeQueueEntry], &'a PackRef),
    Eject(IssueNumber, &'a EjectReason),
}

impl QueueAction {
    pub(crate) fn key(&self) -> ActionKey<'_> {
        match self {
            QueueAction::Enqueue { n, head } => ActionKey::Enqueue(*n, *head),
            QueueAction::Dequeue { n } => ActionKey::Dequeue(*n),
            QueueAction::Build { base, entries, pack } => ActionKey::Build(base, entries, pack.pack()),
            QueueAction::Eject { n, reason } => ActionKey::Eject(*n, reason),
        }
    }

    pub(crate) fn subject(&self) -> IssueNumber {
        match self {
            QueueAction::Enqueue { n, .. } | QueueAction::Dequeue { n } | QueueAction::Eject { n, .. } => *n,
            QueueAction::Build { entries, .. } => entries.first().map_or(IssueNumber(0), |e| e.pr),
        }
    }
}

/// A queue command. The edge submits `Enqueue` and `Dequeue` for a user; the `MergeQueue` consumer submits `Build` and
/// `Eject` with no actor, since a consumer's command names no credential (`design/DESIGN.md`, Shape,
/// `CredentialShard`) [inference: DESIGN gives `QueueOp` a `Principal` for every action].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueueOp {
    pub op: QueueAction,
    pub actor: Option<Principal>,
}

impl QueueOp {
    /// Pure. Refuses `Enqueue` unless the pull request is open, at the head given, not queued, and passes every rule
    /// but its required checks; `Build` unless every entry's pull request is the next one in the queue at its pinned
    /// head and the `parent_tip` chain holds from the base tip or the last built entry; `Eject` and `Dequeue` of a pull
    /// request that is not queued.
    pub fn decide(
        &self,
        v: &RepoView<'_>,
        deps: &[(ChainId, IdentityDep)],
    ) -> Result<SmallVec<[RepoEvent; 4]>, MergeError> {
        let core = v.core();
        let user = match (&self.op, &self.actor) {
            (QueueAction::Enqueue { .. } | QueueAction::Dequeue { .. }, Some(a)) => {
                let (user, perm) = authorize(a, v, deps)?;
                if perm < Some(RepoPermission::Write) {
                    return Err(MergeError::Forbidden);
                }
                Some((user, perm))
            }
            (QueueAction::Enqueue { .. } | QueueAction::Dequeue { .. }, None) => {
                return Err(MergeError::Unauthorized(graftyard_identity::CredentialRefusal::NoToken));
            }
            (_, Some(_)) => return Err(MergeError::Forbidden),
            (_, None) => None,
        };
        let ev = match &self.op {
            QueueAction::Enqueue { n, head } => {
                let (user, perm) = user.expect("a user");
                let ph = open_pull(core, *n)?;
                if ph.head_sha != *head {
                    return Err(MergeError::HeadModified);
                }
                if queued(core, *n).is_some() {
                    return Err(MergeError::Queued);
                }
                let rules = rules_over(core, &ph.base);
                let settings = queue_settings(&rules).ok_or(MergeError::NoQueue)?;
                // A base too long for its queue refs cannot have a queue.
                if !queue_refs_fit(&ph.base) {
                    return Err(MergeError::NoQueue);
                }
                crate::merge::gate_holds(core, *n, *head)?;
                let pr = PrView {
                    n: *n,
                    head: *head,
                    method: settings.method,
                    unresolved: crate::tiers::unresolved_threads(core, *n),
                    up_to_date: true,
                    actor: Some(user),
                    admin: perm == Some(RepoPermission::Admin),
                    via: Via::Enqueue,
                };
                protect(core, &rules, &pr, &[]).map_err(MergeError::Protection)?;
                QueueEvent::Enqueued { n: *n, base: ph.base, head: *head, actor: user }
            }
            QueueAction::Dequeue { n } | QueueAction::Eject { n, .. } => {
                let (base, seq) = queued(core, *n).ok_or(MergeError::NotQueued)?;
                let refs = unbuild_refs(&queue_of(core, &base), &base, seq);
                let reason = match &self.op {
                    QueueAction::Eject { reason, .. } => reason.clone(),
                    _ => EjectReason::Dequeued,
                };
                QueueEvent::Ejected { n: *n, reason, refs }
            }
            QueueAction::Build { base, entries, pack } => build(v, base, entries, pack)?,
        };
        Ok(smallvec![RepoEvent::Queue(ev)])
    }
}

/// `Build`'s check and its `GroupBuilt`.
fn build(
    v: &RepoView<'_>,
    base: &RefName,
    entries: &[MergeQueueEntry],
    pack: &DurablePack,
) -> Result<QueueEvent, MergeError> {
    let core = v.core();
    let tip = v.ref_value(base).ok_or(MergeError::BaseModified)?;
    let first = entries.first().ok_or(MergeError::StaleChain)?;
    let rows = queue_of(core, base);
    let built = rows.iter().take_while(|(_, r)| r.built.is_some()).count();
    let replace = first.parent_tip == tip;
    let start = if replace {
        0
    } else {
        let last = built.checked_sub(1).and_then(|i| rows[i].1.built).map(|(e, _)| e.group_tip);
        if last != Some(first.parent_tip) {
            return Err(MergeError::StaleChain);
        }
        built
    };
    for (i, e) in entries.iter().enumerate() {
        if i > 0 && e.parent_tip != entries[i - 1].group_tip {
            return Err(MergeError::StaleChain);
        }
        let (_, row) = rows.get(start + i).filter(|(_, r)| r.n == e.pr).ok_or(MergeError::NotQueued)?;
        if open_pull(core, e.pr)?.head_sha != row.head {
            return Err(MergeError::HeadModified);
        }
    }
    let mut refs: Vec<RefUpdate> = Vec::new();
    let created: Vec<RefName> =
        entries.iter().map(|e| queue_ref(base, e).ok_or(MergeError::StaleChain)).collect::<Result<_, _>>()?;
    if replace {
        refs.extend(unbuild_refs(&rows, base, i64::MIN).into_iter().filter(|u| !created.contains(&u.name)));
    }
    for (e, name) in entries.iter().zip(created) {
        let have = v.ref_value(&name);
        if have != Some(e.group_tip) {
            refs.push(RefUpdate { name, expect: have, new: Some(e.group_tip) });
        }
    }
    Ok(QueueEvent::GroupBuilt {
        base: base.clone(),
        entries: entries.iter().copied().collect(),
        refs,
        pack: Some(pack.pack().clone()),
    })
}

// ---- QueueMerge ----

/// One group's landing, all or nothing. `plans` are built entries in queue order from the queue's head, and `entry`
/// is the last of them. The consumer submits it with no actor, and each `Merged` names the enqueuing user
/// [inference: DESIGN gives `QueueMerge` a `Principal`, which a consumer does not have].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueueMerge {
    pub plans: Vec<(IssueNumber, MergePlan)>,
    pub entry: MergeQueueEntry,
}

impl QueueMerge {
    /// Pure. Checks that the base tip equals the first `parent_tip`, and for each plan that it is the next built entry,
    /// `base_old` is its `parent_tip`, `commit` its `group_tip`, and `head` the pinned head of an open pull request, and
    /// that `check_protection` passes with required checks read on the queue refs: every entry's under `AllGreen`, the
    /// last entry's under `HeadGreen`. Under `HeadGreen` an entry whose own queue ref has a failed or pending conclusion
    /// refuses too, so a group never lands past a failed or re-requested check [inference: DESIGN reads only the last
    /// entry's checks under `HeadGreen`; this closes the race in which the consumer planned before a failure was
    /// reported]. Any
    /// failure refuses the whole command with the failing pull request's number. Otherwise one `Merged` per pull
    /// request, each moving the base from `parent_tip` to `group_tip`, then `Landed`.
    #[allow(clippy::type_complexity)]
    pub fn decide(
        &self,
        v: &RepoView<'_>,
    ) -> Result<(SmallVec<[RepoEvent; 4]>, Vec<(IssueNumber, ObjectId)>), (IssueNumber, MergeError)> {
        let core = v.core();
        let head_n = self.plans.first().map_or(self.entry.pr, |(n, _)| *n);
        let (base, _) = queued(core, head_n).ok_or((head_n, MergeError::NotQueued))?;
        let rows = queue_of(core, &base);
        let rules = rules_over(core, &base);
        let settings = queue_settings(&rules).ok_or((head_n, MergeError::NoQueue))?;
        let mut tip = v.ref_value(&base).ok_or((head_n, MergeError::BaseModified))?;
        let mut landed: Vec<(&QueueRow, MergeQueueEntry, &MergePlan)> = Vec::new();
        for (i, (n, plan)) in self.plans.iter().enumerate() {
            let n = *n;
            let row = rows.get(i).map(|(_, r)| r).filter(|r| r.n == n).ok_or((n, MergeError::NotQueued))?;
            let (e, _) = row.built.ok_or((n, MergeError::NotQueued))?;
            if e.parent_tip != tip {
                return Err((n, if i == 0 { MergeError::BaseModified } else { MergeError::StaleChain }));
            }
            if plan.base_old != e.parent_tip || plan.commit != e.group_tip {
                return Err((n, MergeError::StaleChain));
            }
            let ph = open_pull(core, n).map_err(|err| (n, err))?;
            if plan.head != row.head || ph.head_sha != row.head {
                return Err((n, MergeError::HeadModified));
            }
            tip = e.group_tip;
            landed.push((row, e, plan));
        }
        let last = landed.last().map(|(_, e, _)| *e);
        if last != Some(self.entry) || landed.len() > usize::from(settings.max_merge.max(1)) {
            return Err((self.entry.pr, MergeError::StaleChain));
        }
        let head_checks = required_on(core, &self.entry.group_tip);
        for (row, e, plan) in &landed {
            let own = required_on(core, &e.group_tip);
            let required = match settings.grouping {
                crate::issues::Grouping::AllGreen => &own,
                crate::issues::Grouping::HeadGreen => {
                    if let Some(v) = held(&rules, &own) {
                        return Err((row.n, MergeError::Protection(v)));
                    }
                    &head_checks
                }
            };
            let pr = PrView {
                n: row.n,
                head: row.head,
                method: plan.method,
                unresolved: crate::tiers::unresolved_threads(core, row.n),
                up_to_date: true,
                actor: Some(row.by),
                admin: false,
                via: Via::Queue,
            };
            // Under `HeadGreen` a check read on the last entry's ref is the last entry's failure.
            let culprit = |v: &RuleViolation| match (settings.grouping, v) {
                (
                    crate::issues::Grouping::HeadGreen,
                    RuleViolation::CheckExpected { .. }
                    | RuleViolation::CheckPending { .. }
                    | RuleViolation::CheckFailing { .. },
                ) => self.entry.pr,
                _ => row.n,
            };
            protect(core, &rules, &pr, required).map_err(|err| (culprit(&err), MergeError::Protection(err)))?;
        }
        let mut events: SmallVec<[RepoEvent; 4]> = SmallVec::new();
        let mut out = Vec::new();
        for (row, e, plan) in &landed {
            let ph = crate::tiers::pull_head(core, row.n).expect("an open pull request");
            events.push(RepoEvent::Merged {
                n: row.n,
                base: RefUpdate { name: base.clone(), expect: Some(e.parent_tip), new: Some(e.group_tip) },
                head: row.head,
                method: plan.method,
                also: head_deletion(v, &ph, row.head),
                pack: Some(plan.pack.pack().clone()),
                verified: plan.verified,
                engine: plan.engine.clone(),
                by: row.by,
            });
            out.push((row.n, e.group_tip));
        }
        let refs = landed
            .iter()
            .filter_map(|(_, e, _)| {
                Some(RefUpdate { name: queue_ref(&base, e)?, expect: Some(e.group_tip), new: None })
            })
            .collect();
        events.push(RepoEvent::Queue(QueueEvent::Landed { base, through: self.entry.pr, refs }));
        Ok((events, out))
    }
}

/// A required check of any rule with a failed or pending conclusion in `own`: an entry with none at all does not
/// hold a `HeadGreen` group, but one whose own run failed or is running again does.
fn held(rules: &[BranchRule], own: &[crate::merge::RequiredCheckView]) -> Option<RuleViolation> {
    rules.iter().filter_map(|r| r.protection.as_ref()?.checks.as_ref()).find_map(|c| {
        c.checks.iter().find_map(|(name, app)| match check_state(name, *app, own) {
            Err(v @ (RuleViolation::CheckFailing { .. } | RuleViolation::CheckPending { .. })) => Some(v),
            _ => None,
        })
    })
}
