//! The issue, pull request, review, and check model (`design/SHARED.md`, "Issues and pull requests"): its types and
//! the stored formats of the `RepoEvent` payloads that carry them.
//!
//! Each payload encodes by `graftyard_fold::canon`, which is `crates/raft`'s `Enc` rule, so a field's position and an
//! enum variant's declaration order are part of the stored format: add new fields and variants at the end only.
//! An event's actor is the account id, as `RepoEvent::Refs` stores it: SHARED's `Actor` has App, Job, and Runner
//! variants that Milestones 3 and 4 add.

use graftyard_identity::UserId;
use graftyard_types::{AccountId, DbId, RepoId, Timestamp};
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;

use crate::refs::{ObjectId, RefName};

/// An event's actor: the account, or `None` for the host's own bots (`PrSync`, `RunOwner`).
pub type ActorId = Option<AccountId>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct IssueNumber(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum IssueKind {
    Issue,
    PullRequest,
}

/// The pull request's head: a branch of a repository in the base repository's network.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeadSpec {
    pub repo: RepoId,
    pub branch: RefName,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PrState {
    Open { draft: bool },
    Closed { at: Timestamp, final_head: ObjectId },
    Merged { at: Timestamp, by: UserId, merge_commit: ObjectId, final_head: ObjectId },
}

/// Keyed by the SHA pair, so a stale result is visibly stale instead of needing invalidation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mergeability {
    pub base: ObjectId,
    pub head: ObjectId,
    pub result: MergeabilityResult,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MergeabilityResult {
    Clean { merge: ObjectId },
    Conflict { paths: SmallVec<[Box<str>; 4]> },
}

/// SHARED's `Consumer` ("Event fan-out"), with DESIGN's `MergeQueue`, `Counts`, and `Insights`. Each keeps its own
/// cursor per chain. A variant's position is its stored tag: add new ones at the end only.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Consumer {
    Webhooks,
    ActionsTrigger,
    RunOwner,
    PrSync,
    MergeQueue,
    Notify,
    Search,
    Audit,
    Counts,
    Insights,
}

impl Consumer {
    pub fn tag(self) -> i64 {
        self as i64
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MergeMethod {
    Merge,
    Squash,
    Rebase,
}

/// The identity crate's, where an App's record lives.
pub use graftyard_identity::{AppId, Url};
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TeamId(pub DbId);
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct CommentId(pub DbId);
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ReviewId(pub DbId);
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct LabelId(pub DbId);
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct CheckRunId(pub DbId);
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct CheckSuiteId(pub DbId);
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct StatusId(pub DbId);

/// A label name: at most 50 characters [unverified], compared case-insensitively within a repository.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LabelName(pub Box<str>);

pub const TITLE_MAX: usize = 256;
pub const BODY_MAX: usize = 65_536;
pub const LABEL_MAX: usize = 50;

/// Captured at `IssueOpened`, and inside `PrCapture` at `PrOpened`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueCapture {
    pub id: DbId,
    pub author: ActorId,
    pub title: Box<str>,
    pub body: Box<str>,
    pub labels: SmallVec<[LabelId; 4]>,
    pub assignees: SmallVec<[AccountId; 2]>,
}

/// What changed, for a webhook's `changes` [inference: SHARED names `Changes` without fields].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Changes {
    pub title: Option<Box<str>>,
    pub body: Option<Box<str>>,
    pub base: Option<RefName>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrCapture {
    pub head_sha: ObjectId,
    pub base_sha: ObjectId,
    pub state: PrState,
    pub labels: Vec<LabelName>,
    pub issue: IssueCapture,
    pub changes_from: Option<Changes>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StateReason {
    Completed,
    NotPlanned,
    Reopened,
}

/// GitHub's lock reasons.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LockReason {
    OffTopic,
    TooHeated,
    Resolved,
    Spam,
}

/// Metadata tier, plus Core when it changes a pull request's `State` or `Draft`. `State` reopens a closed pull
/// request; closing one is `PrClosed`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum IssueChange {
    Title(Box<str>),
    Body(Box<str>),
    State { open: bool, reason: Option<StateReason> },
    Draft(bool),
    Locked(Option<LockReason>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiffSide {
    Left,
    Right,
}

/// Where a review comment was written. Reads re-anchor it lazily against the current head.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineAnchor {
    pub commit: ObjectId,
    pub path: Box<str>,
    pub line: u32,
    pub start_line: Option<u32>,
    pub side: DiffSide,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommentTarget {
    Issue(IssueNumber),
    Review { n: IssueNumber, review: Option<ReviewId>, at: LineAnchor },
    Commit { sha: ObjectId, at: Option<LineAnchor> },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommentEvent {
    Created { id: CommentId, on: CommentTarget, author: ActorId, body: Box<str>, reply_to: Option<CommentId> },
    Edited { id: CommentId, body: Box<str>, actor: ActorId },
    Deleted { id: CommentId, actor: ActorId },
    Resolved { thread: CommentId, resolved: bool, actor: ActorId },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReviewState {
    Pending,
    Approved,
    ChangesRequested,
    Commented,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Grantee {
    User(UserId),
    Team(TeamId),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReviewEvent {
    Submitted { id: ReviewId, n: IssueNumber, reviewer: ActorId, state: ReviewState, commit: ObjectId, body: Box<str> },
    Dismissed { id: ReviewId, message: Box<str>, actor: ActorId },
    Requested { n: IssueNumber, reviewers: SmallVec<[Grantee; 2]>, removed: bool, actor: ActorId },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LabelEvent {
    Defined { id: LabelId, name: LabelName, color: [u8; 3], description: Option<Box<str>> },
    Deleted { id: LabelId },
    Applied { n: IssueNumber, add: SmallVec<[LabelId; 4]>, remove: SmallVec<[LabelId; 4]>, actor: ActorId },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Pusher {
    User(UserId),
    Team(TeamId),
    App(AppId),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequiredChecks {
    pub strict: bool,
    pub checks: SmallVec<[(Box<str>, Option<AppId>); 4]>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequiredReviews {
    pub count: u8,
    pub dismiss_stale: bool,
    pub code_owners: bool,
    pub last_push: bool,
    pub dismissers: SmallVec<[Pusher; 2]>,
    pub bypass: SmallVec<[Pusher; 2]>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Grouping {
    AllGreen,
    HeadGreen,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeQueueSettings {
    pub check_timeout_minutes: u16,
    pub grouping: Grouping,
    pub max_build: u8,
    pub min_merge: u8,
    pub max_merge: u8,
    pub min_wait_minutes: u16,
    pub method: MergeMethod,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Protection {
    pub checks: Option<RequiredChecks>,
    pub reviews: Option<RequiredReviews>,
    pub push: Option<SmallVec<[Pusher; 4]>>,
    pub enforce_admins: bool,
    pub linear_history: bool,
    pub force_pushes: bool,
    pub deletions: bool,
    pub block_creations: bool,
    pub conversation_resolution: bool,
    pub lock_branch: bool,
    pub signatures: bool,
    pub merge_queue: Option<MergeQueueSettings>,
}

/// Core tier. `protection: None` deletes the rule.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BranchRule {
    pub id: DbId,
    pub pattern: Box<str>,
    pub protection: Option<Protection>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Visibility {
    Public,
    Private,
    Internal,
}

pub const CHECK_RETENTION_DEFAULT: u16 = 90;
pub const CHECK_RETENTION_MAX: u16 = 400;

/// Core tier, and the metadata tier keeps `check_retention_days` for pruning. Every `Settings` entry carries the
/// whole record.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoSettings {
    pub owner: AccountId,
    pub enterprise: Option<AccountId>,
    pub visibility: Visibility,
    pub default_branch: RefName,
    pub archived: bool,
    pub description: Option<Box<str>>,
    pub homepage: Option<Url>,
    pub topics: SmallVec<[Box<str>; 4]>,
    pub has_issues: bool,
    pub has_wiki: bool,
    pub has_projects: bool,
    pub merge_methods: SmallVec<[MergeMethod; 3]>,
    pub delete_branch_on_merge: bool,
    pub allow_auto_merge: bool,
    pub allow_update_branch: bool,
    /// 1..=400, default 90, as GHES 3.22.
    pub check_retention_days: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CheckStatus {
    Queued,
    InProgress,
    Completed,
    Waiting,
    Requested,
    Pending,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CheckConclusion {
    ActionRequired,
    Cancelled,
    Failure,
    Neutral,
    Success,
    Skipped,
    Stale,
    TimedOut,
    StartupFailure,
}

impl CheckConclusion {
    /// Success, Neutral, and Skipped satisfy a required check, as on GitHub [unverified for GHES 3.22].
    pub fn passes(self) -> bool {
        matches!(self, CheckConclusion::Success | CheckConclusion::Neutral | CheckConclusion::Skipped)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AnnotationLevel {
    Notice,
    Warning,
    Failure,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckAnnotation {
    pub path: Box<str>,
    pub start_line: u32,
    pub end_line: u32,
    pub level: AnnotationLevel,
    pub message: Box<str>,
    pub title: Option<Box<str>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckOutput {
    pub title: Box<str>,
    pub summary: Box<str>,
    pub text: Option<Box<str>>,
    pub annotations: Vec<CheckAnnotation>,
}

/// The run's whole state after one create or update. `version` counts updates, and the highest version wins.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckRunUpdate {
    pub run: CheckRunId,
    pub suite: CheckSuiteId,
    pub app: AppId,
    pub head_sha: ObjectId,
    pub name: Box<str>,
    pub version: u64,
    pub status: CheckStatus,
    pub conclusion: Option<CheckConclusion>,
    pub started: Option<Timestamp>,
    pub completed: Option<Timestamp>,
    pub external_id: Option<Box<str>>,
    pub details_url: Option<Url>,
    pub output: Option<CheckOutput>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckSuiteUpdate {
    pub suite: CheckSuiteId,
    pub app: AppId,
    pub head_sha: ObjectId,
    pub head_branch: Option<RefName>,
    pub version: u64,
    pub status: CheckStatus,
    pub conclusion: Option<CheckConclusion>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StatusState {
    Error,
    Failure,
    Pending,
    Success,
}

/// The latest status per (sha, context) is that context's state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusUpdate {
    pub id: StatusId,
    pub sha: ObjectId,
    pub context: Box<str>,
    pub state: StatusState,
    pub target_url: Option<Url>,
    pub description: Option<Box<str>>,
    pub creator: AccountId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum CheckSource {
    Run { app: AppId, run: CheckRunId },
    Status { creator: AccountId },
}

/// The core `pull_head` row: what merges and pruning read.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullHead {
    pub head: HeadSpec,
    pub base: RefName,
    pub head_sha: ObjectId,
    pub state: PrState,
}

// ---- Releases, release assets, and LFS (`design/SHARED.md`, "Event types"; `build/BUILD-PLAN.md`, M2-WP4) ----

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct RepoReleaseId(pub DbId);
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct AssetId(pub DbId);
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct LockId(pub DbId);

/// SHARED's `ReleaseRecord`. `target` is the commit the tag names or will name once a draft is published. `latest` is
/// the client's `make_latest`: of the published releases that are neither drafts nor prereleases and have it set, the
/// newest is the latest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseRecord {
    pub tag: RefName,
    pub target: ObjectId,
    pub name: Box<str>,
    pub body: Box<str>,
    pub draft: bool,
    pub prerelease: bool,
    pub latest: bool,
    pub author: ActorId,
}

/// SHARED's `AssetRecord`. Its `blob` comes only from a `DurableBlob` (`crate::blob`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetRecord {
    pub name: Box<str>,
    pub label: Option<Box<str>>,
    pub content_type: Box<str>,
    pub blob: crate::blob::BlobRef,
}

/// SHARED's `LfsLock(LfsLockEvent)`: a lock on one path, which `git lfs lock` takes and `git lfs unlock` releases.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LfsLockEvent {
    Created { id: LockId, path: Box<str>, refspec: Option<RefName>, owner: AccountId },
    Deleted { id: LockId, actor: ActorId },
}
