//! The issue, pull request, review, label, rule, settings, and check commands of the repository chain
//! (`design/SHARED.md`, "Issues and pull requests"; `build/BUILD-PLAN.md`, M2-WP2).
//!
//! `IssueCmd::decide` is pure. It reads a `RepoView`: both tiers at `applied`, with the earlier commands' events of the
//! same entry written on top by the same code `apply` runs, plus the credential and owner `Authz` folds the leader read
//! once through `fresh`. `record_check` and `record_status` are `CheckOp`s: one `CheckRun` or `Status` event, plus a
//! `RequiredCheck` when a rule requires the check and the fresh core tier shows its SHA as the head of an open pull
//! request, a merge-queue ref, or a protected branch tip.

use graftyard_fold::{Overlay, Read, Tiered, Val, has};
use graftyard_identity::{
    Actor, AuthzRead, CredentialRefusal, IdentityDep, Own, Principal, RepoPermission, check_credential,
    credential_chains,
};
use graftyard_raft::TierSet;
use graftyard_types::{AccountId, ChainId, DbId, RequestId, Tier, Timestamp};
use smallvec::{SmallVec, smallvec};

use crate::issues::{
    BODY_MAX, BranchRule, CHECK_RETENTION_MAX, CheckConclusion, CheckRunUpdate, CheckSource, CheckStatus, CommentEvent,
    CommentId, CommentTarget, Grantee, HeadSpec, IssueCapture, IssueChange, IssueNumber, LABEL_MAX, LabelEvent,
    LabelId, LabelName, PrCapture, PrState, Protection, RepoSettings, ReviewEvent, ReviewId, ReviewState, StatusState,
    StatusUpdate, TITLE_MAX, Visibility,
};
use crate::refs::{ObjectId, RefName};
use crate::repo::RepoEvent;
use crate::tiers::{
    CommentRow, IssueRow, LabelRow, RepoTiers, get_rec, ids, int, n_key, pull_head, pulls_from, ref_key, sha_key,
    tier_of,
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
            self.tiers.write(ev, Tier::Core, self.at, &mut self.core);
        }
        if has(set, Tier::Metadata) {
            self.tiers.write(ev, Tier::Metadata, self.at, &mut self.meta);
        }
    }

    pub fn core(&self) -> &dyn Read {
        &self.core
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

/// What one command asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IssueOp {
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
}

fn actor_id(p: &Principal) -> Option<AccountId> {
    match p.actor {
        Actor::User(u) => Some(u.0),
        Actor::Anonymous => None,
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
            IssueOp::RecordCheck(_) | IssueOp::RecordStatus(_) => both,
            // A new review thread, a resolution, or a deletion may change the core unresolved count.
            IssueOp::Comment { on: CommentTarget::Review { .. }, reply_to: None, .. } => both,
            IssueOp::Resolve { .. } | IssueOp::DeleteComment { .. } => both,
            _ => TierSet::METADATA,
        }
    }

    /// The actor's credential chains and the owner's `Authz`, named by the core `settings` row.
    pub fn deps(&self, view: &RepoView<'_>) -> SmallVec<[ChainId; 3]> {
        let mut d = credential_chains(&self.actor.credential_shard(), None);
        d.push(ChainId::Authz(view.owner()));
        d
    }

    /// Pure: the view, and the deps the leader read once per entry.
    pub fn decide(
        &self,
        v: &RepoView<'_>,
        deps: &[(ChainId, IdentityDep)],
    ) -> Result<(SmallVec<[RepoEvent; 4]>, IssueOut), IssueErr> {
        let cred = self.actor.credential_shard();
        if cred.account.is_none() || cred.key.is_none() {
            return Err(IssueErr::Unauthorized(CredentialRefusal::NoToken));
        }
        check_credential(&cred, Own::Other, deps).map_err(IssueErr::Unauthorized)?;
        let owner = v.owner();
        let authz = deps.iter().find_map(|(c, d)| match d {
            IdentityDep::Authz(f) if *c == ChainId::Authz(owner) => Some(f as &dyn AuthzRead),
            _ => None,
        });
        let perm = permission(&self.actor, v, authz);
        let me = actor_id(&self.actor);
        let need = |p: RepoPermission| if perm >= Some(p) { Ok(()) } else { Err(IssueErr::Forbidden) };
        let author_or = |author: Option<AccountId>, p: RepoPermission| {
            if author.is_some() && author == me { Ok(()) } else { need(p) }
        };
        let issue = |n: IssueNumber| -> Result<(), IssueErr> {
            if n.0 == 0 || n.0 > v.last_number() { Err(IssueErr::NotFound) } else { Ok(()) }
        };
        let one = |e: RepoEvent| smallvec![e];
        match &self.op {
            IssueOp::OpenIssue { title, body, labels, assignees } => {
                need(RepoPermission::Read)?;
                text_ok(title, body)?;
                for l in labels {
                    get_rec::<LabelRow>(v.meta(), "label", &[int(l.0.0)]).ok_or(IssueErr::UnknownLabel(*l))?;
                }
                let n = IssueNumber(v.last_number() + 1);
                let capture = IssueCapture {
                    id: v.next_id(Tier::Core)?,
                    author: me,
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
                Ok((one(ev), IssueOut::Number(n)))
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
                Ok((one(RepoEvent::IssueChanged { n: *n, change: change.clone(), actor: me }), IssueOut::Done))
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
                let ev = CommentEvent::Created {
                    id: CommentId(id),
                    on: on.clone(),
                    author: me,
                    body: body.clone(),
                    reply_to: *reply_to,
                };
                Ok((one(RepoEvent::Comment(ev)), IssueOut::Id(id)))
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
                Ok((one(RepoEvent::Review(ev)), IssueOut::Id(id)))
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
            IssueOp::RecordCheck(run) => {
                need(RepoPermission::Write)?;
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
                Ok((evs, IssueOut::Done))
            }
            IssueOp::RecordStatus(st) => {
                need(RepoPermission::Write)?;
                if me != Some(st.creator) {
                    return Err(IssueErr::Invalid("a status's creator is its actor"));
                }
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
                Ok((evs, IssueOut::Done))
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
                _ => {}
            }
        }
        IssueOut::Done
    }
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

/// The actor's permission on the repository: `Admin` for an owner admin, its grant otherwise, and `Read` for anyone
/// signed in on a public repository.
fn permission(actor: &Principal, v: &RepoView<'_>, authz: Option<&dyn AuthzRead>) -> Option<RepoPermission> {
    let Actor::User(user) = actor.actor else { return None };
    let authz = authz.filter(|a| a.owner() == Some(v.owner()))?;
    if authz.is_admin(user) {
        return Some(RepoPermission::Admin);
    }
    let grant = authz.grant(v.tiers.repo, user);
    let public = v.settings().is_some_and(|s| s.visibility == Visibility::Public);
    grant.max(public.then_some(RepoPermission::Read))
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

fn glob(p: &[u8], s: &[u8]) -> bool {
    match p.first() {
        None => s.is_empty(),
        Some(b'*') if p.get(1) == Some(&b'*') => (0..=s.len()).any(|i| glob(&p[2..], &s[i..])),
        Some(b'*') => {
            let stop = s.iter().position(|c| *c == b'/').unwrap_or(s.len());
            (0..=stop).any(|i| glob(&p[1..], &s[i..]))
        }
        Some(b'?') => s.first().is_some_and(|c| *c != b'/') && glob(&p[1..], &s[1..]),
        Some(b'[') => {
            let Some(end) = p[1..].iter().position(|c| *c == b']').map(|e| e + 1) else {
                return s.first() == Some(&b'[') && glob(&p[1..], &s[1..]);
            };
            let Some(&c) = s.first() else { return false };
            let (neg, set) = match p[1] {
                b'!' | b'^' => (true, &p[2..end]),
                _ => (false, &p[1..end]),
            };
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
            c != b'/' && hit != neg && glob(&p[end + 1..], &s[1..])
        }
        Some(c) => s.first() == Some(c) && glob(&p[1..], &s[1..]),
    }
}
