//! The repository chain's two fold tiers (`design/SHARED.md`, "Folds"; `build/BUILD-PLAN.md`, M2-WP1 and M2-WP2):
//! `tier_of`, each tier's tables, and what each event writes.
//!
//! Core holds what git, merges, and authorization read: `ref`, `rule`, `settings`, `counter` (the issue-number
//! counter), `pull_head` and its two indexes `pull_by_head` and `open_head`, `review_verdict` and its index
//! `verdict_by_review`, `required_check`, and the count of unresolved review threads per pull request (`unresolved`,
//! with `review_thread` mapping each thread to its pull request), which a rule requiring conversation resolution
//! reads. Metadata holds `issue` with its indexes `issue_order` (kind, open, newest first) and `issue_by_id`,
//! `issue_label`, `issue_assignee`, `label`, `pull`, `review` and its index by pull
//! request `review_by_pull`, `review_request`, `comment` and its index by issue `comment_by_issue`, `check_run` and
//! `check_suite` keyed by head SHA first, `status` keyed by SHA and newest first, `check_expiry` (the pruning index),
//! and `retention` (`RepoSettings.check_retention_days`). Each tier also keeps `ids`, its own `DbId` cursor
//! (`design/DESIGN.md`, Shape, "Chains": "Each fold tier keeps its own `IdBlock` cursor"), over its own half of every
//! block (`tier_block`), so the two never assign the same id, before a `MetaSplit` or after it, when the parent's core
//! and the child's metadata tier assign alone. An event that touches Core takes its id from Core, because after a split
//! its entry commits on the parent; an event that touches only Metadata takes it from Metadata. A row's record is one
//! postcard blob; its key columns are real columns a host indexes.
//!
//! Required checks are pruned in Core when their pull request closes or its head moves, never by time. Check runs,
//! suites, and statuses are pruned in Metadata once an entry's `at` passes the row's time (the `at` of the entry that
//! last wrote it) plus the retention, so every member prunes the same rows.

use graftyard_fold::{Read, Rows, Schema, TableDef, Tiered, Val, has};
use graftyard_identity::IdCursor;
use graftyard_raft::{Format, TierSet};
use graftyard_types::{AccountId, DbId, IdBlock, Index, NetworkId, RepoId, Tier, Timestamp};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::blob::BlobRef;
use crate::issues::{AssetRecord, LfsLockEvent, MilestoneEvent, ReleaseRecord, RepoReleaseId};
use crate::issues::{
    CHECK_RETENTION_DEFAULT, CheckConclusion, CheckRunUpdate, CheckSource, CheckSuiteUpdate, CommentEvent,
    CommentTarget, Consumer, Grantee, HeadSpec, IssueChange, IssueKind, IssueNumber, LabelEvent, LabelName, LockReason,
    Mergeability, MergeabilityResult, PrState, PullHead, ReviewEvent, ReviewState, StateReason, StatusUpdate,
};
use graftyard_identity::{InvitationEvent, InvitationRow};

use crate::refs::{ObjectId, RefName};
use crate::repo::{REPO_FORMAT_ROLLUP, RepoEvent};

const DAY_MS: u64 = 86_400_000;

/// The tiers an event touches. Every event the design lists for a tier is here; an `IssueChanged` that changes a
/// state or draft flag touches Core too whether or not `n` is a pull request, because the event alone cannot say.
pub fn tier_of(e: &RepoEvent) -> TierSet {
    let both = TierSet::CORE.union(TierSet::METADATA);
    match e {
        RepoEvent::Refs { .. }
        | RepoEvent::Refused { .. }
        | RepoEvent::Promoted(_)
        | RepoEvent::Archived(_)
        | RepoEvent::Compacted(_) => TierSet::CORE,
        RepoEvent::RequiredCheck { .. } => TierSet::CORE,
        RepoEvent::Rule(_) => TierSet::CORE,
        RepoEvent::IssueOpened { .. } | RepoEvent::PrOpened { .. } | RepoEvent::PrClosed { .. } => both,
        RepoEvent::Settings(_) => both,
        RepoEvent::IssueChanged { change: IssueChange::State { .. } | IssueChange::Draft(_), .. } => both,
        RepoEvent::IssueChanged { .. } => TierSet::METADATA,
        RepoEvent::Review(
            ReviewEvent::Submitted { .. } | ReviewEvent::Dismissed { .. } | ReviewEvent::SingleComment { .. },
        ) => both,
        RepoEvent::Review(ReviewEvent::Requested { .. }) => TierSet::METADATA,
        // The core count of unresolved review threads per pull request (`review_thread` and `unresolved`). SHARED's
        // `tier_of` says "Comment only Metadata, except a `Resolved`, which touches both", but `Resolved` names only
        // the thread, so Core must also see the comment that starts a thread (it is unresolved from then, and its row
        // maps the thread to its pull request) and every deletion (the event does not say whether it named a thread).
        RepoEvent::Comment(
            CommentEvent::Created { on: CommentTarget::Review { .. }, reply_to: None, .. }
            | CommentEvent::Resolved { .. }
            | CommentEvent::Deleted { .. },
        ) => both,
        RepoEvent::Comment(_) | RepoEvent::Labels(_) => TierSet::METADATA,
        RepoEvent::CheckRun(_) | RepoEvent::CheckSuite(_) | RepoEvent::Status(_) => TierSet::METADATA,
        RepoEvent::Release { tag: Some(_), .. } => both,
        RepoEvent::Release { tag: None, .. } => TierSet::METADATA,
        RepoEvent::ReleaseAsset { .. } | RepoEvent::LfsObject { .. } | RepoEvent::LfsLock(_) => TierSet::METADATA,
        RepoEvent::TestMerged { .. } | RepoEvent::ConsumerCursor { .. } => TierSet::CORE,
        RepoEvent::Merged { .. } => both,
        RepoEvent::Queue(_) => TierSet::CORE,
        RepoEvent::Collaborator { .. } | RepoEvent::Invitation(_) => TierSet::CORE,
        RepoEvent::Provenance { .. } => TierSet::METADATA,
        RepoEvent::AgentMeta { .. } | RepoEvent::AgentGate(_) | RepoEvent::AgentPolicy(_) => TierSet::CORE,
        RepoEvent::TaskCreated(_) => both,
        RepoEvent::TaskUpdated { pr: None, summary: None, .. } => TierSet::CORE,
        RepoEvent::TaskUpdated { .. } => both,
        RepoEvent::TaskResumed { .. } => both,
        RepoEvent::Created { .. } | RepoEvent::Deleted { .. } => TierSet::CORE,
        RepoEvent::LeaseAcquire { .. } | RepoEvent::LeaseRenew { .. } | RepoEvent::LeaseRelease { .. } => TierSet::CORE,
        RepoEvent::PathSet(_) => TierSet::METADATA,
        RepoEvent::Fanout { .. } | RepoEvent::Genesis { .. } => TierSet::CORE,
        RepoEvent::Hook { .. } | RepoEvent::HookCursor(_) => TierSet::CORE,
        RepoEvent::Watchers(_) => TierSet::METADATA,
        RepoEvent::Split(x) => crate::split::tiers(x),
        RepoEvent::ActionsSecret { .. } | RepoEvent::ActionsVariable { .. } => TierSet::CORE,
        RepoEvent::WorkflowJob(_) | RepoEvent::Assigned { .. } | RepoEvent::Milestones(_) => TierSet::METADATA,
    }
}

const fn t(name: &'static str, key: &'static [&'static str]) -> TableDef {
    TableDef { name, key }
}

pub const CORE: Schema = Schema {
    tier: Tier::Core,
    tables: &[
        t("counter", &[]),
        t("ids", &[]),
        t("open_head", &["sha", "n"]),
        t("pull_by_head", &["repo", "branch", "n"]),
        t("pull_head", &["n"]),
        t("ref", &["name"]),
        t("required_check", &["sha", "name", "source"]),
        t("review_thread", &["thread"]),
        t("review_verdict", &["n", "reviewer"]),
        t("rule", &["id"]),
        // M3-WP3: outside collaborators by account, and pending repository invitations by id.
        t("collaborator", &["who"]),
        t("invitation", &["id"]),
        t("settings", &[]),
        t("unresolved", &["n"]),
        t("verdict_by_review", &["review"]),
        // The last `Promoted` and `Archived` of a log range, and each tier's committed snapshots by (tier, to), which
        // `verify` and the snapshot writer read.
        t("promoted", &[]),
        t("archived", &[]),
        t("snapshot", &["tier", "to"]),
        // Each pull request's last test merge (`Mergeability`, keyed by the SHA pair it holds), and each consumer's
        // cursor on this chain.
        t("mergeability", &["n"]),
        t("consumer_cursor", &["consumer"]),
        // Each base branch's merge queue in order, and each queued pull request's place in it (`crate::queue`).
        t("queue", &["base", "seq"]),
        t("queue_pr", &["n"]),
        // AM1-WP2: a pull request's `AgentMeta`, each qualifying approval by (pull request, head, approver), and the
        // repository's `AgentPolicy`. Written only by agent events, so a repository without agents has no row.
        t("agent_meta", &["n"]),
        t("agent_gate", &["n", "head", "user"]),
        t("agent_policy", &[]),
        // AM1-WP3: each task's core part by its id (`crate::task::TaskCore`), and the ids of tasks whose stored state
        // is not final, which the open-task caps scan.
        t("agent_task", &["session"]),
        t("agent_task_open", &["session"]),
        // AM1-WP4: each lease by id (`crate::lease::LeaseRow`), the next lease's sequence number, and the indexes the
        // caps, the task API, expiry, plan blocks, and the `AgentsExclusive` check read: by holder, App, session,
        // expiry time, plan pull request, and each Exclusive glob's literal prefix.
        t("lease", &["id"]),
        t("lease_seq", &[]),
        t("lease_by_holder", &["holder", "id"]),
        t("lease_by_app", &["app", "id"]),
        t("lease_by_session", &["session", "id"]),
        t("lease_expiry", &["expires", "id"]),
        t("lease_plan", &["n", "holder", "id"]),
        t("lease_exclusive", &["prefix", "id"]),
        // Format 32.13: when the repository was created over the API, and its `Deleted` tombstone. After AM1-WP4's lease
        // tables, so the schema keeps main's order and appends.
        t("created", &[]),
        t("deleted", &[]),
        // Format 32.15: the network's committed k (`RepoEvent::Fanout`).
        t("fanout", &[]),
        // M4-WP1, 32.16: repository webhooks by id, and the `Webhooks` consumer's `HookCursor`.
        t("hook", &["id"]),
        t("hook_cursor", &[]),
        // Format 32.20: a metadata split's state (`crate::split::SplitState`).
        t("split", &["k"]),
        // M4-WP3, 32.23: Actions secrets and variables by scope (`""` or `env:<name>`) and name.
        t("actions_secret", &["scope", "name"]),
        t("actions_variable", &["scope", "name"]),
        // roll-up, 32.25: every `Promoted` committed in a 32.25 entry by its `from`, and every `Compacted` by its `from`.
        t("promoted_log", &["from"]),
        t("compacted", &["from"]),
        // AM3-WP2a, 32.34: a fork's network and where it was cut (`RepoEvent::Genesis`); a root has no row.
        t("fork", &[]),
    ],
};

pub const METADATA: Schema = Schema {
    tier: Tier::Metadata,
    tables: &[
        t("check_expiry", &["at", "kind", "key"]),
        t("check_run", &["sha", "run"]),
        // R24, 32.24: each Actions job's last `workflow_job` step, by job id, pruned with the check rows.
        t("workflow_job", &["job"]),
        t("check_suite", &["sha", "suite"]),
        t("comment", &["id"]),
        t("comment_by_issue", &["n", "id"]),
        t("ids", &[]),
        t("issue", &["n"]),
        t("issue_by_id", &["id"]),
        t("issue_assignee", &["n", "account"]),
        t("issue_label", &["n", "label"]),
        t("issue_order", &["kind", "open", "rev_n"]),
        t("label", &["id"]),
        t("pull", &["n"]),
        t("retention", &[]),
        t("review", &["id"]),
        t("review_by_pull", &["n", "id"]),
        t("review_request", &["n", "grantee"]),
        t("status", &["sha", "rev_id"]),
        // Releases keyed by id negated, so a forward scan lists the newest first; `release_by_tag` maps a tag to its
        // release. Assets by id, with `asset_by_release`. LFS objects by oid, locks by id with `lfs_lock_by_path`.
        t("release", &["rev_id"]),
        t("release_by_tag", &["tag"]),
        t("asset", &["id"]),
        t("asset_by_release", &["release", "id"]),
        t("lfs_object", &["oid"]),
        t("lfs_lock", &["id"]),
        t("lfs_lock_by_path", &["path"]),
        // AM1-WP2: each pushed commit's provenance, by commit and the ref the push moved.
        t("provenance", &["commit", "ref"]),
        // AM1-WP3: each task's metadata part (`crate::task::TaskMeta`).
        t("agent_task_meta", &["session"]),
        // AM1-WP4: each agent ref's `PathSet` (`crate::agent::PathSet`).
        t("path_set", &["ref"]),
        // M4-WP2, 32.19: the watcher index the `Counts` consumer writes (`crate::watch`), with the stargazer and
        // subscriber orders, each thread's explicit subscriptions, and the counts.
        t("watcher", &["account"]),
        t("stargazer", &["starred_at", "account"]),
        t("subscriber", &["account"]),
        t("thread_sub", &["n", "account"]),
        t("watch_count", &[]),
        // Format 32.20: the parent's slim cursor and a child member's restamp (`crate::split`).
        t("split", &["k"]),
        // Milestones, 32.37: milestones by number, the highest number used, each issue's milestone, and each milestone's
        // issues by state, which its `open_issues` and `closed_issues` count.
        t("milestone", &["n"]),
        t("milestone_last", &[]),
        t("issue_milestone", &["n"]),
        t("milestone_issue", &["milestone", "open", "n"]),
    ],
};

// ---- rows ----

pub fn int(v: i64) -> Val {
    Val::Int(v)
}
pub fn n_key(n: IssueNumber) -> Val {
    Val::Int(i64::from(n.0))
}
pub fn sha_key(o: &ObjectId) -> Val {
    Val::Blob(o.0.to_vec().into())
}
pub fn ref_key(r: &RefName) -> Val {
    Val::Blob(r.as_bytes().to_vec().into())
}
fn text(s: &str) -> Val {
    Val::Text(s.into())
}

/// The core `required_check` key's third column: which source reports the name, so a re-run by the same App or a
/// new status from the same creator replaces the row it supersedes.
pub(crate) fn source_key(s: &CheckSource) -> Val {
    match s {
        CheckSource::Run { app, .. } => Val::Text(format!("app:{}", app.0.0).into()),
        CheckSource::Status { creator } => Val::Text(format!("status:{}", creator.0.0).into()),
    }
}

/// When the repository was created over the API (format 32.13), if it was.
pub fn created(core: &(impl Read + ?Sized)) -> Option<Timestamp> {
    get_rec(core, "created", &[])
}

/// A fork's core row (`RepoEvent::Genesis`, format 32.34): its network, and the repository and index it was cut from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForkRow {
    pub network: NetworkId,
    pub fork_of: Option<(RepoId, Index)>,
}
/// The repository's network as its chain names it: a fork's `fork` row, else the repository's own id.
pub fn network(core: &(impl Read + ?Sized), repo: RepoId) -> NetworkId {
    get_rec::<ForkRow>(core, "fork", &[]).map_or(NetworkId::of_root(repo), |f| f.network)
}
/// The fork's row, `None` for a root.
pub fn fork(core: &(impl Read + ?Sized)) -> Option<ForkRow> {
    get_rec(core, "fork", &[])
}

/// The repository's `Deleted` tombstone: kept until this time, and who deleted it.
pub fn deleted(core: &(impl Read + ?Sized)) -> Option<(Timestamp, Option<AccountId>)> {
    get_rec(core, "deleted", &[])
}

/// The repository's default branch: the `settings` row's, else `refs/heads/main`, which is how every read of a
/// repository with no `Settings` row takes it (`crate::transfer`'s defaults). Both HEADs read it, the shard's through
/// the member's `ranges` read and the edge's v2 ls-refs (G26), so the two never disagree. A pre-F59 create whose edge
/// died after `RepoOwner` has no row and no finisher to write one.
pub fn default_branch(core: &(impl Read + ?Sized)) -> RefName {
    get_rec::<crate::issues::RepoSettings>(core, "settings", &[])
        .map_or_else(|| RefName::parse(b"refs/heads/main").expect("a valid ref"), |s| s.default_branch)
}

pub fn get_rec<T: DeserializeOwned>(r: &(impl Read + ?Sized), table: &str, key: &[Val]) -> Option<T> {
    match r.get(table, key)?.first() {
        Some(Val::Blob(b)) => Some(postcard::from_bytes(b).expect("a fold row this release wrote")),
        _ => panic!("row {table} has no record"),
    }
}

pub(crate) fn put_rec<T: Serialize>(w: &mut dyn Rows, table: &str, key: Vec<Val>, rec: &T) {
    let b = postcard::to_allocvec(rec).expect("a fold row encodes");
    w.put(table, key, vec![Val::Blob(b.into())]);
}

fn mark(w: &mut dyn Rows, table: &str, key: Vec<Val>) {
    w.put(table, key, Vec::new());
}

/// The core `required_check` row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct RequiredRow {
    pub source: CheckSource,
    pub version: u64,
    pub conclusion: Option<CheckConclusion>,
}

impl RequiredRow {
    /// Whether `new` replaces this row of the same (sha, name, source). Run and status ids are `DbId`s a group assigns
    /// in increasing order, so a re-run's later run, or a later status, wins over an older one even when the older one's
    /// update commits last; within one run the higher version wins, so a repeat changes nothing. A status's `version`
    /// is its `StatusId`.
    pub(crate) fn superseded_by(&self, new: &RequiredRow) -> bool {
        let order = |r: &RequiredRow| match r.source {
            CheckSource::Run { run, .. } => (run.0.0, r.version),
            CheckSource::Status { .. } => (r.version as i64, 0),
        };
        order(new) > order(self)
    }
}

/// The core `review_verdict` row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct VerdictRow {
    pub review: crate::issues::ReviewId,
    pub state: ReviewState,
    pub commit: ObjectId,
}

/// The metadata `issue` row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct IssueRow {
    pub id: graftyard_types::DbId,
    pub kind: IssueKind,
    pub open: bool,
    pub author: Option<AccountId>,
    pub title: Box<str>,
    pub body: Box<str>,
    pub created: Timestamp,
    pub updated: Timestamp,
    pub closed: Option<Timestamp>,
    pub comments: u32,
    pub locked: Option<LockReason>,
    /// The reason of the last `State` change, which GraphQL's `stateReason` reads.
    pub reason: Option<StateReason>,
}

/// The metadata `pull` row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct PullRow {
    pub head: HeadSpec,
    pub base: RefName,
    pub head_sha: ObjectId,
    pub base_sha: ObjectId,
    pub draft: bool,
}

/// The metadata `comment` row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct CommentRow {
    pub on: CommentTarget,
    pub author: Option<AccountId>,
    pub body: Box<str>,
    pub reply_to: Option<crate::issues::CommentId>,
    pub resolved: bool,
    pub created: Timestamp,
    pub updated: Timestamp,
}

/// The metadata `review` row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct ReviewRow {
    pub n: IssueNumber,
    pub reviewer: Option<AccountId>,
    pub state: ReviewState,
    pub commit: ObjectId,
    pub body: Box<str>,
    pub dismissed: Option<Box<str>>,
    pub at: Timestamp,
}

/// Issue `n`'s assignees, from the metadata `issue_assignee` rows.
pub fn assignees(meta: &(impl Read + ?Sized), n: IssueNumber) -> Vec<AccountId> {
    meta.prefix("issue_assignee", &[n_key(n)])
        .into_iter()
        .filter_map(|(k, _)| match k.get(1) {
            Some(Val::Int(a)) => Some(AccountId(graftyard_types::DbId(*a))),
            _ => None,
        })
        .collect()
}

/// Who assigned `a` to issue `n` through the `Assigned` entry that added it, while `a` stays assigned: `None` when `a`
/// is not assigned, was assigned by an App, or came with the issue's `IssueOpened`, whose rows record no one.
pub fn assigned_by(meta: &(impl Read + ?Sized), n: IssueNumber, a: AccountId) -> Option<graftyard_identity::UserId> {
    let row = meta.get("issue_assignee", &[n_key(n), int(a.0.0)])?;
    let by: Option<AccountId> = match row.first() {
        Some(Val::Blob(b)) => postcard::from_bytes(b).expect("a fold row this release wrote"),
        _ => None,
    };
    by.filter(|b| graftyard_identity::AppId::of_bot(*b).is_none()).map(graftyard_identity::UserId)
}

/// The metadata `label` row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct LabelRow {
    pub name: LabelName,
    pub color: [u8; 3],
    pub description: Option<Box<str>>,
}

/// The metadata `milestone` row (format 32.37).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct MilestoneRow {
    pub id: DbId,
    pub title: Box<str>,
    pub description: Option<Box<str>>,
    pub open: bool,
    pub due_on: Option<Timestamp>,
    pub creator: Option<AccountId>,
    pub created: Timestamp,
    pub updated: Timestamp,
    pub closed: Option<Timestamp>,
}

/// Milestone `n`'s row.
pub fn milestone(meta: &(impl Read + ?Sized), n: u32) -> Option<MilestoneRow> {
    get_rec(meta, "milestone", &[int(i64::from(n))])
}

/// Every milestone, by number.
pub fn milestones(meta: &(impl Read + ?Sized)) -> Vec<(u32, MilestoneRow)> {
    meta.prefix("milestone", &[])
        .into_iter()
        .filter_map(|(k, _)| match k.first() {
            Some(Val::Int(n)) => Some((*n as u32, get_rec(meta, "milestone", &k)?)),
            _ => None,
        })
        .collect()
}

/// The highest milestone number used, deleted ones included, so a number is never handed out twice.
pub fn milestone_last(meta: &(impl Read + ?Sized)) -> u32 {
    get_rec(meta, "milestone_last", &[]).unwrap_or(0)
}

/// Issue `n`'s milestone number.
pub fn issue_milestone(meta: &(impl Read + ?Sized), n: IssueNumber) -> Option<u32> {
    get_rec(meta, "issue_milestone", &[n_key(n)])
}

/// Milestone `m`'s open and closed issues and pull requests, from the `milestone_issue` index.
pub fn milestone_counts(meta: &(impl Read + ?Sized), m: u32) -> (usize, usize) {
    let count = |open: bool| meta.prefix("milestone_issue", &[int(i64::from(m)), int(i64::from(open))]).len();
    (count(true), count(false))
}

/// The metadata `release` row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct ReleaseRow {
    pub rec: ReleaseRecord,
    pub created: Timestamp,
    /// When it was first published, `None` while a draft.
    pub published: Option<Timestamp>,
}

/// The metadata `asset` row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct AssetRow {
    pub release: RepoReleaseId,
    pub rec: AssetRecord,
    pub uploader: Option<AccountId>,
    pub created: Timestamp,
    pub updated: Timestamp,
}

/// The metadata `lfs_object` row: the blob a `verify` committed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct LfsRow {
    pub blob: BlobRef,
    pub at: Timestamp,
}

/// The metadata `lfs_lock` row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct LockRow {
    pub path: Box<str>,
    pub refspec: Option<RefName>,
    pub owner: AccountId,
    pub at: Timestamp,
}

pub fn release_key(id: RepoReleaseId) -> Val {
    int(-id.0.0)
}

pub fn release_row(r: &(impl Read + ?Sized), id: RepoReleaseId) -> Option<ReleaseRow> {
    get_rec(r, "release", &[release_key(id)])
}

pub fn release_by_tag(r: &(impl Read + ?Sized), tag: &RefName) -> Option<RepoReleaseId> {
    get_rec::<i64>(r, "release_by_tag", &[ref_key(tag)]).map(|i| RepoReleaseId(DbId(i)))
}

pub fn lfs_key(oid: &graftyard_types::Digest) -> Val {
    Val::Blob(oid.0.to_vec().into())
}

/// A metadata check row: the entry time that last wrote it, which pruning reads, and the record.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct Timed<T> {
    pub at: Timestamp,
    pub rec: T,
}

const KIND_RUN: i64 = 0;
const KIND_SUITE: i64 = 1;
const KIND_STATUS: i64 = 2;
const KIND_WORKFLOW_JOB: i64 = 3;

fn check_table(kind: i64) -> &'static str {
    match kind {
        KIND_RUN => "check_run",
        KIND_SUITE => "check_suite",
        KIND_WORKFLOW_JOB => "workflow_job",
        _ => "status",
    }
}

/// A job's last `workflow_job` step, while its row is kept.
pub fn workflow_job(r: &(impl Read + ?Sized), job: DbId) -> Option<crate::workflow_job::WorkflowJobUpdate> {
    get_rec::<Timed<crate::workflow_job::WorkflowJobUpdate>>(r, "workflow_job", &[int(job.0)]).map(|t| t.rec)
}

/// The `check_expiry` key of a row of `kind` written at `at`: its key columns ride in one blob.
fn expiry_key(at: Timestamp, kind: i64, key: &[Val]) -> Vec<Val> {
    let b = postcard::to_allocvec(key).expect("a key encodes");
    vec![int(at.0 as i64), int(kind), Val::Blob(b.into())]
}

/// Writes a metadata check row stamped with `at`, moving its `check_expiry` row from its previous time.
fn put_timed<T: Serialize + DeserializeOwned>(w: &mut dyn Rows, kind: i64, key: Vec<Val>, at: Timestamp, rec: T) {
    let table = check_table(kind);
    if let Some(old) = get_rec::<Timed<T>>(w, table, &key) {
        w.delete("check_expiry", &expiry_key(old.at, kind, &key));
    }
    mark(w, "check_expiry", expiry_key(at, kind, &key));
    put_rec(w, table, key, &Timed { at, rec });
}

// ---- reads shared by apply and decide ----

pub fn pull_head(r: &(impl Read + ?Sized), n: IssueNumber) -> Option<PullHead> {
    get_rec(r, "pull_head", &[n_key(n)])
}

/// The head branches of `repo`'s open pull requests whose head is `sha` (`open_head`): where `AgentGate` for `sha` is
/// read (`crate::merge::agent_gate_holds`).
pub fn pulls_with_head(r: &(impl Read + ?Sized), repo: RepoId, sha: ObjectId) -> Vec<RefName> {
    r.prefix("open_head", &[sha_key(&sha)])
        .into_iter()
        .filter_map(|(k, _)| match k.get(1) {
            Some(Val::Int(n)) => pull_head(r, IssueNumber(*n as u32)),
            _ => None,
        })
        .filter(|ph| ph.head.repo == repo)
        .map(|ph| ph.head.branch)
        .collect()
}

/// Open pull requests whose head is `branch` of `repo`.
pub(crate) fn pulls_from(r: &(impl Read + ?Sized), repo: RepoId, branch: &RefName) -> Vec<IssueNumber> {
    r.prefix("pull_by_head", &[int(repo.0.0), ref_key(branch)])
        .into_iter()
        .filter_map(|(k, _)| match k.get(2) {
            Some(Val::Int(n)) => Some(IssueNumber(*n as u32)),
            _ => None,
        })
        .collect()
}

/// The core `review_thread` row: the pull request a review thread is on, and whether it is resolved.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct ThreadRow {
    pub n: IssueNumber,
    pub resolved: bool,
}

/// The core count of unresolved review threads on pull request `n`, which `check_protection` reads when a rule sets
/// `conversation_resolution`.
pub fn unresolved_threads(core: &(impl Read + ?Sized), n: IssueNumber) -> u32 {
    get_rec(core, "unresolved", &[n_key(n)]).unwrap_or(0)
}

/// Adds `by` to `n`'s unresolved count, keeping no row for zero.
fn count_unresolved(w: &mut dyn Rows, n: IssueNumber, by: i32) {
    let c = unresolved_threads(w, n).checked_add_signed(by).expect("an unresolved count never goes below zero");
    if c == 0 { w.delete("unresolved", &[n_key(n)]) } else { put_rec(w, "unresolved", vec![n_key(n)], &c) }
}

/// The tier an event's ids come from: Core when the event touches Core, else Metadata.
pub fn id_tier(set: TierSet) -> Tier {
    if has(set, Tier::Core) { Tier::Core } else { Tier::Metadata }
}

/// `tier`'s half of a leased block: Metadata assigns its lower half, Core its upper half, so the tiers' cursors never
/// meet. `None` for Metadata's half of a one-id block.
pub fn tier_block(block: IdBlock, tier: Tier) -> Option<IdBlock> {
    let low = block.len() / 2;
    match tier {
        Tier::Metadata => IdBlock::new(block.base(), low),
        Tier::Core => IdBlock::new(DbId(block.base().0 + i64::from(low)), block.len() - low),
    }
}

/// A tier's own `DbId` cursor.
pub(crate) fn ids(r: &(impl Read + ?Sized)) -> IdCursor {
    get_rec(r, "ids", &[]).unwrap_or_default()
}

/// Records that `ev`'s write assigned `id`, in the tier `id_tier` gives it, so only that tier's cursor moves.
fn assigned(w: &mut dyn Rows, ev: &RepoEvent, tier: Tier, id: DbId) {
    if id_tier(tier_of(ev)) != tier {
        return;
    }
    let mut c = ids(w);
    c.apply_assigned(id);
    put_rec(w, "ids", Vec::new(), &c);
}

fn retention_days(r: &(impl Read + ?Sized)) -> u16 {
    get_rec(r, "retention", &[]).unwrap_or(CHECK_RETENTION_DEFAULT)
}

/// A tier's snapshot rows, `(to, entry time at to, archived record)`, in index order.
pub fn snapshots(
    r: &(impl Read + ?Sized),
    tier: Tier,
) -> Vec<(graftyard_types::Index, Timestamp, crate::pack::Archived)> {
    r.prefix("snapshot", &[tier_key(tier)])
        .into_iter()
        .filter_map(|(k, _)| get_rec::<crate::pack::Archived>(r, "snapshot", &k))
        .map(|a| (a.to, a.to_at, a))
        .collect()
}

fn tier_key(t: Tier) -> Val {
    int(match t {
        Tier::Core => 0,
        Tier::Metadata => 1,
    })
}

/// A committed snapshot of `tier` at `to`: its row, then every row `snapshots_to_keep` no longer keeps at this
/// entry's time, so each tier keeps one snapshot at or below every index inside `RESTORE_WINDOW` and every member
/// drops the same rows. The snapshot's objects in R2 go once their row has (`workers/site`).
fn snapshot_committed(w: &mut dyn Rows, tier: Tier, to: graftyard_types::Index, a: &crate::pack::Archived) {
    put_rec(w, "snapshot", vec![tier_key(tier), int(to.0 as i64)], a);
    let all = snapshots(w, tier);
    let keep = graftyard_fold::snapshots_to_keep(&all.iter().map(|(i, t, _)| (*i, *t)).collect::<Vec<_>>(), a.to_at);
    for (i, _, _) in all {
        if !keep.contains(&i) {
            w.delete("snapshot", &[tier_key(tier), int(i.0 as i64)]);
        }
    }
}

// ---- writes ----

/// The repository chain's tiers. `repo` is the chain's own repository, which a same-repository head belongs to, and
/// `owner` the account whose `Authz` gates it until the first `Settings` entry names one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RepoTiers {
    pub repo: RepoId,
    pub owner: AccountId,
}

/// Drops every required check of `sha` unless another open pull request still has it as its head.
fn prune_required(w: &mut dyn Rows, sha: &ObjectId) {
    if !w.prefix("open_head", &[sha_key(sha)]).is_empty() {
        return;
    }
    for (k, _) in w.prefix("required_check", &[sha_key(sha)]) {
        w.delete("required_check", &k);
    }
}

/// Moves open pull request `n`'s head to `new`, pruning the old head's required checks.
fn move_head(w: &mut dyn Rows, n: IssueNumber, new: ObjectId) {
    let Some(mut ph) = pull_head(w, n) else { return };
    if !matches!(ph.state, PrState::Open { .. }) || ph.head_sha == new {
        return;
    }
    let old = ph.head_sha;
    w.delete("open_head", &[sha_key(&old), n_key(n)]);
    mark(w, "open_head", vec![sha_key(&new), n_key(n)]);
    ph.head_sha = new;
    put_rec(w, "pull_head", vec![n_key(n)], &ph);
    prune_required(w, &old);
}

/// Opens or reopens `n` in Core at `head_sha`.
fn open_core(w: &mut dyn Rows, n: IssueNumber, ph: &PullHead) {
    mark(w, "open_head", vec![sha_key(&ph.head_sha), n_key(n)]);
    mark(w, "pull_by_head", vec![int(ph.head.repo.0.0), ref_key(&ph.head.branch), n_key(n)]);
    put_rec(w, "pull_head", vec![n_key(n)], ph);
}

/// Closes `n` in Core: it leaves the head indexes and its head's required checks are pruned.
fn close_core(w: &mut dyn Rows, n: IssueNumber, state: PrState) {
    let Some(mut ph) = pull_head(w, n) else { return };
    if !matches!(ph.state, PrState::Open { .. }) {
        return;
    }
    w.delete("open_head", &[sha_key(&ph.head_sha), n_key(n)]);
    w.delete("pull_by_head", &[int(ph.head.repo.0.0), ref_key(&ph.head.branch), n_key(n)]);
    ph.state = state;
    put_rec(w, "pull_head", vec![n_key(n)], &ph);
    prune_required(w, &ph.head_sha);
}

/// `refs/pull/{n}/head`.
pub fn head_ref(n: IssueNumber) -> RefName {
    RefName::parse(format!("refs/pull/{}/head", n.0).as_bytes()).expect("a pull ref is a ref name")
}

/// `refs/pull/{n}/merge`, which a `TestMerged` moves.
pub fn merge_ref(n: IssueNumber) -> RefName {
    RefName::parse(format!("refs/pull/{}/merge", n.0).as_bytes()).expect("a pull ref is a ref name")
}

/// Pull request `n`'s last test merge.
pub fn mergeability(r: &(impl Read + ?Sized), n: IssueNumber) -> Option<Mergeability> {
    get_rec(r, "mergeability", &[n_key(n)])
}

/// Every event at or below this index is handled by `consumer`; 0 before its first cursor entry.
pub fn consumer_cursor(r: &(impl Read + ?Sized), consumer: Consumer) -> Index {
    get_rec(r, "consumer_cursor", &[int(consumer.tag())]).unwrap_or_default()
}

/// `refs/pull/{n}/head`'s number.
fn pull_ref(name: &RefName) -> Option<IssueNumber> {
    let s = std::str::from_utf8(name.as_bytes()).ok()?;
    let n = s.strip_prefix("refs/pull/")?.strip_suffix("/head")?;
    n.parse().ok().filter(|v: &u32| *v > 0 && n == v.to_string()).map(IssueNumber)
}

impl RepoTiers {
    /// One ref update, and every open pull request whose head is that ref follows it.
    fn write_ref(&self, w: &mut dyn Rows, u: &crate::refs::RefUpdate) {
        match u.new {
            Some(o) => put_rec(w, "ref", vec![ref_key(&u.name)], &o),
            None => w.delete("ref", &[ref_key(&u.name)]),
        }
        let Some(new) = u.new else { return };
        if let Some(n) = pull_ref(&u.name) {
            move_head(w, n, new);
        }
        for n in pulls_from(w, self.repo, &u.name) {
            move_head(w, n, new);
        }
    }

    fn write_core(&self, ev: &RepoEvent, at: Timestamp, format: Format, w: &mut dyn Rows) {
        match ev {
            RepoEvent::Refs { updates, .. } => {
                for u in updates {
                    self.write_ref(w, u);
                }
            }
            RepoEvent::Merged { n, head, base, by, .. } => {
                for u in ev.updates() {
                    self.write_ref(w, u);
                }
                let merge_commit = base.new.expect("a merge moves its base");
                close_core(w, *n, PrState::Merged { at, by: *by, merge_commit, final_head: *head });
            }
            RepoEvent::Queue(q) => {
                for u in q.refs() {
                    self.write_ref(w, u);
                    // A deleted queue ref's required checks go with it, so the core tier stays small.
                    if u.new.is_none()
                        && let Some(old) = u.expect
                    {
                        prune_required(w, &old);
                    }
                }
                crate::queue::write(w, q, at);
            }
            RepoEvent::Refused { .. } => {}
            RepoEvent::Promoted(p) => {
                put_rec(w, "promoted", Vec::new(), p);
                // Only a 32.25 entry logs its promotion for roll-up: a 32.1 `Promoted` committed before roll-up
                // existed, and a refold on this release must give the digest its members kept (F88).
                if format >= REPO_FORMAT_ROLLUP {
                    put_rec(w, "promoted_log", vec![Val::Int(p.from.0 as i64)], p);
                }
            }
            RepoEvent::Compacted(c) => put_rec(w, "compacted", vec![Val::Int(c.from.0 as i64)], c),
            RepoEvent::Archived(a) => match &a.key {
                crate::key::R2Key::Snapshot { tier, to, .. } => snapshot_committed(w, *tier, *to, a),
                _ => put_rec(w, "archived", Vec::new(), a),
            },
            RepoEvent::IssueOpened { n, capture } => {
                assigned(w, ev, Tier::Core, capture.id);
                put_rec(w, "counter", Vec::new(), &n.0)
            }
            RepoEvent::PrOpened { n, head, base, capture } => {
                assigned(w, ev, Tier::Core, capture.issue.id);
                put_rec(w, "counter", Vec::new(), &n.0);
                let ph = PullHead {
                    head: head.clone(),
                    base: base.clone(),
                    head_sha: capture.head_sha,
                    state: capture.state,
                };
                open_core(w, *n, &ph);
            }
            RepoEvent::PrClosed { n, final_head } => close_core(w, *n, PrState::Closed { at, final_head: *final_head }),
            RepoEvent::IssueChanged { n, change, .. } => {
                let Some(mut ph) = pull_head(w, *n) else { return };
                match (change, ph.state) {
                    (IssueChange::State { open: true, .. }, PrState::Closed { .. }) => {
                        // A same-repository head reopens at its branch's tip, which may have moved while closed.
                        if ph.head.repo == self.repo
                            && let Some(tip) = get_rec::<ObjectId>(w, "ref", &[ref_key(&ph.head.branch)])
                        {
                            ph.head_sha = tip;
                        }
                        ph.state = PrState::Open { draft: false };
                        open_core(w, *n, &ph);
                    }
                    (IssueChange::State { open: false, .. }, PrState::Open { .. }) => {
                        close_core(w, *n, PrState::Closed { at, final_head: ph.head_sha })
                    }
                    (IssueChange::Draft(d), PrState::Open { .. }) => {
                        ph.state = PrState::Open { draft: *d };
                        put_rec(w, "pull_head", vec![n_key(*n)], &ph);
                    }
                    _ => {}
                }
            }
            RepoEvent::Review(ReviewEvent::Submitted { id, n, reviewer, state, commit, .. }) => {
                assigned(w, ev, Tier::Core, id.0);
                if *state == ReviewState::Pending {
                    return;
                }
                let who = int(reviewer.map_or(0, |a| a.0.0));
                if let Some(old) = get_rec::<VerdictRow>(w, "review_verdict", &[n_key(*n), who.clone()]) {
                    w.delete("verdict_by_review", &[int(old.review.0.0)]);
                }
                put_rec(
                    w,
                    "review_verdict",
                    vec![n_key(*n), who.clone()],
                    &VerdictRow { review: *id, state: *state, commit: *commit },
                );
                put_rec(w, "verdict_by_review", vec![int(id.0.0)], &(n.0, reviewer.map_or(0, |a| a.0.0)));
            }
            RepoEvent::Review(ReviewEvent::Dismissed { id, .. }) => {
                if let Some((n, who)) = get_rec::<(u32, i64)>(w, "verdict_by_review", &[int(id.0.0)]) {
                    w.delete("verdict_by_review", &[int(id.0.0)]);
                    w.delete("review_verdict", &[n_key(IssueNumber(n)), int(who)]);
                }
            }
            RepoEvent::Review(ReviewEvent::Requested { .. }) => {}
            // A single-comment review is the reviewer's verdict only over none or a COMMENTED one.
            RepoEvent::Review(ReviewEvent::SingleComment { id, n, reviewer, commit }) => {
                assigned(w, ev, Tier::Core, id.0);
                let who = int(reviewer.map_or(0, |a| a.0.0));
                let old = get_rec::<VerdictRow>(w, "review_verdict", &[n_key(*n), who.clone()]);
                if old.as_ref().is_some_and(|o| o.state != ReviewState::Commented) {
                    return;
                }
                if let Some(old) = old {
                    w.delete("verdict_by_review", &[int(old.review.0.0)]);
                }
                let row = VerdictRow { review: *id, state: ReviewState::Commented, commit: *commit };
                put_rec(w, "review_verdict", vec![n_key(*n), who], &row);
                put_rec(w, "verdict_by_review", vec![int(id.0.0)], &(n.0, reviewer.map_or(0, |a| a.0.0)));
            }
            RepoEvent::Rule(r) => {
                assigned(w, ev, Tier::Core, r.id);
                match &r.protection {
                    Some(_) => put_rec(w, "rule", vec![int(r.id.0)], r),
                    None => w.delete("rule", &[int(r.id.0)]),
                }
            }
            RepoEvent::Settings(s) => put_rec(w, "settings", Vec::new(), s),
            RepoEvent::Collaborator { who, permission: Some(p) } => put_rec(w, "collaborator", vec![int(who.0.0)], p),
            RepoEvent::Collaborator { who, permission: None } => w.delete("collaborator", &[int(who.0.0)]),
            RepoEvent::Invitation(InvitationEvent::Sent { id, invitee, grant, inviter, expires }) => {
                assigned(w, ev, Tier::Core, id.0);
                let row =
                    InvitationRow { invitee: *invitee, grant: grant.clone(), inviter: *inviter, expires: *expires };
                put_rec(w, "invitation", vec![int(id.0.0)], &row);
            }
            RepoEvent::Invitation(
                InvitationEvent::Accepted { id }
                | InvitationEvent::Declined { id }
                | InvitationEvent::Cancelled { id, .. },
            ) => w.delete("invitation", &[int(id.0.0)]),
            RepoEvent::RequiredCheck { sha, name, source, version, conclusion } => {
                let key = vec![sha_key(sha), text(name), source_key(source)];
                let row = RequiredRow { source: *source, version: *version, conclusion: *conclusion };
                let newer = get_rec::<RequiredRow>(w, "required_check", &key).is_none_or(|o| o.superseded_by(&row));
                if newer {
                    put_rec(w, "required_check", key, &row);
                }
            }
            RepoEvent::Comment(CommentEvent::Created {
                id,
                on: CommentTarget::Review { n, .. },
                reply_to: None,
                ..
            }) => {
                assigned(w, ev, Tier::Core, id.0);
                put_rec(w, "review_thread", vec![int(id.0.0)], &ThreadRow { n: *n, resolved: false });
                count_unresolved(w, *n, 1);
            }
            RepoEvent::Comment(CommentEvent::Resolved { thread, resolved, .. }) => {
                let key = vec![int(thread.0.0)];
                if let Some(mut t) = get_rec::<ThreadRow>(w, "review_thread", &key)
                    && t.resolved != *resolved
                {
                    t.resolved = *resolved;
                    put_rec(w, "review_thread", key, &t);
                    count_unresolved(w, t.n, if *resolved { -1 } else { 1 });
                }
            }
            RepoEvent::Comment(CommentEvent::Deleted { id, .. }) => {
                if let Some(t) = get_rec::<ThreadRow>(w, "review_thread", &[int(id.0.0)]) {
                    w.delete("review_thread", &[int(id.0.0)]);
                    if !t.resolved {
                        count_unresolved(w, t.n, -1);
                    }
                }
            }
            RepoEvent::Comment(_) | RepoEvent::Labels(_) => {}
            RepoEvent::CheckRun(_) | RepoEvent::CheckSuite(_) | RepoEvent::Status(_) => {}
            RepoEvent::Release { id, tag, .. } => {
                assigned(w, ev, Tier::Core, id.0);
                if let Some(u) = tag {
                    match u.new {
                        Some(o) => put_rec(w, "ref", vec![ref_key(&u.name)], &o),
                        None => w.delete("ref", &[ref_key(&u.name)]),
                    }
                }
            }
            RepoEvent::ReleaseAsset { .. } | RepoEvent::LfsObject { .. } | RepoEvent::LfsLock(_) => {}
            // `PrSync::decide_sync` commits one only for an open pull request's current head.
            RepoEvent::TestMerged { n, base, head, result, .. } => {
                let m = Mergeability { base: *base, head: *head, result: result.clone() };
                put_rec(w, "mergeability", vec![n_key(*n)], &m);
                let merge_ref = merge_ref(*n);
                match result {
                    MergeabilityResult::Clean { merge } => put_rec(w, "ref", vec![ref_key(&merge_ref)], merge),
                    MergeabilityResult::Conflict { .. } => w.delete("ref", &[ref_key(&merge_ref)]),
                }
            }
            RepoEvent::ConsumerCursor { consumer, through } => {
                let key = vec![int(consumer.tag())];
                if get_rec::<Index>(w, "consumer_cursor", &key).is_none_or(|c| c < *through) {
                    put_rec(w, "consumer_cursor", key, through);
                }
            }
            RepoEvent::Provenance { .. } | RepoEvent::Watchers(_) => {}
            RepoEvent::AgentMeta { n, meta } => put_rec(w, "agent_meta", vec![n_key(*n)], meta),
            RepoEvent::AgentGate(g) => {
                mark(w, "agent_gate", vec![n_key(g.pr), sha_key(&g.head), int(g.satisfied_by.0.0.0)])
            }
            RepoEvent::AgentPolicy(p) => put_rec(w, "agent_policy", Vec::new(), p),
            RepoEvent::TaskCreated(t) => crate::task::write_created_core(w, t, at),
            RepoEvent::TaskUpdated { session, state, .. } => crate::task::write_updated_core(w, session, *state, at),
            RepoEvent::TaskResumed { session, .. } => crate::task::write_resumed_core(w, session, at),
            RepoEvent::Created { .. } => {
                if get_rec::<Timestamp>(w, "created", &[]).is_none() {
                    put_rec(w, "created", Vec::new(), &at);
                }
            }
            RepoEvent::Deleted { retain_until, actor } => put_rec(w, "deleted", Vec::new(), &(*retain_until, *actor)),
            RepoEvent::Hook { id, record, .. } => {
                // A create's id is the core tier's next one, which the tier's cursor passes, as a release's does.
                if record.is_some() {
                    assigned(w, ev, Tier::Core, id.0);
                }
                crate::hooks::write_core(w, ev, at)
            }
            RepoEvent::HookCursor(_) => crate::hooks::write_core(w, ev, at),
            RepoEvent::LeaseAcquire { lease, plan } => crate::lease::write_acquire(w, lease, *plan),
            RepoEvent::LeaseRenew { id, expires } => crate::lease::write_renew(w, *id, *expires),
            RepoEvent::LeaseRelease { ids, .. } => {
                for id in ids {
                    crate::lease::write_release(w, *id);
                }
            }
            RepoEvent::PathSet(_) => {}
            RepoEvent::Fanout { k } => put_rec(w, "fanout", Vec::new(), k),
            RepoEvent::Genesis { network, fork_of, .. } => {
                if network.root() != self.repo {
                    put_rec(w, "fork", Vec::new(), &ForkRow { network: *network, fork_of: *fork_of })
                }
            }
            RepoEvent::Split(x) => crate::split::write_core(w, x, at),
            RepoEvent::ActionsSecret { .. } | RepoEvent::ActionsVariable { .. } => crate::secrets::write_core(w, ev),
            RepoEvent::WorkflowJob(_) | RepoEvent::Assigned { .. } | RepoEvent::Milestones(_) => {}
        }
    }

    fn write_meta(&self, ev: &RepoEvent, at: Timestamp, w: &mut dyn Rows) {
        let issue = |w: &dyn Rows, n: IssueNumber| get_rec::<IssueRow>(w, "issue", &[n_key(n)]);
        let touch = |w: &mut dyn Rows, n: IssueNumber, f: &dyn Fn(&mut IssueRow)| {
            if let Some(mut row) = get_rec::<IssueRow>(w, "issue", &[n_key(n)]) {
                let was = row.open;
                f(&mut row);
                row.updated = at;
                if was != row.open {
                    w.delete("issue_order", &order_key(row.kind, was, n));
                    mark(w, "issue_order", order_key(row.kind, row.open, n));
                    // Milestones (32.37): its milestone's counts move in the same entry. An issue with no milestone has no row.
                    if let Some(m) = issue_milestone(w, n) {
                        w.delete("milestone_issue", &milestone_issue_key(m, was, n));
                        mark(w, "milestone_issue", milestone_issue_key(m, row.open, n));
                    }
                }
                put_rec(w, "issue", vec![n_key(n)], &row);
            }
        };
        match ev {
            RepoEvent::IssueOpened { n, capture }
            | RepoEvent::PrOpened { n, capture: crate::issues::PrCapture { issue: capture, .. }, .. } => {
                let kind =
                    if matches!(ev, RepoEvent::PrOpened { .. }) { IssueKind::PullRequest } else { IssueKind::Issue };
                assigned(w, ev, Tier::Metadata, capture.id);
                put_rec(
                    w,
                    "issue",
                    vec![n_key(*n)],
                    &IssueRow {
                        id: capture.id,
                        kind,
                        open: true,
                        author: capture.author,
                        title: capture.title.clone(),
                        body: capture.body.clone(),
                        created: at,
                        updated: at,
                        closed: None,
                        comments: 0,
                        locked: None,
                        reason: None,
                    },
                );
                mark(w, "issue_order", order_key(kind, true, *n));
                put_rec(w, "issue_by_id", vec![int(capture.id.0)], &n.0);
                for l in &capture.labels {
                    mark(w, "issue_label", vec![n_key(*n), int(l.0.0)]);
                }
                for a in &capture.assignees {
                    mark(w, "issue_assignee", vec![n_key(*n), int(a.0.0)]);
                }
                if let RepoEvent::PrOpened { head, base, capture: c, .. } = ev {
                    let draft = matches!(c.state, PrState::Open { draft: true });
                    put_rec(
                        w,
                        "pull",
                        vec![n_key(*n)],
                        &PullRow {
                            head: head.clone(),
                            base: base.clone(),
                            head_sha: c.head_sha,
                            base_sha: c.base_sha,
                            draft,
                        },
                    );
                }
            }
            RepoEvent::PrClosed { n, .. } | RepoEvent::Merged { n, .. } => {
                touch(w, *n, &|r| {
                    r.open = false;
                    r.closed = Some(at);
                });
                // The head ref's `PathSet` row goes when its pull request closes. A repository with none has no row,
                // so this deletes nothing and changes no digest.
                if let Some(p) = get_rec::<PullRow>(w, "pull", &[n_key(*n)])
                    && p.head.repo == self.repo
                {
                    w.delete("path_set", &[ref_key(&p.head.branch)]);
                }
            }
            RepoEvent::IssueChanged { n, change, .. } => match change {
                IssueChange::Title(t) => touch(w, *n, &|r| r.title = t.clone()),
                IssueChange::Body(b) => touch(w, *n, &|r| r.body = b.clone()),
                IssueChange::State { open, reason } => touch(w, *n, &|r| {
                    r.open = *open;
                    r.closed = (!open).then_some(at);
                    r.reason = *reason;
                }),
                IssueChange::Locked(l) => touch(w, *n, &|r| r.locked = *l),
                IssueChange::Draft(d) => {
                    if let Some(mut p) = get_rec::<PullRow>(w, "pull", &[n_key(*n)]) {
                        p.draft = *d;
                        put_rec(w, "pull", vec![n_key(*n)], &p);
                        touch(w, *n, &|_| {});
                    }
                }
            },
            RepoEvent::Comment(c) => match c {
                CommentEvent::Created { id, on, author, body, reply_to } => {
                    assigned(w, ev, Tier::Metadata, id.0);
                    put_rec(
                        w,
                        "comment",
                        vec![int(id.0.0)],
                        &CommentRow {
                            on: on.clone(),
                            author: *author,
                            body: body.clone(),
                            reply_to: *reply_to,
                            resolved: false,
                            created: at,
                            updated: at,
                        },
                    );
                    if let Some(n) = target_issue(on) {
                        touch(w, n, &|r| r.comments += 1);
                    }
                    if let CommentTarget::Issue(n) = on {
                        mark(w, "comment_by_issue", vec![n_key(*n), int(id.0.0)]);
                    }
                }
                CommentEvent::Edited { id, body, .. } => {
                    if let Some(mut row) = get_rec::<CommentRow>(w, "comment", &[int(id.0.0)]) {
                        row.body = body.clone();
                        row.updated = at;
                        put_rec(w, "comment", vec![int(id.0.0)], &row);
                    }
                }
                CommentEvent::Deleted { id, .. } => {
                    if let Some(row) = get_rec::<CommentRow>(w, "comment", &[int(id.0.0)]) {
                        w.delete("comment", &[int(id.0.0)]);
                        if let CommentTarget::Issue(n) = row.on {
                            w.delete("comment_by_issue", &[n_key(n), int(id.0.0)]);
                        }
                        if let Some(n) = target_issue(&row.on) {
                            touch(w, n, &|r| r.comments = r.comments.saturating_sub(1));
                        }
                    }
                }
                CommentEvent::Resolved { thread, resolved, .. } => {
                    if let Some(mut row) = get_rec::<CommentRow>(w, "comment", &[int(thread.0.0)]) {
                        row.resolved = *resolved;
                        put_rec(w, "comment", vec![int(thread.0.0)], &row);
                    }
                }
            },
            RepoEvent::Review(r) => match r {
                ReviewEvent::Submitted { id, n, reviewer, state, commit, body } => {
                    assigned(w, ev, Tier::Metadata, id.0);
                    put_rec(
                        w,
                        "review",
                        vec![int(id.0.0)],
                        &ReviewRow {
                            n: *n,
                            reviewer: *reviewer,
                            state: *state,
                            commit: *commit,
                            body: body.clone(),
                            dismissed: None,
                            at,
                        },
                    );
                    mark(w, "review_by_pull", vec![n_key(*n), int(id.0.0)]);
                    touch(w, *n, &|_| {});
                }
                ReviewEvent::SingleComment { id, n, reviewer, commit } => {
                    assigned(w, ev, Tier::Metadata, id.0);
                    let row = ReviewRow {
                        n: *n,
                        reviewer: *reviewer,
                        state: ReviewState::Commented,
                        commit: *commit,
                        body: "".into(),
                        dismissed: None,
                        at,
                    };
                    put_rec(w, "review", vec![int(id.0.0)], &row);
                    mark(w, "review_by_pull", vec![n_key(*n), int(id.0.0)]);
                    touch(w, *n, &|_| {});
                }
                ReviewEvent::Dismissed { id, message, .. } => {
                    if let Some(mut row) = get_rec::<ReviewRow>(w, "review", &[int(id.0.0)]) {
                        row.dismissed = Some(message.clone());
                        put_rec(w, "review", vec![int(id.0.0)], &row);
                    }
                }
                ReviewEvent::Requested { n, reviewers, removed, .. } => {
                    for g in reviewers {
                        let key = vec![n_key(*n), grantee_key(g)];
                        if *removed { w.delete("review_request", &key) } else { mark(w, "review_request", key) }
                    }
                }
            },
            RepoEvent::Labels(l) => match l {
                LabelEvent::Defined { id, name, color, description } => {
                    assigned(w, ev, Tier::Metadata, id.0);
                    put_rec(
                        w,
                        "label",
                        vec![int(id.0.0)],
                        &LabelRow { name: name.clone(), color: *color, description: description.clone() },
                    )
                }
                LabelEvent::Deleted { id } => {
                    w.delete("label", &[int(id.0.0)]);
                    // Every issue's row for the label; the table is keyed by issue first.
                    for (k, _) in w.prefix("issue_label", &[]) {
                        if k.get(1) == Some(&int(id.0.0)) {
                            w.delete("issue_label", &k);
                        }
                    }
                }
                LabelEvent::Applied { n, add, remove, .. } => {
                    for l in add {
                        mark(w, "issue_label", vec![n_key(*n), int(l.0.0)]);
                    }
                    for l in remove {
                        w.delete("issue_label", &[n_key(*n), int(l.0.0)]);
                    }
                    if issue(w, *n).is_some() {
                        touch(w, *n, &|_| {});
                    }
                }
            },
            RepoEvent::Settings(s) => put_rec(w, "retention", Vec::new(), &s.check_retention_days),
            RepoEvent::CheckRun(u) => {
                // A REST create's run id is this tier's next one (`IssueOp::CreateCheck`), which the cursor passes; an
                // id from outside the tier's block moves nothing.
                assigned(w, ev, Tier::Metadata, u.run.0);
                put_newer(w, KIND_RUN, vec![sha_key(&u.head_sha), int(u.run.0.0)], at, u, |u: &CheckRunUpdate| {
                    u.version
                })
            }
            RepoEvent::CheckSuite(u) => {
                put_newer(w, KIND_SUITE, vec![sha_key(&u.head_sha), int(u.suite.0.0)], at, u, |u: &CheckSuiteUpdate| {
                    u.version
                })
            }
            RepoEvent::Status(s) => {
                let key = vec![sha_key(&s.sha), int(-s.id.0.0)];
                put_newer(w, KIND_STATUS, key, at, s, |_: &StatusUpdate| 0)
            }
            RepoEvent::Release { id, record, .. } => {
                assigned(w, ev, Tier::Metadata, id.0);
                let old = release_row(w, *id);
                if let Some(o) = &old {
                    w.delete("release_by_tag", &[ref_key(&o.rec.tag)]);
                }
                match record {
                    Some(rec) => {
                        let published = old.as_ref().and_then(|o| o.published).or((!rec.draft).then_some(at));
                        let created = old.as_ref().map_or(at, |o| o.created);
                        put_rec(w, "release_by_tag", vec![ref_key(&rec.tag)], &id.0.0);
                        put_rec(
                            w,
                            "release",
                            vec![release_key(*id)],
                            &ReleaseRow { rec: rec.clone(), created, published },
                        );
                    }
                    None => {
                        w.delete("release", &[release_key(*id)]);
                        for (k, _) in w.prefix("asset_by_release", &[int(id.0.0)]) {
                            w.delete("asset", &k[1..]);
                            w.delete("asset_by_release", &k);
                        }
                    }
                }
            }
            RepoEvent::ReleaseAsset { release, id, asset, actor } => {
                assigned(w, ev, Tier::Metadata, id.0);
                let key = vec![int(id.0.0)];
                match asset {
                    Some(rec) => {
                        let old = get_rec::<AssetRow>(w, "asset", &key);
                        let created = old.as_ref().map_or(at, |o| o.created);
                        let uploader = old.as_ref().map_or(*actor, |o| o.uploader);
                        mark(w, "asset_by_release", vec![int(release.0.0), int(id.0.0)]);
                        let row = AssetRow { release: *release, rec: rec.clone(), uploader, created, updated: at };
                        put_rec(w, "asset", key, &row);
                    }
                    None => {
                        w.delete("asset", &key);
                        w.delete("asset_by_release", &[int(release.0.0), int(id.0.0)]);
                    }
                }
            }
            RepoEvent::LfsObject { blob, .. } => {
                put_rec(w, "lfs_object", vec![lfs_key(&blob.oid)], &LfsRow { blob: *blob, at })
            }
            RepoEvent::LfsLock(LfsLockEvent::Created { id, path, refspec, owner }) => {
                assigned(w, ev, Tier::Metadata, id.0);
                put_rec(w, "lfs_lock_by_path", vec![text(path)], &id.0.0);
                let row = LockRow { path: path.clone(), refspec: refspec.clone(), owner: *owner, at };
                put_rec(w, "lfs_lock", vec![int(id.0.0)], &row);
            }
            RepoEvent::LfsLock(LfsLockEvent::Deleted { id, .. }) => {
                if let Some(row) = get_rec::<LockRow>(w, "lfs_lock", &[int(id.0.0)]) {
                    w.delete("lfs_lock_by_path", &[text(&row.path)]);
                    w.delete("lfs_lock", &[int(id.0.0)]);
                }
            }
            RepoEvent::Provenance { list, .. } => {
                for p in list {
                    put_rec(w, "provenance", vec![sha_key(&p.commit), ref_key(&p.refname)], &p.sources);
                }
            }
            RepoEvent::TaskCreated(t) => crate::task::write_created_meta(w, t, at),
            RepoEvent::TaskUpdated { session, pr, summary, .. } => {
                crate::task::write_updated_meta(w, session, *pr, summary.as_deref())
            }
            RepoEvent::PathSet(p) => put_rec(w, "path_set", vec![ref_key(&p.refname)], p),
            RepoEvent::Watchers(us) => us.iter().for_each(|u| crate::watch::write_meta(w, u)),
            RepoEvent::WorkflowJob(u) => put_newer(
                w,
                KIND_WORKFLOW_JOB,
                vec![int(u.job.0)],
                at,
                u,
                |u: &crate::workflow_job::WorkflowJobUpdate| u.action as u64,
            ),
            RepoEvent::Split(x) => crate::split::write_meta(w, x),
            // An added assignee's row keeps who added it (`assigned_by`); an `IssueOpened` assignee's row is empty.
            RepoEvent::Assigned { n, add, remove, actor } => {
                for a in remove {
                    w.delete("issue_assignee", &[n_key(*n), int(a.0.0)]);
                }
                for a in add {
                    put_rec(w, "issue_assignee", vec![n_key(*n), int(a.0.0)], actor);
                }
                touch(w, *n, &|_| {});
            }
            RepoEvent::TaskResumed { session, snapshot } => crate::task::write_resumed_meta(w, session, *snapshot),
            RepoEvent::Milestones(m) => match m {
                MilestoneEvent::Defined { n, id, title, description, open, due_on, actor } => {
                    let was = milestone(w, *n);
                    if was.is_none() {
                        assigned(w, ev, Tier::Metadata, *id);
                        let last = (*n).max(milestone_last(w));
                        put_rec(w, "milestone_last", Vec::new(), &last);
                    }
                    let closed = match (&was, open) {
                        (_, true) => None,
                        (Some(MilestoneRow { closed: Some(c), .. }), false) => Some(*c),
                        (_, false) => Some(at),
                    };
                    let row = MilestoneRow {
                        id: *id,
                        title: title.clone(),
                        description: description.clone(),
                        open: *open,
                        due_on: *due_on,
                        creator: was.as_ref().map_or(*actor, |r| r.creator),
                        created: was.as_ref().map_or(at, |r| r.created),
                        updated: at,
                        closed,
                    };
                    put_rec(w, "milestone", vec![int(i64::from(*n))], &row);
                }
                MilestoneEvent::Deleted { n } => {
                    w.delete("milestone", &[int(i64::from(*n))]);
                    for (k, _) in w.prefix("milestone_issue", &[int(i64::from(*n))]) {
                        if let Some(Val::Int(i)) = k.get(2) {
                            w.delete("issue_milestone", &[int(*i)]);
                        }
                        w.delete("milestone_issue", &k);
                    }
                }
                MilestoneEvent::Set { issue: n, milestone: to, .. } => {
                    let Some(row) = issue(w, *n) else { return };
                    if let Some(from) = issue_milestone(w, *n) {
                        w.delete("milestone_issue", &milestone_issue_key(from, row.open, *n));
                        w.delete("issue_milestone", &[n_key(*n)]);
                    }
                    if let Some(m) = to {
                        put_rec(w, "issue_milestone", vec![n_key(*n)], m);
                        mark(w, "milestone_issue", milestone_issue_key(*m, row.open, *n));
                    }
                    touch(w, *n, &|_| {});
                }
            },
            _ => {}
        }
    }
}

/// The `milestone_issue` key: the milestone, the issue's state, and its number.
fn milestone_issue_key(m: u32, open: bool, n: IssueNumber) -> Vec<Val> {
    vec![int(i64::from(m)), int(i64::from(open)), n_key(n)]
}

/// Writes a check row unless the stored one has a version at or above it, so a repeat or a late older update
/// changes nothing.
fn put_newer<T: Clone + Serialize + DeserializeOwned>(
    w: &mut dyn Rows,
    kind: i64,
    key: Vec<Val>,
    at: Timestamp,
    rec: &T,
    version: impl Fn(&T) -> u64,
) {
    if let Some(old) = get_rec::<Timed<T>>(w, check_table(kind), &key)
        && version(&old.rec) >= version(rec)
    {
        return;
    }
    put_timed(w, kind, key, at, rec.clone());
}

pub fn grantee_key(g: &Grantee) -> Val {
    match g {
        Grantee::User(u) => Val::Text(format!("user:{}", u.0.0.0).into()),
        Grantee::Team(t) => Val::Text(format!("team:{}", t.0.0).into()),
    }
}

/// A pull request's `AgentMeta`, if an `AgentApp` opened it.
pub fn agent_meta(r: &(impl Read + ?Sized), n: IssueNumber) -> Option<crate::agent::AgentMeta> {
    get_rec(r, "agent_meta", &[n_key(n)])
}

/// The repository's agent policy, or the default when none was set.
pub fn agent_policy(r: &(impl Read + ?Sized)) -> crate::agent::AgentPolicy {
    get_rec(r, "agent_policy", &[]).unwrap_or_default()
}

/// The users whose approval wrote `AgentGate` for pull request `n` at `head`.
pub fn agent_gates(r: &(impl Read + ?Sized), n: IssueNumber, head: &ObjectId) -> Vec<graftyard_identity::UserId> {
    r.prefix("agent_gate", &[n_key(n), sha_key(head)])
        .into_iter()
        .filter_map(|(k, _)| match k.get(2) {
            Some(Val::Int(u)) => Some(graftyard_identity::UserId(AccountId(DbId(*u)))),
            _ => None,
        })
        .collect()
}

/// The `issue_order` key: kind, open, and the number negated, so a forward scan lists the newest first.
pub fn order_key(kind: IssueKind, open: bool, n: IssueNumber) -> Vec<Val> {
    vec![int(kind as i64), int(i64::from(open)), int(-i64::from(n.0))]
}

/// The issue or pull request a comment counts toward.
pub(crate) fn target_issue(on: &CommentTarget) -> Option<IssueNumber> {
    match on {
        CommentTarget::Issue(n) | CommentTarget::Review { n, .. } => Some(*n),
        CommentTarget::Commit { .. } => None,
    }
}

impl Tiered for RepoTiers {
    type Event = RepoEvent;

    /// `ExtendLeases`: every lease with `expires` after `since` moves by `by` (Core only).
    fn on_extend(&self, tier: Tier, since: Timestamp, by: std::time::Duration, w: &mut dyn Rows) {
        if tier == Tier::Core {
            crate::lease::extend(w, since, by);
        }
    }

    fn tier_of(ev: &RepoEvent) -> TierSet {
        tier_of(ev)
    }

    fn schema(tier: Tier) -> Schema {
        match tier {
            Tier::Core => CORE,
            Tier::Metadata => METADATA,
        }
    }

    fn write(&self, ev: &RepoEvent, tier: Tier, at: Timestamp, format: Format, rows: &mut dyn Rows) {
        match tier {
            Tier::Core => self.write_core(ev, at, format, rows),
            Tier::Metadata => self.write_meta(ev, at, rows),
        }
    }

    /// Each tier's cursor moves to its own half of the block.
    fn id_block(&self, block: IdBlock, tier: Tier, w: &mut dyn Rows) {
        if let Some(half) = tier_block(block, tier) {
            let mut c = ids(w);
            c.apply_block(half);
            put_rec(w, "ids", Vec::new(), &c);
        }
    }

    /// Core: drops leases that expired at or before `at` (`crate::lease::prune`). Metadata: check retention.
    fn on_entry(&self, tier: Tier, at: Timestamp, w: &mut dyn Rows) {
        if tier != Tier::Metadata {
            crate::lease::prune(w, at);
            return;
        }
        // Metadata: deletes every check run, suite, and status whose time plus the retention is before `at`.
        let keep = u64::from(retention_days(w)) * DAY_MS;
        loop {
            let batch = w.scan("check_expiry", &[], 256);
            let mut any = false;
            for (k, _) in &batch {
                let (Some(Val::Int(t)), Some(Val::Int(kind)), Some(Val::Blob(b))) = (k.first(), k.get(1), k.get(2))
                else {
                    panic!("check_expiry key");
                };
                if (*t as u64).saturating_add(keep) >= at.0 {
                    return;
                }
                let key: Vec<Val> = postcard::from_bytes(b).expect("check_expiry key blob");
                w.delete(check_table(*kind), &key);
                w.delete("check_expiry", k);
                any = true;
            }
            if !any || batch.len() < 256 {
                return;
            }
        }
    }
}
