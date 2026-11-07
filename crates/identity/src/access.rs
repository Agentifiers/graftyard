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

use crate::apps::Access;
use crate::authz::{AccountKind, AuthzRead, RepoPermission};
use crate::credential::{Actor, Credential, OrgId, Principal, UserId};
use crate::stateless::InstallationClaims;
use crate::tokens::RepoScope;

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
    if let (Actor::App { .. }, Credential::Stateless(c)) = (p.actor, &p.credential) {
        return app_permission(c, authz, repo);
    }
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

/// An installation token's permission on the repository: SHARED's "`authorize` intersects the claims with the
/// installation's current selection in the `Authz` fold". The installation must still be on the repository's owner,
/// for the token's App, with the repository in both the claims and the current row, and the token minted no earlier
/// than `not_before`. `contents` gives `Read` or `Write`, and any other permission `Read` [inference: GitHub's
/// `metadata` read is implied by every installation]. The lower of the claims' and the row's level wins.
pub fn app_permission(
    c: &InstallationClaims,
    authz: &[&dyn AuthzRead],
    repo: &dyn RepoAccess,
) -> Option<RepoPermission> {
    let row = app_row(c, authz, repo)?;
    let level = |p: &crate::apps::PermissionSet| match p.0.get("contents") {
        Some(Access::Write | Access::Admin) => Some(RepoPermission::Write),
        Some(Access::Read) => Some(RepoPermission::Read),
        None if !p.0.is_empty() => Some(RepoPermission::Read),
        None => None,
    };
    level(&c.permissions).min(level(&row.permissions))
}

/// The installation's current row when the token reaches the repository: live on the owner, for the token's App,
/// minted at or after `not_before`, with the repository in both the claims and the row.
fn app_row<'a>(
    c: &InstallationClaims,
    authz: &[&'a dyn AuthzRead],
    repo: &dyn RepoAccess,
) -> Option<&'a crate::authz::InstallationRow> {
    let a = authz.iter().find(|a| a.owner() == Some(repo.owner()))?;
    let state = a.installation(c.installation)?;
    let row = state.row.as_ref()?;
    if row.app != c.app || c.iat < state.not_before {
        return None;
    }
    let reaches = |s: &RepoScope| match s {
        RepoScope::All => true,
        RepoScope::Selected(r) => r.contains(&repo.repo()),
    };
    (reaches(&row.repos) && reaches(&c.repos)).then_some(row)
}

/// An installation token's write by GitHub's per-resource permissions: `Allow` when the token reaches the repository
/// and, for any one of `needs`, both the claims and the installation's row grant at least that level. A token that
/// reaches the repository but lacks the permission is `Forbidden`, GitHub's 403 "Resource not accessible by
/// integration" (`https://docs.github.com/en/enterprise-server@3.22/rest/using-the-rest-api/troubleshooting-the-rest-api`,
/// "Resource not accessible"). One that does not reach a private repository is `Hidden`, 404. Empty `needs` is never
/// allowed.
pub fn authorize_app(
    c: &InstallationClaims,
    needs: &[(&str, Access)],
    authz: &[&dyn AuthzRead],
    repo: &dyn RepoAccess,
) -> Decision {
    let Some(row) = app_row(c, authz, repo) else {
        return if repo.visibility() == Visibility::Public { Decision::Forbidden } else { Decision::Hidden };
    };
    let granted = |name: &str| c.permissions.0.get(name).min(row.permissions.0.get(name)).copied();
    if needs.iter().any(|(name, level)| granted(name) >= Some(*level)) { Decision::Allow } else { Decision::Forbidden }
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
