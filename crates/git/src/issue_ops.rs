//! The issue, pull request, review, label, rule, settings, and check commands of the repository chain
//! (`design/SHARED.md`, "Issues and pull requests"; `build/BUILD-PLAN.md`, M2-WP2).
//!
//! `IssueCmd::decide` is pure. It reads a `RepoView`: both tiers at `applied`, with the earlier commands' events of the
//! same entry written on top by the same code `apply` runs, plus the credential and owner `Authz` folds the leader read
//! once through `fresh`. `record_check` and `record_status` are `CheckOp`s: one `CheckRun` or `Status` event, plus a
//! `RequiredCheck` when a rule requires the check and the fresh core tier shows its SHA as the head of an open pull
//! request, a merge-queue ref, or a protected branch tip.

pub mod call;

use graftyard_fold::{Overlay, Read, Tiered, Val, has};
use graftyard_identity::{
    Access as AppAccess, Actor, AuthzRead, Credential, CredentialRefusal, Decision, INVITATION_TTL, IdentityDep,
    InvitationEvent, InvitationGrant, InvitationId, InvitationRow, Invitee, Own, Principal, RepoAccess, RepoPermission,
    UserId, check_credential, credential_chains,
};
use graftyard_raft::TierSet;
use graftyard_types::{AccountId, ChainId, DbId, RequestId, Tier, Timestamp};
use smallvec::{SmallVec, smallvec};

use crate::blob::DurableBlob;
use crate::issues::{AssetId, AssetRecord, LfsLockEvent, LockId, ReleaseRecord, RepoReleaseId};
use crate::issues::{
    BODY_MAX, BranchRule, CHECK_RETENTION_MAX, CheckConclusion, CheckRunUpdate, CheckSource, CheckStatus, CommentEvent,
    CommentId, CommentTarget, Grantee, HeadSpec, IssueCapture, IssueChange, IssueNumber, LABEL_MAX, LabelEvent,
    LabelId, LabelName, PrCapture, PrState, Protection, RepoSettings, ReviewEvent, ReviewId, ReviewState, StatusId,
    StatusState, StatusUpdate, TITLE_MAX, Visibility,
};
use crate::refs::{ObjectId, RefName, RefUpdate};
use crate::repo::RepoEvent;
use crate::tiers::{
    AssetRow, CommentRow, IssueRow, LabelRow, LockRow, RepoTiers, get_rec, ids, int, n_key, pull_head, pulls_from,
    ref_key, release_by_tag, release_row, sha_key, tier_of,
};

/// Both tiers at `applied`, with an entry's earlier events on top: what `decide` and `verify` read.
pub struct RepoView<'a> {
    tiers: RepoTiers,
    at: Timestamp,
    core: Overlay<'a, dyn Read + 'a>,
    meta: Overlay<'a, dyn Read + 'a>,
}

impl<'a> RepoView<'a> {
    /// `at` is the entry's time, which the earlier events' writes stamp.
    pub fn new(tiers: RepoTiers, core: &'a dyn Read, meta: &'a dyn Read, at: Timestamp) -> RepoView<'a> {
        RepoView { tiers, at, core: Overlay::new(core), meta: Overlay::new(meta) }
    }

    /// Writes an earlier command's event, so the next command decides on it.
    pub fn apply(&mut self, ev: &RepoEvent) {
        let set = tier_of(ev);
        if has(set, Tier::Core) {
            self.tiers.write(ev, Tier::Core, self.at, crate::repo::SCRATCH_FORMAT, &mut self.core);
        }
        if has(set, Tier::Metadata) {
            self.tiers.write(ev, Tier::Metadata, self.at, crate::repo::SCRATCH_FORMAT, &mut self.meta);
        }
    }

    pub fn core(&self) -> &dyn Read {
        &self.core
    }

    /// The chain's own repository.
    pub fn repo(&self) -> graftyard_types::RepoId {
        self.tiers.repo
    }

    pub fn meta(&self) -> &dyn Read {
        &self.meta
    }

    /// The core `settings` row.
    pub fn settings(&self) -> Option<RepoSettings> {
        get_rec(&self.core, "settings", &[])
    }

    /// The owner whose `Authz` chain gates the repository: `RepoSettings.owner`, or the chain's owner before the first
    /// `Settings` entry.
    pub fn owner(&self) -> AccountId {
        self.settings().map_or(self.tiers.owner, |s| s.owner)
    }

    /// The highest issue number used, from the core counter.
    pub fn last_number(&self) -> u32 {
        get_rec(&self.core, "counter", &[]).unwrap_or(0)
    }

    pub fn ref_value(&self, name: &RefName) -> Option<ObjectId> {
        get_rec(&self.core, "ref", &[ref_key(name)])
    }

    /// The next free id of `tier`'s own cursor: Core for an event that touches Core, Metadata otherwise
    /// (`crate::tiers::id_tier`).
    fn next_id(&self, tier: Tier) -> Result<DbId, IssueErr> {
        let c = if tier == Tier::Core { ids(&self.core) } else { ids(&self.meta) };
        c.peek().ok_or(IssueErr::NeedIdBlock)
    }

    fn rules(&self) -> Vec<BranchRule> {
        self.core.prefix("rule", &[]).into_iter().filter_map(|(k, _)| get_rec(&self.core, "rule", &k)).collect()
    }
}

/// What one command asks for. `B` is `DurableBlob` where a command is decided, and `BlobRef` on the wire to a member,
/// which rebuilds the `DurableBlob` with `BlobStore::confirm` before it admits the command (`map_blob`).
/// Wire tags: postcard stores and sends the variants by position, JSON by name (`crates/git/tests/wire_tags.golden`).
/// Add a variant only at the end: never insert, remove, reorder, or rename one.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum IssueOp<B = DurableBlob> {
    OpenIssue {
        title: Box<str>,
        body: Box<str>,
        labels: SmallVec<[LabelId; 4]>,
        assignees: SmallVec<[AccountId; 2]>,
    },
    /// `head_sha` and `base_sha` are the refs the edge read fresh, which `decide` checks for a same-repository head.
    OpenPr {
        head: HeadSpec,
        base: RefName,
        title: Box<str>,
        body: Box<str>,
        draft: bool,
        head_sha: ObjectId,
        base_sha: ObjectId,
    },
    Close {
        n: IssueNumber,
    },
    Change {
        n: IssueNumber,
        change: IssueChange,
    },
    Comment {
        on: CommentTarget,
        body: Box<str>,
        reply_to: Option<CommentId>,
    },
    EditComment {
        id: CommentId,
        body: Box<str>,
    },
    DeleteComment {
        id: CommentId,
    },
    Resolve {
        thread: CommentId,
        resolved: bool,
    },
    Review {
        n: IssueNumber,
        state: ReviewState,
        commit: ObjectId,
        body: Box<str>,
    },
    Dismiss {
        review: ReviewId,
        message: Box<str>,
    },
    RequestReviews {
        n: IssueNumber,
        reviewers: SmallVec<[Grantee; 2]>,
        removed: bool,
    },
    DefineLabel {
        id: Option<LabelId>,
        name: LabelName,
        color: [u8; 3],
        description: Option<Box<str>>,
    },
    DeleteLabel {
        id: LabelId,
    },
    ApplyLabels {
        n: IssueNumber,
        add: SmallVec<[LabelId; 4]>,
        remove: SmallVec<[LabelId; 4]>,
    },
    /// `id: None` creates a rule; `protection: None` deletes one.
    SetRule {
        id: Option<DbId>,
        pattern: Box<str>,
        protection: Option<Protection>,
    },
    Settings(RepoSettings),
    /// SHARED's `record_check`.
    RecordCheck(CheckRunUpdate),
    /// SHARED's `record_status`.
    RecordStatus(StatusUpdate),
    /// Creates (`id: None`) or edits a release. `target` is the commit the edge resolved `target_commitish` to; when
    /// the tag already exists its value wins. Publishing a release whose tag does not exist creates the tag in the same
    /// entry, at `target`.
    PutRelease {
        id: Option<RepoReleaseId>,
        tag: RefName,
        target: ObjectId,
        name: Box<str>,
        body: Box<str>,
        draft: bool,
        prerelease: bool,
        latest: bool,
    },
    DeleteRelease {
        id: RepoReleaseId,
    },
    /// An uploaded asset. It takes a `DurableBlob`, which only `BlobStore::place` builds, so the 201 that renders from
    /// this command's `&Acked` comes after two buckets hold the bytes (BF-25).
    PutAsset {
        release: RepoReleaseId,
        name: Box<str>,
        label: Option<Box<str>>,
        content_type: Box<str>,
        blob: B,
    },
    DeleteAsset {
        id: AssetId,
    },
    /// An LFS object the transfer placed: the `LfsObject` row that `verify` and a download batch read.
    LfsObject(B),
    LockLfs {
        path: Box<str>,
        refspec: Option<RefName>,
    },
    /// `force` releases another account's lock, which needs `Admin` [guess: GitHub documents force unlock for
    /// repository admins].
    UnlockLfs {
        id: LockId,
        force: bool,
    },
    // ---- M3-WP3 ----
    /// `PUT /repos/{o}/{r}/collaborators/{user}` for a user who is not a member of an org owner: a repository
    /// `Invitation`. A member's permission is a `Grant` on the org's `Authz` chain, so this is refused for one.
    InviteCollaborator {
        user: AccountId,
        permission: graftyard_identity::RepoPermission,
    },
    /// The invitee answers. Accepting appends `Collaborator` in the same entry.
    AnswerInvitation {
        id: graftyard_identity::InvitationId,
        accept: bool,
    },
    /// `DELETE /repos/{o}/{r}/collaborators/{user}`: removes the outside collaborator's row.
    RemoveCollaborator {
        user: AccountId,
    },
    // ---- AM1-WP2 ----
    /// `PUT /api/graftyard/v1/repos/{o}/{r}/agents/policy`: the repository's whole agent policy, admin only.
    SetAgentPolicy(crate::agent::AgentPolicy),
    // ---- format 32.11 ----
    /// `DELETE /repos/{o}/{r}/invitations/{id}`: a repository admin withdraws a pending invitation, as `Cancelled`.
    CancelInvitation {
        id: graftyard_identity::InvitationId,
    },
    // ---- AM1-WP3, format 32.12 (`crate::task`) ----
    /// `POST /api/graftyard/v1/repos/{o}/{r}/tasks` with `kind: external`, by an `AgentApp`'s installation token.
    CreateTask(crate::task::NewTask),
    /// `PATCH .../tasks/{id}`, by the task's App.
    UpdateTask {
        id: graftyard_identity::SessionId,
        state: Option<crate::task::TaskApiState>,
        pr: Option<IssueNumber>,
        summary: Option<Box<str>>,
    },
    /// `POST .../tasks/{id}/cancel`, by the requester, a repository admin, or the task's App.
    CancelTask {
        id: graftyard_identity::SessionId,
    },
    // ---- format 32.13 (`crate::issue_ops::call::REPO_FORMAT_REPOS`) ----
    /// `POST /orgs/{org}/repos` and `POST /user/repos`, after the name's `CreateRepo` committed on the owner's `Authz`
    /// chain: the repository's first settings and its creation time, once.
    InitRepo(RepoSettings),
    /// `DELETE /repos/{o}/{r}`: the chain's `Deleted` tombstone, kept until `retain_until`, after which the chain
    /// refuses every write. A repeat on a deleted chain commits nothing, so the edge can finish the name's
    /// `DeleteRepo` after a crash.
    DeleteRepo {
        retain_until: Timestamp,
    },
    // ---- AM1-WP4, format 32.14 (`crate::lease`) ----
    /// `POST /api/graftyard/v1/repos/{o}/{r}/leases`.
    AcquireLease(crate::lease::NewLease),
    /// `POST .../leases/{id}/renew`, by the holder.
    RenewLease {
        id: crate::lease::LeaseId,
        ttl_seconds: Option<u32>,
    },
    /// `DELETE .../leases/{id}`, by the holder or a repository admin.
    ReleaseLease {
        id: crate::lease::LeaseId,
    },
    // ---- M4-WP1, format 32.16 (`crate::hooks`) ----
    /// `POST /repos/{o}/{r}/hooks` (`id: None`) and `PATCH .../hooks/{id}`: the whole record, its secret already
    /// wrapped by `graft-ops`. Admin only.
    PutHook {
        id: Option<crate::hooks::HookId>,
        record: crate::hooks::HookRecord,
    },
    /// `DELETE /repos/{o}/{r}/hooks/{id}`.
    DeleteHook {
        id: crate::hooks::HookId,
    },
    // ---- format 32.18 (`call::REPO_FORMAT_CHECKS`) ----
    /// `POST /repos/{o}/{r}/check-runs`: a new run, version 1, by an App's installation token. `decide` gives it its
    /// run id, and the same id as its suite's, from the metadata tier's `IdBlock` cursor, so whatever ids `run` and
    /// `suite` carry here are ignored.
    CreateCheck(CheckRunUpdate),
    // ---- M4-WP3, format 32.23 (`crate::secrets`) ----
    /// `PUT /repos/{o}/{r}/actions/secrets/{name}` and `PUT .../environments/{env}/secrets/{name}`: the sealed bytes
    /// the client sent, under the owner's key `key_id`. Admin only.
    PutActionsSecret {
        scope: graftyard_identity::sealed::SecretScope,
        name: graftyard_identity::sealed::ActionsName,
        sealed: Box<[u8]>,
        key_id: Box<str>,
    },
    DeleteActionsSecret {
        scope: graftyard_identity::sealed::SecretScope,
        name: graftyard_identity::sealed::ActionsName,
    },
    /// `POST .../actions/variables` (`create: true`, refused for an existing name) and `PATCH .../variables/{name}`
    /// (`create: false`, refused for a missing one), and the environment routes.
    PutActionsVariable {
        scope: graftyard_identity::sealed::SecretScope,
        name: graftyard_identity::sealed::ActionsName,
        value: Box<str>,
        create: bool,
    },
    DeleteActionsVariable {
        scope: graftyard_identity::sealed::SecretScope,
        name: graftyard_identity::sealed::ActionsName,
    },
    // ---- AM3-WP3, format 32.28 (`crate::repo::REPO_FORMAT_RUNS`) ----
    /// `POST /api/graftyard/v1/repos/{o}/{r}/tasks` without `kind`: a Graftyard-run task, by a human writer
    /// (`crate::task::decide`).
    CreateRun(crate::task::NewRun),
    // ---- F20, format 32.30 (`call::REPO_FORMAT_ASSIGN`) ----
    /// REST `POST`/`DELETE .../issues/{n}/assignees`, a `PATCH` of the issue's `assignees`, and GraphQL
    /// `addAssigneesToAssignable`/`removeAssigneesFromAssignable`: accounts to add and remove, each a user or an App's
    /// bot account (`AppId::bot`). `decide` drops what the caller may not change, as GHES 3.22 does.
    Assign {
        n: IssueNumber,
        add: SmallVec<[AccountId; 2]>,
        remove: SmallVec<[AccountId; 2]>,
    },
}

impl<B> IssueOp<B> {
    /// The blob the command names, if any.
    pub fn blob(&self) -> Option<&B> {
        match self {
            IssueOp::PutAsset { blob, .. } | IssueOp::LfsObject(blob) => Some(blob),
            _ => None,
        }
    }

    /// The same command over another blob type: `f` runs on its blob, if it names one.
    pub fn map_blob<C, E>(self, f: impl FnOnce(B) -> Result<C, E>) -> Result<IssueOp<C>, E> {
        use IssueOp as O;
        Ok(match self {
            O::PutAsset { release, name, label, content_type, blob } => {
                O::PutAsset { release, name, label, content_type, blob: f(blob)? }
            }
            O::LfsObject(b) => O::LfsObject(f(b)?),
            O::OpenIssue { title, body, labels, assignees } => O::OpenIssue { title, body, labels, assignees },
            O::OpenPr { head, base, title, body, draft, head_sha, base_sha } => {
                O::OpenPr { head, base, title, body, draft, head_sha, base_sha }
            }
            O::Close { n } => O::Close { n },
            O::Change { n, change } => O::Change { n, change },
            O::Comment { on, body, reply_to } => O::Comment { on, body, reply_to },
            O::EditComment { id, body } => O::EditComment { id, body },
            O::DeleteComment { id } => O::DeleteComment { id },
            O::Resolve { thread, resolved } => O::Resolve { thread, resolved },
            O::Review { n, state, commit, body } => O::Review { n, state, commit, body },
            O::Dismiss { review, message } => O::Dismiss { review, message },
            O::RequestReviews { n, reviewers, removed } => O::RequestReviews { n, reviewers, removed },
            O::DefineLabel { id, name, color, description } => O::DefineLabel { id, name, color, description },
            O::DeleteLabel { id } => O::DeleteLabel { id },
            O::ApplyLabels { n, add, remove } => O::ApplyLabels { n, add, remove },
            O::SetRule { id, pattern, protection } => O::SetRule { id, pattern, protection },
            O::Settings(s) => O::Settings(s),
            O::RecordCheck(c) => O::RecordCheck(c),
            O::RecordStatus(s) => O::RecordStatus(s),
            O::PutRelease { id, tag, target, name, body, draft, prerelease, latest } => {
                O::PutRelease { id, tag, target, name, body, draft, prerelease, latest }
            }
            O::DeleteRelease { id } => O::DeleteRelease { id },
            O::DeleteAsset { id } => O::DeleteAsset { id },
            O::LockLfs { path, refspec } => O::LockLfs { path, refspec },
            O::UnlockLfs { id, force } => O::UnlockLfs { id, force },
            O::InviteCollaborator { user, permission } => O::InviteCollaborator { user, permission },
            O::AnswerInvitation { id, accept } => O::AnswerInvitation { id, accept },
            O::RemoveCollaborator { user } => O::RemoveCollaborator { user },
            O::SetAgentPolicy(p) => O::SetAgentPolicy(p),
            O::CancelInvitation { id } => O::CancelInvitation { id },
            O::CreateTask(t) => O::CreateTask(t),
            O::UpdateTask { id, state, pr, summary } => O::UpdateTask { id, state, pr, summary },
            O::CancelTask { id } => O::CancelTask { id },
            O::InitRepo(s) => O::InitRepo(s),
            O::DeleteRepo { retain_until } => O::DeleteRepo { retain_until },
            O::AcquireLease(l) => O::AcquireLease(l),
            O::RenewLease { id, ttl_seconds } => O::RenewLease { id, ttl_seconds },
            O::ReleaseLease { id } => O::ReleaseLease { id },
            O::PutHook { id, record } => O::PutHook { id, record },
            O::DeleteHook { id } => O::DeleteHook { id },
            O::CreateCheck(c) => O::CreateCheck(c),
            O::PutActionsSecret { scope, name, sealed, key_id } => O::PutActionsSecret { scope, name, sealed, key_id },
            O::DeleteActionsSecret { scope, name } => O::DeleteActionsSecret { scope, name },
            O::PutActionsVariable { scope, name, value, create } => {
                O::PutActionsVariable { scope, name, value, create }
            }
            O::DeleteActionsVariable { scope, name } => O::DeleteActionsVariable { scope, name },
            O::CreateRun(n) => O::CreateRun(n),
            O::Assign { n, add, remove } => O::Assign { n, add, remove },
        })
    }

    /// Whether this is a lease command (`crate::lease::decide`).
    pub fn is_lease(&self) -> bool {
        matches!(self, IssueOp::AcquireLease(_) | IssueOp::RenewLease { .. } | IssueOp::ReleaseLease { .. })
    }

    /// Whether this is a secret or variable command, format 32.23.
    pub fn is_secret(&self) -> bool {
        matches!(
            self,
            IssueOp::PutActionsSecret { .. }
                | IssueOp::DeleteActionsSecret { .. }
                | IssueOp::PutActionsVariable { .. }
                | IssueOp::DeleteActionsVariable { .. }
        )
    }

    /// Whether this is a hook command, format 32.16.
    pub fn is_hook(&self) -> bool {
        matches!(self, IssueOp::PutHook { .. } | IssueOp::DeleteHook { .. })
    }

    /// Whether this is a task command (`crate::task::decide`), which never counts toward DEMOTE_PER_MIN.
    pub fn is_task(&self) -> bool {
        matches!(
            self,
            IssueOp::CreateTask(_) | IssueOp::UpdateTask { .. } | IssueOp::CancelTask { .. } | IssueOp::CreateRun(_)
        )
    }
}

/// A command on the repository chain. `request` is the edge's id for the client request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IssueCmd {
    pub op: IssueOp,
    pub actor: Principal,
    pub request: RequestId,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IssueOut {
    Number(IssueNumber),
    Id(DbId),
    Done,
    /// A task command applied: the task's id, whose row the leader renders.
    Task(graftyard_identity::SessionId),
    /// A task command found the task in a derived final state (its installation gone, or TASK_IDLE passed) and
    /// recorded it, and changed nothing else: the client gets 409 with that state.
    TaskFinal(graftyard_identity::SessionId, crate::task::TaskApiState),
    /// A lease acquire or renew applied: the lease's id, whose row the leader renders.
    Lease(crate::lease::LeaseId),
    /// A secret or variable that had no row was written: GHES answers 201, and 204 for an update.
    Created,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IssueErr {
    Unauthorized(CredentialRefusal),
    Forbidden,
    NotFound,
    /// The pull request is closed or merged.
    NotOpen,
    NotPull,
    Invalid(&'static str),
    /// The group holds no free id: the host leases a block and retries.
    NeedIdBlock,
    /// A pull request from this head to this base is already open.
    Exists,
    HeadMissing,
    BaseMissing,
    HeadMoved,
    UnknownLabel(LabelId),
    /// GitHub's "Can not approve your own pull request".
    OwnApproval,
    /// `owner` and `enterprise` change only by a transfer or an enterprise change.
    OwnerChange,
    /// The user is a member of the org that owns the repository: their permission is a `Grant` on its `Authz` chain.
    OrgMember,
    /// The member's metadata tier is still loading (`RepoScratch::view`), or the blob the command names is not in two
    /// buckets: the client retries.
    Unavailable,
    /// An agent rule (`crate::agent`). GitHub's 403 shape, with a text that names the rule.
    Agent(crate::agent::AgentRefusal),
    /// A task command's refusal (`crate::task::TaskErr`).
    Task(crate::task::TaskErr),
    /// A lease command's refusal (`crate::lease::LeaseErr`).
    Lease(crate::lease::LeaseErr),
}

fn actor_id(p: &Principal) -> Option<AccountId> {
    match p.actor {
        Actor::User(u) => Some(u.0),
        Actor::App { .. } | Actor::Anonymous => None,
    }
}

fn chars(s: &str) -> usize {
    s.chars().count()
}

impl IssueCmd {
    pub fn tiers(&self) -> TierSet {
        let both = TierSet::CORE.union(TierSet::METADATA);
        match &self.op {
            IssueOp::OpenIssue { .. } | IssueOp::OpenPr { .. } | IssueOp::Close { .. } | IssueOp::Settings(_) => both,
            IssueOp::Change { change: IssueChange::State { .. } | IssueChange::Draft(_), .. } => both,
            IssueOp::Review { .. } | IssueOp::Dismiss { .. } => both,
            IssueOp::SetRule { .. } => TierSet::CORE,
            IssueOp::RecordCheck(_) | IssueOp::CreateCheck(_) | IssueOp::RecordStatus(_) => both,
            // A release may create its tag in Core.
            IssueOp::PutRelease { .. } => both,
            // A new review thread, a resolution, or a deletion may change the core unresolved count; a review comment
            // outside a review submits its single-comment review, whose verdict is Core's.
            IssueOp::Comment { on: CommentTarget::Review { .. }, reply_to: None, .. } => both,
            IssueOp::Comment { on: CommentTarget::Review { review: None, .. }, .. } => both,
            IssueOp::Resolve { .. } | IssueOp::DeleteComment { .. } => both,
            IssueOp::InviteCollaborator { .. }
            | IssueOp::AnswerInvitation { .. }
            | IssueOp::RemoveCollaborator { .. }
            | IssueOp::CancelInvitation { .. } => TierSet::CORE,
            IssueOp::SetAgentPolicy(_) | IssueOp::DeleteRepo { .. } => TierSet::CORE,
            IssueOp::PutHook { .. } | IssueOp::DeleteHook { .. } => TierSet::CORE,
            IssueOp::PutActionsSecret { .. }
            | IssueOp::DeleteActionsSecret { .. }
            | IssueOp::PutActionsVariable { .. }
            | IssueOp::DeleteActionsVariable { .. } => TierSet::CORE,
            // `Settings` writes the metadata tier's `retention` too.
            IssueOp::InitRepo(_) => both,
            // A task's core row changes on every task command, and its metadata row on a create, a `pr`, or a summary.
            IssueOp::CreateTask(_)
            | IssueOp::UpdateTask { .. }
            | IssueOp::CancelTask { .. }
            | IssueOp::CreateRun(_) => both,
            IssueOp::AcquireLease(_) | IssueOp::RenewLease { .. } | IssueOp::ReleaseLease { .. } => TierSet::CORE,
            // A body edit may replace a plan-block lease (Core).
            IssueOp::Change { change: IssueChange::Body(_), .. } => both,
            _ => TierSet::METADATA,
        }
    }

    /// The actor's credential chains and the owner's `Authz`, named by the core `settings` row.
    pub fn deps(&self, view: &RepoView<'_>) -> SmallVec<[ChainId; 3]> {
        let mut d = credential_chains(&self.actor.credential_shard(), None);
        d.extend(crate::repo::authz_chains(Some(view.owner()), view.settings().and_then(|s| s.enterprise)));
        crate::repo::with_install(&mut d, &self.actor);
        let install = ChainId::Authz(graftyard_identity::install_account());
        if matches!(self.op, IssueOp::Assign { .. }) && !d.contains(&install) {
            d.push(install);
        }
        d
    }

    /// Pure: the view, and the deps the leader read once per entry. Writes no agent event, as a command of format
    /// 32.5 or 32.6 must not (`decide_in`).
    pub fn decide(
        &self,
        v: &RepoView<'_>,
        deps: &[(ChainId, IdentityDep)],
    ) -> Result<(SmallVec<[RepoEvent; 4]>, IssueOut), IssueErr> {
        self.decide_in(v, deps, false, None, false, false, false)
    }

    /// `decide`, where `agent` says the command is format 32.8 (`crate::repo::REPO_FORMAT_AGENT`), so its entry may
    /// carry `AgentMeta`, `AgentGate`, and `AgentPolicy`. The agent refusals write nothing and apply in any format.
    /// `reviews` says it is format 32.22 (`call::REPO_FORMAT_REVIEWS`), so a review comment outside a review submits
    /// its review as `ReviewEvent::SingleComment`. `bot_author` says it is format 32.29 (`call::REPO_FORMAT_ROUTES`), so
    /// an App's `OpenIssue` is authored by its bot account (`AppId::bot`).
    #[expect(clippy::too_many_arguments, reason = "each flag is one format the call's entry already decided")]
    pub fn decide_in(
        &self,
        v: &RepoView<'_>,
        deps: &[(ChainId, IdentityDep)],
        agent: bool,
        session: Option<&graftyard_identity::SessionId>,
        leases: bool,
        reviews: bool,
        bot_author: bool,
    ) -> Result<(SmallVec<[RepoEvent; 4]>, IssueOut), IssueErr> {
        let cred = self.actor.credential_shard();
        let app = match (&self.actor.actor, &self.actor.credential) {
            (Actor::App { .. }, Credential::Stateless(c)) => Some(c),
            _ => None,
        };
        if cred.key.is_none() || (cred.account.is_none() && app.is_none()) {
            return Err(IssueErr::Unauthorized(CredentialRefusal::NoToken));
        }
        check_credential(&cred, Own::Other, deps).map_err(IssueErr::Unauthorized)?;
        // An `Agent` is refused before any permission check, so its reply names the rule it broke (agents recover by
        // reading it) rather than GitHub's bare 403.
        let install = crate::repo::install_authz(deps);
        if let crate::agent::ActorKind::Agent(_) = crate::agent::actor_kind(&self.actor, install) {
            agent_refusal(&self.op, v).map_or(Ok(()), |r| Err(IssueErr::Agent(r)))?;
        }
        let owner = v.owner();
        let folds = crate::repo::authz_folds(deps);
        let authz = folds.iter().copied().find(|a| a.owner() == Some(owner));
        // An installation token is authorized once, by the GitHub App permission the command needs, intersected with
        // the installation's current row; past that gate it may do what a role check below would ask of a user.
        let perm = match app {
            Some(c) => {
                let access = Access { repo: v.tiers.repo, owner, core: v.core() };
                match graftyard_identity::authorize_app(c, &self.app_needs(v), &folds, &access) {
                    Decision::Allow => Some(RepoPermission::Admin),
                    Decision::Forbidden => return Err(IssueErr::Forbidden),
                    Decision::Hidden => return Err(IssueErr::NotFound),
                }
            }
            None => {
                // A `github_pat_` or `ghu_` acts only within its own scope and permissions, by the same per-resource
                // names an App's command needs.
                let access = Access { repo: v.tiers.repo, owner, core: v.core() };
                let fields = crate::repo::stored_fields(&self.actor, deps);
                match graftyard_identity::token_allows(fields, &self.app_needs(v), &access) {
                    Decision::Allow => permission(&self.actor, v, &folds),
                    Decision::Forbidden => return Err(IssueErr::Forbidden),
                    Decision::Hidden => return Err(IssueErr::NotFound),
                }
            }
        };
        let me = actor_id(&self.actor);
        let need = |p: RepoPermission| if perm >= Some(p) { Ok(()) } else { Err(IssueErr::Forbidden) };
        let author_or = |author: Option<AccountId>, p: RepoPermission| {
            if author.is_some() && author == me { Ok(()) } else { need(p) }
        };
        let issue = |n: IssueNumber| -> Result<(), IssueErr> {
            if n.0 == 0 || n.0 > v.last_number() { Err(IssueErr::NotFound) } else { Ok(()) }
        };
        let one = |e: RepoEvent| smallvec![e];
        // A deleted repository takes no client write; a repeated delete commits nothing.
        if crate::tiers::deleted(v.core()).is_some() {
            return match &self.op {
                IssueOp::DeleteRepo { .. } => {
                    need(RepoPermission::Admin)?;
                    Ok((SmallVec::new(), IssueOut::Done))
                }
                _ => Err(IssueErr::NotFound),
            };
        }
        match &self.op {
            IssueOp::OpenIssue { title, body, labels, assignees } => {
                need(RepoPermission::Read)?;
                text_ok(title, body)?;
                for l in labels {
                    get_rec::<LabelRow>(v.meta(), "label", &[int(l.0.0)]).ok_or(IssueErr::UnknownLabel(*l))?;
                }
                let n = IssueNumber(v.last_number() + 1);
                // A REST create by an App (32.29) is its bot's, as GHES shows `github-actions[bot]` as `user`; an
                // App's GraphQL `createIssue` (32.9) keeps its author-less bytes.
                let author = match self.actor.actor {
                    Actor::App { app, .. } if bot_author => Some(app.bot()),
                    _ => me,
                };
                let capture = IssueCapture {
                    id: v.next_id(Tier::Core)?,
                    author,
                    title: title.clone(),
                    body: body.clone(),
                    labels: labels.clone(),
                    assignees: assignees.clone(),
                };
                Ok((one(RepoEvent::IssueOpened { n, capture }), IssueOut::Number(n)))
            }
            IssueOp::OpenPr { head, base, title, body, draft, head_sha, base_sha } => {
                need(RepoPermission::Read)?;
                text_ok(title, body)?;
                if v.ref_value(base) != Some(*base_sha) {
                    return Err(IssueErr::BaseMissing);
                }
                if head.repo == v.tiers.repo {
                    match v.ref_value(&head.branch) {
                        None => return Err(IssueErr::HeadMissing),
                        Some(o) if o != *head_sha => return Err(IssueErr::HeadMoved),
                        Some(_) => {}
                    }
                    if head.branch == *base {
                        return Err(IssueErr::Invalid("head and base are the same branch"));
                    }
                }
                let open = pulls_from(v.core(), head.repo, &head.branch);
                if open.iter().any(|n| pull_head(v.core(), *n).is_some_and(|p| p.base == *base)) {
                    return Err(IssueErr::Exists);
                }
                let n = IssueNumber(v.last_number() + 1);
                let issue = IssueCapture {
                    id: v.next_id(Tier::Core)?,
                    author: me,
                    title: title.clone(),
                    body: body.clone(),
                    labels: SmallVec::new(),
                    assignees: SmallVec::new(),
                };
                let capture = PrCapture {
                    head_sha: *head_sha,
                    base_sha: *base_sha,
                    state: PrState::Open { draft: *draft },
                    labels: Vec::new(),
                    issue,
                    changes_from: None,
                };
                let ev = RepoEvent::PrOpened { n, head: head.clone(), base: base.clone(), capture };
                let mut evs = one(ev);
                // `AgentMeta` when the creating token belongs to an `AgentApp`. `session` and `requester` come from a
                // task of that App on this repository: the token's `task` claim, or the session the client named in
                // `X-Graftyard-Agent-Session`. Without one both are empty and the gate needs one more approval.
                let creator = match self.actor.actor {
                    Actor::App { app, .. } => Some(app),
                    _ => self.credential_app(deps),
                };
                if agent
                    && let Some(app) = creator
                    && crate::agent::agent_app(install, app).is_some()
                {
                    let row = crate::agent::agent_app(install, app).expect("checked");
                    if row.kind == graftyard_identity::AgentKind::GraftyardRun && !draft {
                        return Err(IssueErr::Agent(crate::agent::AgentRefusal::of(
                            crate::agent::AgentRule::DraftOnly,
                        )));
                    }
                    let named = crate::task::claim(&self.actor).or(session);
                    let task = named.and_then(|s| crate::task::task_core(v.core(), s).map(|t| (s, t)));
                    let (session, requester) = match task {
                        Some((s, t)) if t.app == app => (Some(s.clone()), t.requester),
                        _ => (None, None),
                    };
                    let meta = crate::agent::AgentMeta { app, session, requester, repo: v.tiers.repo };
                    evs.push(RepoEvent::AgentMeta { n, meta });
                }
                // A `graftyard-plan` block in a draft's body becomes its editor's Shared lease (format 32.14).
                if leases
                    && *draft
                    && let Some(owner) = authz
                {
                    let lv = crate::lease::LeaseView { core: v.core(), owner, repo: v.tiers.repo, at: v.at };
                    evs.extend(crate::lease::plan_events(&lv, &self.actor, self.credential_app(deps), n, body));
                }
                Ok((evs, IssueOut::Number(n)))
            }
            IssueOp::Close { n } => {
                issue(*n)?;
                let author = self.author(v, *n);
                match pull_head(v.core(), *n) {
                    Some(p) => {
                        author_or(author, RepoPermission::Triage)?;
                        if !matches!(p.state, PrState::Open { .. }) {
                            return Err(IssueErr::NotOpen);
                        }
                        Ok((one(RepoEvent::PrClosed { n: *n, final_head: p.head_sha }), IssueOut::Done))
                    }
                    None => {
                        author_or(author, RepoPermission::Triage)?;
                        let change = IssueChange::State { open: false, reason: None };
                        Ok((one(RepoEvent::IssueChanged { n: *n, change, actor: me }), IssueOut::Done))
                    }
                }
            }
            IssueOp::Change { n, change } => {
                issue(*n)?;
                let pr = pull_head(v.core(), *n);
                author_or(self.author(v, *n), RepoPermission::Triage)?;
                match change {
                    IssueChange::Title(t) => text_ok(t, "")?,
                    IssueChange::Body(b) => text_ok("t", b)?,
                    IssueChange::Draft(_) if pr.is_none() => return Err(IssueErr::NotPull),
                    IssueChange::Draft(_) if !pr.as_ref().is_some_and(|p| matches!(p.state, PrState::Open { .. })) => {
                        return Err(IssueErr::NotOpen);
                    }
                    IssueChange::Locked(_) => need(RepoPermission::Triage)?,
                    // Closing a pull request is `PrClosed`.
                    IssueChange::State { open: false, .. } if pr.is_some() => {
                        let p = pr.expect("a pull request");
                        if !matches!(p.state, PrState::Open { .. }) {
                            return Err(IssueErr::NotOpen);
                        }
                        return Ok((one(RepoEvent::PrClosed { n: *n, final_head: p.head_sha }), IssueOut::Done));
                    }
                    IssueChange::State { open: true, .. } => match pr {
                        Some(p) if matches!(p.state, PrState::Merged { .. }) => return Err(IssueErr::NotOpen),
                        Some(p) if p.head.repo == v.tiers.repo && v.ref_value(&p.head.branch).is_none() => {
                            return Err(IssueErr::HeadMissing);
                        }
                        _ => {}
                    },
                    _ => {}
                }
                let mut evs = one(RepoEvent::IssueChanged { n: *n, change: change.clone(), actor: me });
                // A draft's body edit replaces its editor's plan-block lease in the edit's own command (32.14).
                if let IssueChange::Body(b) = change
                    && leases
                    && pr.as_ref().is_some_and(|p| matches!(p.state, PrState::Open { draft: true }))
                    && let Some(owner) = authz
                {
                    let lv = crate::lease::LeaseView { core: v.core(), owner, repo: v.tiers.repo, at: v.at };
                    evs.extend(crate::lease::plan_events(&lv, &self.actor, self.credential_app(deps), *n, b));
                }
                Ok((evs, IssueOut::Done))
            }
            IssueOp::Comment { on, body, reply_to } => {
                need(RepoPermission::Read)?;
                text_ok("t", body)?;
                match on {
                    CommentTarget::Issue(n) => issue(*n)?,
                    CommentTarget::Review { n, .. } => {
                        pull_head(v.core(), *n).ok_or(IssueErr::NotPull)?;
                    }
                    CommentTarget::Commit { .. } => {}
                }
                if let Some(r) = reply_to {
                    get_rec::<CommentRow>(v.meta(), "comment", &[int(r.0.0)]).ok_or(IssueErr::NotFound)?;
                }
                // A review thread's first comment touches Core (`tier_of`), so its id comes from Core.
                let thread = matches!(on, CommentTarget::Review { .. }) && reply_to.is_none();
                let id = v.next_id(if thread { Tier::Core } else { Tier::Metadata })?;
                // A review comment outside a review, a reply too, is as on GHES a single-comment review's: a COMMENTED
                // review with no body on the comment's commit, after the comment so `replay` answers the comment's
                // id. The review's id is Core's next: after the thread's own in Core's block for a new thread. In
                // format 32.22 it is a `SingleComment`, which leaves an APPROVED or CHANGES_REQUESTED verdict as it
                // is, as GHES does. In an earlier format it is a `Submitted`, which would replace that verdict, so a
                // commenter who holds one gets no review there.
                let single = match on {
                    CommentTarget::Review { n, review: None, at } => {
                        let who = int(me.map_or(0, |a| a.0.0));
                        let verdict =
                            get_rec::<crate::tiers::VerdictRow>(v.core(), "review_verdict", &[n_key(*n), who]);
                        match verdict.map(|r| r.state) {
                            Some(ReviewState::Approved | ReviewState::ChangesRequested) if !reviews => None,
                            _ => {
                                let review = if thread {
                                    let block = ids(v.core()).block();
                                    block.and_then(|b| b.id(b.index_of(id)? + 1)).ok_or(IssueErr::NeedIdBlock)?
                                } else {
                                    v.next_id(Tier::Core)?
                                };
                                Some((*n, ReviewId(review), at.clone()))
                            }
                        }
                    }
                    _ => None,
                };
                let on = match &single {
                    Some((n, review, at)) => CommentTarget::Review { n: *n, review: Some(*review), at: at.clone() },
                    None => on.clone(),
                };
                let ev = CommentEvent::Created {
                    id: CommentId(id),
                    on,
                    author: me,
                    body: body.clone(),
                    reply_to: *reply_to,
                };
                let mut evs = one(RepoEvent::Comment(ev));
                if let Some((n, review, at)) = single {
                    evs.push(RepoEvent::Review(if reviews {
                        ReviewEvent::SingleComment { id: review, n, reviewer: me, commit: at.commit }
                    } else {
                        ReviewEvent::Submitted {
                            id: review,
                            n,
                            reviewer: me,
                            state: ReviewState::Commented,
                            commit: at.commit,
                            body: "".into(),
                        }
                    }));
                }
                Ok((evs, IssueOut::Id(id)))
            }
            IssueOp::EditComment { id, body } => {
                let row = get_rec::<CommentRow>(v.meta(), "comment", &[int(id.0.0)]).ok_or(IssueErr::NotFound)?;
                author_or(row.author, RepoPermission::Write)?;
                text_ok("t", body)?;
                let ev = CommentEvent::Edited { id: *id, body: body.clone(), actor: me };
                Ok((one(RepoEvent::Comment(ev)), IssueOut::Done))
            }
            IssueOp::DeleteComment { id } => {
                let row = get_rec::<CommentRow>(v.meta(), "comment", &[int(id.0.0)]).ok_or(IssueErr::NotFound)?;
                author_or(row.author, RepoPermission::Write)?;
                Ok((one(RepoEvent::Comment(CommentEvent::Deleted { id: *id, actor: me })), IssueOut::Done))
            }
            IssueOp::Resolve { thread, resolved } => {
                let row = get_rec::<CommentRow>(v.meta(), "comment", &[int(thread.0.0)]).ok_or(IssueErr::NotFound)?;
                if !matches!(row.on, CommentTarget::Review { .. }) || row.reply_to.is_some() {
                    return Err(IssueErr::Invalid("only a review thread's first comment resolves"));
                }
                author_or(row.author, RepoPermission::Write)?;
                let ev = CommentEvent::Resolved { thread: *thread, resolved: *resolved, actor: me };
                Ok((one(RepoEvent::Comment(ev)), IssueOut::Done))
            }
            IssueOp::Review { n, state, commit, body } => {
                need(RepoPermission::Read)?;
                text_ok("t", body)?;
                let p = pull_head(v.core(), *n).ok_or(IssueErr::NotPull)?;
                if !matches!(p.state, PrState::Open { .. }) {
                    return Err(IssueErr::NotOpen);
                }
                if *state == ReviewState::Approved && self.author(v, *n).is_some_and(|a| Some(a) == me) {
                    return Err(IssueErr::OwnApproval);
                }
                let id = v.next_id(Tier::Core)?;
                let ev = ReviewEvent::Submitted {
                    id: ReviewId(id),
                    n: *n,
                    reviewer: me,
                    state: *state,
                    commit: *commit,
                    body: body.clone(),
                };
                let mut evs = one(RepoEvent::Review(ev));
                // A qualifying APPROVE of a PR with `AgentMeta` writes `AgentGate` for the head it approves: a writer's
                // credential that is not an `AgentApp`'s (its user-to-server tokens included), who is not the
                // requester, and approves the current head. Pushers to the head ref are App installations: no user
                // pushes for a task, so no user pusher is excluded yet [inference].
                if agent
                    && *state == ReviewState::Approved
                    && let Some(meta) = crate::tiers::agent_meta(v.core(), *n)
                    && let Actor::User(u) = self.actor.actor
                    && perm >= Some(RepoPermission::Write)
                    && meta.requester != Some(u)
                    && *commit == p.head_sha
                    && self.credential_app(deps).is_none_or(|a| crate::agent::agent_app(install, a).is_none())
                    && crate::tiers::agent_gates(v.core(), *n, &p.head_sha).iter().all(|g| *g != u)
                {
                    evs.push(RepoEvent::AgentGate(crate::agent::AgentGate {
                        pr: *n,
                        head: p.head_sha,
                        satisfied_by: u,
                    }));
                }
                Ok((evs, IssueOut::Id(id)))
            }
            IssueOp::Dismiss { review, message } => {
                need(RepoPermission::Write)?;
                get_rec::<(u32, i64)>(v.core(), "verdict_by_review", &[int(review.0.0)]).ok_or(IssueErr::NotFound)?;
                let ev = ReviewEvent::Dismissed { id: *review, message: message.clone(), actor: me };
                Ok((one(RepoEvent::Review(ev)), IssueOut::Done))
            }
            IssueOp::RequestReviews { n, reviewers, removed } => {
                pull_head(v.core(), *n).ok_or(IssueErr::NotPull)?;
                author_or(self.author(v, *n), RepoPermission::Triage)?;
                let ev = ReviewEvent::Requested { n: *n, reviewers: reviewers.clone(), removed: *removed, actor: me };
                Ok((one(RepoEvent::Review(ev)), IssueOut::Done))
            }
            IssueOp::DefineLabel { id, name, color, description } => {
                need(RepoPermission::Write)?;
                if name.0.is_empty() || chars(&name.0) > LABEL_MAX {
                    return Err(IssueErr::Invalid("label name"));
                }
                if description.as_deref().is_some_and(|d| chars(d) > 100) {
                    return Err(IssueErr::Invalid("label description"));
                }
                let lower = name.0.to_lowercase();
                let taken = v.meta().prefix("label", &[]).into_iter().any(|(k, _)| {
                    let other = get_rec::<LabelRow>(v.meta(), "label", &k).expect("listed");
                    other.name.0.to_lowercase() == lower && Some(&k[0]) != id.map(|i| int(i.0.0)).as_ref()
                });
                if taken {
                    return Err(IssueErr::Exists);
                }
                let id = match id {
                    Some(i) => {
                        get_rec::<LabelRow>(v.meta(), "label", &[int(i.0.0)]).ok_or(IssueErr::NotFound)?;
                        *i
                    }
                    None => LabelId(v.next_id(Tier::Metadata)?),
                };
                let ev =
                    LabelEvent::Defined { id, name: name.clone(), color: *color, description: description.clone() };
                Ok((one(RepoEvent::Labels(ev)), IssueOut::Id(id.0)))
            }
            IssueOp::DeleteLabel { id } => {
                need(RepoPermission::Write)?;
                get_rec::<LabelRow>(v.meta(), "label", &[int(id.0.0)]).ok_or(IssueErr::NotFound)?;
                Ok((one(RepoEvent::Labels(LabelEvent::Deleted { id: *id })), IssueOut::Done))
            }
            IssueOp::Assign { n, add, remove } => {
                issue(*n)?;
                // GHES 3.22 "silently ignores" assignees a caller without push access sets, and logins that cannot be
                // assigned: such a request changes nothing and answers the issue as it stands.
                if perm < Some(RepoPermission::Triage) {
                    return Ok((SmallVec::new(), IssueOut::Done));
                }
                let have = crate::tiers::assignees(v.meta(), *n);
                // The one assignable bot: `graftyard-agent`'s, where it and `graftyard-workspace` are installed.
                let bot = authz_fold(deps, graftyard_identity::install_account())
                    .zip(authz_fold(deps, owner))
                    .and_then(|(i, o)| crate::task::run_apps(i, o, v.tiers.repo))
                    .map(|(app, _)| app.bot());
                let assignable = |a: &AccountId| match graftyard_identity::AppId::of_bot(*a) {
                    Some(_) => Some(*a) == bot,
                    None => permission(&user_principal(*a), v, &folds) >= Some(RepoPermission::Write),
                };
                let mut add_now: SmallVec<[AccountId; 2]> = SmallVec::new();
                for a in add.iter().filter(|a| !have.contains(a) && assignable(a)) {
                    if !add_now.contains(a) && have.len() + add_now.len() < ASSIGNEES_MAX {
                        add_now.push(*a);
                    }
                }
                let remove_now: SmallVec<[AccountId; 2]> =
                    remove.iter().filter(|a| have.contains(a)).copied().collect();
                if add_now.is_empty() && remove_now.is_empty() {
                    return Ok((SmallVec::new(), IssueOut::Done));
                }
                let actor = match self.actor.actor {
                    Actor::App { app, .. } => Some(app.bot()),
                    _ => me,
                };
                Ok((one(RepoEvent::Assigned { n: *n, add: add_now, remove: remove_now, actor }), IssueOut::Done))
            }
            IssueOp::ApplyLabels { n, add, remove } => {
                issue(*n)?;
                need(RepoPermission::Triage)?;
                for l in add {
                    get_rec::<LabelRow>(v.meta(), "label", &[int(l.0.0)]).ok_or(IssueErr::UnknownLabel(*l))?;
                }
                let ev = LabelEvent::Applied { n: *n, add: add.clone(), remove: remove.clone(), actor: me };
                Ok((one(RepoEvent::Labels(ev)), IssueOut::Done))
            }
            IssueOp::SetRule { id, pattern, protection } => {
                need(RepoPermission::Admin)?;
                if pattern.is_empty() {
                    return Err(IssueErr::Invalid("empty rule pattern"));
                }
                let id = match id {
                    Some(i) => {
                        get_rec::<BranchRule>(v.core(), "rule", &[int(i.0)]).ok_or(IssueErr::NotFound)?;
                        *i
                    }
                    None if protection.is_none() => return Err(IssueErr::Invalid("deleting a rule needs its id")),
                    None => v.next_id(Tier::Core)?,
                };
                let rule = BranchRule { id, pattern: pattern.clone(), protection: protection.clone() };
                Ok((one(RepoEvent::Rule(rule)), IssueOut::Id(id)))
            }
            IssueOp::Settings(s) => {
                need(RepoPermission::Admin)?;
                if !(1..=CHECK_RETENTION_MAX).contains(&s.check_retention_days) {
                    return Err(IssueErr::Invalid("check_retention_days is 1 to 400"));
                }
                if s.merge_methods.is_empty() {
                    return Err(IssueErr::Invalid("at least one merge method"));
                }
                let cur = v.settings();
                let (o, e) = cur.as_ref().map_or((owner, None), |c| (c.owner, c.enterprise));
                if s.owner != o || (cur.is_some() && s.enterprise != e) {
                    return Err(IssueErr::OwnerChange);
                }
                Ok((one(RepoEvent::Settings(s.clone())), IssueOut::Done))
            }
            IssueOp::InitRepo(s) => {
                need(RepoPermission::Admin)?;
                if !(1..=CHECK_RETENTION_MAX).contains(&s.check_retention_days) || s.merge_methods.is_empty() {
                    return Err(IssueErr::Invalid("a new repository's settings"));
                }
                // Once: the chain's first settings. A re-sent create finds them and commits nothing.
                if v.settings().is_some() || crate::tiers::created(v.core()).is_some() {
                    return Ok((SmallVec::new(), IssueOut::Done));
                }
                // The owner's `CreateRepoWith` is the commit point. A late create after the finisher aborted it must
                // not initialize the chain again; stored commands still replay exactly as their leader decided them.
                let live = deps.iter().any(|(chain, dep)| match (chain, dep) {
                    (ChainId::Authz(o), IdentityDep::Authz(f)) if *o == owner && f.owner == Some(owner) => f
                        .names
                        .values()
                        .any(|e| matches!(e, graftyard_identity::RepoEntry::Live { repo, .. } if *repo == v.repo())),
                    _ => false,
                });
                if !live {
                    return Err(IssueErr::NotFound);
                }
                if s.owner != owner || s.enterprise != authz.and_then(|a| a.enterprise()) {
                    return Err(IssueErr::OwnerChange);
                }
                Ok((smallvec![RepoEvent::Settings(s.clone()), RepoEvent::Created { actor: me }], IssueOut::Done))
            }
            IssueOp::DeleteRepo { retain_until } => {
                need(RepoPermission::Admin)?;
                if *retain_until <= v.at {
                    return Err(IssueErr::Invalid("retain_until is after the entry"));
                }
                Ok((one(RepoEvent::Deleted { retain_until: *retain_until, actor: me }), IssueOut::Done))
            }
            IssueOp::RecordCheck(_) | IssueOp::CreateCheck(_) => {
                need(RepoPermission::Write)?;
                // A create's run takes the metadata tier's next id, whose `CheckRun` write moves the cursor past it, so
                // the next create in the same entry or a later one takes the following id.
                let created;
                let (run, out) = match &self.op {
                    IssueOp::CreateCheck(c) => {
                        let id = v.next_id(Tier::Metadata)?;
                        let run = crate::issues::CheckRunId(id);
                        created =
                            CheckRunUpdate { run, suite: crate::issues::CheckSuiteId(id), version: 1, ..c.clone() };
                        (&created, IssueOut::Id(id))
                    }
                    IssueOp::RecordCheck(run) => (run, IssueOut::Done),
                    _ => unreachable!("a check command"),
                };
                if let Actor::App { app, .. } = self.actor.actor
                    && run.app != app
                {
                    return Err(IssueErr::Invalid("a check run's app is its actor"));
                }
                // A first version may not take another run's id: one on another head SHA, or of another App or name.
                // A late re-delivery of the same run's first version passes, and its apply drops it by version.
                let other = |k: &Vec<Val>| {
                    matches!(k.get(1), Some(Val::Int(id)) if *id == run.run.0.0)
                        && get_rec::<crate::tiers::Timed<CheckRunUpdate>>(v.meta(), "check_run", k).is_some_and(|t| {
                            (t.rec.head_sha, t.rec.app, &t.rec.name) != (run.head_sha, run.app, &run.name)
                        })
                };
                if run.version == 1 && v.meta().prefix("check_run", &[]).iter().any(|(k, _)| other(k)) {
                    return Err(IssueErr::Exists);
                }
                let mut evs = one(RepoEvent::CheckRun(run.clone()));
                if required(v, run.head_sha, &run.name, Some(run.app)) {
                    let done = run.status == CheckStatus::Completed;
                    evs.push(RepoEvent::RequiredCheck {
                        sha: run.head_sha,
                        name: run.name.clone(),
                        source: CheckSource::Run { app: run.app, run: run.run },
                        version: run.version,
                        conclusion: if done { run.conclusion } else { None },
                    });
                }
                Ok((evs, out))
            }
            IssueOp::RecordStatus(st) => {
                need(RepoPermission::Write)?;
                // An App's status is its bot account's (`AppId::bot`), as GHES shows `github-actions[bot]`.
                let creator = match self.actor.actor {
                    Actor::App { app, .. } => Some(app.bot()),
                    _ => me,
                };
                if creator != Some(st.creator) {
                    return Err(IssueErr::Invalid("a status's creator is its actor"));
                }
                // A status's id is its version: for one (sha, context), the higher id wins. The edge mints it from
                // its clock, which another edge's may trail, so `decide` takes it above every status the commit
                // already holds: ids on one commit then increase in commit order, whatever the edges' clocks say
                // (F51). `status` is keyed (sha, -id), so the commit's first row holds its highest id.
                let top = v.meta().scan("status", &[sha_key(&st.sha)], 1).into_iter().find_map(|(k, _)| {
                    match (k.first(), k.get(1)) {
                        (Some(sha), Some(Val::Int(neg))) if *sha == sha_key(&st.sha) => Some(-neg),
                        _ => None,
                    }
                });
                let id = top.map_or(st.id.0.0, |t| st.id.0.0.max(t + 1));
                let st = StatusUpdate { id: StatusId(DbId(id)), ..st.clone() };
                let mut evs = one(RepoEvent::Status(st.clone()));
                if required(v, st.sha, &st.context, None) {
                    let conclusion = match st.state {
                        StatusState::Success => Some(CheckConclusion::Success),
                        StatusState::Error | StatusState::Failure => Some(CheckConclusion::Failure),
                        StatusState::Pending => None,
                    };
                    evs.push(RepoEvent::RequiredCheck {
                        sha: st.sha,
                        name: st.context.clone(),
                        source: CheckSource::Status { creator: st.creator },
                        version: st.id.0.0 as u64,
                        conclusion,
                    });
                }
                Ok((evs, IssueOut::Id(st.id.0)))
            }
            IssueOp::PutRelease { id, tag, target, name, body, draft, prerelease, latest } => {
                need(RepoPermission::Write)?;
                if chars(name) > TITLE_MAX || chars(body) > BODY_MAX {
                    return Err(IssueErr::Invalid("release name or body too long"));
                }
                if !tag.as_bytes().starts_with(b"refs/tags/") {
                    return Err(IssueErr::Invalid("a release tag is under refs/tags/"));
                }
                let by_tag = release_by_tag(v.meta(), tag);
                let old = match id {
                    Some(i) => Some(release_row(v.meta(), *i).ok_or(IssueErr::NotFound)?),
                    None => None,
                };
                if by_tag.is_some() && by_tag != *id {
                    return Err(IssueErr::Exists);
                }
                let existing = v.ref_value(tag);
                let target = existing.unwrap_or(*target);
                let create = (!draft && existing.is_none()).then(|| RefUpdate {
                    name: tag.clone(),
                    expect: None,
                    new: Some(target),
                });
                let id = match id {
                    Some(i) => *i,
                    None => RepoReleaseId(v.next_id(if create.is_some() { Tier::Core } else { Tier::Metadata })?),
                };
                let author = old.map_or(me, |o| o.rec.author);
                let record = ReleaseRecord {
                    tag: tag.clone(),
                    target,
                    name: name.clone(),
                    body: body.clone(),
                    draft: *draft,
                    prerelease: *prerelease,
                    latest: *latest,
                    author,
                };
                Ok((one(RepoEvent::Release { id, record: Some(record), tag: create, actor: me }), IssueOut::Id(id.0)))
            }
            IssueOp::DeleteRelease { id } => {
                need(RepoPermission::Write)?;
                release_row(v.meta(), *id).ok_or(IssueErr::NotFound)?;
                Ok((one(RepoEvent::Release { id: *id, record: None, tag: None, actor: me }), IssueOut::Done))
            }
            IssueOp::PutAsset { release, name, label, content_type, blob } => {
                need(RepoPermission::Write)?;
                release_row(v.meta(), *release).ok_or(IssueErr::NotFound)?;
                if name.is_empty() || chars(name) > TITLE_MAX {
                    return Err(IssueErr::Invalid("asset name"));
                }
                let taken =
                    v.meta().prefix("asset_by_release", &[int(release.0.0)]).into_iter().any(|(k, _)| {
                        get_rec::<AssetRow>(v.meta(), "asset", &k[1..]).is_some_and(|a| a.rec.name == *name)
                    });
                if taken {
                    return Err(IssueErr::Exists);
                }
                let id = AssetId(v.next_id(Tier::Metadata)?);
                let asset = AssetRecord {
                    name: name.clone(),
                    label: label.clone(),
                    content_type: content_type.clone(),
                    blob: blob.blob(),
                };
                let ev = RepoEvent::ReleaseAsset { release: *release, id, asset: Some(asset), actor: me };
                Ok((one(ev), IssueOut::Id(id.0)))
            }
            IssueOp::DeleteAsset { id } => {
                need(RepoPermission::Write)?;
                let row = get_rec::<AssetRow>(v.meta(), "asset", &[int(id.0.0)]).ok_or(IssueErr::NotFound)?;
                let ev = RepoEvent::ReleaseAsset { release: row.release, id: *id, asset: None, actor: me };
                Ok((one(ev), IssueOut::Done))
            }
            IssueOp::LfsObject(blob) => {
                need(RepoPermission::Write)?;
                Ok((one(RepoEvent::LfsObject { blob: blob.blob(), actor: me }), IssueOut::Done))
            }
            IssueOp::LockLfs { path, refspec } => {
                need(RepoPermission::Write)?;
                let owner = me.ok_or(IssueErr::Forbidden)?;
                if path.is_empty() {
                    return Err(IssueErr::Invalid("lock path"));
                }
                if v.meta().get("lfs_lock_by_path", &[Val::Text(path.as_ref().into())]).is_some() {
                    return Err(IssueErr::Exists);
                }
                let id = LockId(v.next_id(Tier::Metadata)?);
                let ev = LfsLockEvent::Created { id, path: path.clone(), refspec: refspec.clone(), owner };
                Ok((one(RepoEvent::LfsLock(ev)), IssueOut::Id(id.0)))
            }
            IssueOp::UnlockLfs { id, force } => {
                need(RepoPermission::Write)?;
                let row = get_rec::<LockRow>(v.meta(), "lfs_lock", &[int(id.0.0)]).ok_or(IssueErr::NotFound)?;
                if Some(row.owner) != me {
                    if !force {
                        return Err(IssueErr::Forbidden);
                    }
                    need(RepoPermission::Admin)?;
                }
                Ok((one(RepoEvent::LfsLock(LfsLockEvent::Deleted { id: *id, actor: me })), IssueOut::Done))
            }
            IssueOp::InviteCollaborator { user, permission } => {
                need(RepoPermission::Admin)?;
                if is_org_member(authz, owner, *user) {
                    return Err(IssueErr::OrgMember);
                }
                if collaborator(v.core(), *user).is_some() {
                    return Err(IssueErr::Exists);
                }
                let id = InvitationId(v.next_id(Tier::Core)?);
                let ev = InvitationEvent::Sent {
                    id,
                    invitee: Invitee::User(UserId(*user)),
                    grant: InvitationGrant::Repo(*permission),
                    inviter: me,
                    expires: Timestamp(v.at.0.saturating_add(INVITATION_TTL.as_millis() as u64)),
                };
                Ok((one(RepoEvent::Invitation(ev)), IssueOut::Id(id.0)))
            }
            IssueOp::AnswerInvitation { id, accept } => {
                let row = get_rec::<InvitationRow>(v.core(), "invitation", &[int(id.0.0)])
                    .filter(|r| r.expires > v.at)
                    .ok_or(IssueErr::NotFound)?;
                let (Invitee::User(invitee), InvitationGrant::Repo(p)) = (row.invitee, row.grant) else {
                    return Err(IssueErr::NotFound);
                };
                if me != Some(invitee.0) {
                    return Err(IssueErr::Forbidden);
                }
                if !accept {
                    return Ok((one(RepoEvent::Invitation(InvitationEvent::Declined { id: *id })), IssueOut::Done));
                }
                // Joined the org since the invitation: the grant belongs on the org's chain, and nothing is granted.
                if is_org_member(authz, owner, invitee.0) {
                    return Err(IssueErr::OrgMember);
                }
                let evs = smallvec![
                    RepoEvent::Invitation(InvitationEvent::Accepted { id: *id }),
                    RepoEvent::Collaborator { who: invitee.0, permission: Some(p) },
                ];
                Ok((evs, IssueOut::Done))
            }
            IssueOp::CancelInvitation { id } => {
                need(RepoPermission::Admin)?;
                get_rec::<InvitationRow>(v.core(), "invitation", &[int(id.0.0)])
                    .filter(|r| r.expires > v.at)
                    .ok_or(IssueErr::NotFound)?;
                let ev = InvitationEvent::Cancelled { id: *id, actor: me };
                Ok((one(RepoEvent::Invitation(ev)), IssueOut::Done))
            }
            IssueOp::RemoveCollaborator { user } => {
                need(RepoPermission::Admin)?;
                if collaborator(v.core(), *user).is_none() {
                    return Err(IssueErr::NotFound);
                }
                Ok((one(RepoEvent::Collaborator { who: *user, permission: None }), IssueOut::Done))
            }
            IssueOp::SetAgentPolicy(p) => {
                need(RepoPermission::Admin)?;
                if !agent {
                    return Err(IssueErr::Invalid("an agent policy is format 32.8"));
                }
                if p.labels.iter().any(|l| l.is_empty() || chars(l) > LABEL_MAX) {
                    return Err(IssueErr::Invalid("an agent label is 1 to 50 characters"));
                }
                if p.extra_paths.iter().any(|g| graftyard_identity::glob::PathGlob::parse(g).is_err()) {
                    return Err(IssueErr::Invalid("an extra path is a path glob"));
                }
                if crate::tiers::agent_policy(v.core()) == *p {
                    return Ok((SmallVec::new(), IssueOut::Done));
                }
                Ok((one(RepoEvent::AgentPolicy(p.clone())), IssueOut::Done))
            }
            // Decided on the scratch, which carries the entry's time (`crate::issue_ops::call::IssueCall::decide`).
            IssueOp::CreateTask(_)
            | IssueOp::UpdateTask { .. }
            | IssueOp::CancelTask { .. }
            | IssueOp::CreateRun(_) => Err(IssueErr::Invalid("a task command is decided by crate::task::decide")),
            IssueOp::AcquireLease(_) | IssueOp::RenewLease { .. } | IssueOp::ReleaseLease { .. } => {
                Err(IssueErr::Invalid("a lease command is decided by crate::lease::decide"))
            }
            IssueOp::PutHook { id, record } => {
                need(RepoPermission::Admin)?;
                hook_ok(record)?;
                let existing = crate::hooks::hooks(v.core());
                let (id, created) = match id {
                    Some(i) => {
                        let old = crate::hooks::hook(v.core(), *i).ok_or(IssueErr::NotFound)?;
                        if old == *record {
                            return Ok((SmallVec::new(), IssueOut::Id(i.0)));
                        }
                        (*i, false)
                    }
                    None => {
                        if existing.len() >= crate::hooks::HOOKS_PER_REPO_MAX {
                            return Err(IssueErr::Invalid("a repository has at most 20 hooks"));
                        }
                        if existing.iter().any(|(_, h)| h.url == record.url) {
                            return Err(IssueErr::Exists);
                        }
                        (crate::hooks::HookId(v.next_id(Tier::Core)?), true)
                    }
                };
                let ev = RepoEvent::Hook { id, record: Some(record.clone()), actor: me, created };
                Ok((one(ev), IssueOut::Id(id.0)))
            }
            IssueOp::DeleteHook { id } => {
                need(RepoPermission::Admin)?;
                crate::hooks::hook(v.core(), *id).ok_or(IssueErr::NotFound)?;
                Ok((one(RepoEvent::Hook { id: *id, record: None, actor: me, created: false }), IssueOut::Done))
            }
            IssueOp::PutActionsSecret { scope, name, sealed, key_id } => {
                need(RepoPermission::Admin)?;
                secret_scope_ok(scope)?;
                // A sealed box is the 32-byte ephemeral key, the 16-byte tag, and the value.
                if sealed.len() <= 48 || sealed.len() > 48 + crate::secrets::VALUE_MAX_BYTES || key_id.is_empty() {
                    return Err(IssueErr::Invalid("a secret is a sealed box of at most 48 KB under the owner's key"));
                }
                let had = crate::secrets::secret(v.core(), scope, name).is_some();
                if !had && crate::secrets::secrets(v.core(), scope).len() >= crate::secrets::SECRETS_PER_SCOPE_MAX {
                    return Err(IssueErr::Invalid("a repository or environment has at most 100 secrets"));
                }
                let record = graftyard_identity::sealed::SecretRecord {
                    sealed: sealed.clone(),
                    key_id: key_id.clone(),
                    updated: v.at,
                };
                let ev = RepoEvent::ActionsSecret {
                    scope: scope.clone(),
                    name: name.clone(),
                    record: Some(record),
                    actor: me,
                };
                Ok((one(ev), if had { IssueOut::Done } else { IssueOut::Created }))
            }
            IssueOp::DeleteActionsSecret { scope, name } => {
                need(RepoPermission::Admin)?;
                crate::secrets::secret(v.core(), scope, name).ok_or(IssueErr::NotFound)?;
                let ev = RepoEvent::ActionsSecret { scope: scope.clone(), name: name.clone(), record: None, actor: me };
                Ok((one(ev), IssueOut::Done))
            }
            IssueOp::PutActionsVariable { scope, name, value, create } => {
                need(RepoPermission::Admin)?;
                secret_scope_ok(scope)?;
                if value.len() > crate::secrets::VALUE_MAX_BYTES {
                    return Err(IssueErr::Invalid("a variable is at most 48 KB"));
                }
                let old = crate::secrets::variable(v.core(), scope, name);
                match (create, &old) {
                    (true, Some(_)) => return Err(IssueErr::Exists),
                    (false, None) => return Err(IssueErr::NotFound),
                    (false, Some(o)) if o.value == *value => return Ok((SmallVec::new(), IssueOut::Done)),
                    _ => {}
                }
                if old.is_none()
                    && crate::secrets::variables(v.core(), scope).len() >= crate::secrets::VARIABLES_PER_SCOPE_MAX
                {
                    return Err(IssueErr::Invalid("a repository or environment has at most 500 variables"));
                }
                let record = graftyard_identity::sealed::VariableRecord { value: value.clone(), updated: v.at };
                let ev = RepoEvent::ActionsVariable {
                    scope: scope.clone(),
                    name: name.clone(),
                    record: Some(record),
                    actor: me,
                };
                Ok((one(ev), if *create { IssueOut::Created } else { IssueOut::Done }))
            }
            IssueOp::DeleteActionsVariable { scope, name } => {
                need(RepoPermission::Admin)?;
                crate::secrets::variable(v.core(), scope, name).ok_or(IssueErr::NotFound)?;
                let ev =
                    RepoEvent::ActionsVariable { scope: scope.clone(), name: name.clone(), record: None, actor: me };
                Ok((one(ev), IssueOut::Done))
            }
        }
    }

    /// A task command's `decide` (`crate::task::decide`): the credential check, then the caller's permission, then the
    /// task rules, at the entry's time `at` after index `applied`. An installation token needs `issues: write` and
    /// `contents: read`, as the design gates external create and cancel.
    pub fn decide_task(
        &self,
        v: &RepoView<'_>,
        deps: &[(ChainId, IdentityDep)],
        at: Timestamp,
        applied: graftyard_types::Index,
        leases: bool,
        writes: u32,
    ) -> Result<(SmallVec<[RepoEvent; 4]>, IssueOut), IssueErr> {
        let cred = self.actor.credential_shard();
        let app = match (&self.actor.actor, &self.actor.credential) {
            (Actor::App { .. }, Credential::Stateless(c)) => Some(c),
            _ => None,
        };
        if cred.key.is_none() || (cred.account.is_none() && app.is_none()) {
            return Err(IssueErr::Unauthorized(CredentialRefusal::NoToken));
        }
        check_credential(&cred, Own::Other, deps).map_err(IssueErr::Unauthorized)?;
        let owner = v.owner();
        let folds = crate::repo::authz_folds(deps);
        let perm = match app {
            Some(c) => {
                let access = Access { repo: v.tiers.repo, owner, core: v.core() };
                let issues = graftyard_identity::authorize_app(c, &self.app_needs(v), &folds, &access);
                let contents = graftyard_identity::authorize_app(c, &[("contents", AppAccess::Read)], &folds, &access);
                match (issues, contents) {
                    (Decision::Allow, Decision::Allow) => Some(RepoPermission::Admin),
                    (Decision::Hidden, _) | (_, Decision::Hidden) => return Err(IssueErr::NotFound),
                    _ => return Err(IssueErr::Forbidden),
                }
            }
            None => permission(&self.actor, v, &folds),
        };
        if perm.is_none() {
            return Err(IssueErr::NotFound);
        }
        // A Graftyard-run create: a user's credential that may write contents within its own limits, never an
        // `AgentApp`'s user-to-server token, and the install's two Graftyard-run Apps installed on the repository.
        let (human_writer, run) = match &self.op {
            IssueOp::CreateRun(_) => {
                let install = crate::repo::install_authz(deps);
                let access = Access { repo: v.tiers.repo, owner, core: v.core() };
                let fields = crate::repo::stored_fields(&self.actor, deps);
                let own = graftyard_identity::token_allows(fields, &[("contents", AppAccess::Write)], &access);
                let agent_token =
                    self.credential_app(deps).is_some_and(|a| crate::agent::agent_app(install, a).is_some());
                let human = app.is_none() && own == Decision::Allow && !agent_token;
                let run = authz_fold(deps, graftyard_identity::install_account())
                    .zip(authz_fold(deps, owner))
                    .and_then(|(i, o)| crate::task::run_apps(i, o, v.tiers.repo));
                (human, run)
            }
            _ => (false, None),
        };
        let assigner = match (&self.op, self.actor.actor) {
            (IssueOp::CreateRun(r), Actor::App { app, .. }) => {
                r.issue.and_then(|n| crate::tiers::assigned_by(v.meta(), n, app.bot()))
            }
            _ => None,
        };
        let ctx = crate::task::TaskCtx {
            repo: v.tiers.repo,
            at,
            applied,
            owner: folds.iter().copied().find(|a| a.owner() == Some(owner)),
            install: crate::repo::install_authz(deps),
            perm,
            leases,
            request: self.request,
            human_writer,
            run,
            writes,
            assigner,
        };
        crate::task::decide(&self.op, &self.actor, v.core(), &ctx).map_err(IssueErr::Task)
    }

    /// A lease command's `decide` (`crate::lease::decide`): the credential check, then whether the caller may push
    /// (a user's `Write`, an installation's `contents: write`, within a stored token's own limits) and is a repository
    /// admin, then the lease rules at the entry's time `at`.
    pub fn decide_lease(
        &self,
        v: &RepoView<'_>,
        deps: &[(ChainId, IdentityDep)],
        at: Timestamp,
    ) -> Result<(SmallVec<[RepoEvent; 4]>, IssueOut), IssueErr> {
        let cred = self.actor.credential_shard();
        let app = match (&self.actor.actor, &self.actor.credential) {
            (Actor::App { .. }, Credential::Stateless(c)) => Some(c),
            _ => None,
        };
        if cred.key.is_none() || (cred.account.is_none() && app.is_none()) {
            return Err(IssueErr::Unauthorized(CredentialRefusal::NoToken));
        }
        check_credential(&cred, Own::Other, deps).map_err(IssueErr::Unauthorized)?;
        let owner = v.owner();
        let folds = crate::repo::authz_folds(deps);
        let authz = folds.iter().copied().find(|a| a.owner() == Some(owner)).ok_or(IssueErr::NotFound)?;
        let access = Access { repo: v.tiers.repo, owner, core: v.core() };
        let write = [("contents", AppAccess::Write)];
        let (push, admin) = match app {
            Some(c) => match graftyard_identity::authorize_app(c, &write, &folds, &access) {
                Decision::Allow => (true, false),
                Decision::Forbidden => (false, false),
                Decision::Hidden => return Err(IssueErr::NotFound),
            },
            None => {
                let p = permission(&self.actor, v, &folds).ok_or(IssueErr::NotFound)?;
                let fields = crate::repo::stored_fields(&self.actor, deps);
                let own = graftyard_identity::token_allows(fields, &write, &access) == Decision::Allow;
                (own && p >= RepoPermission::Write, p >= RepoPermission::Admin)
            }
        };
        let ctx = crate::lease::LeaseCtx {
            view: crate::lease::LeaseView { core: v.core(), owner: authz, repo: v.tiers.repo, at },
            install: crate::repo::install_authz(deps),
            push,
            admin,
            token_app: self.credential_app(deps),
        };
        crate::lease::decide(&self.op, &self.actor, &ctx).map_err(IssueErr::Lease)
    }

    /// The App behind a stored user-to-server token, from its `Tokens` row the leader read.
    fn credential_app(&self, deps: &[(ChainId, IdentityDep)]) -> Option<graftyard_identity::AppId> {
        let graftyard_identity::Credential::Stored(hash) = &self.actor.credential else { return None };
        deps.iter().find_map(|(_, d)| match d {
            IdentityDep::Tokens(t) => graftyard_identity::TokensRead::fields(t, hash).and_then(|f| f.app),
            _ => None,
        })
    }

    /// The GitHub App permissions, any one of which lets an installation token run this command, by GitHub's
    /// documented per-endpoint permissions [inference: the endpoints' "Fine-grained access" sections, mapped onto these
    /// commands]. A command no App may run (answering a user's invitation, a status, whose creator is an account)
    /// needs nothing listed and is refused.
    pub(crate) fn app_needs(&self, v: &RepoView<'_>) -> SmallVec<[(&'static str, AppAccess); 2]> {
        let w = |p: &'static str| (p, AppAccess::Write);
        let on = |n: IssueNumber| if pull_head(v.core(), n).is_some() { w("pull_requests") } else { w("issues") };
        let comment = |id: &CommentId| match get_rec::<CommentRow>(v.meta(), "comment", &[int(id.0.0)]).map(|r| r.on) {
            Some(CommentTarget::Issue(n)) => smallvec![w("issues"), on(n)],
            Some(CommentTarget::Review { .. }) => smallvec![w("pull_requests")],
            Some(CommentTarget::Commit { .. }) => smallvec![w("contents")],
            None => smallvec![w("issues"), w("pull_requests")],
        };
        match &self.op {
            IssueOp::OpenIssue { .. } => smallvec![w("issues")],
            IssueOp::OpenPr { .. }
            | IssueOp::Review { .. }
            | IssueOp::Dismiss { .. }
            | IssueOp::RequestReviews { .. }
            | IssueOp::Resolve { .. } => smallvec![w("pull_requests")],
            IssueOp::Close { n } | IssueOp::Change { n, .. } => smallvec![on(*n)],
            IssueOp::Comment { on: CommentTarget::Issue(n), .. } => smallvec![w("issues"), on(*n)],
            IssueOp::Comment { on: CommentTarget::Review { .. }, .. } => smallvec![w("pull_requests")],
            IssueOp::Comment { on: CommentTarget::Commit { .. }, .. } => smallvec![w("contents")],
            IssueOp::EditComment { id, .. } | IssueOp::DeleteComment { id } => comment(id),
            IssueOp::DefineLabel { .. }
            | IssueOp::DeleteLabel { .. }
            | IssueOp::ApplyLabels { .. }
            | IssueOp::Assign { .. } => {
                smallvec![w("issues"), w("pull_requests")]
            }
            IssueOp::SetRule { .. }
            | IssueOp::Settings(_)
            | IssueOp::InviteCollaborator { .. }
            | IssueOp::RemoveCollaborator { .. }
            | IssueOp::CancelInvitation { .. }
            | IssueOp::SetAgentPolicy(_)
            | IssueOp::InitRepo(_)
            | IssueOp::DeleteRepo { .. } => smallvec![w("administration")],
            IssueOp::RecordCheck(_) | IssueOp::CreateCheck(_) => smallvec![w("checks")],
            IssueOp::PutRelease { .. }
            | IssueOp::DeleteRelease { .. }
            | IssueOp::PutAsset { .. }
            | IssueOp::DeleteAsset { .. }
            | IssueOp::LfsObject(_)
            | IssueOp::LockLfs { .. }
            | IssueOp::UnlockLfs { .. } => smallvec![w("contents")],
            // GHES's `statuses` App permission for an App's status (a `GITHUB_TOKEN`'s `statuses: write`); a user's
            // token keeps no per-resource need, so its statuses are limited by its role alone, as before.
            IssueOp::RecordStatus(_) if matches!(self.actor.actor, Actor::App { .. }) => smallvec![w("statuses")],
            IssueOp::RecordStatus(_) | IssueOp::AnswerInvitation { .. } => SmallVec::new(),
            // External create and cancel need `issues: write` (and `contents: read`, which `crate::task` checks).
            IssueOp::CreateTask(_)
            | IssueOp::UpdateTask { .. }
            | IssueOp::CancelTask { .. }
            | IssueOp::CreateRun(_) => {
                smallvec![w("issues")]
            }
            IssueOp::AcquireLease(_) | IssueOp::RenewLease { .. } | IssueOp::ReleaseLease { .. } => {
                smallvec![w("contents")]
            }
            // GitHub's App permission for repository webhooks.
            IssueOp::PutHook { .. } | IssueOp::DeleteHook { .. } => smallvec![w("repository_hooks")],
            // GitHub's App permissions for Actions secrets and variables.
            IssueOp::PutActionsSecret { .. } | IssueOp::DeleteActionsSecret { .. } => smallvec![w("secrets")],
            IssueOp::PutActionsVariable { .. } | IssueOp::DeleteActionsVariable { .. } => {
                smallvec![w("actions_variables")]
            }
        }
    }

    /// The issue's author, from the metadata tier.
    fn author(&self, v: &RepoView<'_>, n: IssueNumber) -> Option<AccountId> {
        get_rec::<IssueRow>(v.meta(), "issue", &[n_key(n)]).and_then(|r| r.author)
    }

    /// The output for a duplicate request, from the events its first commit stored.
    pub fn replay(&self, events: &[RepoEvent]) -> IssueOut {
        for e in events {
            match e {
                RepoEvent::IssueOpened { n, .. } | RepoEvent::PrOpened { n, .. } => return IssueOut::Number(*n),
                RepoEvent::Comment(CommentEvent::Created { id, .. }) => return IssueOut::Id(id.0),
                RepoEvent::Review(ReviewEvent::Submitted { id, .. }) => return IssueOut::Id(id.0),
                RepoEvent::Labels(LabelEvent::Defined { id, .. }) => return IssueOut::Id(id.0),
                RepoEvent::Rule(r) if matches!(self.op, IssueOp::SetRule { .. }) => return IssueOut::Id(r.id),
                RepoEvent::Release { id, record: Some(_), .. } => return IssueOut::Id(id.0),
                RepoEvent::ReleaseAsset { id, asset: Some(_), .. } => return IssueOut::Id(id.0),
                RepoEvent::LfsLock(LfsLockEvent::Created { id, .. }) => return IssueOut::Id(id.0),
                RepoEvent::Status(st) => return IssueOut::Id(st.id.0),
                RepoEvent::CheckRun(c) if matches!(self.op, IssueOp::CreateCheck(_)) => return IssueOut::Id(c.run.0),
                RepoEvent::TaskCreated(t) => return IssueOut::Task(t.session.clone()),
                RepoEvent::TaskUpdated { session, state, .. } => return crate::task::replay(&self.op, session, *state),
                RepoEvent::LeaseAcquire { lease, .. } if self.op.is_lease() => return IssueOut::Lease(lease.id),
                RepoEvent::LeaseRenew { id, .. } if self.op.is_lease() => return IssueOut::Lease(*id),
                RepoEvent::Hook { id, record: Some(_), .. } => return IssueOut::Id(id.0),
                _ => {}
            }
        }
        IssueOut::Done
    }
}

/// The agent rules an `Agent`'s issue command breaks (`design/DESIGN.md`, Shape, "Agent load and agent safety"): an
/// APPROVE review, ready-for-review, rule and settings edits, a label outside the policy's allowlist, a status or check
/// run named `graftyard/...`, and a release (which may create a tag).
fn agent_refusal<B>(op: &IssueOp<B>, v: &RepoView<'_>) -> Option<crate::agent::AgentRefusal> {
    use crate::agent::{AgentRefusal, AgentRule};
    let graftyard = |name: &str| {
        let text = format!("refused: agents may not write {name}; graftyard/ contexts are host-owned");
        AgentRefusal::new(AgentRule::GraftyardContext, text)
    };
    match op {
        IssueOp::Review { state: ReviewState::Approved, .. } => Some(AgentRefusal::of(AgentRule::Approve)),
        IssueOp::Change { change: IssueChange::Draft(false), .. } => Some(AgentRefusal::of(AgentRule::ReadyForReview)),
        IssueOp::SetRule { .. }
        | IssueOp::Settings(_)
        | IssueOp::SetAgentPolicy(_)
        | IssueOp::InitRepo(_)
        | IssueOp::DeleteRepo { .. } => Some(AgentRefusal::of(AgentRule::Rules)),
        // DESIGN, Shape, "Agent load and agent safety": an `Agent` may not edit webhooks.
        IssueOp::PutHook { .. } | IssueOp::DeleteHook { .. } => {
            Some(AgentRefusal::new(AgentRule::Rules, "refused: agents may not edit webhooks"))
        }
        // An agent writing a secret could exfiltrate it through a workflow it also pushes [inference: DESIGN's agent
        // rules refuse settings edits, and secrets are settings].
        IssueOp::PutActionsSecret { .. }
        | IssueOp::DeleteActionsSecret { .. }
        | IssueOp::PutActionsVariable { .. }
        | IssueOp::DeleteActionsVariable { .. } => {
            Some(AgentRefusal::new(AgentRule::Rules, "refused: agents may not edit Actions secrets or variables"))
        }
        IssueOp::PutRelease { .. } | IssueOp::DeleteRelease { .. } | IssueOp::PutAsset { .. } => {
            Some(AgentRefusal::of(AgentRule::Release))
        }
        IssueOp::RecordStatus(st) if st.context.starts_with("graftyard/") => Some(graftyard(&st.context)),
        IssueOp::RecordCheck(c) | IssueOp::CreateCheck(c) if c.name.starts_with("graftyard/") => {
            Some(graftyard(&c.name))
        }
        IssueOp::ApplyLabels { add, .. } => {
            let allowed = crate::tiers::agent_policy(v.core()).labels;
            add.iter().find_map(|l| {
                let name = get_rec::<LabelRow>(v.meta(), "label", &[int(l.0.0)]).map(|r| r.name.0)?;
                (!allowed.iter().any(|a| **a == *name)).then(|| {
                    let text = format!("refused: label {name:?} is not in this repository's agent label allowlist");
                    AgentRefusal::new(AgentRule::Label, text)
                })
            })
        }
        _ => None,
    }
}

/// A hook record the chain accepts: an `http` or `https` URL, and at least one event, each GHES 3.22's or `*`.
/// A repository chain holds only repository and environment rows; an org's are on its `Authz` chain.
fn secret_scope_ok(s: &graftyard_identity::sealed::SecretScope) -> Result<(), IssueErr> {
    match s {
        graftyard_identity::sealed::SecretScope::Org { .. } => Err(IssueErr::Invalid("an org secret is the org's")),
        graftyard_identity::sealed::SecretScope::Env(e) if e.0.is_empty() || e.0.len() > 255 => {
            Err(IssueErr::Invalid("an environment name is 1 to 255 characters"))
        }
        _ => Ok(()),
    }
}

fn hook_ok(r: &crate::hooks::HookRecord) -> Result<(), IssueErr> {
    let url = &*r.url.0;
    let host = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://"));
    if !host.is_some_and(|h| !h.is_empty() && !h.starts_with('/')) || url.len() > 2_048 {
        return Err(IssueErr::Invalid("a hook URL is http or https"));
    }
    if r.events.is_empty() || r.events.iter().any(|e| &*e.0 != "*" && !crate::hooks::EVENTS.contains(&&*e.0)) {
        return Err(IssueErr::Invalid("a hook's events are GHES 3.22 event names"));
    }
    Ok(())
}

fn text_ok(title: &str, body: &str) -> Result<(), IssueErr> {
    if title.trim().is_empty() || chars(title) > TITLE_MAX {
        return Err(IssueErr::Invalid("a title is 1 to 256 characters"));
    }
    if chars(body) > BODY_MAX {
        return Err(IssueErr::Invalid("a body is at most 65,536 characters"));
    }
    Ok(())
}

/// An outside collaborator's row in the core tier.
pub fn collaborator(core: &(impl Read + ?Sized), user: AccountId) -> Option<RepoPermission> {
    get_rec(core, "collaborator", &[int(user.0.0)])
}

/// Whether `user` is a member of `owner` when `owner` is an org, by the owner's `Authz` fold the leader read.
fn is_org_member(authz: Option<&dyn AuthzRead>, owner: AccountId, user: AccountId) -> bool {
    authz.is_some_and(|a| {
        a.owner() == Some(owner)
            && a.opened() == Some(graftyard_identity::AccountKind::Org)
            && a.role(UserId(user)).is_some()
    })
}

/// The repository chain's part of `authorize`: its core tier's settings and collaborator rows.
pub struct Access<'a> {
    pub repo: graftyard_types::RepoId,
    pub owner: AccountId,
    pub core: &'a dyn Read,
}

impl RepoAccess for Access<'_> {
    fn repo(&self) -> graftyard_types::RepoId {
        self.repo
    }
    fn owner(&self) -> AccountId {
        self.owner
    }
    fn visibility(&self) -> Visibility {
        get_rec::<crate::issues::RepoSettings>(self.core, "settings", &[]).map_or(Visibility::Private, |s| s.visibility)
    }
    fn collaborator(&self, user: UserId) -> Option<RepoPermission> {
        collaborator(self.core, user.0)
    }
}

/// Account `o`'s own `Authz` fold among the deps.
fn authz_fold(deps: &[(ChainId, IdentityDep)], o: AccountId) -> Option<&graftyard_identity::AuthzFold> {
    deps.iter().find_map(|(c, d)| match (c, d) {
        (ChainId::Authz(x), IdentityDep::Authz(f)) if *x == o && f.owner == Some(o) => Some(f),
        _ => None,
    })
}

/// A user as `permission` reads one for an assignee: no token limits and no SSO session, so access an org grants
/// only under enforced SSO does not count [inference: GHES lists as assignable those with push access].
fn user_principal(a: AccountId) -> Principal {
    Principal {
        actor: Actor::User(UserId(a)),
        scopes: graftyard_identity::ScopeSet(1),
        credential: Credential::Stored([0; 32]),
        sso_orgs: Default::default(),
    }
}

/// GitHub's cap on an issue's assignees: "Adds up to 10 assignees" (GHES 3.22 REST, `issues/add-assignees`) [doc].
pub const ASSIGNEES_MAX: usize = 10;

/// The actor's permission on the repository through identity's `permission` over the owner's `Authz` fold and the
/// core tier's collaborator rows, and `Read` for anyone signed in on a public repository.
pub(crate) fn permission(actor: &Principal, v: &RepoView<'_>, folds: &[&dyn AuthzRead]) -> Option<RepoPermission> {
    if matches!(actor.actor, Actor::Anonymous) {
        return None;
    }
    let access = Access { repo: v.tiers.repo, owner: v.owner(), core: v.core() };
    let p = graftyard_identity::permission(actor, folds, &access);
    let public = access.visibility() == Visibility::Public;
    p.max(public.then_some(RepoPermission::Read))
}

/// Whether a rule requires the check `name` (from `app`, `None` for a status) on `sha`: the SHA is the head of an open
/// pull request, a merge-queue ref, or a protected branch tip, and the rule over the branch that guards it lists the
/// check with no app or with this app.
pub fn required(v: &RepoView<'_>, sha: ObjectId, name: &str, app: Option<crate::issues::AppId>) -> bool {
    let mut guarded: Vec<RefName> = v
        .core()
        .prefix("open_head", &[sha_key(&sha)])
        .into_iter()
        .filter_map(|(k, _)| match k.get(1) {
            Some(Val::Int(n)) => pull_head(v.core(), IssueNumber(*n as u32)).map(|p| p.base),
            _ => None,
        })
        .collect();
    let rules = v.rules();
    // Branch tips: the `ref` table from `refs/heads/` while names keep that prefix.
    let heads = b"refs/heads/";
    let mut from = vec![Val::Blob(heads.to_vec().into())];
    'scan: loop {
        let batch = v.core().scan("ref", &from, 256);
        let full = batch.len() == 256;
        for (k, _) in batch {
            let Some(Val::Blob(b)) = k.first() else { break 'scan };
            if !b.starts_with(heads) {
                break 'scan;
            }
            if k == from && full {
                continue;
            }
            from = k.clone();
            let Ok(name) = RefName::parse(b) else { continue };
            if v.ref_value(&name) != Some(sha) {
                continue;
            }
            if let Some(base) = queue_base(&name) {
                guarded.push(base);
            } else if rules.iter().any(|r| r.protection.is_some() && branch_matches(&r.pattern, &name)) {
                guarded.push(name);
            }
        }
        if !full {
            break;
        }
    }
    guarded.iter().any(|b| {
        rules.iter().filter(|r| branch_matches(&r.pattern, b)).any(|r| {
            let checks = r.protection.as_ref().and_then(|p| p.checks.as_ref());
            checks.is_some_and(|c| c.checks.iter().any(|(n, a)| &**n == name && (a.is_none() || *a == app)))
        })
    })
}

/// The base branch of a merge-queue ref `refs/heads/gh-readonly-queue/{base}/pr-{n}-{sha}`.
pub fn queue_base(name: &RefName) -> Option<RefName> {
    let s = std::str::from_utf8(name.as_bytes()).ok()?;
    let rest = s.strip_prefix("refs/heads/gh-readonly-queue/")?;
    let (base, _) = rest.rsplit_once("/pr-")?;
    RefName::parse(format!("refs/heads/{base}").as_bytes()).ok()
}

/// Whether a branch rule's pattern matches `name`'s branch, as GitHub's fnmatch patterns do: `*` and `?` stop at `/`,
/// `**` crosses it, `[...]` is a class with ranges and a leading `!` or `^`. A ref outside `refs/heads/` matches
/// nothing.
pub fn branch_matches(pattern: &str, name: &RefName) -> bool {
    let Some(branch) = std::str::from_utf8(name.as_bytes()).ok().and_then(|s| s.strip_prefix("refs/heads/")) else {
        return false;
    };
    glob(pattern.as_bytes(), branch.as_bytes())
}

/// One token of a branch pattern, in pattern order.
enum Tok<'a> {
    /// `**`: any run of bytes, `/` included.
    Any,
    /// `*`: any run of bytes but `/`.
    Star,
    /// `?`: one byte but `/`.
    One,
    /// `[...]`: one byte but `/` in (or, negated, not in) the set.
    Class {
        neg: bool,
        set: &'a [u8],
    },
    Lit(u8),
}

fn tokens(mut p: &[u8]) -> Vec<Tok<'_>> {
    let mut out = Vec::new();
    while let Some(&c) = p.first() {
        let (t, len) = match c {
            b'*' if p.get(1) == Some(&b'*') => (Tok::Any, 2),
            b'*' => (Tok::Star, 1),
            b'?' => (Tok::One, 1),
            b'[' => match p[1..].iter().position(|c| *c == b']').map(|e| e + 1) {
                None => (Tok::Lit(b'['), 1),
                Some(end) => match p[1] {
                    b'!' | b'^' => (Tok::Class { neg: true, set: &p[2..end] }, end + 1),
                    _ => (Tok::Class { neg: false, set: &p[1..end] }, end + 1),
                },
            },
            c => (Tok::Lit(c), 1),
        };
        out.push(t);
        p = &p[len..];
    }
    out
}

fn class_hit(set: &[u8], c: u8) -> bool {
    let mut hit = false;
    let mut i = 0;
    while i < set.len() {
        if i + 2 < set.len() && set[i + 1] == b'-' {
            hit |= (set[i]..=set[i + 2]).contains(&c);
            i += 3;
        } else {
            hit |= set[i] == c;
            i += 1;
        }
    }
    hit
}

/// Whether `s` matches pattern `p`, in O(|p|·|s|): `ok[j]` is whether the tokens after the current one match `s[j..]`,
/// filled from the last token back.
fn glob(p: &[u8], s: &[u8]) -> bool {
    let n = s.len();
    let mut ok = vec![false; n + 1];
    ok[n] = true;
    let mut cur = vec![false; n + 1];
    for t in tokens(p).iter().rev() {
        for j in (0..=n).rev() {
            let c = s.get(j).copied();
            cur[j] = match t {
                Tok::Any => ok[j] || (j < n && cur[j + 1]),
                Tok::Star => ok[j] || (c.is_some_and(|c| c != b'/') && cur[j + 1]),
                Tok::One => c.is_some_and(|c| c != b'/') && ok[j + 1],
                Tok::Class { neg, set } => c.is_some_and(|c| c != b'/' && class_hit(set, c) != *neg) && ok[j + 1],
                Tok::Lit(l) => c == Some(*l) && ok[j + 1],
            };
        }
        std::mem::swap(&mut ok, &mut cur);
    }
    ok[0]
}
