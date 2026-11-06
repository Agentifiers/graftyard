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

use graftyard_identity::{Actor, Credential, IdentityDep, OrgId, Principal, ScopeSet, UserId, credential_chains};
use graftyard_raft::{Format, TierSet};
use graftyard_types::{AccountId, ChainId, DbId, Digest, RequestId, Timestamp};
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

#[derive(Clone, Debug, PartialEq)]
pub struct IssueCall {
    pub op: IssueOp<BlobRef>,
    pub actor: Principal,
    pub request: RequestId,
    /// What the leader renders from the `&Acked`, opaque here (`graftyard_api::hosted::Render`).
    pub render: Value,
    /// The blob `op` names, rebuilt by the member that admits the command. Never sent.
    pub confirmed: Option<DurableBlob>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct IssueCallOut {
    pub out: IssueOut,
    pub render: Value,
}

impl IssueCall {
    pub fn format(&self) -> Format {
        match self.op {
            IssueOp::InviteCollaborator { .. }
            | IssueOp::AnswerInvitation { .. }
            | IssueOp::RemoveCollaborator { .. } => REPO_FORMAT_COLLABORATORS,
            _ => REPO_FORMAT_ISSUES,
        }
    }

    pub fn new(op: IssueOp<BlobRef>, actor: Principal, request: RequestId, render: Value) -> IssueCall {
        IssueCall { op, actor, request, render, confirmed: None }
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
}

impl Serialize for IssueCall {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let (token, session) = self.actor.credential.wire();
        let f = self.format();
        Wire {
            session,
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
        }
        .serialize(s)
    }
}

impl<'de> Deserialize<'de> for IssueCall {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<IssueCall, D::Error> {
        let v = Value::deserialize(d)?;
        let f = v.get("format").cloned().unwrap_or(Value::Null);
        let reads = [REPO_FORMAT_ISSUES, REPO_FORMAT_COLLABORATORS];
        if !reads.iter().any(|r| f == serde_json::json!([r.family, r.version])) {
            return Err(serde::de::Error::custom(format!("issue command format {f} is not one this release reads")));
        }
        let w: Wire = serde_json::from_value(v).map_err(serde::de::Error::custom)?;
        let actor = Principal {
            actor: w.account.map_or(Actor::Anonymous, |a| Actor::User(UserId(AccountId(DbId(a))))),
            scopes: ScopeSet(w.scopes),
            credential: Credential::from_wire(w.token.0, w.session),
            sso_orgs: w.sso.into_iter().map(|o| OrgId(AccountId(DbId(o)))).collect(),
        };
        Ok(IssueCall::new(w.op, actor, w.request, w.render))
    }
}

impl IssueCall {
    pub fn tiers(&self) -> TierSet {
        self.cmd().map_or(TierSet::METADATA, |cmd| cmd.tiers())
    }

    /// `IssueCmd::deps`: the actor's credential chains and the owner's and enterprise's `Authz`, named by the fold's
    /// `RepoSettings`.
    pub fn deps(&self, fold: &RepoFold) -> SmallVec<[ChainId; 3]> {
        let mut d = credential_chains(&self.actor.credential_shard(), None);
        d.extend(crate::repo::authz_chains(fold.authz_owner(), fold.authz_enterprise()));
        d
    }

    pub fn decide(
        &self,
        s: &RepoScratch<'_>,
        deps: &[(ChainId, IdentityDep)],
    ) -> Result<(SmallVec<[RepoEvent; 4]>, IssueCallOut), IssueErr> {
        let cmd = self.cmd()?;
        // A member whose metadata tier is still loading cannot decide on it.
        let (core, meta) = s.view().ok_or(IssueErr::Unavailable)?;
        // `RepoView::apply` stamps writes with its time; `decide` writes nothing, so no time is needed.
        let view = RepoView::new(s.fold().tiers(), core, meta, Timestamp(0));
        let (evs, out) = cmd.decide(&view, deps)?;
        Ok((evs, IssueCallOut { out, render: self.render.clone() }))
    }

    pub fn replay(&self, events: &[RepoEvent]) -> IssueCallOut {
        // `IssueCmd::replay` reads the events and only whether the command was a `SetRule`, so a command whose blob
        // is not confirmed here replays as any other.
        let op = match &self.op {
            IssueOp::SetRule { id, pattern, protection } => {
                IssueOp::SetRule { id: *id, pattern: pattern.clone(), protection: protection.clone() }
            }
            _ => IssueOp::Close { n: crate::issues::IssueNumber(0) },
        };
        let out = IssueCmd { op, actor: self.actor.clone(), request: self.request }.replay(events);
        IssueCallOut { out, render: self.render.clone() }
    }
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
