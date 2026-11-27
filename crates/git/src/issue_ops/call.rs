//! An issue, pull request, release, or LFS command as a repository member commits it, inside
//! `crate::repo::RepoCmd::Issue`: the `IssueCmd` of the parent module with the reply the leader renders from its
//! `&Acked` (`design/DESIGN.md`, Shape: `Acked` "cannot cross a binding, so the member renders the reply bytes and the
//! edge frames them").
//!
//! An `IssueCall` is format 32.5. Its events include `Release`, `ReleaseAsset`, `LfsObject`, and `LfsLock` (tags 17 to
//! 20), so an entry that carries one is format 32.5, and a follower whose release does not read 32.5 refuses it. The
//! edge sends one only once `FormatsEnable` has enabled 32.5 (`design/SHARED.md`, the two-release rule). 32.3 is
//! `PrSync`'s and 32.4 the merge commands' (`crate::merge::REPO_FORMAT_V4`).
//!
//! A command that names a blob crosses the binding with its `BlobRef`; the member rebuilds the `DurableBlob` with
//! `BlobStore::confirm` before it admits the command (`IssueCall::confirm`), so `decide` refuses `Unavailable` unless two
//! buckets hold the bytes (BF-25).

use graftyard_identity::stateless::InstallationClaims;
use graftyard_identity::{Actor, Credential, IdentityDep, OrgId, Principal, ScopeSet, UserId, credential_chains};
use graftyard_raft::{Format, TierSet};
use graftyard_types::{AccountId, ChainId, DbId, Digest, RequestId};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use smallvec::SmallVec;

use super::{IssueCmd, IssueErr, IssueOp, IssueOut, RepoView};
use crate::blob::{BlobRef, DurableBlob};
use crate::repo::{RepoEvent, RepoFold, RepoScratch};

/// The format of an `IssueCall`, and of every entry that carries one.
pub const REPO_FORMAT_ISSUES: Format = Format { family: 0x20, version: 5 };
/// M3-WP3's collaborator and repository-invitation commands, whose events (`Collaborator` 25, `Invitation` 26) a
/// release that reads only 32.5 cannot apply.
pub const REPO_FORMAT_COLLABORATORS: Format = Format { family: 0x20, version: 6 };
/// An issue command under a `ghs_` installation token: the JSON form with an App actor and its claims (`claims`). A
/// release that reads only 32.5 to 32.7 cannot tell the App from an anonymous caller, so such a command is format
/// 32.9, and the edge sends one only once `FormatsEnable` has enabled it. Version 7 is the stateless push's, and 8 is
/// AM1-WP2's agent push (branch `am1-wp2`).
pub const REPO_FORMAT_APP_CALLS: Format = Format { family: 0x20, version: 9 };
/// `CancelInvitation` (BF-34's `DELETE /repos/{o}/{r}/invitations/{id}`): a release that reads only up to 32.9 has no
/// such command. Version 10 is taken by BF-30's transfer `Settings` (branch of the BF-30 work), so this is 32.11.
pub const REPO_FORMAT_INVITATIONS: Format = Format { family: 0x20, version: 11 };
/// `InitRepo` and `DeleteRepo` (`rest:repos`' create and delete) and their events `Created` and `Deleted`.
pub use crate::repo::REPO_FORMAT_REPOS;
/// `IssueOp::CreateCheck` (`POST /repos/{o}/{r}/check-runs`), whose run id `decide` takes from the metadata tier's
/// cursor and whose `CheckRun` moves that cursor: a release that reads only up to 32.17 would apply the entry without
/// moving it and could hand the same id out again. Always under an App's claims.
pub const REPO_FORMAT_CHECKS: Format = Format { family: 0x20, version: 18 };
/// A review comment outside a review whose entry carries its single-comment review as `ReviewEvent::SingleComment`
/// (tag 3), which a release that reads only up to 32.21 cannot decode, and which leaves an APPROVED or
/// CHANGES_REQUESTED verdict alone. 32.20 is B0-30's `REPO_FORMAT_SPLIT` and 32.21 M4-WP3's `REPO_FORMAT_ACTIONS`
/// (other branches). The edge marks it (`IssueCall::reviews`) once `FormatsEnable` enabled it. Under a `ghs_` token it
/// carries the App's claims, as 32.9 does.
pub const REPO_FORMAT_REVIEWS: Format = Format { family: 0x20, version: 22 };
/// `bf77-routes` (BF-77's token half): a REST `POST /repos/{o}/{r}/issues` (`IssueCall::rest_issue`), whose 201 a
/// release without `What::IssueCreated` cannot render after the entry commits, and an App's `RecordStatus`, which a
/// release without the `statuses` permission and the bot creator (`AppId::bot`) refuses. Laid out as 32.9 for an App
/// and as 32.5 for a user. A member that cannot read it refuses it before it commits, and the edge sends it only once
/// `FormatsEnable` enabled it; a command already in 32.8, 32.12, 32.14 or 32.28 keeps that format.
pub const REPO_FORMAT_ROUTES: Format = Format { family: 0x20, version: 29 };
/// F20's assignment changes (`decisions/DECISIONS.md`, "`assign`"): `IssueOp::Assign` (tag 46) and its event
/// `RepoEvent::Assigned` (tag 48), which a release that reads only up to 32.29 cannot decode. An `Assign` always takes
/// it, whoever sends it: it carries an App's claims, a task's included, or none for a user. The edge sends it only once
/// `FormatsEnable` enabled it.
pub const REPO_FORMAT_ASSIGN: Format = Format { family: 0x20, version: 30 };
/// F137: a REST `POST /repos/{o}/{r}/issues/{n}/comments` (`IssueCall::rest_comment`), whose 201 a release without
/// `What::IssueCommentCreated` cannot render after the entry commits, and which an App's token authors as its bot
/// account (`AppId::bot`), as 32.29 does for an issue. It carries an App's claims, a task's included, or none for a
/// user, as 32.30 does. A member that cannot read it refuses it before it commits, and the edge sends it only once
/// `FormatsEnable` enabled it.
pub const REPO_FORMAT_ISSUE_COMMENT: Format = Format { family: 0x20, version: 32 };
/// AM3-WP3's host pause (`decisions/DECISIONS.md`, "`host-pause`"): a title or body `Change` or an `Assign`
/// (`IssueCall::pausable`) whose entry may carry the pause (`TaskUpdated` to `waiting_for_user`) or the resume
/// (`RepoEvent::TaskResumed`, tag 49) of the issue's Graftyard-run tasks, which a release that reads only up to 32.30
/// cannot decode. Once `FormatsEnable` enabled it the edge marks every such command, whether or not the issue has a
/// task, so a task created after the edge's read is still paused. It carries an App's claims, a task's included, or
/// none for a user, as 32.30 does, and a body edit's lease events, as 32.14 does. 32.31 is unused, and 32.32 is F137's
/// issue comments.
pub const REPO_FORMAT_HOST_PAUSE: Format = Format { family: 0x20, version: 33 };
/// F137's REST edits (`decisions/DECISIONS.md`, "`issue-writes`"): a multi-field `PATCH /repos/{o}/{r}/issues/{n}`
/// (`IssueOp::Edit`, tag 47), and a REST comment `PATCH` (`EditComment`) or lock `PUT`/`DELETE` (`Change` of
/// `Locked`, with `LockReason::Unspecified` for a lock that names no reason) the edge marks (`IssueCall::rest_edit`),
/// whose replies a release without `What::IssueUpdated`, `What::IssueCommentUpdated` and `What::IssueLocked` cannot
/// render after the entry commits. It carries what 32.33 does: an App's claims, a task's included, or none for a
/// user, and for an `Edit` the pause and a body edit's lease events. A member that cannot read it refuses it before it
/// commits, and the edge sends it only once `FormatsEnable` enabled it. 32.34 is the forks' (`REPO_FORMAT_FORKS`): a new
/// format is above every format on main when it merges.
pub const REPO_FORMAT_ISSUE_EDIT: Format = Format { family: 0x20, version: 35 };
/// F137's REST label update (`decisions/DECISIONS.md`, "`label-routes`"): `PATCH /repos/{o}/{r}/labels/{name}`, a
/// `DefineLabel` of an existing label's id the edge marks (`IssueCall::rest_label`), whose reply a release without
/// `What::LabelUpdated` cannot render after the entry commits. It carries what 32.35 does. A member that cannot read it
/// refuses it before it commits, and the edge sends it only once `FormatsEnable` enabled it. It is main's highest
/// repository format plus one when it merged ("`format-order`").
pub const REPO_FORMAT_LABEL_UPDATE: Format = Format { family: 0x20, version: 36 };
/// AM3-WP2a, base forks (`build/BUILD-PLAN.md`): `RepoEvent::Genesis` (tag 50), whose fork's core `fork` row names the
/// repository's network. A member that cannot read it refuses it before it commits, and the edge sends it only once
/// `FormatsEnable` enabled it. It is above every format on main when it merged, not 32.31: an entry takes the highest
/// format of its commands, so a `Genesis` batched with a 32.33 command commits as 32.33, and a member that reads 32.33
/// but not a lower `Genesis` format would pass the entry's format check and then meet tag 50 (`decisions/DECISIONS.md`,
/// "`format-order`").
pub const REPO_FORMAT_FORKS: Format = Format { family: 0x20, version: 34 };

#[derive(Clone, Debug, PartialEq)]
pub struct IssueCall {
    pub op: IssueOp<BlobRef>,
    pub actor: Principal,
    pub request: RequestId,
    /// Format 32.8 (`crate::repo::REPO_FORMAT_AGENT`): the edge sends it only once `FormatsEnable` enabled 32.8, and
    /// only then may the command's entry carry the agent events. `SetAgentPolicy` is always 32.8.
    pub agent: bool,
    /// The session an `OpenPr` names in `X-Graftyard-Agent-Session` (format 32.12): `decide` takes it into
    /// `AgentMeta` only for a task of the opening App.
    pub session: Option<graftyard_identity::SessionId>,
    /// Format 32.14 (`crate::repo::REPO_FORMAT_LEASES`): the edge enabled it, so the command's entry may carry lease
    /// events: a draft's plan-block lease, or a task's leases released by its cancel or finish. A lease command is
    /// always 32.14.
    pub leases: bool,
    /// Format 32.22 (`REPO_FORMAT_REVIEWS`): the edge enabled it and the command is a review comment outside a review
    /// (`IssueCall::single_comment`), so its review is a `SingleComment`.
    pub reviews: bool,
    /// Format 32.29 (`REPO_FORMAT_ROUTES`): an `OpenIssue` from REST `POST .../issues`, whose reply is
    /// `What::IssueCreated`.
    pub rest_issue: bool,
    /// Format 32.32 (`REPO_FORMAT_ISSUE_COMMENT`): an issue `Comment` from REST `POST .../issues/{n}/comments`, whose
    /// reply is `What::IssueCommentCreated`.
    pub rest_comment: bool,
    /// Format 32.33 (`REPO_FORMAT_HOST_PAUSE`): the edge enabled it and the command is `pausable`, so its entry may
    /// pause or resume the issue's Graftyard-run tasks.
    pub pause: bool,
    /// Format 32.35 (`REPO_FORMAT_ISSUE_EDIT`): a REST comment edit or lock change (`IssueCall::edit_mark`), whose
    /// reply only a 32.35 member renders. An `Edit` is always 32.35.
    pub rest_edit: bool,
    /// Format 32.36 (`REPO_FORMAT_LABEL_UPDATE`): a REST label update (`IssueCall::label_mark`), whose reply only a
    /// 32.36 member renders.
    pub rest_label: bool,
    /// What the leader renders from the `&Acked`, opaque here (`graftyard_api::hosted::Render`).
    pub render: Value,
    /// The blob `op` names, rebuilt by the member that admits the command. Never sent.
    pub confirmed: Option<DurableBlob>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct IssueCallOut {
    pub out: IssueOut,
    pub render: Value,
    /// The owner's `Authz` dependency of the command's entry, when its render carries none: the leader's member renders
    /// the reply from it (`graftyard_api::hosted::fill_authz`, `decisions/DECISIONS.md`, "`leader-render`"). One `Rc`
    /// for every command `decide` saw it for, so the resolved tickets a leader keeps hold one fold. None on `replay`.
    pub authz: Option<std::rc::Rc<graftyard_identity::AuthzFold>>,
}

impl IssueCall {
    /// The actor whose committed writes a Graftyard-run create reads (`crate::task::decide`): its user, keyed as
    /// `group::AdmitWire::of` keys it. Only a create from a user reads one; every other command none.
    pub fn demote_actor(&self) -> Option<graftyard_raft::admit::ActorKey> {
        match (&self.op, self.actor.actor) {
            (IssueOp::CreateRun(_), Actor::User(u)) => Some(graftyard_raft::admit::ActorKey(u.0.0.0 as u64)),
            _ => None,
        }
    }

    pub fn format(&self) -> Format {
        // 32.14 first: a lease command, or a command the edge marked, which may carry lease events. It may carry
        // 32.8's and 32.12's too.
        // 32.16 first of all: a hook command is never an agent, task, or lease command, and carries an App's claims
        // as 32.9 does.
        if self.op.is_hook() {
            return crate::hooks::REPO_FORMAT_HOOKS;
        }
        // 32.23 likewise: a secret or variable command is never an agent, task, or lease command.
        if self.op.is_secret() {
            return crate::secrets::REPO_FORMAT_SECRETS;
        }
        // 32.36 likewise: a REST label update.
        if self.rest_label {
            return REPO_FORMAT_LABEL_UPDATE;
        }
        // 32.35 likewise: a multi-field issue edit, or a REST comment edit or lock change.
        if self.rest_edit || matches!(self.op, IssueOp::Edit { .. }) {
            return REPO_FORMAT_ISSUE_EDIT;
        }
        // 32.33 likewise, before 32.30 and 32.14: it carries what each of them does for these commands.
        if self.pause {
            return REPO_FORMAT_HOST_PAUSE;
        }
        // 32.30 likewise: an assignment change is never an agent, task, lease, or review command.
        if matches!(self.op, IssueOp::Assign { .. }) {
            return REPO_FORMAT_ASSIGN;
        }
        // 32.32 likewise: a REST issue comment, whatever claims its token carries.
        if self.rest_comment {
            return REPO_FORMAT_ISSUE_COMMENT;
        }
        if self.reviews {
            return REPO_FORMAT_REVIEWS;
        }
        if matches!(self.op, IssueOp::CreateCheck(_)) {
            return REPO_FORMAT_CHECKS;
        }
        if self.op.is_lease() || self.leases {
            return crate::repo::REPO_FORMAT_LEASES;
        }
        // 32.28 before 32.12: a Graftyard-run create, which a release that reads only 32.12 cannot decode.
        if matches!(self.op, IssueOp::CreateRun(_)) {
            return crate::repo::REPO_FORMAT_RUNS;
        }
        // 32.12 next: a task command, a `task` claim on the token, or a session the client named, none of which an
        // earlier release reads. It may carry 32.8's agent events too.
        if self.op.is_task() || crate::task::claim(&self.actor).is_some() || self.session.is_some() {
            return crate::repo::REPO_FORMAT_TASKS;
        }
        // 32.8 next: only it may carry agent events, so an App's 32.8 command carries its claims as 32.9 does.
        if self.agent || matches!(self.op, IssueOp::SetAgentPolicy(_)) {
            return crate::repo::REPO_FORMAT_AGENT;
        }
        // A repository's create and delete, under a user's credential only: the edge sends neither under an
        // installation token, whose claims 32.13 does not carry.
        if matches!(self.op, IssueOp::InitRepo(_) | IssueOp::DeleteRepo { .. }) {
            return REPO_FORMAT_REPOS;
        }
        // 32.29: a REST issue create, and an App's commit status.
        if self.rest_issue
            || (matches!(self.op, IssueOp::RecordStatus(_)) && matches!(self.actor.actor, Actor::App { .. }))
        {
            return REPO_FORMAT_ROUTES;
        }
        if matches!(self.actor.credential, Credential::Stateless(_)) {
            return REPO_FORMAT_APP_CALLS;
        }
        match self.op {
            IssueOp::InviteCollaborator { .. }
            | IssueOp::AnswerInvitation { .. }
            | IssueOp::RemoveCollaborator { .. } => REPO_FORMAT_COLLABORATORS,
            IssueOp::CancelInvitation { .. } => REPO_FORMAT_INVITATIONS,
            _ => REPO_FORMAT_ISSUES,
        }
    }

    pub fn new(op: IssueOp<BlobRef>, actor: Principal, request: RequestId, render: Value) -> IssueCall {
        IssueCall {
            op,
            actor,
            request,
            render,
            confirmed: None,
            agent: false,
            session: None,
            leases: false,
            reviews: false,
            rest_issue: false,
            rest_comment: false,
            pause: false,
            rest_edit: false,
            rest_label: false,
        }
    }

    /// Whether `op` may pause or resume a task in format 32.33: a title or body edit, or an assignment change.
    pub fn pausable<B>(op: &IssueOp<B>) -> bool {
        matches!(
            op,
            IssueOp::Change { change: crate::issues::IssueChange::Title(_) | crate::issues::IssueChange::Body(_), .. }
                | IssueOp::Assign { .. }
        )
    }

    /// Whether `op` is a command 32.35 marks when it comes from REST: a comment edit, or a lock change.
    pub fn edit_mark<B>(op: &IssueOp<B>) -> bool {
        matches!(
            op,
            IssueOp::EditComment { .. } | IssueOp::Change { change: crate::issues::IssueChange::Locked(_), .. }
        )
    }

    /// Whether `op` is a command 32.36 marks when it comes from REST: a `DefineLabel` of an existing label's id.
    pub fn label_mark<B>(op: &IssueOp<B>) -> bool {
        matches!(op, IssueOp::DefineLabel { id: Some(_), .. })
    }

    /// Whether `op` is a review comment outside a review, which 32.22 gives a `SingleComment` review. Never a lease,
    /// task, hook, or agent command, so the mark takes no other format's place.
    pub fn single_comment<B>(op: &IssueOp<B>) -> bool {
        matches!(op, IssueOp::Comment { on: crate::issues::CommentTarget::Review { review: None, .. }, .. })
    }

    /// Whether `op` is a comment on an issue or a pull request's conversation, which 32.32 marks when it comes from
    /// REST.
    pub fn issue_comment<B>(op: &IssueOp<B>) -> bool {
        matches!(op, IssueOp::Comment { on: crate::issues::CommentTarget::Issue(_), .. })
    }

    /// Records the `DurableBlob` the admitting member rebuilt for the blob `op` names. The blob is its oid and size:
    /// the sites the sender saw (none for an LFS `verify`) are replaced by the ones `BlobStore::confirm` found.
    pub fn confirm(&mut self, d: DurableBlob) {
        if self.op.blob().is_some_and(|b| same_bytes(b, &d.blob())) {
            self.confirmed = Some(d);
        }
    }

    fn cmd(&self) -> Result<IssueCmd, IssueErr> {
        let confirmed = self.confirmed;
        let op = self.op.clone().map_blob(|b| match confirmed {
            Some(d) if same_bytes(&b, &d.blob()) => Ok(d),
            _ => Err(IssueErr::Unavailable),
        })?;
        Ok(IssueCmd { op, actor: self.actor.clone(), request: self.request })
    }
}

fn same_bytes(a: &BlobRef, b: &BlobRef) -> bool {
    a.oid == b.oid && a.bytes == b.bytes
}

/// The JSON an `IssueCall` crosses the binding as. `format` comes first and is checked first, so a member that does
/// not read this format refuses the command without reading the rest.
#[derive(Serialize, Deserialize)]
struct Wire {
    format: (u16, u16),
    op: IssueOp<BlobRef>,
    account: Option<i64>,
    scopes: u64,
    token: Digest,
    sso: Vec<i64>,
    request: RequestId,
    render: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    session: Option<graftyard_types::Ulid>,
    /// A `ghs_` token's verified claims, in format 32.9, and in 32.8 and 32.12 for an App's agent command. Only 32.12
    /// carries their `task`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    claims: Option<InstallationClaims>,
    /// `IssueCall::session`, in format 32.12 only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    agent_session: Option<graftyard_identity::SessionId>,
}

impl Serialize for IssueCall {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let (token, session) = self.actor.credential.wire();
        let f = self.format();
        Wire {
            session,
            claims: match &self.actor.credential {
                Credential::Stateless(c) => Some((**c).clone()),
                _ => None,
            },
            format: (f.family, f.version),
            op: self.op.clone(),
            account: match self.actor.actor {
                Actor::User(u) => Some(u.0.0.0),
                Actor::App { .. } | Actor::Anonymous => None,
            },
            scopes: self.actor.scopes.0,
            token: Digest(token),
            sso: self.actor.sso_orgs.iter().map(|o| o.0.0.0).collect(),
            request: self.request,
            render: self.render.clone(),
            agent_session: self.session.clone(),
        }
        .serialize(s)
    }
}

impl<'de> Deserialize<'de> for IssueCall {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<IssueCall, D::Error> {
        let v = Value::deserialize(d)?;
        let f = v.get("format").cloned().unwrap_or(Value::Null);
        let reads = [
            REPO_FORMAT_ISSUES,
            REPO_FORMAT_COLLABORATORS,
            crate::repo::REPO_FORMAT_AGENT,
            REPO_FORMAT_APP_CALLS,
            REPO_FORMAT_INVITATIONS,
            crate::repo::REPO_FORMAT_TASKS,
            REPO_FORMAT_REPOS,
            crate::repo::REPO_FORMAT_LEASES,
            crate::hooks::REPO_FORMAT_HOOKS,
            REPO_FORMAT_CHECKS,
            REPO_FORMAT_REVIEWS,
            crate::secrets::REPO_FORMAT_SECRETS,
            crate::repo::REPO_FORMAT_RUNS,
            REPO_FORMAT_ROUTES,
            REPO_FORMAT_ASSIGN,
            REPO_FORMAT_ISSUE_COMMENT,
            REPO_FORMAT_HOST_PAUSE,
            REPO_FORMAT_ISSUE_EDIT,
            REPO_FORMAT_LABEL_UPDATE,
        ];
        if !reads.iter().any(|r| f == serde_json::json!([r.family, r.version])) {
            return Err(serde::de::Error::custom(format!("issue command format {f} is not one this release reads")));
        }
        let fmt = |x: Format| f == serde_json::json!([x.family, x.version]);
        let leases = fmt(crate::repo::REPO_FORMAT_LEASES);
        // 32.28 (a Graftyard-run create) carries any principal, so an App's or a claimed token's create reaches
        // `decide` and is refused there, as 32.12 does.
        let runs = fmt(crate::repo::REPO_FORMAT_RUNS);
        // 32.14 is a superset of 32.12: it carries a task claim and a session as 32.12 does.
        let tasks = fmt(crate::repo::REPO_FORMAT_TASKS) || leases || runs;
        let (app, agent) = (fmt(REPO_FORMAT_APP_CALLS), fmt(crate::repo::REPO_FORMAT_AGENT));
        // 32.16 carries an App's claims, or none for a user, as 32.8 does.
        let hooks = fmt(crate::hooks::REPO_FORMAT_HOOKS);
        // 32.18 always carries an App's claims.
        let checks = fmt(REPO_FORMAT_CHECKS);
        // 32.22 carries an App's claims, or none for a user, as 32.16 does.
        let reviews = fmt(REPO_FORMAT_REVIEWS);
        // 32.23 carries an App's claims, or none for a user, as 32.16 does.
        let secrets = fmt(crate::secrets::REPO_FORMAT_SECRETS);
        // 32.29 carries an App's claims, or none for a user, as 32.16 does.
        let routes = fmt(REPO_FORMAT_ROUTES);
        // 32.30 carries an App's claims, a task claim included, or none for a user, and 32.32 and 32.33 what 32.30 does.
        let comment = fmt(REPO_FORMAT_ISSUE_COMMENT);
        let pause = fmt(REPO_FORMAT_HOST_PAUSE);
        // 32.35 carries what 32.33 does.
        let edit = fmt(REPO_FORMAT_ISSUE_EDIT);
        // 32.36 carries what 32.35 does.
        let label = fmt(REPO_FORMAT_LABEL_UPDATE);
        let assign = fmt(REPO_FORMAT_ASSIGN) || comment || pause || edit || label;
        let w: Wire = serde_json::from_value(v).map_err(serde::de::Error::custom)?;
        let task_claim = w.claims.as_ref().is_some_and(|c| c.task.is_some());
        if !tasks && (w.agent_session.is_some() || (task_claim && !assign)) {
            return Err(serde::de::Error::custom("a task claim or a session rides only in format 32.12"));
        }
        let actor = match (app || agent || tasks || hooks || checks || reviews || secrets || routes || assign, w.claims)
        {
            (true, Some(c)) => Principal::installation(&c),
            (false, Some(_)) | (true, None)
                if !agent && !tasks && !hooks && !reviews && !secrets && !routes && !assign =>
            {
                return Err(serde::de::Error::custom(
                    "claims ride only in format 32.8 or 32.9, and 32.9 always has them",
                ));
            }
            _ => Principal {
                actor: w.account.map_or(Actor::Anonymous, |a| Actor::User(UserId(AccountId(DbId(a))))),
                scopes: ScopeSet(w.scopes),
                credential: Credential::from_wire(w.token.0, w.session),
                sso_orgs: w.sso.into_iter().map(|o| OrgId(AccountId(DbId(o)))).collect(),
            },
        };
        let mut c = IssueCall::new(w.op, actor, w.request, w.render);
        c.agent = agent;
        c.session = w.agent_session;
        // In 32.14 the tag carries the mark, and `agent` is implied (`agent_events`).
        c.leases = leases && !c.op.is_lease();
        c.agent = agent;
        c.reviews = reviews && IssueCall::single_comment(&c.op);
        c.rest_issue = routes && matches!(c.op, IssueOp::OpenIssue { .. });
        c.rest_comment = comment && IssueCall::issue_comment(&c.op);
        c.pause = pause && IssueCall::pausable(&c.op);
        c.rest_edit = edit && IssueCall::edit_mark(&c.op);
        c.rest_label = label && IssueCall::label_mark(&c.op);
        // One form per command: a format that does not match the command is malformed.
        let got = c.format();
        if !fmt(got) {
            return Err(serde::de::Error::custom("issue command format does not match the command"));
        }
        Ok(c)
    }
}

impl IssueCall {
    pub fn tiers(&self) -> TierSet {
        // A 32.33 command may write a task's core row (`TaskUpdated`) and its metadata row (`TaskResumed`).
        if self.pause || matches!(self.op, IssueOp::Edit { .. }) {
            return TierSet::CORE.union(TierSet::METADATA);
        }
        self.cmd().map_or(TierSet::METADATA, |cmd| cmd.tiers())
    }

    /// `IssueCmd::deps`: the actor's credential chains and the owner's and enterprise's `Authz`, named by the fold's
    /// `RepoSettings`. An App's command, and any 32.8 command, also reads the install's `Authz(Genesis.account)` for its
    /// `AgentApp` rows: an App's for its class, and a user's whose token may be an `AgentApp`'s or whose approval may
    /// write `AgentGate`. Every other command reads what it did.
    pub fn deps(&self, fold: &RepoFold) -> SmallVec<[ChainId; 3]> {
        let mut d = credential_chains(&self.actor.credential_shard(), None);
        d.extend(crate::repo::authz_chains(fold.authz_owner(), fold.authz_enterprise()));
        crate::repo::with_install(&mut d, &self.actor);
        let install = ChainId::Authz(graftyard_identity::install_account());
        // An assignment, or an edit that sets assignees, reads the install's `AgentApp` rows for the one assignable bot.
        if (self.agent_events() || matches!(self.op, IssueOp::Assign { .. } | IssueOp::Edit { .. }))
            && !d.contains(&install)
        {
            d.push(install);
        }
        d
    }

    pub fn decide(
        &self,
        s: &RepoScratch<'_>,
        deps: &[(ChainId, IdentityDep)],
    ) -> Result<(SmallVec<[RepoEvent; 4]>, IssueCallOut), IssueErr> {
        let cmd = self.cmd()?;
        // A member whose metadata tier is still loading cannot decide on it. A lease command reads the core tier
        // alone (L0-16), so it decides there with an empty metadata view.
        let empty = graftyard_fold::MemStore::default();
        let (core, meta) = match s.view() {
            Some(v) => v,
            None if self.op.is_lease() => (s.core(), &empty as &dyn graftyard_fold::Read),
            None => return Err(IssueErr::Unavailable),
        };
        // The entry's time: an invitation's `expires` is set from it and checked against it.
        let view = RepoView::new(s.fold().tiers(), core, meta, s.at());
        // 32.33 carries a body edit's lease events too, and so does a 32.35 `Edit`, which carries its pause.
        let edit = matches!(self.op, IssueOp::Edit { .. });
        let leases = self.format() == crate::repo::REPO_FORMAT_LEASES || self.pause || edit;
        let (evs, out) = if self.op.is_task() {
            cmd.decide_task(&view, deps, s.at(), s.applied(), leases, s.actor_writes())?
        } else if self.op.is_lease() {
            cmd.decide_lease(&view, deps, s.at())?
        } else {
            let bot = matches!(self.format(), REPO_FORMAT_ROUTES | REPO_FORMAT_ISSUE_COMMENT);
            let pause = (self.pause || edit).then(|| s.applied());
            cmd.decide_in(&view, deps, self.agent_events(), self.session.as_ref(), leases, self.reviews, bot, pause)?
        };
        let authz = self.render_lacks_authz().then(|| owner_authz(s.fold(), deps)).flatten();
        Ok((evs, IssueCallOut { out, render: self.render.clone(), authz }))
    }

    /// Whether the edge sent the render without the owner's `Authz` (`HostedDir.authz` null), for the leader to fill.
    pub fn render_lacks_authz(&self) -> bool {
        self.render.get("dir").is_some_and(|d| d.get("authz").is_none_or(Value::is_null))
    }

    /// Whether the command's entry may carry the agent events: formats 32.8, 32.12, and 32.14.
    pub fn agent_events(&self) -> bool {
        let f = self.format();
        f == crate::repo::REPO_FORMAT_AGENT
            || f == crate::repo::REPO_FORMAT_TASKS
            || f == crate::repo::REPO_FORMAT_LEASES
            || f == crate::repo::REPO_FORMAT_RUNS
    }

    pub fn replay(&self, events: &[RepoEvent]) -> IssueCallOut {
        // `IssueCmd::replay` reads the events and only whether the command was a `SetRule`, so a command whose blob
        // is not confirmed here replays as any other.
        let op = match &self.op {
            IssueOp::SetRule { id, pattern, protection } => {
                IssueOp::SetRule { id: *id, pattern: pattern.clone(), protection: protection.clone() }
            }
            // A task command's replay reads which command it was (`crate::task::replay`); none names a blob.
            IssueOp::CreateTask(t) => IssueOp::CreateTask(t.clone()),
            IssueOp::CreateRun(n) => IssueOp::CreateRun(n.clone()),
            IssueOp::UpdateTask { id, state, pr, summary } => {
                IssueOp::UpdateTask { id: id.clone(), state: *state, pr: *pr, summary: summary.clone() }
            }
            IssueOp::CancelTask { id } => IssueOp::CancelTask { id: id.clone() },
            IssueOp::AcquireLease(l) => IssueOp::AcquireLease(l.clone()),
            IssueOp::RenewLease { id, ttl_seconds } => IssueOp::RenewLease { id: *id, ttl_seconds: *ttl_seconds },
            IssueOp::ReleaseLease { id } => IssueOp::ReleaseLease { id: *id },
            // A check-run create's replay answers the run id its first decide assigned.
            IssueOp::CreateCheck(c) => IssueOp::CreateCheck(c.clone()),
            _ => IssueOp::Close { n: crate::issues::IssueNumber(0) },
        };
        let out = IssueCmd { op, actor: self.actor.clone(), request: self.request }.replay(events);
        IssueCallOut { out, render: self.render.clone(), authz: None }
    }
}

thread_local! {
    /// The owner `Authz` fold the last `decide` that filled one shared (`owner_authz`).
    static LAST_AUTHZ: std::cell::RefCell<Option<std::rc::Rc<graftyard_identity::AuthzFold>>> =
        const { std::cell::RefCell::new(None) };
}

/// The owner's `Authz` dependency among `deps`, for the chain `deps()` names on `fold` (`authz_owner`): the fold
/// shared by the last command that found an equal one, so commands decided against one fold hold one copy.
fn owner_authz(fold: &RepoFold, deps: &[(ChainId, IdentityDep)]) -> Option<std::rc::Rc<graftyard_identity::AuthzFold>> {
    let owner = fold.authz_owner()?;
    let f = deps.iter().find_map(|(c, d)| match (c, d) {
        (ChainId::Authz(o), IdentityDep::Authz(f)) if *o == owner && f.owner == Some(owner) => Some(f),
        _ => None,
    })?;
    LAST_AUTHZ.with(|last| {
        let mut last = last.borrow_mut();
        match last.as_ref() {
            Some(rc) if **rc == *f => Some(rc.clone()),
            _ => {
                let rc = std::rc::Rc::new(f.clone());
                *last = Some(rc.clone());
                Some(rc)
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::issues::{CommentTarget, IssueNumber};
    use crate::repo::RepoCmd;

    fn call() -> IssueCall {
        let actor = Principal {
            actor: Actor::User(UserId(AccountId(DbId(7)))),
            scopes: ScopeSet(1),
            credential: Credential::Stored([9; 32]),
            sso_orgs: SmallVec::new(),
        };
        let op = IssueOp::Comment { on: CommentTarget::Issue(IssueNumber(1)), body: "hi".into(), reply_to: None };
        IssueCall::new(op, actor, RequestId(graftyard_types::Ulid(5)), serde_json::json!({ "what": 1 }))
    }

    #[test]
    fn an_issue_call_round_trips_and_names_its_format() {
        use graftyard_raft::Command;
        let c = RepoCmd::Issue(Box::new(call()));
        let v = serde_json::to_value(&c).unwrap();
        assert_eq!(v["issue"]["format"], serde_json::json!([0x20, 5]));
        assert_eq!(serde_json::from_value::<RepoCmd>(v).unwrap(), c);
        assert_eq!(c.format(), REPO_FORMAT_ISSUES);
        assert_ne!(REPO_FORMAT_ISSUES, crate::merge::REPO_FORMAT_V4);
    }

    #[test]
    fn another_issue_format_is_refused_before_the_rest_is_read() {
        let mut v = serde_json::to_value(RepoCmd::Issue(Box::new(call()))).unwrap();
        v["issue"]["format"] = serde_json::json!([0x20, 7]);
        v["issue"]["op"] = serde_json::json!("a field a later release added");
        let e = serde_json::from_value::<RepoCmd>(v).unwrap_err().to_string();
        assert!(e.contains("is not one this release reads"), "{e}");
    }

    /// The edge's `verify` names no sites, and an upload names the ones its put saw; the member's `confirm` names
    /// the ones it found, which decide.
    #[test]
    fn a_confirmed_blob_is_matched_by_its_bytes_not_its_sites() {
        use crate::mem::MemBucket;
        use graftyard_types::{Region, Site, SiteId, SiteLink, SiteTable};
        let site = |id: u8, region| Site {
            id: SiteId(id),
            script: format!("s{id}"),
            account: "x".into(),
            region,
            jurisdiction: None,
            bucket: format!("b{id}"),
            release: None,
            link: SiteLink::Binding { member_class: "Member".into() },
            spare: false,
        };
        let sites = SiteTable(vec![site(0, Region::Enam), site(1, Region::Weur)]);
        let (a, b) = (MemBucket::default(), MemBucket::default());
        let bk = [(SiteId(0), &a), (SiteId(1), &b)];
        let store = crate::blob::BlobStore { sites: &sites, buckets: &bk, copies: crate::Copies::TwoRegions };
        let meta = crate::ObjectMeta {
            chain: ChainId::Repo(graftyard_types::RepoId(DbId(7))),
            request: RequestId(graftyard_types::Ulid(1)),
        };
        let placed = futures_executor::block_on(store.place(graftyard_types::RepoId(DbId(7)), b"obj", None, meta))
            .unwrap()
            .blob();
        let found = futures_executor::block_on(store.confirm(graftyard_types::RepoId(DbId(7)), placed.oid, 3)).unwrap();
        let mut c = call();
        c.op = IssueOp::LfsObject(BlobRef { sites: Default::default(), ..placed });
        c.confirm(found);
        assert_eq!(c.cmd().unwrap().op.blob().map(|d| d.blob()), Some(found.blob()));
        let mut wrong = call();
        wrong.op = IssueOp::LfsObject(BlobRef { bytes: 4, ..placed });
        wrong.confirm(found);
        assert_eq!(wrong.cmd().unwrap_err(), IssueErr::Unavailable, "another size is another blob");
    }

    #[test]
    fn a_blob_command_without_a_confirmed_blob_is_unavailable() {
        let mut c = call();
        let b = BlobRef { oid: Digest([1; 32]), bytes: 3, sites: graftyard_types::SiteSet::default() };
        c.op = IssueOp::LfsObject(b);
        assert_eq!(c.cmd().unwrap_err(), IssueErr::Unavailable);
    }
}
