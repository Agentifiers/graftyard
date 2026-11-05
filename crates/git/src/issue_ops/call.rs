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
    pub fn new(op: IssueOp<BlobRef>, actor: Principal, request: RequestId, render: Value) -> IssueCall {
        IssueCall { op, actor, request, render, confirmed: None }
    }

    /// Records the `DurableBlob` the admitting member rebuilt for the blob `op` names.
    pub fn confirm(&mut self, d: DurableBlob) {
        if self.op.blob() == Some(&d.blob()) {
            self.confirmed = Some(d);
        }
    }

    fn cmd(&self) -> Result<IssueCmd, IssueErr> {
        let confirmed = self.confirmed;
        let op = self.op.clone().map_blob(|b| match confirmed {
            Some(d) if d.blob() == b => Ok(d),
            _ => Err(IssueErr::Unavailable),
        })?;
        Ok(IssueCmd { op, actor: self.actor.clone(), request: self.request })
    }
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
}

impl Serialize for IssueCall {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let Credential::Stored(token) = self.actor.credential;
        Wire {
            format: (REPO_FORMAT_ISSUES.family, REPO_FORMAT_ISSUES.version),
            op: self.op.clone(),
            account: match self.actor.actor {
                Actor::User(u) => Some(u.0.0.0),
                Actor::Anonymous => None,
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
        if f != serde_json::json!([REPO_FORMAT_ISSUES.family, REPO_FORMAT_ISSUES.version]) {
            return Err(serde::de::Error::custom(format!("issue command format {f} is not one this release reads")));
        }
        let w: Wire = serde_json::from_value(v).map_err(serde::de::Error::custom)?;
        let actor = Principal {
            actor: w.account.map_or(Actor::Anonymous, |a| Actor::User(UserId(AccountId(DbId(a))))),
            scopes: ScopeSet(w.scopes),
            credential: Credential::Stored(w.token.0),
            sso_orgs: w.sso.into_iter().map(|o| OrgId(AccountId(DbId(o)))).collect(),
        };
        Ok(IssueCall::new(w.op, actor, w.request, w.render))
    }
}

impl IssueCall {
    pub fn tiers(&self) -> TierSet {
        self.cmd().map_or(TierSet::METADATA, |cmd| cmd.tiers())
    }

    /// `IssueCmd::deps`: the actor's credential chains and the owner's `Authz`, named by the fold's `RepoSettings`.
    pub fn deps(&self, fold: &RepoFold) -> SmallVec<[ChainId; 3]> {
        let mut d = credential_chains(&self.actor.credential_shard(), None);
        if let Some(o) = fold.authz_owner() {
            d.push(ChainId::Authz(o));
        }
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
        v["issue"]["format"] = serde_json::json!([0x20, 6]);
        v["issue"]["op"] = serde_json::json!("a field a later release added");
        let e = serde_json::from_value::<RepoCmd>(v).unwrap_err().to_string();
        assert!(e.contains("is not one this release reads"), "{e}");
    }

    #[test]
    fn a_blob_command_without_a_confirmed_blob_is_unavailable() {
        let mut c = call();
        let b = BlobRef { oid: Digest([1; 32]), bytes: 3, sites: graftyard_types::SiteSet::default() };
        c.op = IssueOp::LfsObject(b);
        assert_eq!(c.cmd().unwrap_err(), IssueErr::Unavailable);
    }
}
