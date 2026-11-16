//! The repository chain's `Push` command and its ref fold (`design/SHARED.md`, "Refs and pushes"; `design/DESIGN.md`,
//! Shape, "The write path"). A `graftyard_raft::Command`: pure, deciding each update's `expect` against the fold
//! plus the earlier commands of the same entry.
//!
//! The fold holds what M1 needs of the repository's core tier: its refs, and the `RepoSettings` owner that names the
//! owner's `Authz` chain in `deps`. `Genesis` and `Settings` are later work, so a fold is built with its owner.

use std::collections::BTreeMap;

use graftyard_fold::{MemStore, Overlay, Read, TierStore, Tiered, Val, fold_digest, has};
use graftyard_identity::stateless::InstallationClaims;
use graftyard_identity::{
    Access, Actor, AppId, AuthzRead, Credential, CredentialRefusal, Decision, IdentityDep, InstallationId, Jti, Kid,
    Own, PermissionSet, Principal, RepoScope, ScopeSet, UserId, WriteAction, check_credential, credential_chains,
};
use graftyard_raft::{Command, Enc, Ev, Fold, Format, LogEntry, Record, Scratch, TierSet};
use graftyard_types::{
    AccountId, ChainId, CredentialShard, Digest, Index, RepoId, RequestId, SiteSet, Tier, Timestamp, Ulid,
};
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;

use crate::agent::{
    AgentGate, AgentMeta, AgentPolicy, AgentRefusal, AgentRule, ChangedPaths, ProtectedHits, Provenance, SessionId,
    Source, TrailerKey,
};
use crate::blob::BlobRef;
use crate::issues::MergeMethod;
use crate::issues::{
    AssetId, AssetRecord, BranchRule, CheckConclusion, CheckRunUpdate, CheckSource, CheckSuiteUpdate, CommentEvent,
    Consumer, HeadSpec, IssueCapture, IssueChange, IssueNumber, LabelEvent, LfsLockEvent, MergeabilityResult,
    PrCapture, ReleaseRecord, RepoReleaseId, RepoSettings, ReviewEvent, StatusUpdate,
};
use crate::key::R2Key;
use crate::merge::{EngineVersion, TreeDiffDigest};
use crate::pack::{Archived, PackId, PackRef, Promoted};
use crate::queue::QueueEvent;
use crate::refs::{Atomicity, ObjectId, RefName, RefUpdate};
use crate::tiers::{RepoTiers, get_rec, ref_key, tier_of};

/// The repository chain's command records. Version 2 adds `Push.agent`; an edge sends it only once `FormatsEnable`
/// has enabled it. No registry of family numbers exists yet (`crates/identity` took 0x10 to 0x13), so 0x20 is this
/// crate's choice.
pub const REPO_FORMAT_V1: Format = Format { family: 0x20, version: 1 };
pub const REPO_FORMAT_V2: Format = Format { family: 0x20, version: 2 };
/// Version 7 is version 2 with an installation token's principal: actor tag 2 (`App`) and credential tag 2
/// (`Stateless`, its claims). Versions 3 to 6 are the other repository commands'. An edge sends it only once
/// `FormatsEnable` has enabled it; a push under a stored token or a session keeps version 1 or 2.
pub const REPO_FORMAT_STATELESS: Format = Format { family: 0x20, version: 7 };
/// AM1-WP2's agent layer: a push that carries `validate`'s `provenance`, `protected`, and `changed`, any principal, and
/// the agent events (`Provenance`, `AgentMeta`, `AgentGate`, `AgentPolicy`, tags 27 to 30) and the `Refused` reason
/// `Agent` (tag 7). An edge sends it only once `FormatsEnable` has enabled it, and a command in an earlier format never
/// produces one of those encodings (`Push::decide`, `IssueCmd::decide`).
pub const REPO_FORMAT_AGENT: Format = Format { family: 0x20, version: 8 };
/// AM1-WP3's tasks: the task commands (`crate::task`), their events `TaskCreated` and `TaskUpdated` (tags 31 and 32),
/// and any command whose installation token carries a `task` claim, whose claims encode it. A superset of 32.8: such a
/// push carries `validate`'s outputs as 32.8 does. Versions 10 and 11 are other packages'. An edge sends it only once
/// `FormatsEnable` has enabled it; a command without a task and without a claim never takes it.
pub const REPO_FORMAT_TASKS: Format = Format { family: 0x20, version: 12 };
/// `rest:repos`' create and delete (branch `m3-repos`): `IssueOp::InitRepo` and `IssueOp::DeleteRepo`, and their
/// events `Created` and `Deleted` (tags 33 and 34). Version 13, after AM1-WP3's 12. An edge sends it only once
/// `FormatsEnable` has enabled it.
pub const REPO_FORMAT_REPOS: Format = Format { family: 0x20, version: 13 };
/// AM1-WP4's leases and `PathSet`: the lease commands (`crate::lease`) and their events `LeaseAcquire`, `LeaseRenew`,
/// and `LeaseRelease` (tags 35 to 37), and `PathSet` (38); tags 33 and 34 are 32.13's `Created` and `Deleted`. A
/// superset of 32.12: a push or issue command the edge marks `leases` (`Push::leases`, `IssueCall::leases`) encodes as
/// 32.12 does and may write those events, and a push stores a per-ref `Leased` refusal. Version 13 is another
/// package's. An edge sends it only once `FormatsEnable` has enabled it; a command without the mark never takes it, so
/// every earlier form keeps its bytes.
pub const REPO_FORMAT_LEASES: Format = Format { family: 0x20, version: 14 };
/// `fanout` (`design/DESIGN.md`, Shape, `NetworkLoad` and `fanout`): `RepoEvent::Fanout`, tag 39, which a network
/// group's leader records itself when k changes. No client command carries it, so every earlier form keeps its bytes.
pub const REPO_FORMAT_FANOUT: Format = Format { family: 0x20, version: 15 };

/// At most this many ref updates in one `--atomic` push [guess at GitHub's own limit].
pub const ATOMIC_REFS_MAX: usize = 10_000;

/// The design's `PushOptions`: `git push -o` values, in order.
pub type PushOptions = Vec<Box<str>>;

/// SHARED's `Push`. `actor` is the authenticated caller; `credential()` comes from it. `request` is the edge's id for
/// the client request. `provenance`, `protected`, and `changed` come from the git cache's `validate`. They are empty
/// in formats 1, 2, and 7, which carry them as counts of zero, and only format 8 (`REPO_FORMAT_AGENT`) carries any.
#[derive(Clone, Debug, PartialEq)]
pub struct Push {
    pub updates: Vec<RefUpdate>,
    pub atomicity: Atomicity,
    pub pack: Option<PackRef>,
    pub actor: Principal,
    pub options: PushOptions,
    pub request: RequestId,
    /// The client's `agent=` capability. Format 2 and later.
    pub agent: Option<Box<str>>,
    pub provenance: Vec<Provenance>,
    pub protected: Vec<ProtectedHits>,
    pub changed: Vec<ChangedPaths>,
    /// Format 32.14 (`REPO_FORMAT_LEASES`): the edge enabled it, so the push's entry may carry `PathSet` and a
    /// `Leased` refusal. Not encoded: the format tag carries it.
    pub leases: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RejectReason {
    /// The ref's value is not the push's old value: git prints `fetch first`.
    Stale,
    /// Branch rules are Milestone 2; nothing produces this yet.
    NonFastForward,
    /// `refs/pull/` is the host's: GitHub answers `deny updating a hidden ref`.
    Hidden,
    TooManyRefs,
    /// An `AllOrNothing` push in which another ref failed. Not in the design's `RejectReason`, which has no reason for
    /// the refs that would have passed.
    Atomic,
    /// The name and an existing ref (or another ref of the same push) are a directory and a file of each other, such as
    /// `refs/heads/a` and `refs/heads/a/b`: git's refs cannot hold both, so the shard's repository could not either.
    /// Not in the design's `RejectReason`. Found by git's t5541 #20 (`tests/m1/results/m1-wp6.md`).
    NameConflict,
    /// Every ref of an `AllOrNothing` push in which a ref hit a `NameConflict`: upstream receive-pack finds those only
    /// when it commits the transaction, and then reports `atomic transaction failed` on every ref (t5541 #20).
    AtomicTransaction,
    /// An agent rule (`crate::agent`), with the text that names the allowed prefix or the protected path. Only a
    /// format-8 push stores one; an earlier format's is refused whole (`PushErr::Agent`).
    Agent(AgentRefusal),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RefOutcome {
    Ok,
    Rejected(RejectReason),
}

/// One `RefOutcome` per update, in the push's order, with its ref, so `report_status` needs only the `Acked`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushOut {
    pub refs: Vec<(RefName, RefOutcome)>,
}

impl PushOut {
    pub fn outcomes(&self) -> Vec<RefOutcome> {
        self.refs.iter().map(|(_, o)| o.clone()).collect()
    }
}

/// A refusal of the whole command. Nothing is applied and no `request` row is written, so a re-send is decided again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PushErr {
    /// 401.
    Unauthorized(CredentialRefusal),
    /// 403 (404 for a private repository is the renderer's call).
    Forbidden,
    /// An atomic push over `ATOMIC_REFS_MAX`. The edge refuses these before `commit`.
    TooManyRefs { got: usize },
    /// The same ref twice in one push.
    Duplicate(RefName),
    /// An agent rule refused a push whose format cannot store a per-ref agent reason, or the push lacks the
    /// `validate` outputs its class needs (`AgentRule::Reclass`). Nothing is applied.
    Agent(AgentRefusal),
}

/// Wire tags: postcard stores and sends the variants by position, JSON by name (`crates/git/tests/wire_tags.golden`).
/// Add a variant only at the end: never insert, remove, reorder, or rename one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RepoEvent {
    /// SHARED's `Refs { push, actor, updates, pack }`: one push's applied updates and the pack that carries their
    /// objects. `push` is its request id, and `actor` the account, which is all the fold reads of the `Actor`.
    Refs {
        push: RequestId,
        actor: Option<AccountId>,
        updates: Vec<RefUpdate>,
        pack: Option<PackRef>,
    },
    /// The refs one push's `decide` refused, each with its reason, so a re-send answered from the push's `request`
    /// row reports that reason (`Push::replay`) and not `Stale`. It changes no ref, and a consumer of `Refs` skips it.
    /// Not in SHARED's event list; found by git's t5541 #20 (`tests/m1/results/m1-wp8.md`).
    Refused {
        push: RequestId,
        refs: Vec<(RefName, RejectReason)>,
    },
    /// The design's `EntryBody::Promoted`: the objects of every inline pack in `from..=to` packed into one stored pack
    /// by the network's home shard, committed by the leader in an entry of its own with no request
    /// (`graftyard_raft::Member::record`). Ranges follow each other from index 1, and the pack is always stored.
    Promoted(Promoted),
    /// The design's `EntryBody::Archived`: the entries `from..=to` are in the archive object `key`, put create-only in
    /// two buckets, so members truncate their logs through `to` once they can HEAD it in two. It never runs past the
    /// last `Promoted`, and ranges follow each other from index 1.
    Archived(Archived),
    // Milestone 2 (`design/SHARED.md`, "Event types" and "Issues and pull requests"). Tags 4 onward, in this order;
    // each payload encodes by `graftyard_fold::canon`. `crate::tiers::tier_of` gives each its tiers.
    IssueOpened {
        n: IssueNumber,
        capture: IssueCapture,
    },
    PrOpened {
        n: IssueNumber,
        head: HeadSpec,
        base: RefName,
        capture: PrCapture,
    },
    PrClosed {
        n: IssueNumber,
        final_head: ObjectId,
    },
    IssueChanged {
        n: IssueNumber,
        change: IssueChange,
        actor: Option<AccountId>,
    },
    Comment(CommentEvent),
    Review(ReviewEvent),
    Labels(LabelEvent),
    Rule(BranchRule),
    Settings(RepoSettings),
    /// A required conclusion for `sha`. `conclusion: None` is a queued or in-progress run or a pending status, so a
    /// re-run blocks a merge again. For one (sha, name, source), the highest `version` wins.
    RequiredCheck {
        sha: ObjectId,
        name: Box<str>,
        source: CheckSource,
        version: u64,
        conclusion: Option<CheckConclusion>,
    },
    CheckRun(CheckRunUpdate),
    CheckSuite(CheckSuiteUpdate),
    Status(StatusUpdate),
    /// Metadata, plus Core when `tag` is set: publishing a release whose tag does not exist creates the tag in the same
    /// entry. `record: None` deletes the release.
    Release {
        id: RepoReleaseId,
        record: Option<ReleaseRecord>,
        tag: Option<RefUpdate>,
        actor: Option<AccountId>,
    },
    /// Metadata. Committed by the upload after the bytes are a `DurableBlob`. `asset: None` deletes it.
    ReleaseAsset {
        release: RepoReleaseId,
        id: AssetId,
        asset: Option<AssetRecord>,
        actor: Option<AccountId>,
    },
    /// Metadata. Committed by the LFS `verify` action after the bytes are a `DurableBlob`; the batch API answers a
    /// download from these rows.
    LfsObject {
        blob: BlobRef,
        actor: Option<AccountId>,
    },
    /// Metadata [inference: SHARED's `RepoEvent` lists `LfsLock` but its `tier_of` names no tier; a lock gates no git
    /// operation here, since the server does not refuse a push to a locked path, so Core does not need it].
    LfsLock(LfsLockEvent),
    /// Core. One lazy test merge of pull request `n` (SHARED, "Event types"): `apply` records the `Mergeability` and
    /// moves `refs/pull/N/merge` to the merge commit, or deletes it on a conflict. `pack` holds the merge commit's
    /// objects, so every shard can serve the ref.
    TestMerged {
        n: IssueNumber,
        base: ObjectId,
        head: ObjectId,
        result: MergeabilityResult,
        pack: Option<PackRef>,
    },
    /// Core. A consumer's cursor on this chain: every event at or below `through` is handled. It commits in the same
    /// entry as the work it covers (`crate::prsync`), so the position is the group's and survives a leader change.
    /// Not in SHARED's event list, which names cursors but no event that moves one [inference].
    ConsumerCursor {
        consumer: Consumer,
        through: Index,
    },
    /// Both tiers. SHARED's `Merged`: pull request `n` merged at `head`, `base` moving the base branch from the
    /// pre-merge tip to the merge commit, `also` the head branch's deletion. `by` is who merged, which `PrState::Merged`
    /// records [inference: SHARED's `Merged` has no actor field, but its `PrState::Merged` names one] (M2-WP3,
    /// `crate::merge`).
    Merged {
        n: IssueNumber,
        base: RefUpdate,
        head: ObjectId,
        method: MergeMethod,
        also: Vec<RefUpdate>,
        pack: Option<PackRef>,
        verified: TreeDiffDigest,
        engine: EngineVersion,
        by: UserId,
    },
    /// Core. SHARED's `Queue`: the merge queue's events (`crate::queue`).
    Queue(QueueEvent),
    /// Core. An outside collaborator's permission, or `None` to remove it (M3-WP3). Never written for a current
    /// member of an org owner: that is a `Grant` on the org's `Authz` chain.
    Collaborator {
        who: AccountId,
        permission: Option<graftyard_identity::RepoPermission>,
    },
    /// Core. An invitation to become an outside collaborator. Accepting it appends `Collaborator` in the same entry.
    Invitation(graftyard_identity::InvitationEvent),
    // AM1-WP2, format 32.8: tags 27 to 30.
    /// Metadata. The provenance of a format-8 push's applied refs, in the push's entry (`crate::agent::link`).
    Provenance {
        push: RequestId,
        list: Vec<Provenance>,
    },
    /// Core. Pull request `n` was opened by an `AgentApp`'s credential, in the `PrOpened`'s entry. It keeps
    /// `AgentMeta.app` (all of it) in the core `agent_meta` row beside `pull_head`, so `Merge` reads it.
    AgentMeta {
        n: IssueNumber,
        meta: AgentMeta,
    },
    /// Core, in the APPROVE review's entry.
    AgentGate(AgentGate),
    /// Core. The repository's agent policy, whole.
    AgentPolicy(AgentPolicy),
    // AM1-WP3, format 32.12: tags 31 and 32.
    /// Core and Metadata, as `PrOpened`: the task's core row and its metadata row (`crate::task`).
    TaskCreated(crate::task::AgentTask),
    /// Core, always, because the core row keeps when the task last changed (TASK_IDLE), and Metadata too when it sets
    /// `pr` or `summary`. `state` is a new state the task's App set, a cancel, or a derived state (`Cancelled` after
    /// its installation went, `TimedOut` after TASK_IDLE) that the task's next command records.
    TaskUpdated {
        session: SessionId,
        state: Option<crate::task::TaskApiState>,
        pr: Option<IssueNumber>,
        summary: Option<Box<str>>,
        actor: crate::task::TaskActor,
    },
    // Format 32.13 (`crate::issue_ops::call::REPO_FORMAT_REPOS`): tags 33 and 34.
    /// Core. The repository was created over REST or GraphQL (`IssueOp::InitRepo`), in the entry that writes its first
    /// `Settings`: the core `created` row keeps the entry's time, which GHES's `created_at` reads. Not in SHARED's
    /// event list, which has no creation time for a repository [inference].
    Created {
        actor: Option<AccountId>,
    },
    /// Core. SHARED's `Deleted { retain_until, actor }`: the repository was deleted and is kept until `retain_until`.
    /// The core `deleted` row then refuses every client write on the chain.
    Deleted {
        retain_until: Timestamp,
        actor: Option<AccountId>,
    },
    // AM1-WP4, format 32.14: tags 35 to 38 (33 and 34 are 32.13's `Created` and `Deleted`).
    /// Core. `granted` is the entry's `at`. `plan` is the draft pull request whose `graftyard-plan` block declared it
    /// (the design's `plan_block: bool`, with the number its liveness reads).
    LeaseAcquire {
        lease: crate::lease::Lease,
        plan: Option<IssueNumber>,
    },
    /// Core. Changes only `expires`.
    LeaseRenew {
        id: crate::lease::LeaseId,
        expires: Timestamp,
    },
    /// Core.
    LeaseRelease {
        ids: SmallVec<[crate::lease::LeaseId; 4]>,
        by: crate::lease::ActorId,
    },
    /// Metadata, in the push's entry: one row per ref, deleted when the ref's pull request closes.
    PathSet(crate::agent::PathSet),
    // Format 32.15 (`REPO_FORMAT_FANOUT`): tag 39.
    /// Core. The k both warm sites hold for the network (`crate::shard::fanout`), committed by the network group's
    /// leader in an entry of its own with no request, only when k changes. Its core `fanout` row is what a shard's
    /// `ranges` read answers.
    Fanout {
        k: u8,
    },
    // M4-WP1, format 32.16 (`crate::hooks::REPO_FORMAT_HOOKS`): tags 40 and 41.
    /// Core. SHARED's `Hook { id, record, actor }`, plus `created`, which `decide` sets for a hook that had no row,
    /// so the `Webhooks` consumer pings a new hook and not an edited one. `record: None` deletes it.
    Hook {
        id: crate::hooks::HookId,
        record: Option<crate::hooks::HookRecord>,
        actor: Option<AccountId>,
        created: bool,
    },
    /// Core. The `Webhooks` consumer's `HookCursor` on this chain, written only by `HookCursorCmd`.
    HookCursor(crate::hooks::HookCursor),
    // M4-WP2, format 32.19 (`crate::watch::REPO_FORMAT_WATCH`): tag 42.
    /// Metadata. The `Counts` consumer's watcher rows (`crate::watch`), each newer than the row it replaces.
    Watchers(Vec<crate::watch::WatcherUpdate>),
    /// Tag 43, held for `b030-split`'s `Split(crate::split::SplitEntry)` (format 32.20), so the tags after it match
    /// on every branch. Never written or decided: a release that has `Split` replaces this variant with it.
    Reserved43,
    // M4-WP3, format 32.23 (`crate::secrets::REPO_FORMAT_SECRETS`): tags 44 and 45.
    /// Core. SHARED's repository or environment secret: the sealed bytes the client sent. `record: None` deletes it.
    ActionsSecret {
        scope: graftyard_identity::sealed::SecretScope,
        name: graftyard_identity::sealed::ActionsName,
        record: Option<graftyard_identity::sealed::SecretRecord>,
        actor: Option<AccountId>,
    },
    /// Core. SHARED's repository or environment variable. `record: None` deletes it.
    ActionsVariable {
        scope: graftyard_identity::sealed::SecretScope,
        name: graftyard_identity::sealed::ActionsName,
        record: Option<graftyard_identity::sealed::VariableRecord>,
        actor: Option<AccountId>,
    },
    // R24, format 32.24 (`crate::workflow_job::REPO_FORMAT_WORKFLOW_JOB`): tag 46.
    /// Metadata. One Actions job's `workflow_job` step, which `RunOwner` appends (`crate::workflow_job`).
    WorkflowJob(crate::workflow_job::WorkflowJobUpdate),
}

impl RepoEvent {
    /// The ref updates the event applies, in order: a `Refs`'s, a `Release`'s tag, a `Merged`'s base and then `also`,
    /// a `Queue` event's queue refs, and none for a `Refused`.
    pub fn updates(&self) -> Vec<&RefUpdate> {
        match self {
            RepoEvent::Refs { updates, .. } => updates.iter().collect(),
            RepoEvent::Release { tag: Some(t), .. } => vec![t],
            RepoEvent::Merged { base, also, .. } => std::iter::once(base).chain(also).collect(),
            RepoEvent::Queue(q) => q.refs().iter().collect(),
            _ => Vec::new(),
        }
    }

    /// Every ref the event sets (`Some`) or deletes (`None`) in the core tier's `ref` table, in the order `apply`
    /// writes them: `updates`, and a `TestMerged`'s `refs/pull/{n}/merge`. What a git cache shard's catch-up applies
    /// from the entries it reads, so it never reads the whole ref set to stay current.
    pub fn ref_changes(&self) -> Vec<(RefName, Option<ObjectId>)> {
        match self {
            RepoEvent::TestMerged { n, result, .. } => vec![(
                crate::tiers::merge_ref(*n),
                match result {
                    crate::issues::MergeabilityResult::Clean { merge } => Some(*merge),
                    crate::issues::MergeabilityResult::Conflict { .. } => None,
                },
            )],
            _ => self.updates().into_iter().map(|u| (u.name.clone(), u.new)).collect(),
        }
    }
}

/// An entry's ref changes in order, as `LogPacks::refs` carries them: name and new oid in hex, `None` for a delete.
pub fn entry_ref_changes(evs: &[RepoEvent]) -> Vec<(String, Option<String>)> {
    evs.iter().flat_map(RepoEvent::ref_changes).map(|(n, o)| (n.display(), o.map(|o| o.hex()))).collect()
}

/// The most refs one paged ref read answers (`RepoFold::refs_page`, `crate::group::refs_read`): about 80 bytes of JSON
/// a ref, so a page stays under 1 MB whatever the repository's ref count. `repo:65536` holds 571,872 refs, and its
/// whole set in one reply ran the leader out of memory (`tests/m3/results/wire-fix.md`).
pub const REFS_PAGE_MAX: usize = 10_000;

/// The repository chain's fold: its two tiers (`design/SHARED.md`, "Folds"; `crate::tiers`). The core tier's `ref`
/// table is the only copy of the refs: `Push` decides on it and a `fresh` read answers from it, so a push and a pull
/// request's head read one source. `repo` and `owner` are what every member knows without the log; the owner names the
/// `Authz` chain in `deps` until a `Settings` entry names another.
///
/// The tiers are stores the fold owns: `MemStore` in tests, and in a site member handles on its SQLite tables, which
/// clone as handles on the same tables (`graftyard_fold::TierStore`). Until the metadata tier is `loaded`, the fold
/// applies the core tier alone, and a core request never waits for the metadata tier.
#[derive(Clone, Debug)]
pub struct RepoFold {
    pub repo: Option<RepoId>,
    pub owner: Option<AccountId>,
    pub core: Box<dyn TierStore>,
    pub meta: Box<dyn TierStore>,
}

impl Default for RepoFold {
    fn default() -> RepoFold {
        RepoFold { repo: None, owner: None, core: Box::new(MemStore::default()), meta: Box::new(MemStore::default()) }
    }
}

/// Two folds are equal when their tiers hold the same rows: the same `fold_digest` per tier.
impl PartialEq for RepoFold {
    fn eq(&self, o: &RepoFold) -> bool {
        let meta = |f: &RepoFold| f.meta.loaded().then(|| fold_digest(&*f.meta, Tier::Metadata));
        (self.repo, self.owner) == (o.repo, o.owner)
            && fold_digest(&*self.core, Tier::Core) == fold_digest(&*o.core, Tier::Core)
            && meta(self) == meta(o)
    }
}

impl RepoFold {
    pub fn new(repo: RepoId, owner: AccountId) -> RepoFold {
        RepoFold { repo: Some(repo), owner: Some(owner), ..RepoFold::default() }
    }

    /// A fold over tiers a host holds.
    pub fn with_tiers(
        repo: Option<RepoId>,
        owner: Option<AccountId>,
        core: Box<dyn TierStore>,
        meta: Box<dyn TierStore>,
    ) -> RepoFold {
        RepoFold { repo, owner, core, meta }
    }

    /// The tiers' chain-specific half. A fold with no repository (the CLI's latency probe) uses repository 0.
    pub fn tiers(&self) -> RepoTiers {
        RepoTiers {
            repo: self.repo.unwrap_or(RepoId(graftyard_types::DbId(0))),
            owner: self.owner.unwrap_or(AccountId(graftyard_types::DbId(0))),
        }
    }

    pub fn get(&self, name: &RefName) -> Option<ObjectId> {
        get_rec(&*self.core, "ref", &[ref_key(name)])
    }

    /// Every ref, in name order.
    pub fn refs(&self) -> Vec<(RefName, ObjectId)> {
        self.core
            .prefix("ref", &[])
            .into_iter()
            .filter_map(|(k, v)| match (k.first(), v.first()) {
                (Some(Val::Blob(n)), Some(Val::Blob(o))) => {
                    Some((RefName::parse(n).ok()?, postcard::from_bytes(o).ok()?))
                }
                _ => None,
            })
            .collect()
    }

    /// One page of a paged ref read: at most `limit` refs whose names start with `prefix`, in name order, after the
    /// name `after` (from the first when `None`), and whether more follow. It reads at most `limit + 2` rows of the
    /// core tier, so its cost does not grow with the repository's ref count.
    pub fn refs_page(&self, prefix: &[u8], after: Option<&[u8]>, limit: usize) -> (Vec<(RefName, ObjectId)>, bool) {
        let start = match after {
            Some(a) if a >= prefix => a,
            _ => prefix,
        };
        let rows = self.core.scan("ref", &[Val::Blob(start.into())], limit + 2);
        let mut refs = rows
            .into_iter()
            .filter_map(|(k, v)| match (k.first(), v.first()) {
                (Some(Val::Blob(n)), Some(Val::Blob(o))) => Some((n.clone(), o.clone())),
                _ => None,
            })
            .filter(|(n, _)| Some(&**n) != after)
            .take_while(|(n, _)| n.starts_with(prefix))
            .filter_map(|(n, o)| Some((RefName::parse(&n).ok()?, postcard::from_bytes(&o).ok()?)));
        let page: Vec<(RefName, ObjectId)> = refs.by_ref().take(limit).collect();
        (page, refs.next().is_some())
    }

    /// Whether the core tier holds more than `max` refs, read in at most `max + 1` rows.
    pub fn refs_over(&self, max: usize) -> bool {
        self.core.scan("ref", &[], max + 1).len() > max
    }

    pub fn promoted(&self) -> Option<Promoted> {
        get_rec(&*self.core, "promoted", &[])
    }

    pub fn archived(&self) -> Option<Archived> {
        get_rec(&*self.core, "archived", &[])
    }

    /// The owner whose `Authz` chain gates the repository: `RepoSettings.owner` once a `Settings` entry committed.
    pub fn authz_owner(&self) -> Option<AccountId> {
        get_rec::<RepoSettings>(&*self.core, "settings", &[]).map(|s| s.owner).or(self.owner)
    }

    /// The enterprise whose `Authz` chain `deps` also reads: `RepoSettings.enterprise`, none before a `Settings`.
    pub fn authz_enterprise(&self) -> Option<AccountId> {
        get_rec::<RepoSettings>(&*self.core, "settings", &[]).and_then(|s| s.enterprise)
    }

    /// The first index still in the chain's log (`ChainLog::first_index`).
    /// The k the group's leader last committed for the network (`RepoEvent::Fanout`), 1 before any.
    pub fn fanout_k(&self) -> u8 {
        get_rec::<u8>(&*self.core, "fanout", &[]).unwrap_or(1)
    }

    pub fn first_index(&self) -> Index {
        Index(self.archived().map_or(0, |a| a.to.0) + 1)
    }

    /// Applies one committed entry to the core tier, and to the metadata tier when it is loaded. A tier refuses an
    /// entry that does not follow its own `applied`; that is a host bug, so it panics as `GroupFold::apply` does.
    pub fn apply_core(&mut self, e: &LogEntry<RepoEvent>) {
        let t = self.tiers();
        graftyard_fold::apply(&t, &mut *self.core, Tier::Core, e).expect("core tier applies the next entry");
    }

    pub fn apply_meta(&mut self, e: &LogEntry<RepoEvent>) {
        let t = self.tiers();
        if self.meta.loaded() {
            graftyard_fold::apply(&t, &mut *self.meta, Tier::Metadata, e)
                .expect("metadata tier applies the next entry");
        }
    }
}

/// `verify`'s rule for a `Promoted` after `prev`: the range starts right after the previous one (at 1 for the first),
/// is not empty, and names a stored pack.
fn check_promoted(prev: Option<&Promoted>, p: &Promoted) -> Result<(), Box<str>> {
    let from = Index(prev.map_or(0, |q| q.to.0) + 1);
    if p.from != from || p.to < p.from {
        return Err(format!("promoted {:?}..={:?} where the next range starts at {from:?}", p.from, p.to).into());
    }
    if !matches!(p.pack, PackRef::Stored { .. }) {
        return Err("a promoted pack is always stored".into());
    }
    Ok(())
}

/// `verify`'s rule for an `Archived` after `prev` with `promoted` the last promotion: the range starts right after
/// the previous one, ends at or before the last promotion's `to`, and its key is the `Archive` key of this chain,
/// range, and digest. Stricter than the design, which lets a range that names no inline pack go without `Promoted`
/// [inference]: the fold cannot see the range's entries, so every archived range must be promoted first. An
/// `Archived` whose key is a `Snapshot` of this chain names a snapshot of one tier at `to`, from 0, with no ordering
/// against the log's archive ranges.
fn check_archived(
    repo: Option<RepoId>,
    prev: Option<&Archived>,
    promoted: Option<&Promoted>,
    a: &Archived,
) -> Result<(), Box<str>> {
    if let R2Key::Snapshot { chain, from, to, digest, .. } = &a.key {
        if repo.map(ChainId::Repo) != Some(*chain) || *from != Index(0) || *to != a.to || *digest != a.digest {
            return Err(format!("snapshot key {} does not name this chain at {:?} from 0", a.key, a.to).into());
        }
        return Ok(());
    }
    let from = Index(prev.map_or(0, |q| q.to.0) + 1);
    if a.from != from || a.to < a.from {
        return Err(format!("archived {:?}..={:?} where the next range starts at {from:?}", a.from, a.to).into());
    }
    if promoted.is_none_or(|p| a.to > p.to) {
        return Err(format!("archived through {:?} past the last promotion", a.to).into());
    }
    let key = repo.map(|r| R2Key::Archive { chain: ChainId::Repo(r), from: a.from, to: a.to, digest: a.digest });
    if key.as_ref() != Some(&a.key) {
        return Err(format!("archive key {} does not name this chain, range, and digest", a.key).into());
    }
    if a.to_at < a.from_at {
        return Err("archived range ends before it starts".into());
    }
    Ok(())
}

/// The fold at `applied` with an entry's earlier events written on top, by the same code `apply` runs: what `decide`
/// and `verify` read. Only the tiers the entry's tag names are written, as only those apply it; `check` refuses an
/// event whose tiers the tag leaves out. `check` reads only the core tier, so a follower whose metadata tier is still
/// loading verifies as any other; a command whose `decide` reads the metadata tier finds `view()` empty then.
pub struct RepoScratch<'a> {
    base: &'a RepoFold,
    tag: TierSet,
    at: Timestamp,
    core: Overlay<'a, dyn Read + 'a>,
    meta: Option<Overlay<'a, dyn Read + 'a>>,
}

impl RepoScratch<'_> {
    /// The core tier with the entry's earlier events on top.
    pub fn core(&self) -> &dyn Read {
        &self.core
    }

    pub fn get(&self, name: &RefName) -> Option<ObjectId> {
        get_rec(&self.core, "ref", &[ref_key(name)])
    }

    pub fn fold(&self) -> &RepoFold {
        self.base
    }

    /// The entry's time: the leader's `now` when it decides, the entry's `at` when a follower verifies.
    pub fn at(&self) -> Timestamp {
        self.at
    }

    /// The index the fold has applied, which this entry follows: what a task's `snapshot` records.
    pub fn applied(&self) -> graftyard_types::Index {
        graftyard_fold::meta(&*self.base.core).applied
    }

    /// The owner whose `Authz` chain gates the repository, with the entry's earlier `Settings` on top.
    pub fn authz_owner(&self) -> Option<AccountId> {
        get_rec::<RepoSettings>(&self.core, "settings", &[]).map(|s| s.owner).or(self.base.owner)
    }

    /// Both tiers as `IssueCmd::decide` reads them. `None` while the metadata tier is not loaded.
    pub fn view(&self) -> Option<(&dyn Read, &dyn Read)> {
        Some((&self.core, self.meta.as_ref()?))
    }

    /// The refs, in the fold or written earlier in the entry, that `name` is a directory or a file of.
    pub fn conflicts(&self, name: &RefName) -> Vec<RefName> {
        let b = name.as_bytes();
        let mut out: Vec<RefName> = Vec::new();
        let mut consider = |n: &RefName| {
            if dir_or_file(name, n) && self.get(n).is_some() && !out.contains(n) {
                out.push(n.clone());
            }
        };
        // Its parents: each prefix that ends before a '/'.
        for (i, _) in b.iter().enumerate().filter(|(_, c)| **c == b'/') {
            if let Ok(p) = RefName::parse(&b[..i]) {
                consider(&p);
            }
        }
        // Its children: refs under "name/". Ref keys are blobs, which sort by bytes, so they are one run from "name/".
        let mut dir = b.to_vec();
        dir.push(b'/');
        let mut from = vec![Val::Blob(dir.clone().into())];
        // Few refs sit under a name, often none, so the scan reads a few rows at a time: one large read here costs
        // every push in an entry a copy of the rows the entry wrote after it (BF-20).
        loop {
            let batch = self.core.scan("ref", &from, 8);
            let full = batch.len() == 8;
            for (k, _) in &batch {
                let Some(Val::Blob(n)) = k.first() else { continue };
                if !n.starts_with(&dir) {
                    return out;
                }
                if let Ok(n) = RefName::parse(n) {
                    consider(&n);
                }
            }
            match batch.last() {
                Some((k, _)) if full => {
                    let Some(Val::Blob(n)) = k.first() else { return out };
                    // The next key after `n`: `n` followed by a zero byte.
                    let mut next = n.to_vec();
                    next.push(0);
                    from = vec![Val::Blob(next.into())];
                }
                _ => return out,
            }
        }
    }
}

/// Whether one name is a proper path prefix of the other at a '/': `refs/heads/a` and `refs/heads/a/b`.
pub fn dir_or_file(a: &RefName, b: &RefName) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let (short, long) = if a.len() < b.len() { (a, b) } else { (b, a) };
    long.len() > short.len() && long.starts_with(short) && long[short.len()] == b'/'
}

impl Scratch for RepoScratch<'_> {
    type Event = RepoEvent;

    /// `ExtendLeases` on the core tier, as `RepoTiers::on_extend` applies it.
    fn extend(&mut self, x: graftyard_raft::ExtendLeases) {
        if has(self.tag, Tier::Core) {
            crate::lease::extend(&mut self.core, x.since, x.by);
        }
    }

    /// Writes the event into the tiers the entry's tag names, as `apply` will.
    fn apply(&mut self, ev: &RepoEvent) {
        let (t, set) = (self.base.tiers(), tier_of(ev));
        if has(set, Tier::Core) && has(self.tag, Tier::Core) {
            t.write(ev, Tier::Core, self.at, &mut self.core);
        }
        if let Some(m) = self.meta.as_mut()
            && has(set, Tier::Metadata)
            && has(self.tag, Tier::Metadata)
        {
            t.write(ev, Tier::Metadata, self.at, m);
        }
    }

    /// `verify`'s replay: the entry's tag covers the event's tiers, each update's `expect` holds against the running
    /// state, and `Promoted` and `Archived` follow the last committed ranges. An entry whose order or base differs
    /// from the follower's fold is refused. Only the core tier is read, so a member whose metadata tier is not loaded
    /// verifies the same.
    fn check(&self, ev: &RepoEvent) -> Result<(), Box<str>> {
        let needs = tier_of(ev);
        if needs.0 & !self.tag.0 != 0 {
            return Err(format!("the entry is tagged {:?} but the event needs {needs:?}", self.tag).into());
        }
        let promoted: Option<Promoted> = get_rec(&self.core, "promoted", &[]);
        let updates = match ev {
            RepoEvent::Refs { .. } | RepoEvent::Merged { .. } | RepoEvent::Queue(_) => ev.updates(),
            RepoEvent::Promoted(p) => return check_promoted(promoted.as_ref(), p),
            RepoEvent::Archived(a) => {
                let prev: Option<Archived> = get_rec(&self.core, "archived", &[]);
                return check_archived(self.base.repo, prev.as_ref(), promoted.as_ref(), a);
            }
            _ => return Ok(()),
        };
        let mut seen: BTreeMap<&RefName, Option<ObjectId>> = BTreeMap::new();
        for u in updates {
            let have = seen.get(&u.name).copied().unwrap_or_else(|| self.get(&u.name));
            if have != u.expect {
                return Err(format!("{:?} is {:?}, the event expects {:?}", u.name, have, u.expect).into());
            }
            seen.insert(&u.name, u.new);
        }
        Ok(())
    }
}

impl Fold for RepoFold {
    type Event = RepoEvent;
    type Scratch<'a> = RepoScratch<'a>;

    fn scratch(&self) -> RepoScratch<'_> {
        self.scratch_at(Timestamp(0), TierSet(u8::MAX))
    }

    fn scratch_at(&self, at: Timestamp, tiers: TierSet) -> RepoScratch<'_> {
        RepoScratch {
            base: self,
            tag: tiers,
            at,
            core: Overlay::new(&self.core as &dyn Read),
            meta: self.meta.loaded().then(|| Overlay::new(&self.meta as &dyn Read)),
        }
    }

    /// Never called: a tiered fold applies whole entries (`apply_entry`), which carry the index, `prev`, time, and tag
    /// a tier checks and stamps.
    fn apply(&mut self, _ev: &RepoEvent) {
        unreachable!("RepoFold applies whole entries")
    }

    fn apply_entry(&mut self, e: &LogEntry<RepoEvent>) {
        self.apply_core(e);
        self.apply_meta(e);
    }

    fn tiers(ev: &RepoEvent) -> TierSet {
        tier_of(ev)
    }

    /// A `request` row keeps a push's `Refs` without its inline pack's bytes: `Push::replay` reads only the refs,
    /// and the sweeper's `RequestSource` only stored packs (`packs_of`), while the bytes stay in the log entry. Two
    /// days of 1 MB inline packs in every member's memory ran its isolate out of memory (F45).
    fn row_event(ev: &RepoEvent) -> RepoEvent {
        match ev {
            RepoEvent::Refs { push, actor, updates, pack: Some(PackRef::Inline(_)) } => {
                RepoEvent::Refs { push: *push, actor: *actor, updates: updates.clone(), pack: None }
            }
            other => other.clone(),
        }
    }

    /// One mark per write command, on the event that command is for, so DEMOTE_PER_MIN counts commands and not
    /// events: a push's `Refs` (not its `Provenance` or `PathSet`), a pull request's `PrOpened` (not its `AgentMeta`
    /// or plan lease), a review's `Review` (not its `AgentGate`), a check run's `CheckRun` and a status's `Status` (not
    /// their `RequiredCheck`), a creation's `Created` (not its `Settings`). Events with no actor mark nothing, and nor
    /// do the task API's (`TaskCreated`, `TaskUpdated`), which DEMOTE_PER_MIN excludes (DESIGN, Shape, "Agent load
    /// and agent safety"). `push` is a `Refs` that moved a client ref: PrSync's `refs/pull/` moves are host work. Lease
    /// renews and releases carry no actor or ride another command (a plan edit, a task's end), so only a lease API
    /// acquire marks. `Merged` marks `by`, which for a queue landing is the enqueuer, so a queued pull request counts
    /// twice for its enqueuer (enqueue and landing) [inference: the event does not say which path merged it]. A `Refs`
    /// names only a user's account, so an App's own push counts for `hot` but not toward its App's DEMOTE_PER_MIN.
    fn write_mark(ev: &RepoEvent) -> Option<graftyard_raft::admit::WriteMark> {
        use crate::issues::{CommentEvent as C, LabelEvent, LfsLockEvent, ReviewEvent as R};
        use graftyard_identity::InvitationEvent as I;
        use graftyard_raft::admit::{ActorKey, WriteMark};
        // `ActorKey` as `group::AdmitWire::of` keys it: a user's account id, or an App's id with the top bit set.
        let account = |a: AccountId| Some(ActorKey(a.0.0 as u64));
        let app = |a: AppId| Some(ActorKey(1 << 63 | a.0.0 as u64));
        let by = |a: &Option<AccountId>| a.and_then(account);
        let actor = match ev {
            RepoEvent::Refs { actor, updates, .. } => {
                let push = !updates.iter().all(|u| u.name.starts_with("refs/pull/"));
                return Some(WriteMark { actor: by(actor), push });
            }
            RepoEvent::IssueOpened { capture, .. } => by(&capture.author),
            RepoEvent::PrOpened { capture, .. } => by(&capture.issue.author),
            RepoEvent::IssueChanged { actor, .. }
            | RepoEvent::Release { actor, .. }
            | RepoEvent::ReleaseAsset { actor, .. }
            | RepoEvent::LfsObject { actor, .. }
            | RepoEvent::Created { actor }
            | RepoEvent::Deleted { actor, .. }
            | RepoEvent::Hook { actor, .. }
            | RepoEvent::ActionsSecret { actor, .. }
            | RepoEvent::ActionsVariable { actor, .. } => by(actor),
            RepoEvent::Comment(
                C::Created { author: a, .. } | C::Edited { actor: a, .. } | C::Deleted { actor: a, .. },
            )
            | RepoEvent::Comment(C::Resolved { actor: a, .. })
            | RepoEvent::Review(
                R::Submitted { reviewer: a, .. }
                | R::Dismissed { actor: a, .. }
                | R::Requested { actor: a, .. }
                | R::SingleComment { reviewer: a, .. },
            )
            | RepoEvent::Labels(LabelEvent::Applied { actor: a, .. })
            | RepoEvent::LfsLock(LfsLockEvent::Deleted { actor: a, .. })
            | RepoEvent::Invitation(I::Sent { inviter: a, .. } | I::Cancelled { actor: a, .. }) => by(a),
            RepoEvent::LfsLock(LfsLockEvent::Created { owner, .. }) => account(*owner),
            RepoEvent::Status(s) => account(s.creator),
            RepoEvent::CheckRun(c) => app(c.app),
            RepoEvent::CheckSuite(c) => app(c.app),
            RepoEvent::Merged { by: u, .. } | RepoEvent::Queue(QueueEvent::Enqueued { actor: u, .. }) => account(u.0),
            RepoEvent::LeaseAcquire { lease, plan: None } => match lease.holder.actor {
                crate::lease::ActorId::User(u) => account(u.0),
                crate::lease::ActorId::App { app: a, .. } => app(a),
            },
            _ => None,
        };
        actor.map(|a| WriteMark { actor: Some(a), push: false })
    }
}

/// The ref namespaces a push may not write: `refs/pull/`, and the host-owned `refs/graftyard/` (`agent::HOST_REFS`),
/// whose only writer is `graftyard-workspace` appending its live task's session log, and no task exists before
/// AM1-WP3, so every update there is refused as GitHub refuses a hidden ref.
pub fn hidden(name: &RefName) -> bool {
    name.starts_with("refs/pull/") || name.starts_with(crate::agent::HOST_REFS)
}

/// `authorize_write` for a push, over the owner's and the enterprise's `Authz` folds the leader read and the core
/// tier's collaborator rows: an admin, a `Write` grant, a team's, the org's base permission, or a `Collaborator` row.
fn may_write(actor: &Principal, s: &RepoScratch<'_>, deps: &[(ChainId, IdentityDep)]) -> bool {
    let (Some(repo), Some(owner)) = (s.fold().repo, s.authz_owner()) else { return false };
    if crate::tiers::deleted(s.core()).is_some() {
        return false;
    }
    let access = crate::issue_ops::Access { repo, owner, core: s.core() };
    let folds = authz_folds(deps);
    graftyard_identity::authorize_write(actor, WriteAction::Push, &folds, &access) == Decision::Allow
        && graftyard_identity::token_allows(
            stored_fields(actor, deps),
            &[("contents", graftyard_identity::Access::Write)],
            &access,
        ) == Decision::Allow
}

/// A stored token's optional fields (a `github_pat_`'s or `ghu_`'s scope), from its `Tokens` row the leader read as
/// a dependency of this entry.
pub(crate) fn stored_fields<'d>(
    actor: &Principal,
    deps: &'d [(ChainId, IdentityDep)],
) -> Option<&'d graftyard_identity::TokenFields> {
    let graftyard_identity::Credential::Stored(hash) = &actor.credential else { return None };
    let shard = ChainId::Tokens(graftyard_identity::shard_of_hash(hash));
    deps.iter().find_map(|(c, d)| match d {
        IdentityDep::Tokens(t) if *c == shard => graftyard_identity::TokensRead::fields(t, hash),
        _ => None,
    })
}

/// Whether the update would need a branch rule's bypass: a deletion a rule forbids, or a push a rule restricts and
/// allows only because it lists the App. The fold cannot see a non-fast-forward, which `validate` would report.
fn needs_bypass(
    s: &RepoScratch<'_>,
    name: &RefName,
    expect: Option<ObjectId>,
    new: Option<ObjectId>,
    app: AppId,
) -> bool {
    crate::merge::rules_over(s.core(), name).iter().filter_map(|r| r.protection.as_ref()).any(|p| {
        let deletes = new.is_none() && expect.is_some() && !p.deletions;
        let listed = p.push.as_ref().is_some_and(|l| l.contains(&crate::issues::Pusher::App(app)));
        deletes || listed
    })
}

/// Every `Authz` fold among the deps: identity's `permission` takes the owner's and the enterprise's by owner id.
pub(crate) fn authz_folds(deps: &[(ChainId, IdentityDep)]) -> SmallVec<[&dyn AuthzRead; 2]> {
    deps.iter()
        .filter_map(|(c, d)| match (c, d) {
            (ChainId::Authz(o), IdentityDep::Authz(f)) if f.owner() == Some(*o) => Some(f as &dyn AuthzRead),
            _ => None,
        })
        .collect()
}

/// An App's command also reads the install's own `Authz(Genesis.account)` chain, which holds the `AgentApp` rows.
pub(crate) fn with_install(d: &mut SmallVec<[ChainId; 3]>, actor: &Principal) {
    let install = ChainId::Authz(graftyard_identity::install_account());
    if matches!(actor.actor, Actor::App { .. }) && !d.contains(&install) {
        d.push(install);
    }
}

/// The install's own `Authz` fold among the deps.
pub(crate) fn install_authz(deps: &[(ChainId, IdentityDep)]) -> Option<&dyn AuthzRead> {
    crate::agent::install_fold(deps, |d| match d {
        IdentityDep::Authz(f) => Some(f as &dyn AuthzRead),
        _ => None,
    })
}

/// The `Authz` chains a user command on the repository reads: the owner's, and the enterprise's, both from
/// `RepoSettings` once a `Settings` entry committed, so a transfer or an enterprise change moves them by entry.
pub(crate) fn authz_chains(owner: Option<AccountId>, enterprise: Option<AccountId>) -> impl Iterator<Item = ChainId> {
    owner.into_iter().chain(enterprise).map(ChainId::Authz)
}

impl Push {
    /// Whether the push carries any `validate` output, which only format 8 encodes.
    pub fn validated(&self) -> bool {
        !(self.provenance.is_empty() && self.protected.is_empty() && self.changed.is_empty())
    }

    /// The agent rules for an `Agent` pusher, on each update that passed the host-owned and stale checks, in the
    /// design's order. A format-8 push gets a per-ref `RejectReason::Agent`; an earlier format cannot store one, so
    /// the whole push is refused. A protected-path check needs `validate`'s output for the ref: without it the push is
    /// refused `Reclass`, so the edge reads the install's rows again and re-sends with them.
    /// Last, under `AgentsExclusive` (`lease`): an update whose `changed` paths overlap another holder's live
    /// Exclusive lease is refused `Leased`, per ref in format 32.14 and whole in an earlier one, whose entries cannot
    /// carry the new rule.
    fn agent_rules(
        &self,
        s: &RepoScratch<'_>,
        a: &crate::agent::AgentActor,
        install: Option<&dyn AuthzRead>,
        session_ref: Option<&RefName>,
        lease: Option<&crate::lease::LeaseView<'_>>,
        refs: &mut [RefOutcome],
    ) -> Result<(), PushErr> {
        let holder = crate::lease::ActorId::of(&self.actor).map(|actor| crate::lease::LeaseHolder {
            actor,
            app: Some(a.app),
            session: a.task.clone(),
        });
        for (u, r) in self.updates.iter().zip(refs.iter_mut()) {
            if *r != RefOutcome::Ok || Some(&u.name) == session_ref {
                continue;
            }
            let bypass = needs_bypass(s, &u.name, u.expect, u.new, a.app);
            let mut why = crate::agent::refuse_update(install, a, &u.name, u.new, &self.protected, bypass);
            let changed = self.changed.iter().find(|c| c.refname == u.name);
            if why.is_none() && u.new.is_some() && changed.is_none() {
                why = Some(AgentRefusal::of(AgentRule::Reclass));
            }
            if why.is_none()
                && let (Some(v), Some(c), Some(h)) = (lease, changed, holder.as_ref())
            {
                let globs: Vec<&str> = c.paths.iter().map(|p| &**p).collect();
                why = crate::agent::leased(&v.exclusive_conflicts(&globs, h));
            }
            let per_ref = match why.as_ref().map(|w| w.rule) {
                Some(AgentRule::Leased) => self.format() == REPO_FORMAT_LEASES,
                Some(AgentRule::Reclass) | None => false,
                Some(_) => per_ref_agent(self.format()),
            };
            match why {
                None => {}
                Some(w) if per_ref => *r = RefOutcome::Rejected(RejectReason::Agent(w)),
                Some(w) => return Err(PushErr::Agent(w)),
            }
        }
        Ok(())
    }

    /// The `PathSet` rows of a 32.14 push: one per applied create or move that `validate` gave `changed` for, with
    /// the ref's open pull request and the paths of the live leases of the pusher's task, or of the pusher.
    fn path_sets(
        &self,
        s: &RepoScratch<'_>,
        lease: Option<&crate::lease::LeaseView<'_>>,
        applied: &[RefUpdate],
    ) -> Vec<crate::agent::PathSet> {
        if self.format() != REPO_FORMAT_LEASES {
            return Vec::new();
        }
        let planned: Vec<Box<str>> = match (lease, crate::lease::ActorId::of(&self.actor)) {
            (Some(v), Some(actor)) => {
                let task = crate::task::claim(&self.actor);
                let mut p: Vec<Box<str>> = v
                    .live()
                    .into_iter()
                    .filter(|r| match task {
                        Some(t) => r.lease.holder.session.as_ref() == Some(t),
                        None => r.lease.holder.actor == actor,
                    })
                    .flat_map(|r| r.lease.scope.paths.into_iter().map(|g| Box::from(g.as_str())))
                    .collect();
                p.sort();
                p.dedup();
                p
            }
            _ => Vec::new(),
        };
        let repo = s.fold().repo;
        applied
            .iter()
            .filter_map(|u| {
                let head = u.new?;
                let c = self.changed.iter().find(|c| c.refname == u.name)?;
                let pr = repo.and_then(|r| crate::tiers::pulls_from(s.core(), r, &u.name).into_iter().next());
                Some(crate::agent::PathSet {
                    refname: u.name.clone(),
                    pr,
                    head,
                    merge_base: None,
                    paths: c.paths.clone(),
                    planned: planned.clone(),
                    truncated: c.truncated,
                })
            })
            .collect()
    }

    fn decide_refs(
        &self,
        s: &RepoScratch<'_>,
        agent: Option<(&crate::agent::AgentActor, Option<&dyn AuthzRead>)>,
        session_ref: Option<&RefName>,
        lease: Option<&crate::lease::LeaseView<'_>>,
    ) -> Result<(SmallVec<[RepoEvent; 4]>, PushOut), PushErr> {
        if self.atomicity == Atomicity::AllOrNothing && self.updates.len() > ATOMIC_REFS_MAX {
            return Err(PushErr::TooManyRefs { got: self.updates.len() });
        }
        let mut names = std::collections::BTreeSet::new();
        for u in &self.updates {
            if !names.insert(&u.name) {
                return Err(PushErr::Duplicate(u.name.clone()));
            }
        }
        let mut refs: Vec<RefOutcome> = self
            .updates
            .iter()
            .map(|u| {
                if hidden(&u.name) {
                    RefOutcome::Rejected(RejectReason::Hidden)
                } else if s.get(&u.name) != u.expect {
                    RefOutcome::Rejected(RejectReason::Stale)
                } else {
                    RefOutcome::Ok
                }
            })
            .collect();
        // The one write under `refs/graftyard/`: `graftyard-workspace` creating or moving its live task's session ref
        // from the tip it names. Deletion runs only through a retention command. That a move is a fast-forward is
        // `validate`'s to report, as for any non-fast-forward the fold cannot see.
        if let Some(own) = session_ref {
            for (u, r) in self.updates.iter().zip(refs.iter_mut()) {
                if u.name == *own && u.new.is_some() && s.get(&u.name) == u.expect {
                    *r = RefOutcome::Ok;
                }
            }
        }
        if let Some((a, install)) = agent {
            let enforce = lease
                .filter(|v| crate::tiers::agent_policy(v.core).leases == crate::agent::LeaseEnforce::AgentsExclusive);
            self.agent_rules(s, a, install, session_ref, enforce, &mut refs)?;
        }
        // A create whose name is a directory or a file of a ref that stays: one of this push's other passing updates,
        // or a fold ref this push does not delete.
        let after = |n: &RefName, refs: &[RefOutcome]| -> bool {
            match self.updates.iter().zip(refs).find(|(u, r)| &u.name == n && **r == RefOutcome::Ok) {
                Some((u, _)) => u.new.is_some(),
                None => s.get(n).is_some(),
            }
        };
        // Against the outcomes before this pass, so two creates that conflict with each other are both refused.
        let checked = refs.clone();
        for (i, u) in self.updates.iter().enumerate() {
            if checked[i] != RefOutcome::Ok || u.new.is_none() || s.get(&u.name).is_some() {
                continue;
            }
            let clash = s.conflicts(&u.name).iter().any(|n| after(n, &checked))
                || self.updates.iter().enumerate().any(|(j, v)| {
                    j != i && checked[j] == RefOutcome::Ok && v.new.is_some() && dir_or_file(&u.name, &v.name)
                });
            if clash {
                refs[i] = RefOutcome::Rejected(RejectReason::NameConflict);
            }
        }
        let failed = refs.iter().any(|r| *r != RefOutcome::Ok);
        let conflict = refs.contains(&RefOutcome::Rejected(RejectReason::NameConflict));
        let refs = if conflict && self.atomicity == Atomicity::AllOrNothing {
            vec![RefOutcome::Rejected(RejectReason::AtomicTransaction); refs.len()]
        } else if failed && self.atomicity == Atomicity::AllOrNothing {
            refs.into_iter()
                .map(|r| if r == RefOutcome::Ok { RefOutcome::Rejected(RejectReason::Atomic) } else { r })
                .collect()
        } else {
            refs
        };
        let applied: Vec<RefUpdate> =
            self.updates.iter().zip(&refs).filter(|(_, r)| **r == RefOutcome::Ok).map(|(u, _)| u.clone()).collect();
        let refs = PushOut { refs: self.updates.iter().map(|u| u.name.clone()).zip(refs).collect() };
        let refused: Vec<(RefName, RejectReason)> = refs
            .refs
            .iter()
            .filter_map(|(n, o)| match o {
                RefOutcome::Rejected(r) => Some((n.clone(), r.clone())),
                RefOutcome::Ok => None,
            })
            .collect();
        let mut events = SmallVec::new();
        if !applied.is_empty() {
            let account = match self.actor.actor {
                Actor::User(u) => Some(u.0),
                Actor::App { .. } | Actor::Anonymous => None,
            };
            // A pack with zero objects names nothing, and a push of deletes alone has none.
            let pack = if applied.iter().all(|u| u.new.is_none()) { None } else { self.pack.clone() };
            // Provenance of the refs the push moved, as `validate` gave it. A `Session` is kept only when the pusher is
            // an App's installation and the session is a task of that App here, live or final, or its token's live
            // task. A `Session` the edge linked (`crate::agent::link`) that no such task names is dropped, and every
            // other trailer stays `Claimed` (`crate::agent::unlinked`).
            let moved: Vec<Provenance> =
                self.provenance.iter().filter(|p| applied.iter().any(|u| u.name == p.refname)).cloned().collect();
            let own = match self.actor.actor {
                Actor::App { app, .. } => crate::task::own_sessions(
                    s.core(),
                    app,
                    agent.and_then(|(a, _)| a.task.as_ref()),
                    crate::agent::named_sessions(&moved),
                ),
                _ => Vec::new(),
            };
            let list = crate::agent::unlinked(&moved, &own);
            let path_sets = self.path_sets(s, lease, &applied);
            events.push(RepoEvent::Refs { push: self.request, actor: account, updates: applied, pack });
            if !list.is_empty() {
                events.push(RepoEvent::Provenance { push: self.request, list });
            }
            events.extend(path_sets.into_iter().map(RepoEvent::PathSet));
        }
        if !refused.is_empty() {
            events.push(RepoEvent::Refused { push: self.request, refs: refused });
        }
        Ok((events, refs))
    }

    /// The command's canonical encoding with no pack: what `COMMAND_MAX` measures for "the ref updates alone".
    pub fn encoded_len_without_pack(&self) -> usize {
        let mut v = Vec::new();
        self.encode_with(&mut v, None);
        v.len()
    }

    fn encode_with(&self, out: &mut Vec<u8>, pack: Option<&PackRef>) {
        let mut e = Enc(out);
        e.u8(match self.atomicity {
            Atomicity::AllOrNothing => 0,
            Atomicity::PerRef => 1,
        });
        e.count(self.updates.len());
        for u in &self.updates {
            e.bytes(u.name.as_bytes());
            e.option(u.expect.as_ref(), |e, o| e.0.extend_from_slice(&o.0));
            e.option(u.new.as_ref(), |e, o| e.0.extend_from_slice(&o.0));
        }
        e.option(pack, encode_pack);
        encode_principal(&mut e, &self.actor);
        if [REPO_FORMAT_TASKS, REPO_FORMAT_LEASES].contains(&self.format()) {
            encode_task_claim(&mut e, &self.actor);
        }
        e.count(self.options.len());
        for o in &self.options {
            e.str(o);
        }
        e.u128(self.request.0.0);
        // provenance, protected, changed: counts of zero below format 8, which `format()` picks when any is not empty.
        encode_validated(&mut e, self);
        if self.format() != REPO_FORMAT_V1 {
            e.option(self.agent.as_ref(), |e, a| e.str(a));
        }
    }

    /// The bytes a member decodes: the format, then the canonical encoding.
    pub fn to_wire(&self) -> Vec<u8> {
        let f = self.format();
        let mut out = Vec::with_capacity(64 + self.updates.len() * 64);
        out.extend_from_slice(&f.family.to_be_bytes());
        out.extend_from_slice(&f.version.to_be_bytes());
        self.encode(&mut out);
        out
    }

    /// Decodes `to_wire` bytes in any format this release reads. `Err(Some(format))` names a format it does not.
    pub fn from_wire(b: &[u8]) -> Result<Push, WireError> {
        let mut d = Dec(b);
        let format = Format { family: d.u16()?, version: d.u16()? };
        let reads = [
            REPO_FORMAT_V1,
            REPO_FORMAT_V2,
            REPO_FORMAT_STATELESS,
            REPO_FORMAT_AGENT,
            REPO_FORMAT_TASKS,
            REPO_FORMAT_LEASES,
        ];
        if !reads.contains(&format) {
            return Err(WireError::Format(format));
        }
        let atomicity = match d.u8()? {
            0 => Atomicity::AllOrNothing,
            1 => Atomicity::PerRef,
            _ => return Err(WireError::Malformed("atomicity")),
        };
        let n = d.u32()? as usize;
        let mut updates = Vec::with_capacity(n.min(1 << 16));
        for _ in 0..n {
            let name = RefName::parse(d.bytes()?).map_err(|_| WireError::Malformed("ref name"))?;
            let expect = d.option(|d| d.oid())?;
            let new = d.option(|d| d.oid())?;
            updates.push(RefUpdate { name, expect, new });
        }
        let pack = d.option(decode_pack)?;
        let stateless = [REPO_FORMAT_STATELESS, REPO_FORMAT_AGENT, REPO_FORMAT_TASKS, REPO_FORMAT_LEASES];
        let mut actor = decode_principal(&mut d, stateless.contains(&format))?;
        if format == REPO_FORMAT_TASKS || format == REPO_FORMAT_LEASES {
            decode_task_claim(&mut d, &mut actor)?;
        }
        let n = d.u32()? as usize;
        let mut options = Vec::with_capacity(n.min(1 << 10));
        for _ in 0..n {
            options.push(d.str()?.into());
        }
        let request = RequestId(Ulid(d.u128()?));
        let (provenance, protected, changed) = if per_ref_agent(format) {
            decode_validated(&mut d)?
        } else {
            for _ in 0..3 {
                if d.u32()? != 0 {
                    return Err(WireError::Malformed("validate output is empty below format 8"));
                }
            }
            (Vec::new(), Vec::new(), Vec::new())
        };
        let agent = if format != REPO_FORMAT_V1 { d.option(|d| d.str().map(Into::into))? } else { None };
        if !d.0.is_empty() {
            return Err(WireError::Malformed("trailing bytes"));
        }
        let leases = format == REPO_FORMAT_LEASES;
        let push =
            Push { updates, atomicity, pack, actor, options, request, agent, provenance, protected, changed, leases };
        // One form per command, so its digest is one: a format that does not match the command is malformed.
        if push.format() != format {
            return Err(WireError::Malformed("format does not match the command"));
        }
        Ok(push)
    }
}

fn encode_pack(e: &mut Enc, p: &PackRef) {
    match p {
        PackRef::Inline(b) => {
            e.u8(0);
            e.bytes(b);
        }
        PackRef::Stored { id, checksum, bytes, sites } => {
            e.u8(1);
            e.u128(id.0.0);
            e.0.extend_from_slice(&checksum.0);
            e.u64(*bytes);
            e.u64(sites.0);
        }
    }
}

fn encode_ref(e: &mut Enc, r: &RefName) {
    e.bytes(r.as_bytes());
}

fn encode_session(e: &mut Enc, s: &SessionId) {
    match s {
        SessionId::Run(u) => {
            e.u8(0);
            e.u128(*u);
        }
        SessionId::External { app, name } => {
            e.u8(1);
            e.u64(app.0.0 as u64);
            e.str(name);
        }
    }
}

/// `validate`'s three outputs: counts of zero unless the push is format 8.
fn encode_validated(e: &mut Enc, p: &Push) {
    e.count(p.provenance.len());
    for v in &p.provenance {
        e.0.extend_from_slice(&v.commit.0);
        encode_ref(e, &v.refname);
        e.count(v.sources.len());
        for s in &v.sources {
            match s {
                Source::Claimed { key, value } => {
                    e.u8(0);
                    e.u8(*key as u8);
                    e.str(value);
                }
                Source::AiNote { note } => {
                    e.u8(1);
                    e.0.extend_from_slice(&note.0);
                }
                Source::Session(id) => {
                    e.u8(2);
                    encode_session(e, id);
                }
            }
        }
    }
    e.count(p.protected.len());
    for h in &p.protected {
        encode_ref(e, &h.refname);
        e.count(h.paths.len());
        for x in &h.paths {
            e.str(x);
        }
    }
    e.count(p.changed.len());
    for c in &p.changed {
        encode_ref(e, &c.refname);
        e.count(c.paths.len());
        for x in &c.paths {
            e.str(x);
        }
        e.u8(u8::from(c.truncated));
    }
}

type Validated = (Vec<Provenance>, Vec<ProtectedHits>, Vec<ChangedPaths>);

fn decode_validated(d: &mut Dec) -> Result<Validated, WireError> {
    let refname = |d: &mut Dec| RefName::parse(d.bytes()?).map_err(|_| WireError::Malformed("ref name"));
    let paths = |d: &mut Dec| -> Result<Vec<Box<str>>, WireError> {
        let n = d.u32()? as usize;
        let mut out = Vec::with_capacity(n.min(crate::agent::PATHS_MAX));
        for _ in 0..n {
            out.push(d.str()?.into());
        }
        Ok(out)
    };
    let n = d.u32()? as usize;
    let mut provenance = Vec::with_capacity(n.min(1 << 10));
    for _ in 0..n {
        let commit = d.oid()?;
        let refname = refname(d)?;
        let k = d.u32()? as usize;
        let mut sources = SmallVec::new();
        for _ in 0..k {
            sources.push(match d.u8()? {
                0 => {
                    let key = match d.u8()? {
                        0 => TrailerKey::GraftyardSession,
                        1 => TrailerKey::ClaudeSession,
                        2 => TrailerKey::AmpThreadId,
                        3 => TrailerKey::CoAuthoredBy,
                        4 => TrailerKey::AssistedBy,
                        _ => return Err(WireError::Malformed("trailer key")),
                    };
                    Source::Claimed { key, value: d.str()?.into() }
                }
                1 => Source::AiNote { note: d.oid()? },
                2 => Source::Session(decode_session(d)?),
                _ => return Err(WireError::Malformed("source tag")),
            });
        }
        provenance.push(Provenance { commit, refname, sources });
    }
    let n = d.u32()? as usize;
    let mut protected = Vec::with_capacity(n.min(1 << 10));
    for _ in 0..n {
        let refname = refname(d)?;
        protected.push(ProtectedHits { refname, paths: paths(d)?.into_iter().collect() });
    }
    let n = d.u32()? as usize;
    let mut changed = Vec::with_capacity(n.min(1 << 10));
    for _ in 0..n {
        let refname = refname(d)?;
        let p = paths(d)?;
        let truncated = match d.u8()? {
            0 => false,
            1 => true,
            _ => return Err(WireError::Malformed("truncated flag")),
        };
        changed.push(ChangedPaths { refname, paths: p, truncated });
    }
    Ok((provenance, protected, changed))
}

fn encode_principal(e: &mut Enc, p: &Principal) {
    match p.actor {
        Actor::User(u) => {
            e.u8(0);
            e.u64(u.0.0.0 as u64);
        }
        Actor::Anonymous => e.u8(1),
        Actor::App { app, installation } => {
            e.u8(2);
            e.u64(app.0.0 as u64);
            e.u64(installation.0.0 as u64);
        }
    }
    e.u64(p.scopes.0);
    match &p.credential {
        Credential::Stored(h) => {
            e.u8(0);
            e.0.extend_from_slice(h);
        }
        Credential::Session(s) => {
            e.u8(1);
            e.u128(s.0.0);
        }
        Credential::Stateless(c) => {
            e.u8(2);
            encode_claims(e, c);
        }
    }
    e.count(p.sso_orgs.len());
    for o in &p.sso_orgs {
        e.u64(o.0.0.0 as u64);
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum WireError {
    #[error("format {0:?} is not one this release reads")]
    Format(Format),
    #[error("malformed push record: {0}")]
    Malformed(&'static str),
}

struct Dec<'a>(&'a [u8]);

impl<'a> Dec<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], WireError> {
        if self.0.len() < n {
            return Err(WireError::Malformed("truncated"));
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Ok(a)
    }
    fn u8(&mut self) -> Result<u8, WireError> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, WireError> {
        Ok(u16::from_be_bytes(self.take(2)?.try_into().expect("2")))
    }
    fn u32(&mut self) -> Result<u32, WireError> {
        Ok(u32::from_be_bytes(self.take(4)?.try_into().expect("4")))
    }
    fn u64(&mut self) -> Result<u64, WireError> {
        Ok(u64::from_be_bytes(self.take(8)?.try_into().expect("8")))
    }
    fn u128(&mut self) -> Result<u128, WireError> {
        Ok(u128::from_be_bytes(self.take(16)?.try_into().expect("16")))
    }
    fn bytes(&mut self) -> Result<&'a [u8], WireError> {
        let n = self.u32()? as usize;
        self.take(n)
    }
    fn str(&mut self) -> Result<&'a str, WireError> {
        std::str::from_utf8(self.bytes()?).map_err(|_| WireError::Malformed("utf-8"))
    }
    fn oid(&mut self) -> Result<ObjectId, WireError> {
        Ok(ObjectId(self.take(20)?.try_into().expect("20")))
    }
    fn option<T>(&mut self, f: impl FnOnce(&mut Self) -> Result<T, WireError>) -> Result<Option<T>, WireError> {
        match self.u8()? {
            0 => Ok(None),
            1 => f(self).map(Some),
            _ => Err(WireError::Malformed("option tag")),
        }
    }
}

fn decode_pack(d: &mut Dec) -> Result<PackRef, WireError> {
    match d.u8()? {
        0 => Ok(PackRef::Inline(d.bytes()?.to_vec())),
        1 => Ok(PackRef::Stored {
            id: PackId(Ulid(d.u128()?)),
            checksum: Digest(d.take(32)?.try_into().expect("32")),
            bytes: d.u64()?,
            sites: SiteSet(d.u64()?),
        }),
        _ => Err(WireError::Malformed("pack tag")),
    }
}

/// An installation token's claims, in `Enc`'s canonical form: the token text is not carried, only what it proved.
fn encode_claims(e: &mut Enc, c: &InstallationClaims) {
    e.u64(c.app.0.0 as u64);
    e.u64(c.installation.0.0 as u64);
    match &c.repos {
        RepoScope::All => e.u8(0),
        RepoScope::Selected(r) => {
            e.u8(1);
            e.count(r.len());
            for r in r {
                e.u64(r.0.0 as u64);
            }
        }
    }
    e.count(c.permissions.0.len());
    for (k, a) in &c.permissions.0 {
        e.str(k);
        e.u8(*a as u8);
    }
    e.u128(c.jti.0.0);
    e.u64(c.iat.0);
    e.u64(c.exp.0);
    e.u32(c.kid.0);
}

fn decode_claims(d: &mut Dec) -> Result<InstallationClaims, WireError> {
    let db = |v: u64| graftyard_types::DbId(v as i64);
    let app = AppId(db(d.u64()?));
    let installation = InstallationId(db(d.u64()?));
    let repos = match d.u8()? {
        0 => RepoScope::All,
        1 => {
            let n = d.u32()? as usize;
            let mut r = SmallVec::new();
            for _ in 0..n {
                r.push(RepoId(db(d.u64()?)));
            }
            RepoScope::Selected(r)
        }
        _ => return Err(WireError::Malformed("repository scope tag")),
    };
    let n = d.u32()? as usize;
    let mut permissions = BTreeMap::new();
    for _ in 0..n {
        let k: Box<str> = d.str()?.into();
        let a = match d.u8()? {
            0 => Access::Read,
            1 => Access::Write,
            2 => Access::Admin,
            _ => return Err(WireError::Malformed("access tag")),
        };
        permissions.insert(k, a);
    }
    Ok(InstallationClaims {
        app,
        installation,
        repos,
        permissions: PermissionSet(permissions),
        jti: Jti(Ulid(d.u128()?)),
        iat: Timestamp(d.u64()?),
        exp: Timestamp(d.u64()?),
        kid: Kid(d.u32()?),
        // Carried after the principal, in format 32.12 only (`decode_task_claim`).
        task: None,
    })
}

/// A principal in the push encoding's canonical form, App actors and installation claims included: what a format-8
/// issue command carries for an App, whose JSON form has only an account. A `task` claim (format 32.12) follows it,
/// only when set, so a principal without one keeps format 8's bytes.
pub fn principal_bytes(p: &Principal) -> Vec<u8> {
    let mut out = Vec::new();
    encode_principal(&mut Enc(&mut out), p);
    if crate::task::claim(p).is_some() {
        encode_task_claim(&mut Enc(&mut out), p);
    }
    out
}

pub fn principal_from_bytes(b: &[u8]) -> Result<Principal, WireError> {
    let mut d = Dec(b);
    let mut p = decode_principal(&mut d, true)?;
    if !d.0.is_empty() {
        decode_task_claim(&mut d, &mut p)?;
        if crate::task::claim(&p).is_none() {
            return Err(WireError::Malformed("a trailing task claim is set"));
        }
    }
    if !d.0.is_empty() {
        return Err(WireError::Malformed("trailing bytes"));
    }
    Ok(p)
}

/// An installation token's `task` claim, after its principal in format 32.12: an option of the session.
fn encode_task_claim(e: &mut Enc, p: &Principal) {
    e.option(crate::task::claim(p), encode_session);
}

fn decode_task_claim(d: &mut Dec, p: &mut Principal) -> Result<(), WireError> {
    let task = d.option(decode_session)?;
    match &mut p.credential {
        Credential::Stateless(c) => c.task = task,
        _ if task.is_some() => {
            return Err(WireError::Malformed("a task claim on a token that is not an installation's"));
        }
        _ => {}
    }
    Ok(())
}

fn decode_session(d: &mut Dec) -> Result<SessionId, WireError> {
    match d.u8()? {
        0 => Ok(SessionId::Run(d.u128()?)),
        1 => Ok(SessionId::External { app: AppId(graftyard_types::DbId(d.u64()? as i64)), name: d.str()?.into() }),
        _ => Err(WireError::Malformed("session tag")),
    }
}

/// Whether a push of `format` stores a per-ref agent refusal: 32.8 and its supersets 32.12 and 32.14.
fn per_ref_agent(format: Format) -> bool {
    format == REPO_FORMAT_AGENT || format == REPO_FORMAT_TASKS || format == REPO_FORMAT_LEASES
}

fn decode_principal(d: &mut Dec, stateless: bool) -> Result<Principal, WireError> {
    let actor = match d.u8()? {
        0 => Actor::User(UserId(AccountId(graftyard_types::DbId(d.u64()? as i64)))),
        1 => Actor::Anonymous,
        2 if stateless => {
            let app = AppId(graftyard_types::DbId(d.u64()? as i64));
            Actor::App { app, installation: InstallationId(graftyard_types::DbId(d.u64()? as i64)) }
        }
        _ => return Err(WireError::Malformed("actor tag")),
    };
    let scopes = ScopeSet(d.u64()?);
    let credential = match d.u8()? {
        0 => Credential::Stored(d.take(32)?.try_into().expect("32")),
        1 => Credential::Session(graftyard_identity::WebSessionId(graftyard_types::Ulid(d.u128()?))),
        2 if stateless => Credential::Stateless(Box::new(decode_claims(d)?)),
        _ => return Err(WireError::Malformed("credential tag")),
    };
    let n = d.u32()? as usize;
    // Read every org the encoder wrote; only the reservation is capped, and `take` refuses a count past the buffer.
    let mut sso_orgs = SmallVec::with_capacity(n.min(64));
    for _ in 0..n {
        sso_orgs.push(graftyard_identity::OrgId(AccountId(graftyard_types::DbId(d.u64()? as i64))));
    }
    Ok(Principal { actor, scopes, credential, sso_orgs })
}

/// A push's serde form is the base64 of `to_wire`, so a member's JSON `cmd` carries the format explicitly and a field
/// a release does not know is never dropped silently. Deserializing a format this release does not read fails with
/// `WireError::Format`'s message; a host answers `Unsupported` for it (`group::decode_commit`).
impl Serialize for Push {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&crate::pack::b64::encode(&self.to_wire()))
    }
}

impl<'de> Deserialize<'de> for Push {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Push, D::Error> {
        let s = String::deserialize(d)?;
        let b = crate::pack::b64::decode(&s).ok_or_else(|| serde::de::Error::custom("push is not base64"))?;
        Push::from_wire(&b).map_err(serde::de::Error::custom)
    }
}

impl Record for Push {
    fn encode(&self, out: &mut Vec<u8>) {
        self.encode_with(out, self.pack.as_ref());
    }
}

impl Record for RepoEvent {
    fn encode(&self, out: &mut Vec<u8>) {
        let mut e = Enc(out);
        let (push, actor, updates, pack) = match self {
            RepoEvent::Refs { push, actor, updates, pack } => (push, actor, updates, pack),
            RepoEvent::Promoted(p) => {
                e.u8(2);
                e.u64(p.from.0);
                e.u64(p.to.0);
                encode_pack(&mut e, &p.pack);
                return;
            }
            RepoEvent::Archived(a) => {
                e.u8(3);
                e.str(&a.key.to_string());
                e.u64(a.from.0);
                e.u64(a.to.0);
                e.u64(a.from_at.0);
                e.u64(a.to_at.0);
                e.0.extend_from_slice(&a.digest.0);
                return;
            }
            RepoEvent::Refused { push, refs } => {
                e.u8(1);
                e.u128(push.0.0);
                e.count(refs.len());
                for (n, r) in refs {
                    e.bytes(n.as_bytes());
                    e.u8(reason_tag(r));
                    if let RejectReason::Agent(a) = r {
                        graftyard_fold::canon::write(e.0, a);
                    }
                }
                return;
            }
            RepoEvent::IssueOpened { n, capture } => return tagged(out, 4, &(n, capture)),
            RepoEvent::PrOpened { n, head, base, capture } => return tagged(out, 5, &(n, head, base, capture)),
            RepoEvent::PrClosed { n, final_head } => return tagged(out, 6, &(n, final_head)),
            RepoEvent::IssueChanged { n, change, actor } => return tagged(out, 7, &(n, change, actor)),
            RepoEvent::Comment(c) => return tagged(out, 8, c),
            RepoEvent::Review(r) => return tagged(out, 9, r),
            RepoEvent::Labels(l) => return tagged(out, 10, l),
            RepoEvent::Rule(r) => return tagged(out, 11, r),
            RepoEvent::Settings(s) => return tagged(out, 12, s),
            RepoEvent::RequiredCheck { sha, name, source, version, conclusion } => {
                return tagged(out, 13, &(sha, name, source, version, conclusion));
            }
            RepoEvent::CheckRun(c) => return tagged(out, 14, c),
            RepoEvent::CheckSuite(c) => return tagged(out, 15, c),
            RepoEvent::Status(st) => return tagged(out, 16, st),
            RepoEvent::Release { id, record, tag, actor } => return tagged(out, 17, &(id, record, tag, actor)),
            RepoEvent::ReleaseAsset { release, id, asset, actor } => {
                return tagged(out, 18, &(release, id, asset, actor));
            }
            RepoEvent::LfsObject { blob, actor } => return tagged(out, 19, &(blob, actor)),
            RepoEvent::LfsLock(l) => return tagged(out, 20, l),
            RepoEvent::TestMerged { n, base, head, result, pack } => {
                return tagged(out, 21, &(n, base, head, result, pack));
            }
            RepoEvent::ConsumerCursor { consumer, through } => return tagged(out, 22, &(consumer, through)),
            // Tags 21 and 22 are M2-WP5's `TestMerged` and `ConsumerCursor`.
            RepoEvent::Merged { n, base, head, method, also, pack, verified, engine, by } => {
                return tagged(out, 23, &(n, base, head, method, also, pack, verified, engine, by));
            }
            RepoEvent::Queue(q) => return tagged(out, 24, q),
            // M3-WP3: tags 25 and 26.
            RepoEvent::Collaborator { who, permission } => return tagged(out, 25, &(who, permission)),
            RepoEvent::Invitation(i) => return tagged(out, 26, i),
            // AM1-WP2: tags 27 to 30.
            RepoEvent::Provenance { push, list } => return tagged(out, 27, &(push, list)),
            RepoEvent::AgentMeta { n, meta } => return tagged(out, 28, &(n, meta)),
            RepoEvent::AgentGate(g) => return tagged(out, 29, g),
            RepoEvent::AgentPolicy(p) => return tagged(out, 30, p),
            // AM1-WP3: tags 31 and 32.
            RepoEvent::TaskCreated(t) => return tagged(out, 31, t),
            RepoEvent::TaskUpdated { session, state, pr, summary, actor } => {
                return tagged(out, 32, &(session, state, pr, summary, actor));
            }
            // Format 32.13: tags 33 and 34.
            RepoEvent::Created { actor } => return tagged(out, 33, actor),
            RepoEvent::Deleted { retain_until, actor } => return tagged(out, 34, &(retain_until, actor)),
            // AM1-WP4: tags 35 to 38.
            RepoEvent::LeaseAcquire { lease, plan } => return tagged(out, 35, &(lease, plan)),
            RepoEvent::LeaseRenew { id, expires } => return tagged(out, 36, &(id, expires)),
            RepoEvent::LeaseRelease { ids, by } => return tagged(out, 37, &(ids, by)),
            RepoEvent::PathSet(p) => return tagged(out, 38, p),
            // Format 32.15: tag 39.
            RepoEvent::Fanout { k } => return tagged(out, 39, k),
            // M4-WP1, 32.16: tags 40 and 41.
            RepoEvent::Hook { id, record, actor, created } => return tagged(out, 40, &(id, record, actor, created)),
            RepoEvent::HookCursor(c) => return tagged(out, 41, c),
            // M4-WP2, 32.19: tag 42.
            RepoEvent::Watchers(u) => return tagged(out, 42, u),
            RepoEvent::Reserved43 => return tagged(out, 43, &()),
            // M4-WP3, 32.23: tags 44 and 45.
            RepoEvent::ActionsSecret { scope, name, record, actor } => {
                return tagged(out, 44, &(scope, name, record, actor));
            }
            RepoEvent::ActionsVariable { scope, name, record, actor } => {
                return tagged(out, 45, &(scope, name, record, actor));
            }
            // R24, 32.24: tag 46.
            RepoEvent::WorkflowJob(u) => return tagged(out, 46, u),
        };
        e.u8(0);
        e.u128(push.0.0);
        e.option(actor.as_ref(), |e, a| e.u64(a.0.0 as u64));
        e.count(updates.len());
        for u in updates {
            e.bytes(u.name.as_bytes());
            e.option(u.expect.as_ref(), |e, o| e.0.extend_from_slice(&o.0));
            e.option(u.new.as_ref(), |e, o| e.0.extend_from_slice(&o.0));
        }
        e.option(pack.as_ref(), encode_pack);
    }
}

/// A Milestone 2 event's bytes: its fixed tag, then its payload by the canonical rule.
fn tagged<T: Serialize + ?Sized>(out: &mut Vec<u8>, tag: u8, payload: &T) {
    out.push(tag);
    graftyard_fold::canon::write(out, payload);
}

impl Command for Push {
    type Fold = RepoFold;
    type Dep = IdentityDep;
    type Out = PushOut;
    type Err = PushErr;

    fn format(&self) -> Format {
        let stateless =
            matches!(self.actor.credential, Credential::Stateless(_)) || matches!(self.actor.actor, Actor::App { .. });
        if self.leases && self.validated() {
            REPO_FORMAT_LEASES
        } else if crate::task::claim(&self.actor).is_some() {
            REPO_FORMAT_TASKS
        } else if self.validated() {
            REPO_FORMAT_AGENT
        } else if stateless {
            REPO_FORMAT_STATELESS
        } else if self.agent.is_some() {
            REPO_FORMAT_V2
        } else {
            REPO_FORMAT_V1
        }
    }
    /// Core, and Metadata too when the push carries provenance, whose `Provenance` event is a Metadata row, or is
    /// 32.14 with `changed` paths, whose `PathSet` is.
    fn tiers(&self) -> TierSet {
        let path_set = self.format() == REPO_FORMAT_LEASES && !self.changed.is_empty();
        if self.provenance.is_empty() && !path_set { TierSet::CORE } else { TierSet::CORE.union(TierSet::METADATA) }
    }
    /// The actor's `Account` and token rows, and the owner's and enterprise's `Authz`, named by the fold's
    /// `RepoSettings`.
    fn deps(&self, fold: &RepoFold) -> SmallVec<[ChainId; 3]> {
        let mut d = credential_chains(&self.actor.credential_shard(), None);
        d.extend(authz_chains(fold.authz_owner(), fold.authz_enterprise()));
        with_install(&mut d, &self.actor);
        d
    }
    fn decide(
        &self,
        s: &RepoScratch<'_>,
        deps: &[(ChainId, IdentityDep)],
    ) -> Result<(SmallVec<[RepoEvent; 4]>, PushOut), PushErr> {
        let cred: CredentialShard = self.actor.credential_shard();
        // A push names its account and its token, so the leader re-reads both; an installation token names its `jti`.
        if cred.key.is_none() || (cred.account.is_none() && !matches!(self.actor.actor, Actor::App { .. })) {
            return Err(PushErr::Unauthorized(CredentialRefusal::NoToken));
        }
        check_credential(&cred, Own::Other, deps).map_err(PushErr::Unauthorized)?;
        if !may_write(&self.actor, s, deps) {
            return Err(PushErr::Forbidden);
        }
        // The class from the install's rows this entry read, never the edge's: an unflagged App is not an `Agent` in
        // the first decision after the unflag commits (LF-2).
        let install = install_authz(deps);
        let owner = authz_folds(deps).into_iter().find(|a| a.owner().is_some() && a.owner() == s.authz_owner());
        // The lease rows the `AgentsExclusive` check and `PathSet.planned` read, at the entry's time.
        let view = match (owner, s.fold().repo) {
            (Some(owner), Some(repo)) => Some(crate::lease::LeaseView { core: s.core(), owner, repo, at: s.at() }),
            _ => None,
        };
        match crate::agent::actor_kind(&self.actor, install) {
            crate::agent::ActorKind::Agent(mut a) => {
                // A token's `task` claim binds it to that task while the task reads live here (AM1-WP3): its prefix
                // and its requester. A claim on a task that reads cancelled, timed out, or paused binds nothing.
                if let (Some(t), Some(owner)) = (crate::task::claim(&self.actor), owner)
                    && let Some(repo) = s.fold().repo
                    && let Some(row) = crate::task::live_run_task(s.core(), owner, install, repo, s.at(), a.app, t)
                {
                    a.task = Some(t.clone());
                    a.requester = row.requester;
                }
                let session_ref = a.task.as_ref().filter(|_| crate::task::is_workspace(install, a.app)).and_then(|t| {
                    RefName::parse(format!("{}{}", crate::agent::SESSION_REFS, t.text()).as_bytes()).ok()
                });
                self.decide_refs(s, Some((&a, install)), session_ref.as_ref(), view.as_ref())
            }
            // Humans, bots, `Graftyard MCP`, and user tokens are never refused by a lease, but their 32.14 pushes
            // still write `PathSet`.
            _ => self.decide_refs(s, None, None, view.as_ref()),
        }
    }
    /// Each ref as the first decide answered it: `Ok` if a stored `Refs` applied it, else the reason a stored
    /// `Refused` gives. A ref in neither (a row with no `Refused`) is reported `Stale`.
    fn replay(&self, events: &[RepoEvent]) -> PushOut {
        let mut was: BTreeMap<&RefName, RefOutcome> = BTreeMap::new();
        for ev in events {
            match ev {
                RepoEvent::Refs { updates, .. } => was.extend(updates.iter().map(|u| (&u.name, RefOutcome::Ok))),
                RepoEvent::Refused { refs, .. } => {
                    was.extend(refs.iter().map(|(n, r)| (n, RefOutcome::Rejected(r.clone()))))
                }
                _ => {}
            }
        }
        PushOut {
            refs: self
                .updates
                .iter()
                .map(|u| {
                    (u.name.clone(), was.get(&u.name).cloned().unwrap_or(RefOutcome::Rejected(RejectReason::Stale)))
                })
                .collect(),
        }
    }
}

/// A reason's byte in `RepoEvent::Refused`'s canonical encoding, fixed per reason so that reordering the enum cannot
/// change a digest.
fn reason_tag(r: &RejectReason) -> u8 {
    match r {
        RejectReason::Stale => 0,
        RejectReason::NonFastForward => 1,
        RejectReason::Hidden => 2,
        RejectReason::TooManyRefs => 3,
        RejectReason::Atomic => 4,
        RejectReason::NameConflict => 5,
        RejectReason::AtomicTransaction => 6,
        RejectReason::Agent(_) => 7,
    }
}

/// Every pack the events name, for the shard's `IngestCursor` catch-up.
pub fn packs_of(events: &[Ev<Push>]) -> Vec<PackRef> {
    events
        .iter()
        .filter_map(|ev| match ev {
            RepoEvent::Refs { pack, .. } | RepoEvent::TestMerged { pack, .. } | RepoEvent::Merged { pack, .. } => {
                pack.clone()
            }
            RepoEvent::Queue(QueueEvent::GroupBuilt { pack, .. }) => pack.clone(),
            _ => None,
        })
        .collect()
}

/// What a repository member commits: a client's `Push`, a consumer's `PrSync` (`crate::prsync`), or an issue, pull
/// request, release, or LFS command from the edge (`crate::issue_ops::call`). A push keeps its wire form, the base64
/// of `Push::to_wire`, so an edge that sends only pushes is unchanged; a sync is `{"prSync": ..}`, which only a member
/// whose release reads `REPO_FORMAT_V3` decodes, and an issue command `{"issue": ..}`, which names its format first, as
/// does a merge command `{"merge": ..}` (32.4) and the transfer's `Settings`, `{"transfer": ..}` (32.10).
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq)]
pub enum RepoCmd {
    Push(Push),
    Sync(crate::prsync::PrSync),
    Issue(Box<crate::issue_ops::call::IssueCall>),
    /// A merge or merge-queue command (`crate::merge_call`), `{"merge": ..}`, format 32.4.
    Merge(Box<crate::merge_call::MergeCall>),
    /// The transfer's own `Settings` (`crate::transfer`), `{"transfer": ..}`, format 32.10: only a finisher sends it.
    Transfer(crate::transfer::TransferSettings),
    /// The `Webhooks` consumer's cursor (`crate::hooks`), `{"hookCursor": ..}`, format 32.16.
    HookCursor(crate::hooks::HookCursorCmd),
    /// A create's or a delete's finisher step (`crate::lifecycle`), `{"lifecycle": ..}`, format 32.17 (F59).
    Lifecycle(crate::lifecycle::Lifecycle),
    /// The `Counts` consumer's watcher rows (`crate::watch`), `{"counts": ..}`, format 32.19.
    Counts(crate::watch::CountsCmd),
    /// `Notify`'s or `Search`'s cursor (`crate::watch`), `{"consumerCursor": ..}`, format 32.19.
    ConsumerCursor(crate::watch::ConsumerCursorCmd),
    /// `RunOwner`'s check suite, check runs, and required checks (`crate::run_checks`), `{"runChecks": ..}`,
    /// format 32.21.
    RunChecks(crate::run_checks::RunChecksCmd),
}

#[derive(Clone, Debug, PartialEq)]
pub enum RepoOut {
    Push(PushOut),
    Sync(crate::prsync::SyncOut),
    Issue(crate::issue_ops::call::IssueCallOut),
    Merge(crate::merge_call::MergeCallOut),
    Transfer(crate::transfer::TransferOut),
    HookCursor(crate::hooks::HookCursorOut),
    Lifecycle(crate::lifecycle::LifecycleOut),
    Counts(crate::watch::CountsOut),
    RunChecks(crate::run_checks::RunChecksOut),
}

/// An issue command's refusal carries its render request too, so the member renders the client's error reply.
#[derive(Clone, Debug, PartialEq)]
pub enum RepoErr {
    Push(PushErr),
    Issue { err: crate::issue_ops::IssueErr, render: serde_json::Value },
    Merge { n: crate::issues::IssueNumber, err: crate::merge::MergeError, render: serde_json::Value },
    Transfer(crate::transfer::TransferErr),
    Lifecycle(crate::lifecycle::LifecycleErr),
}

impl crate::smart::PushOutOf for RepoOut {
    fn push_out(&self) -> Option<&PushOut> {
        match self {
            RepoOut::Push(p) => Some(p),
            RepoOut::Sync(_)
            | RepoOut::Issue(_)
            | RepoOut::Merge(_)
            | RepoOut::Transfer(_)
            | RepoOut::HookCursor(_)
            | RepoOut::Lifecycle(_)
            | RepoOut::Counts(_)
            | RepoOut::RunChecks(_) => None,
        }
    }
}

impl Serialize for RepoCmd {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        enum Tagged<'a> {
            PrSync(&'a crate::prsync::PrSync),
            Issue(&'a crate::issue_ops::call::IssueCall),
            Merge(&'a crate::merge_call::MergeCall),
            Transfer(&'a crate::transfer::TransferSettings),
            HookCursor(&'a crate::hooks::HookCursorCmd),
            Lifecycle(&'a crate::lifecycle::Lifecycle),
            Counts(&'a crate::watch::CountsCmd),
            ConsumerCursor(&'a crate::watch::ConsumerCursorCmd),
            RunChecks(&'a crate::run_checks::RunChecksCmd),
        }
        match self {
            RepoCmd::Push(p) => p.serialize(s),
            RepoCmd::Sync(c) => Tagged::PrSync(c).serialize(s),
            RepoCmd::Issue(c) => Tagged::Issue(c).serialize(s),
            RepoCmd::Merge(c) => Tagged::Merge(c).serialize(s),
            RepoCmd::Transfer(c) => Tagged::Transfer(c).serialize(s),
            RepoCmd::HookCursor(c) => Tagged::HookCursor(c).serialize(s),
            RepoCmd::Lifecycle(c) => Tagged::Lifecycle(c).serialize(s),
            RepoCmd::Counts(c) => Tagged::Counts(c).serialize(s),
            RepoCmd::ConsumerCursor(c) => Tagged::ConsumerCursor(c).serialize(s),
            RepoCmd::RunChecks(c) => Tagged::RunChecks(c).serialize(s),
        }
    }
}

/// An issue command's own error is kept, so a format this release does not read says so.
impl<'de> Deserialize<'de> for RepoCmd {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<RepoCmd, D::Error> {
        use serde::de::Error;
        let v = serde_json::Value::deserialize(d)?;
        let one = |k: &str| v.as_object().filter(|m| m.len() == 1).and_then(|m| m.get(k)).cloned();
        if v.is_string() {
            serde_json::from_value(v).map(RepoCmd::Push).map_err(D::Error::custom)
        } else if let Some(c) = one("prSync") {
            serde_json::from_value(c).map(RepoCmd::Sync).map_err(D::Error::custom)
        } else if let Some(c) = one("issue") {
            serde_json::from_value(c).map(|c| RepoCmd::Issue(Box::new(c))).map_err(D::Error::custom)
        } else if let Some(c) = one("merge") {
            serde_json::from_value(c).map(|c| RepoCmd::Merge(Box::new(c))).map_err(D::Error::custom)
        } else if let Some(c) = one("transfer") {
            serde_json::from_value(c).map(RepoCmd::Transfer).map_err(D::Error::custom)
        } else if let Some(c) = one("hookCursor") {
            serde_json::from_value(c).map(RepoCmd::HookCursor).map_err(D::Error::custom)
        } else if let Some(c) = one("lifecycle") {
            serde_json::from_value(c).map(RepoCmd::Lifecycle).map_err(D::Error::custom)
        } else if let Some(c) = one("counts") {
            serde_json::from_value(c).map(RepoCmd::Counts).map_err(D::Error::custom)
        } else if let Some(c) = one("consumerCursor") {
            serde_json::from_value(c).map(RepoCmd::ConsumerCursor).map_err(D::Error::custom)
        } else if let Some(c) = one("runChecks") {
            serde_json::from_value(c).map(RepoCmd::RunChecks).map_err(D::Error::custom)
        } else {
            Err(D::Error::custom("not a repository command"))
        }
    }
}

impl Record for RepoCmd {
    fn encode(&self, out: &mut Vec<u8>) {
        match self {
            RepoCmd::Push(p) => p.encode(out),
            RepoCmd::Sync(c) => c.encode(out),
            RepoCmd::Issue(c) => out.extend(serde_json::to_vec(c).expect("an issue command encodes")),
            RepoCmd::Merge(c) => c.cmd.encode(out),
            RepoCmd::Transfer(c) => c.encode(out),
            RepoCmd::HookCursor(c) => c.encode(out),
            RepoCmd::Lifecycle(c) => c.encode(out),
            RepoCmd::Counts(c) => c.encode(out),
            RepoCmd::ConsumerCursor(c) => c.encode(out),
            RepoCmd::RunChecks(c) => c.encode(out),
        }
    }
}

impl RepoCmd {
    /// What the committing leader's marker puts read after an `Acked` (`workers/site`, `Family::after_ok`), or `None`
    /// when they put nothing: a push's stored pack ref, without any inline bytes, and the blob an issue command's
    /// member confirmed, without its reply render. A leader host keeps this with each open ticket in place of the
    /// whole command, which held each render and inline pack a second time beside the core's copy
    /// (`tests/m0/results/admitted-bytes.md`).
    pub fn marked(&self) -> Option<RepoCmd> {
        match self {
            RepoCmd::Push(p) if matches!(p.pack, Some(PackRef::Stored { .. })) => Some(RepoCmd::Push(p.clone())),
            RepoCmd::Issue(c) if c.confirmed.is_some() => {
                let op = c.op.clone();
                let mut slim =
                    crate::issue_ops::call::IssueCall::new(op, c.actor.clone(), c.request, serde_json::Value::Null);
                slim.confirmed = c.confirmed;
                Some(RepoCmd::Issue(Box::new(slim)))
            }
            _ => None,
        }
    }
}

impl Command for RepoCmd {
    type Fold = RepoFold;
    type Dep = IdentityDep;
    type Out = RepoOut;
    type Err = RepoErr;

    fn format(&self) -> Format {
        match self {
            RepoCmd::Push(p) => p.format(),
            RepoCmd::Sync(c) => c.format(),
            RepoCmd::Issue(c) => c.format(),
            RepoCmd::Merge(c) => c.cmd.format(),
            RepoCmd::Transfer(c) => c.format(),
            RepoCmd::HookCursor(_) => crate::hooks::REPO_FORMAT_HOOKS,
            RepoCmd::Lifecycle(c) => c.format(),
            RepoCmd::Counts(_) | RepoCmd::ConsumerCursor(_) => crate::watch::REPO_FORMAT_WATCH,
            RepoCmd::RunChecks(c) => c.format(),
        }
    }
    fn tiers(&self) -> TierSet {
        match self {
            RepoCmd::Push(p) => p.tiers(),
            RepoCmd::Sync(_) => TierSet::CORE,
            RepoCmd::Issue(c) => c.tiers(),
            RepoCmd::Merge(c) => c.cmd.tiers(),
            RepoCmd::Transfer(c) => c.tiers(),
            RepoCmd::HookCursor(c) => c.tiers(),
            RepoCmd::Lifecycle(c) => c.tiers(),
            RepoCmd::Counts(c) => c.tiers(),
            RepoCmd::ConsumerCursor(_) => TierSet::CORE,
            RepoCmd::RunChecks(c) => c.tiers(),
        }
    }
    fn deps(&self, fold: &RepoFold) -> SmallVec<[ChainId; 3]> {
        match self {
            RepoCmd::Push(p) => p.deps(fold),
            RepoCmd::Sync(_) => SmallVec::new(),
            RepoCmd::Issue(c) => c.deps(fold),
            RepoCmd::Merge(c) => c.cmd.deps(fold),
            RepoCmd::Transfer(c) => c.deps(),
            RepoCmd::HookCursor(_) => SmallVec::new(),
            RepoCmd::Lifecycle(c) => c.deps(fold),
            RepoCmd::Counts(_) | RepoCmd::ConsumerCursor(_) | RepoCmd::RunChecks(_) => SmallVec::new(),
        }
    }
    fn decide(
        &self,
        s: &RepoScratch<'_>,
        deps: &[(ChainId, IdentityDep)],
    ) -> Result<(SmallVec<[RepoEvent; 4]>, RepoOut), RepoErr> {
        match self {
            RepoCmd::Push(p) => p.decide(s, deps).map(|(e, o)| (e, RepoOut::Push(o))).map_err(RepoErr::Push),
            RepoCmd::Sync(c) => {
                let (e, o) = c.decide_sync(s);
                Ok((e, RepoOut::Sync(o)))
            }
            RepoCmd::Issue(c) => c
                .decide(s, deps)
                .map(|(e, o)| (e, RepoOut::Issue(o)))
                .map_err(|err| RepoErr::Issue { err, render: c.render.clone() }),
            RepoCmd::Merge(c) => c
                .cmd
                .decide(s, deps)
                .map(|(e, out)| (e, RepoOut::Merge(crate::merge_call::MergeCallOut { out, render: c.render.clone() })))
                .map_err(|(n, err)| RepoErr::Merge { n, err, render: c.render.clone() }),
            RepoCmd::Transfer(c) => {
                c.decide(s, deps).map(|(e, o)| (e, RepoOut::Transfer(o))).map_err(RepoErr::Transfer)
            }
            RepoCmd::HookCursor(c) => {
                let (e, o) = c.decide(s);
                Ok((e, RepoOut::HookCursor(o)))
            }
            RepoCmd::Lifecycle(c) => {
                c.decide(s, deps).map(|(e, o)| (e, RepoOut::Lifecycle(o))).map_err(RepoErr::Lifecycle)
            }
            RepoCmd::Counts(c) => {
                let (e, o) = c.decide(s);
                Ok((e, RepoOut::Counts(o)))
            }
            RepoCmd::ConsumerCursor(c) => {
                let (e, o) = c.decide(s);
                Ok((e, RepoOut::Counts(o)))
            }
            RepoCmd::RunChecks(c) => {
                let (e, o) = c.decide(s);
                Ok((e, RepoOut::RunChecks(o)))
            }
        }
    }
    fn replay(&self, events: &[RepoEvent]) -> RepoOut {
        match self {
            RepoCmd::Push(p) => RepoOut::Push(p.replay(events)),
            RepoCmd::Sync(c) => RepoOut::Sync(Command::replay(c, events)),
            RepoCmd::Issue(c) => RepoOut::Issue(c.replay(events)),
            RepoCmd::Merge(c) => {
                RepoOut::Merge(crate::merge_call::MergeCallOut { out: c.cmd.replay(events), render: c.render.clone() })
            }
            RepoCmd::Transfer(c) => RepoOut::Transfer(c.replay(events)),
            RepoCmd::HookCursor(c) => RepoOut::HookCursor(c.replay(events)),
            RepoCmd::Lifecycle(c) => RepoOut::Lifecycle(c.replay(events)),
            RepoCmd::Counts(c) => RepoOut::Counts(c.replay(events)),
            RepoCmd::ConsumerCursor(c) => RepoOut::Counts(c.replay(events)),
            RepoCmd::RunChecks(c) => RepoOut::RunChecks(c.replay(events)),
        }
    }
    /// An issue command without its `render`, the reply the leader renders from its `&Acked`, which no event stores:
    /// over m3org's Authz fold it was 10,262 to 10,426 of a command's 10.8 KB, and the rest, 538 to 635 bytes, is about
    /// what its events store (LM-1 round 15b, `tests/am1/results/lm1-memory.md` on `lm1-memory`). A merge's `encode`
    /// already leaves its render out.
    fn stored_len(&self) -> usize {
        match self {
            // The command encodes as JSON, so `"render": null` in place of the render is the bare command's length.
            RepoCmd::Issue(c) => {
                let render = serde_json::to_vec(&c.render).map_or(0, |v| v.len());
                self.encoded_len() - render.saturating_sub(4)
            }
            _ => self.encoded_len(),
        }
    }
}
