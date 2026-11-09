//! `authorize` and `authorize_write` over an owner's `Authz` fold and a repository's access rows (M3-WP3).

mod common;

use common::acct;
use graftyard_identity::{
    AccountKind, Actor, AuthzEvent, AuthzFold, Credential, Decision, Principal, ReadAction, RepoAccess, RepoPermission,
    RepoScope, ScopeSet, TokenFields, UserId, Visibility, WriteAction, authorize, authorize_read, authorize_write,
};
use graftyard_raft::Fold;
use graftyard_types::{AccountId, DbId, RepoId};
use smallvec::{SmallVec, smallvec};

struct Repo {
    owner: AccountId,
    visibility: Visibility,
    collaborators: Vec<(UserId, RepoPermission)>,
}

const REPO: RepoId = RepoId(DbId(7));

impl RepoAccess for Repo {
    fn repo(&self) -> RepoId {
        REPO
    }
    fn owner(&self) -> AccountId {
        self.owner
    }
    fn visibility(&self) -> Visibility {
        self.visibility
    }
    fn collaborator(&self, user: UserId) -> Option<RepoPermission> {
        self.collaborators.iter().find(|(u, _)| *u == user).map(|(_, p)| *p)
    }
}

fn user(a: AccountId) -> Principal {
    Principal {
        actor: Actor::User(UserId(a)),
        scopes: ScopeSet(1),
        credential: Credential::Stored([1; 32]),
        sso_orgs: SmallVec::new(),
    }
}

#[test]
fn hidden_forbidden_and_the_higher_of_grant_and_collaborator() {
    let (org, member, outsider, joined) = (acct(0, 9, 1), acct(0, 2, 1), acct(0, 2, 2), acct(0, 2, 3));
    let mut a = AuthzFold::new(org);
    a.apply(&AuthzEvent::Opened { kind: AccountKind::Org, enterprise: None });
    a.apply(&AuthzEvent::Member { user: UserId(member), role: Some(graftyard_identity::OrgRole::Member) });
    a.apply(&AuthzEvent::Member { user: UserId(joined), role: Some(graftyard_identity::OrgRole::Member) });
    a.apply(&AuthzEvent::Grant { repo: REPO, grantee: UserId(member), permission: Some(RepoPermission::Write) });
    a.apply(&AuthzEvent::Grant { repo: REPO, grantee: UserId(joined), permission: Some(RepoPermission::Read) });
    let mut r = Repo {
        owner: org,
        visibility: Visibility::Private,
        collaborators: vec![(UserId(outsider), RepoPermission::Triage), (UserId(joined), RepoPermission::Maintain)],
    };
    let authz: [&dyn graftyard_identity::AuthzRead; 1] = [&a];
    let anon = Principal { actor: Actor::Anonymous, ..user(member) };
    assert_eq!(authorize(&anon, ReadAction::Contents, &authz, &r), Decision::Hidden);
    assert_eq!(authorize(&user(acct(0, 2, 9)), ReadAction::Contents, &authz, &r), Decision::Hidden);
    assert_eq!(authorize_write(&user(member), WriteAction::Push, &authz, &r), Decision::Allow);
    assert_eq!(authorize_write(&user(member), WriteAction::Administer, &authz, &r), Decision::Forbidden);
    assert_eq!(authorize_write(&user(outsider), WriteAction::Triage, &authz, &r), Decision::Allow);
    assert_eq!(authorize_write(&user(outsider), WriteAction::Push, &authz, &r), Decision::Forbidden);
    // A member who was a collaborator before joining keeps the higher of the two.
    assert_eq!(authorize_write(&user(joined), WriteAction::Maintain, &authz, &r), Decision::Allow);
    // A fold of another owner grants nothing.
    let other = AuthzFold::new(acct(0, 9, 2));
    let wrong: [&dyn graftyard_identity::AuthzRead; 1] = [&other];
    assert_eq!(authorize_write(&user(member), WriteAction::Push, &wrong, &r), Decision::Hidden);
    r.visibility = Visibility::Public;
    assert_eq!(authorize(&anon, ReadAction::Contents, &authz, &r), Decision::Allow);
    assert_eq!(authorize_write(&anon, WriteAction::Participate, &authz, &r), Decision::Forbidden);
}

/// The edge's read gate: a private repository is hidden from an anonymous caller, an outsider, a fine-grained token
/// for another repository, and an installation token that does not reach it; it is shown to the owner's admin, a
/// grantee, an outside collaborator, and an org member through the base permission. A public repository is shown to
/// everyone, a token outside its reach included, because the token's own limit only binds a write there.
#[test]
fn the_read_gate_hides_a_private_repository_from_everyone_who_cannot_read_it() {
    let (org, admin, grantee, collab, member, outsider) =
        (acct(0, 9, 1), acct(0, 2, 1), acct(0, 2, 2), acct(0, 2, 3), acct(0, 2, 4), acct(0, 2, 5));
    let mut a = AuthzFold::new(org);
    a.apply(&AuthzEvent::Opened { kind: AccountKind::Org, enterprise: None });
    a.apply(&AuthzEvent::Member { user: UserId(admin), role: Some(graftyard_identity::OrgRole::Admin) });
    a.apply(&AuthzEvent::Grant { repo: REPO, grantee: UserId(grantee), permission: Some(RepoPermission::Read) });
    let mut r = Repo {
        owner: org,
        visibility: Visibility::Private,
        collaborators: vec![(UserId(collab), RepoPermission::Read)],
    };
    let read = |a: &AuthzFold, p: Option<&Principal>, t: Option<&TokenFields>, r: &Repo| {
        authorize_read(p, t, &[a as &dyn graftyard_identity::AuthzRead], r)
    };
    let elsewhere = TokenFields { repos: Some(RepoScope::Selected(smallvec![RepoId(DbId(8))])), ..Default::default() };
    let here = TokenFields { repos: Some(RepoScope::Selected(smallvec![REPO])), ..Default::default() };

    assert_eq!(read(&a, None, None, &r), Decision::Hidden, "anonymous");
    assert_eq!(read(&a, Some(&user(outsider)), None, &r), Decision::Hidden, "an outsider");
    assert_eq!(read(&a, Some(&user(member)), None, &r), Decision::Hidden, "not yet a member");
    for (who, why) in [(admin, "the org's admin"), (grantee, "a grantee"), (collab, "an outside collaborator")] {
        assert_eq!(read(&a, Some(&user(who)), None, &r), Decision::Allow, "{why}");
    }
    assert_eq!(read(&a, Some(&user(grantee)), Some(&elsewhere), &r), Decision::Hidden, "a token for another repo");
    assert_eq!(read(&a, Some(&user(grantee)), Some(&here), &r), Decision::Allow, "a token for this repository");
    // A member reads only through a grant, a team, or the org's base permission, which this org has not set.
    a.apply(&AuthzEvent::Member { user: UserId(member), role: Some(graftyard_identity::OrgRole::Member) });
    assert_eq!(read(&a, Some(&user(member)), None, &r), Decision::Hidden, "a member with no base permission");
    // No `Authz` fold read: only a collaborator row grants, so nothing is shown by mistake.
    assert_eq!(authorize_read(Some(&user(admin)), None, &[], &r), Decision::Hidden, "no owner fold");
    assert_eq!(authorize_read(Some(&user(collab)), None, &[], &r), Decision::Allow, "a collaborator row");
    let other = AuthzFold::new(acct(0, 9, 2));
    assert_eq!(read(&other, Some(&user(admin)), None, &r), Decision::Hidden, "another owner's fold");

    r.visibility = Visibility::Internal;
    assert_eq!(read(&a, None, None, &r), Decision::Hidden, "internal is not public");
    r.visibility = Visibility::Public;
    assert_eq!(read(&a, None, None, &r), Decision::Allow, "public, anonymous");
    assert_eq!(read(&a, Some(&user(outsider)), Some(&elsewhere), &r), Decision::Allow, "public, any token");
}
