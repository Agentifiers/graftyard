//! `authorize` and `authorize_write` over an owner's `Authz` fold and a repository's access rows (M3-WP3).

mod common;

use common::acct;
use graftyard_identity::{
    AccountKind, Actor, AuthzEvent, AuthzFold, Credential, Decision, Principal, ReadAction, RepoAccess, RepoPermission,
    ScopeSet, UserId, Visibility, WriteAction, authorize, authorize_write,
};
use graftyard_raft::Fold;
use graftyard_types::{AccountId, DbId, RepoId};
use smallvec::SmallVec;

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
