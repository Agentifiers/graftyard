//! Merges (`design/SHARED.md`, "Issues and pull requests", same-repository and fork merge; `build/BUILD-PLAN.md`,
//! M2-WP3): `MergePlan`, `check_protection`, `Merge`, and `MergeError`, and `MergeCmd`, the raft command that carries
//! a merge or a merge-queue command (`crate::queue`) to the repository chain's leader.
//!
//! `decide` is pure. The merge commit and its pack come from the git cache as the command's `MergePlan`, the way a
//! test merge reaches the chain as an input, and `decide` re-checks the plan on the core tier: the pull request is
//! open at the plan's head, the base is at the plan's `base_old`, and every rule over the base passes. Only Core is
//! read, so a merge never waits for a metadata load.

use graftyard_fold::{MemStore, Read};
use graftyard_identity::{
    Actor, AuthzRead, Credential, CredentialRefusal, IdentityDep, Own, Principal, RepoPermission, UserId,
    check_credential, credential_chains,
};
use graftyard_raft::{Command, Format, Record, TierSet};
use graftyard_types::{AccountId, ChainId, Digest, Index, Timestamp};
use serde::{Deserialize, Serialize};
use smallvec::{SmallVec, smallvec};

use crate::bucket::StoreError;
use crate::issue_ops::{RepoView, branch_matches, permission};
use crate::issues::{
    AppId, BranchRule, CheckConclusion, CheckSource, IssueNumber, MergeMethod, PrState, Protection, PullHead, Pusher,
    ReviewState,
};
use crate::pack::DurablePack;
use crate::queue::{QueueMerge, QueueOp};
use crate::refs::{ObjectId, RefName, RefUpdate};
use crate::repo::{RepoEvent, RepoFold, RepoScratch};
use crate::tiers::{RequiredRow, VerdictRow, get_rec, n_key, pull_head, sha_key, unresolved_threads};

/// The repository chain's records that carry `Merge`, `QueueOp`, and `QueueMerge` and the `Merged` and `Queue`
/// events. A member reads it only once a release enables it (`design/SHARED.md`, "Record formats and the two-release
/// rule"). M2-WP5 took version 3 for `PrSync`.
pub const REPO_FORMAT_V4: Format = Format { family: 0x20, version: 4 };

/// The digest of the tree diff the git cache checked: the result is the base plus the pull request's own diff
/// (`research/uptime.md#49`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreeDiffDigest(pub Digest);

/// The git version that ran the merge, which each `Merged` records.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineVersion(pub Box<str>);

/// What the planner asks the git cache to merge (`GitCacheShard::merge`), read from the fresh core tier.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MergeRequest {
    pub n: IssueNumber,
    pub base: RefName,
    pub base_old: ObjectId,
    pub head: ObjectId,
    pub method: MergeMethod,
}

/// Produced by the git cache. The tree-diff check has already passed when this exists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MergePlan {
    pub base_old: ObjectId,
    pub head: ObjectId,
    pub commit: ObjectId,
    pub pack: DurablePack,
    pub method: MergeMethod,
    /// Whether `head` contains `base_old`, which a `strict` rule requires [inference: SHARED's plan names no field
    /// for it; the fold cannot see ancestry, and the git cache that runs `merge-tree` can].
    pub up_to_date: bool,
    pub verified: TreeDiffDigest,
    pub engine: EngineVersion,
}

/// SHARED's `Merge`. `expected_head` is GitHub's `sha` parameter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Merge {
    pub n: IssueNumber,
    pub plan: MergePlan,
    pub expected_head: Option<ObjectId>,
    pub actor: Principal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeOut {
    pub merge_commit: ObjectId,
}

/// Why a rule refuses a merge. Stored inside `EjectReason`, so add variants at the end only.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RuleViolation {
    CheckExpected {
        name: Box<str>,
    },
    CheckPending {
        name: Box<str>,
    },
    CheckFailing {
        name: Box<str>,
    },
    ReviewsRequired {
        count: u8,
    },
    ChangesRequested,
    /// Code owners need the `CODEOWNERS` file, which the core tier does not hold, so the rule refuses every merge
    /// until a release reads it.
    CodeOwnerReview,
    /// The last pusher is not in the core tier, so the rule refuses every merge until a release records it.
    LastPushApproval,
    Unresolved {
        count: u32,
    },
    LinearHistory,
    NotUpToDate,
    /// Signature verification is not built, so the rule refuses every merge.
    Signatures,
    Locked,
    PushRestricted,
    MergeQueueRequired,
}

impl RuleViolation {
    /// GitHub's text for the violation [unverified wording for GHES 3.22].
    pub fn message(&self) -> String {
        match self {
            RuleViolation::CheckExpected { name } => format!("Required status check \"{name}\" is expected."),
            RuleViolation::CheckPending { name } => format!("Required status check \"{name}\" is in progress."),
            RuleViolation::CheckFailing { name } => format!("Required status check \"{name}\" is failing."),
            RuleViolation::ReviewsRequired { count: 1 } => {
                "At least 1 approving review is required by reviewers with write access.".into()
            }
            RuleViolation::ReviewsRequired { count } => {
                format!("At least {count} approving reviews are required by reviewers with write access.")
            }
            RuleViolation::ChangesRequested => "Changes have been requested.".into(),
            RuleViolation::CodeOwnerReview => "Waiting on code owner review.".into(),
            RuleViolation::LastPushApproval => "Approval of the most recent reviewable push is required.".into(),
            RuleViolation::Unresolved { .. } => {
                "A conversation must be resolved before this pull request can be merged.".into()
            }
            RuleViolation::LinearHistory => "This branch must not contain merge commits.".into(),
            RuleViolation::NotUpToDate => "Head branch is out of date with the base branch.".into(),
            RuleViolation::Signatures => "Commits must have verified signatures.".into(),
            RuleViolation::Locked => "This branch is locked.".into(),
            RuleViolation::PushRestricted => "You're not authorized to push to this branch.".into(),
            RuleViolation::MergeQueueRequired => "Changes must be made through the merge queue.".into(),
        }
    }
}

/// SHARED's `MergeError`, with the refusals every user command shares and the queue's own. Every variant but `Store`
/// renders GitHub's text and is never retried into success.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MergeError {
    HeadModified,
    BaseModified,
    Protection(RuleViolation),
    NotOpen,
    Store(StoreError),
    Unauthorized(CredentialRefusal),
    Forbidden,
    NotFound,
    Draft,
    MethodNotAllowed,
    /// The pull request is not in the queue, or not where the command places it.
    NotQueued,
    /// Already in the queue.
    Queued,
    /// No rule over the base has a merge queue.
    NoQueue,
    /// A queue entry's `parent_tip` is not the previous entry's `group_tip`, or a plan does not match its entry.
    StaleChain,
}

impl MergeError {
    pub fn message(&self) -> String {
        match self {
            MergeError::HeadModified => "Head branch was modified. Review and try the merge again.".into(),
            MergeError::BaseModified => "Base branch was modified. Review and try the merge again.".into(),
            MergeError::Protection(v) => v.message(),
            MergeError::NotOpen => "Pull request is not open".into(),
            MergeError::Store(e) => e.to_string(),
            MergeError::Unauthorized(_) => "Bad credentials".into(),
            MergeError::Forbidden => "Resource not accessible by integration".into(),
            MergeError::NotFound => "Could not resolve to a node with that global id.".into(),
            MergeError::Draft => "Pull request is in draft state".into(),
            MergeError::MethodNotAllowed => "Merge method is not allowed on this repository".into(),
            MergeError::NotQueued => "Pull request is not in the merge queue".into(),
            MergeError::Queued => "Pull request is already in the merge queue".into(),
            MergeError::NoQueue => "Merge queue is not enabled for this branch".into(),
            MergeError::StaleChain => "Merge queue entry is out of date".into(),
        }
    }
}

/// One required conclusion of the SHA a merge checks: a core `required_check` row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequiredCheckView {
    pub name: Box<str>,
    pub source: CheckSource,
    pub conclusion: Option<CheckConclusion>,
}

/// One current review verdict: a core `review_verdict` row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReviewView {
    pub reviewer: Option<AccountId>,
    pub state: ReviewState,
    pub commit: ObjectId,
}

/// How the merge reaches the base.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Via {
    /// `Merge`: an admin may bypass a rule that does not enforce admins.
    Direct,
    /// `Enqueue`: every rule but the required checks, which run on the queue ref.
    Enqueue,
    /// `QueueMerge`: the group was built on the base tip, so a `strict` rule holds.
    Queue,
}

/// What `check_protection` reads of one pull request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrView {
    pub n: IssueNumber,
    /// The pull request's head, which reviews are on.
    pub head: ObjectId,
    pub method: MergeMethod,
    pub unresolved: u32,
    pub up_to_date: bool,
    /// Who merges: the actor of `Merge`, or the enqueuing user.
    pub actor: Option<UserId>,
    pub admin: bool,
    pub via: Via,
}

/// One required check's state on a SHA, over every source the rule accepts: a failed conclusion first, then a
/// queued or running one, then none at all.
pub fn check_state(name: &str, app: Option<AppId>, required: &[RequiredCheckView]) -> Result<(), RuleViolation> {
    let rows: Vec<&RequiredCheckView> = required
        .iter()
        .filter(|r| &*r.name == name)
        .filter(|r| match (app, r.source) {
            (None, _) => true,
            (Some(a), CheckSource::Run { app, .. }) => a == app,
            (Some(_), CheckSource::Status { .. }) => false,
        })
        .collect();
    if rows.iter().any(|r| r.conclusion.is_some_and(|c| !c.passes())) {
        return Err(RuleViolation::CheckFailing { name: name.into() });
    }
    if rows.iter().any(|r| r.conclusion.is_none()) {
        return Err(RuleViolation::CheckPending { name: name.into() });
    }
    if rows.is_empty() {
        return Err(RuleViolation::CheckExpected { name: name.into() });
    }
    Ok(())
}

/// Every required check of `p` against `required`: the first that does not pass.
pub fn checks_pass(p: &Protection, required: &[RequiredCheckView]) -> Result<(), RuleViolation> {
    let Some(c) = &p.checks else { return Ok(()) };
    c.checks.iter().try_for_each(|(name, app)| check_state(name, *app, required))
}

/// Pure. Reads the rule, `RequiredCheck`, and review-verdict rows from the core tier. `force_pushes`, `deletions`,
/// and `block_creations` govern pushes, not merges.
pub fn check_protection(
    rule: &BranchRule,
    pr: &PrView,
    required: &[RequiredCheckView],
    reviews: &[ReviewView],
) -> Result<(), RuleViolation> {
    let Some(p) = &rule.protection else { return Ok(()) };
    let direct = pr.via == Via::Direct;
    // An admin bypasses a rule that does not include administrators, as `gh pr merge --admin` does [inference:
    // gh's flag only skips its own check, so GitHub's mutation lets an admin through].
    if direct && pr.admin && !p.enforce_admins {
        return Ok(());
    }
    if p.lock_branch {
        return Err(RuleViolation::Locked);
    }
    if direct && p.merge_queue.is_some() {
        return Err(RuleViolation::MergeQueueRequired);
    }
    // A `Team` pusher matches nobody until teams are built.
    if direct
        && let Some(push) = &p.push
        && !pr.actor.is_some_and(|u| push.contains(&Pusher::User(u)))
    {
        return Err(RuleViolation::PushRestricted);
    }
    if p.linear_history && pr.method == MergeMethod::Merge {
        return Err(RuleViolation::LinearHistory);
    }
    if p.signatures {
        return Err(RuleViolation::Signatures);
    }
    if let Some(r) = &p.reviews
        && !pr.actor.is_some_and(|u| r.bypass.contains(&Pusher::User(u)))
    {
        if r.code_owners {
            return Err(RuleViolation::CodeOwnerReview);
        }
        if r.last_push {
            return Err(RuleViolation::LastPushApproval);
        }
        if reviews.iter().any(|v| v.state == ReviewState::ChangesRequested) {
            return Err(RuleViolation::ChangesRequested);
        }
        let approved = reviews
            .iter()
            .filter(|v| v.state == ReviewState::Approved && (!r.dismiss_stale || v.commit == pr.head))
            .count();
        if approved < usize::from(r.count) {
            return Err(RuleViolation::ReviewsRequired { count: r.count });
        }
    }
    if p.conversation_resolution && pr.unresolved > 0 {
        return Err(RuleViolation::Unresolved { count: pr.unresolved });
    }
    if let Some(c) = &p.checks
        && direct
        && c.strict
        && !pr.up_to_date
    {
        return Err(RuleViolation::NotUpToDate);
    }
    if pr.via != Via::Enqueue {
        checks_pass(p, required)?;
    }
    Ok(())
}

// ---- core reads ----

/// Every rule over `base` that protects it, in id order.
pub fn rules_over(core: &dyn Read, base: &RefName) -> Vec<BranchRule> {
    core.prefix("rule", &[])
        .into_iter()
        .filter_map(|(k, _)| get_rec::<BranchRule>(core, "rule", &k))
        .filter(|r| r.protection.is_some() && branch_matches(&r.pattern, base))
        .collect()
}

/// The required conclusions of `sha`.
pub fn required_on(core: &dyn Read, sha: &ObjectId) -> Vec<RequiredCheckView> {
    core.prefix("required_check", &[sha_key(sha)])
        .into_iter()
        .filter_map(|(k, _)| {
            let graftyard_fold::Val::Text(name) = k.get(1)? else { return None };
            let row: RequiredRow = get_rec(core, "required_check", &k)?;
            Some(RequiredCheckView { name: name.clone(), source: row.source, conclusion: row.conclusion })
        })
        .collect()
}

/// The current verdicts on pull request `n`.
pub fn reviews_of(core: &dyn Read, n: IssueNumber) -> Vec<ReviewView> {
    core.prefix("review_verdict", &[n_key(n)])
        .into_iter()
        .filter_map(|(k, _)| {
            let graftyard_fold::Val::Int(who) = k.get(1)? else { return None };
            let row: VerdictRow = get_rec(core, "review_verdict", &k)?;
            let reviewer = (*who != 0).then_some(AccountId(graftyard_types::DbId(*who)));
            Some(ReviewView { reviewer, state: row.state, commit: row.commit })
        })
        .collect()
}

/// Every rule over the base, each against `pr`.
pub fn protect(
    core: &dyn Read,
    rules: &[BranchRule],
    pr: &PrView,
    required: &[RequiredCheckView],
) -> Result<(), RuleViolation> {
    let reviews = reviews_of(core, pr.n);
    rules.iter().try_for_each(|r| check_protection(r, pr, required, &reviews))
}

/// An open, non-draft pull request.
pub(crate) fn open_pull(core: &dyn Read, n: IssueNumber) -> Result<PullHead, MergeError> {
    let ph = pull_head(core, n).ok_or(MergeError::NotFound)?;
    match ph.state {
        PrState::Open { draft: false } => Ok(ph),
        PrState::Open { draft: true } => Err(MergeError::Draft),
        _ => Err(MergeError::NotOpen),
    }
}

/// What to ask the git cache for: the pull request's base, the base's tip, and the head, read from the fresh core
/// tier. The edge calls this, then `GitCacheShard::merge`, then commits `Merge`.
pub fn merge_request(core: &dyn Read, n: IssueNumber, method: MergeMethod) -> Result<MergeRequest, MergeError> {
    let ph = open_pull(core, n)?;
    let base_old = get_rec(core, "ref", &[crate::tiers::ref_key(&ph.base)]).ok_or(MergeError::BaseModified)?;
    Ok(MergeRequest { n, base: ph.base, base_old, head: ph.head_sha, method })
}

/// The actor's user and permission, after the credential check every user command runs.
pub(crate) fn authorize(
    actor: &Principal,
    v: &RepoView<'_>,
    deps: &[(ChainId, IdentityDep)],
) -> Result<(UserId, Option<RepoPermission>), MergeError> {
    let cred = actor.credential_shard();
    if cred.account.is_none() || cred.key.is_none() {
        return Err(MergeError::Unauthorized(CredentialRefusal::NoToken));
    }
    check_credential(&cred, Own::Other, deps).map_err(MergeError::Unauthorized)?;
    let Actor::User(user) = actor.actor else { return Err(MergeError::Unauthorized(CredentialRefusal::NoToken)) };
    let owner = v.owner();
    let authz = deps.iter().find_map(|(c, d)| match d {
        IdentityDep::Authz(f) if *c == ChainId::Authz(owner) => Some(f as &dyn AuthzRead),
        _ => None,
    });
    Ok((user, permission(actor, v, authz)))
}

/// A user command's deps: the actor's credential chains and the owner's `Authz`.
pub(crate) fn user_deps(actor: &Principal, owner: Option<AccountId>) -> SmallVec<[ChainId; 3]> {
    let mut d = credential_chains(&actor.credential_shard(), None);
    if let Some(o) = owner {
        d.push(ChainId::Authz(o));
    }
    d
}

/// The head branch's deletion after a merge, when the settings ask for it and the branch is this repository's, still
/// at the merged head, and under no rule.
pub(crate) fn head_deletion(v: &RepoView<'_>, ph: &PullHead, head: ObjectId) -> Vec<RefUpdate> {
    let on = v.settings().is_some_and(|s| s.delete_branch_on_merge);
    let ours = ph.head.repo == v.repo();
    let at_head = v.ref_value(&ph.head.branch) == Some(head);
    if on && ours && at_head && rules_over(v.core(), &ph.head.branch).is_empty() {
        vec![RefUpdate { name: ph.head.branch.clone(), expect: Some(head), new: None }]
    } else {
        Vec::new()
    }
}

impl Merge {
    pub fn deps(&self, owner: Option<AccountId>) -> SmallVec<[ChainId; 3]> {
        user_deps(&self.actor, owner)
    }

    /// Pure: the core tier with the entry's earlier events, and the deps the leader read once per entry. Re-checks
    /// the plan's head and base and every rule over the base, so a 409 is never retried into success.
    pub fn decide(
        &self,
        v: &RepoView<'_>,
        deps: &[(ChainId, IdentityDep)],
    ) -> Result<(SmallVec<[RepoEvent; 4]>, MergeOut), MergeError> {
        let (user, perm) = authorize(&self.actor, v, deps)?;
        if perm < Some(RepoPermission::Write) {
            return Err(MergeError::Forbidden);
        }
        let core = v.core();
        let ph = open_pull(core, self.n)?;
        let head = ph.head_sha;
        if self.plan.head != head || self.expected_head.is_some_and(|h| h != head) {
            return Err(MergeError::HeadModified);
        }
        if v.ref_value(&ph.base) != Some(self.plan.base_old) {
            return Err(MergeError::BaseModified);
        }
        if v.settings().is_some_and(|s| !s.merge_methods.contains(&self.plan.method)) {
            return Err(MergeError::MethodNotAllowed);
        }
        let pr = PrView {
            n: self.n,
            head,
            method: self.plan.method,
            unresolved: unresolved_threads(core, self.n),
            up_to_date: self.plan.up_to_date,
            actor: Some(user),
            admin: perm == Some(RepoPermission::Admin),
            via: Via::Direct,
        };
        let rules = rules_over(core, &ph.base);
        protect(core, &rules, &pr, &required_on(core, &head)).map_err(MergeError::Protection)?;
        let ev = RepoEvent::Merged {
            n: self.n,
            base: RefUpdate { name: ph.base.clone(), expect: Some(self.plan.base_old), new: Some(self.plan.commit) },
            head,
            method: self.plan.method,
            also: head_deletion(v, &ph, head),
            pack: Some(self.plan.pack.pack().clone()),
            verified: self.plan.verified,
            engine: self.plan.engine.clone(),
            by: user,
        };
        Ok((smallvec![ev], MergeOut { merge_commit: self.plan.commit }))
    }
}

// ---- the raft command ----

/// The commands of this module and `crate::queue` as one `graftyard_raft::Command` over the repository fold. A
/// member that hosts several command kinds wraps this in its own enum and delegates.
#[allow(clippy::large_enum_variant)] // one per command, moved once into the leader's queue
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MergeCmd {
    Merge(Merge),
    Queue(QueueOp),
    Land(QueueMerge),
    /// The `MergeQueue` consumer's cursor, after the batch it covers was handled: `ConsumerCursor` only forward.
    Cursor(Index),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MergeCmdOut {
    Merged(MergeOut),
    Queue,
    /// Each landed pull request's merge commit.
    Landed(Vec<(IssueNumber, ObjectId)>),
    Cursor(Index),
}

/// The canonical form of a principal, for `Record::encode`.
fn principal_key(p: &Principal) -> (Option<i64>, u64, [u8; 32], Vec<i64>) {
    let actor = match p.actor {
        Actor::User(u) => Some(u.0.0.0),
        Actor::Anonymous => None,
    };
    let Credential::Stored(h) = p.credential;
    (actor, p.scopes.0, h, p.sso_orgs.iter().map(|o| o.0.0.0).collect())
}

type PlanKey<'a> = (
    &'a ObjectId,
    &'a ObjectId,
    &'a ObjectId,
    &'a crate::pack::PackRef,
    MergeMethod,
    bool,
    &'a TreeDiffDigest,
    &'a str,
);

pub(crate) fn plan_key(p: &MergePlan) -> PlanKey<'_> {
    (&p.base_old, &p.head, &p.commit, p.pack.pack(), p.method, p.up_to_date, &p.verified, &p.engine.0)
}

impl Record for MergeCmd {
    fn encode(&self, out: &mut Vec<u8>) {
        use graftyard_fold::canon::write;
        match self {
            MergeCmd::Merge(m) => {
                write(out, &(0u8, m.n, plan_key(&m.plan), m.expected_head, principal_key(&m.actor)));
            }
            MergeCmd::Queue(q) => write(out, &(1u8, q.op.key(), q.actor.as_ref().map(principal_key))),
            MergeCmd::Land(l) => {
                let plans: Vec<_> = l.plans.iter().map(|(n, p)| (n, plan_key(p))).collect();
                write(out, &(2u8, plans, &l.entry));
            }
            MergeCmd::Cursor(through) => write(out, &(3u8, through)),
        }
    }
}

/// What `MergeCmd::decide` gives: the events and output, or the pull request it refuses and why.
pub type MergeDecision = Result<(SmallVec<[RepoEvent; 4]>, MergeCmdOut), (IssueNumber, MergeError)>;

impl MergeCmd {
    /// The pull request a refusal names.
    fn subject(&self) -> IssueNumber {
        match self {
            MergeCmd::Merge(m) => m.n,
            MergeCmd::Queue(q) => q.op.subject(),
            MergeCmd::Land(l) => l.entry.pr,
            MergeCmd::Cursor(_) => IssueNumber(0),
        }
    }

    /// Decides on a view of the core tier alone: these commands never read Metadata.
    pub fn decide_on(&self, v: &RepoView<'_>, deps: &[(ChainId, IdentityDep)]) -> MergeDecision {
        let n = self.subject();
        match self {
            MergeCmd::Merge(m) => m.decide(v, deps).map(|(e, o)| (e, MergeCmdOut::Merged(o))).map_err(|e| (n, e)),
            MergeCmd::Queue(q) => q.decide(v, deps).map(|e| (e, MergeCmdOut::Queue)).map_err(|e| (n, e)),
            MergeCmd::Land(l) => l.decide(v).map(|(e, o)| (e, MergeCmdOut::Landed(o))),
            MergeCmd::Cursor(through) => {
                let at = crate::tiers::consumer_cursor(v.core(), crate::issues::Consumer::MergeQueue);
                if *through <= at {
                    return Ok((SmallVec::new(), MergeCmdOut::Cursor(at)));
                }
                let ev = RepoEvent::ConsumerCursor { consumer: crate::issues::Consumer::MergeQueue, through: *through };
                Ok((smallvec![ev], MergeCmdOut::Cursor(*through)))
            }
        }
    }
}

impl Command for MergeCmd {
    type Fold = RepoFold;
    type Dep = IdentityDep;
    type Out = MergeCmdOut;
    type Err = (IssueNumber, MergeError);

    fn format(&self) -> Format {
        REPO_FORMAT_V4
    }

    /// `Merged` touches both tiers (`crate::tiers::tier_of`), so a merge's entry is tagged with both, though `decide`
    /// reads only Core.
    fn tiers(&self) -> TierSet {
        match self {
            MergeCmd::Queue(_) | MergeCmd::Cursor(_) => TierSet::CORE,
            MergeCmd::Merge(_) | MergeCmd::Land(_) => TierSet::CORE.union(TierSet::METADATA),
        }
    }

    /// A user's credential chains and the owner's `Authz`; a consumer's command names neither.
    fn deps(&self, fold: &RepoFold) -> SmallVec<[ChainId; 3]> {
        match self {
            MergeCmd::Merge(m) => m.deps(fold.authz_owner()),
            MergeCmd::Queue(QueueOp { actor: Some(a), .. }) => user_deps(a, fold.authz_owner()),
            MergeCmd::Queue(_) | MergeCmd::Land(_) | MergeCmd::Cursor(_) => SmallVec::new(),
        }
    }

    fn decide(&self, s: &RepoScratch<'_>, deps: &[(ChainId, IdentityDep)]) -> MergeDecision {
        let meta = MemStore::default();
        let v = RepoView::new(s.fold().tiers(), s.core(), &meta, Timestamp(0));
        self.decide_on(&v, deps)
    }

    fn replay(&self, events: &[RepoEvent]) -> MergeCmdOut {
        let merged = || {
            events.iter().filter_map(|e| match e {
                RepoEvent::Merged { n, base, .. } => Some((*n, base.new.expect("a merge moves its base"))),
                _ => None,
            })
        };
        match self {
            MergeCmd::Merge(m) => {
                MergeCmdOut::Merged(MergeOut { merge_commit: merged().next().map_or(m.plan.commit, |(_, c)| c) })
            }
            MergeCmd::Queue(_) => MergeCmdOut::Queue,
            MergeCmd::Land(_) => MergeCmdOut::Landed(merged().collect()),
            MergeCmd::Cursor(t) => MergeCmdOut::Cursor(
                events
                    .iter()
                    .find_map(|e| match e {
                        RepoEvent::ConsumerCursor { through, .. } => Some(*through),
                        _ => None,
                    })
                    .unwrap_or(*t),
            ),
        }
    }
}
