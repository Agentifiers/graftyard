//! `authorize`, `authorize_write`, and `Decision` (`design/SHARED.md`, "Identity and tokens"; `build/BUILD-PLAN.md`,
//! M3-WP3).
//!
//! Pure over the repository's core tier, through `RepoAccess`, and the owner's `Authz` fold, plus the enterprise's for
//! an enterprise-owned org. A user's permission is the highest of what the owner's chain grants and a `Collaborator`
//! row on the repository chain: "`authorize` takes the higher of a member's `Grant` and a `Collaborator` row left from
//! before the user joined, so joining keeps the access the user had". Denial on a private resource is `Hidden`, which
//! renders as 404.

use graftyard_types::{AccountId, RepoId};
use serde::{Deserialize, Serialize};

use crate::authz::{AccountKind, AuthzRead, RepoPermission};
use crate::credential::{Actor, OrgId, Principal, UserId};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Visibility {
    Public,
    Private,
    Internal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Allow,
    /// 403: the caller may read the repository but not do this.
    Forbidden,
    /// 404: the caller may not learn that the repository exists.
    Hidden,
}

/// What a read asks for. Every read needs `Read` on the repository; the variants name what the edge serves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadAction {
    Metadata,
    Contents,
    Issues,
}

/// What a write needs, by GitHub's repository roles [inference: SHARED names `WriteAction` without its variants; these
/// are the roles' documented boundaries].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriteAction {
    /// Open an issue or pull request, comment, react: `Read`.
    Participate,
    /// Label, assign, close others' issues, request reviews: `Triage`.
    Triage,
    /// Push, merge, record a check or status: `Write`.
    Push,
    /// Topics, some settings: `Maintain`.
    Maintain,
    /// Rules, settings, collaborators, transfer: `Admin`.
    Administer,
}

impl WriteAction {
    pub fn needs(self) -> RepoPermission {
        match self {
            WriteAction::Participate => RepoPermission::Read,
            WriteAction::Triage => RepoPermission::Triage,
            WriteAction::Push => RepoPermission::Write,
            WriteAction::Maintain => RepoPermission::Maintain,
            WriteAction::Administer => RepoPermission::Admin,
        }
    }
}

/// The repository chain's part of an authorization: its core tier's settings and collaborator rows.
pub trait RepoAccess {
    fn repo(&self) -> RepoId;
    /// The owner whose `Authz` chain gates the repository (`RepoSettings.owner`).
    fn owner(&self) -> AccountId;
    fn visibility(&self) -> Visibility;
    /// The user's `Collaborator` row.
    fn collaborator(&self, user: UserId) -> Option<RepoPermission>;
}

/// The caller's permission on the repository, or `None`. `authz` holds the folds the leader or edge read: the owner's
/// `Authz`, and the enterprise's when the owner is an enterprise-owned org. A fold for another owner is ignored, so a
/// stale read never grants.
pub fn permission(p: &Principal, authz: &[&dyn AuthzRead], repo: &dyn RepoAccess) -> Option<RepoPermission> {
    let Actor::User(user) = p.actor else { return None };
    let owner = repo.owner();
    let collab = repo.collaborator(user);
    let Some(a) = authz.iter().find(|a| a.owner() == Some(owner)) else { return collab };
    // Access that comes through org membership needs the org's SSO when its policy enforces it; a user account's own
    // access and an outside collaborator's do not.
    let org = a.opened() == Some(AccountKind::Org);
    let sso_ok = !org || !a.sso_enforced() || p.sso_orgs.contains(&OrgId(owner));
    if !sso_ok {
        return collab;
    }
    if a.is_admin(user) {
        return Some(RepoPermission::Admin);
    }
    let member = org && a.role(user).is_some();
    let base = if member { a.base_permission() } else { None };
    let via_owner = a.grant(repo.repo(), user).max(a.team_permission(repo.repo(), user)).max(base);
    let internal = repo.visibility() == Visibility::Internal
        && (member || a.enterprise().is_some_and(|e| enterprise_member(authz, e, user)));
    via_owner.max(collab).max(internal.then_some(RepoPermission::Read))
}

fn enterprise_member(authz: &[&dyn AuthzRead], enterprise: AccountId, user: UserId) -> bool {
    authz.iter().any(|a| a.owner() == Some(enterprise) && a.role(user).is_some())
}

/// Whether the caller may do `a`: a public repository is readable by anyone, signed in or not.
pub fn authorize(p: &Principal, _a: ReadAction, authz: &[&dyn AuthzRead], repo: &dyn RepoAccess) -> Decision {
    if repo.visibility() == Visibility::Public || permission(p, authz, repo).is_some() {
        Decision::Allow
    } else {
        Decision::Hidden
    }
}

/// Never `Stale`: the folds are the ones the leader read through `fresh` for this entry.
pub fn authorize_write(p: &Principal, a: WriteAction, authz: &[&dyn AuthzRead], repo: &dyn RepoAccess) -> Decision {
    let perm = permission(p, authz, repo);
    if perm >= Some(a.needs()) {
        Decision::Allow
    } else if perm.is_some() || repo.visibility() == Visibility::Public {
        Decision::Forbidden
    } else {
        Decision::Hidden
    }
}
