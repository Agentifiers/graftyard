//! M3-WP3's identity gates on real `graftyard_raft::Member`s (`build/BUILD-PLAN.md`, M3-WP3): org invitations and
//! teams (BF-34's `Authz` half), `InstanceAuth` (BF-36), and the account checks every write reads (BF-3). Every
//! dependency is read through `fresh` on its own group, as the leader's host does once per entry.

mod common;

use common::acct;
use common::world::{FreshLeader, OPS, World};
use graftyard_identity::{
    AccountKind, AuthzCmd, AuthzErr, AuthzFold, AuthzOp, AuthzOut, AuthzRead, IdBlock, InstanceAuth, InvitationId,
    Invitee, NameKey, NamesCmd, NamesOp, OidcConfig, OrgRole, SignInMethod, Url, UserId, account_shard,
};
use graftyard_raft::CommitError;
use graftyard_types::{AccountId, ChainId, CredentialShard, DbId, Digest, Timestamp};
use smallvec::smallvec;

fn refused<O: std::fmt::Debug, E: std::fmt::Debug>(r: Result<graftyard_raft::Acked<O>, CommitError<E>>) -> E {
    match r {
        Err(CommitError::Refused { err, .. }) => err,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

/// A user acting with no token row: commands that name only the account read just its `Account` row.
fn as_user(u: AccountId) -> CredentialShard {
    CredentialShard { account: Some(u), key: None }
}

fn account(w: &mut World, u: AccountId) {
    let op = NamesOp::PutAccount { id: u, suspended: false, sessions_not_before: Timestamp(0) };
    w.names_cmd(ChainId::Names(account_shard(u)), NamesCmd { cred: OPS, op }).unwrap();
}

/// An opened org with an id block, an admin, and every account's row.
fn org(w: &mut World, owner: AccountId, admin: AccountId) {
    w.authz_cmd(owner, AuthzCmd { cred: OPS, op: AuthzOp::Open { kind: AccountKind::Org, enterprise: None } }).unwrap();
    let g = w.authz(owner);
    let l = g.leader().unwrap();
    g.id_block(l, IdBlock::whole(0, 40).unwrap()).unwrap();
    g.settle(&mut |c| panic!("{c}"));
    w.authz_cmd(
        owner,
        AuthzCmd { cred: OPS, op: AuthzOp::SetMember { user: UserId(admin), role: Some(OrgRole::Admin) } },
    )
    .unwrap();
    account(w, admin);
}

fn invite(
    w: &mut World,
    owner: AccountId,
    admin: AccountId,
    invitee: Invitee,
    teams: &[graftyard_identity::TeamId],
) -> InvitationId {
    let op = AuthzOp::Invite { invitee, role: OrgRole::Member, teams: teams.iter().copied().collect() };
    match w.authz_cmd(owner, AuthzCmd { cred: as_user(admin), op }).unwrap().out {
        AuthzOut::Invitation(id) => id,
        o => panic!("{o:?}"),
    }
}

/// BF-34, the org half: accepting grants membership and the invitation's teams in the same entry as `Accepted`;
/// declining grants nothing; a lapsed, answered, or someone else's invitation cannot be accepted; an email invitation
/// is accepted only by the account holding the email on `Names`.
#[test]
fn an_org_invitation_accepted_grants_in_one_entry_and_declined_grants_nothing() {
    let mut w = World::default();
    let (owner, admin, alice, bob, carol) = (acct(0, 9, 1), acct(0, 2, 1), acct(0, 2, 2), acct(0, 2, 3), acct(0, 2, 4));
    org(&mut w, owner, admin);
    for u in [alice, bob, carol] {
        account(&mut w, u);
    }
    let team = match w
        .authz_cmd(owner, AuthzCmd { cred: as_user(admin), op: AuthzOp::CreateTeam { name: "core".into() } })
        .unwrap()
        .out
    {
        AuthzOut::Team(t) => t,
        o => panic!("{o:?}"),
    };
    // A non-member cannot join a team directly.
    let join = AuthzOp::SetTeamMember { team, user: UserId(alice), on: true };
    assert_eq!(refused(w.authz_cmd(owner, AuthzCmd { cred: as_user(admin), op: join })), AuthzErr::NotAMember);
    let ia = invite(&mut w, owner, admin, Invitee::User(UserId(alice)), &[team]);
    let ib = invite(&mut w, owner, admin, Invitee::User(UserId(bob)), &[team]);
    // Only the invitee accepts; an admin cannot accept for them.
    assert_eq!(
        refused(w.authz_cmd(owner, AuthzCmd { cred: as_user(bob), op: AuthzOp::Accept { id: ia } })),
        AuthzErr::Forbidden
    );
    assert_eq!(
        refused(w.authz_cmd(owner, AuthzCmd { cred: as_user(admin), op: AuthzOp::Accept { id: ia } })),
        AuthzErr::Forbidden
    );
    let acked = w.authz_cmd(owner, AuthzCmd { cred: as_user(alice), op: AuthzOp::Accept { id: ia } }).unwrap();
    // The acceptance, the membership, and the team membership are one entry.
    let g = w.authz(owner);
    let l = g.leader().unwrap();
    let evs = g.committed_events(l);
    let tail: Vec<_> = evs[evs.len() - 3..].to_vec();
    assert!(
        matches!(
            tail[0],
            graftyard_identity::AuthzEvent::Invitation(graftyard_identity::InvitationEvent::Accepted { .. })
        ),
        "{tail:?}"
    );
    assert!(matches!(tail[1], graftyard_identity::AuthzEvent::Member { role: Some(OrgRole::Member), .. }));
    assert!(matches!(
        tail[2],
        graftyard_identity::AuthzEvent::Team(graftyard_identity::TeamEvent::Member { on: true, .. })
    ));
    let entry = g.m(l).entry_at(acked.index).unwrap();
    assert!(matches!(&entry.body, graftyard_raft::EntryBody::Events(e) if e.len() == 3), "one entry holds all three");
    w.authz_cmd(owner, AuthzCmd { cred: as_user(bob), op: AuthzOp::Decline { id: ib } }).unwrap();
    let a: AuthzFold = w.authz(owner).fresh_leader();
    assert_eq!(a.role(UserId(alice)), Some(OrgRole::Member));
    assert!(a.org().teams[&team].members.contains(&UserId(alice)));
    assert_eq!(a.role(UserId(bob)), None, "declining grants nothing");
    assert!(!a.org().teams[&team].members.contains(&UserId(bob)));
    // Answered invitations are gone.
    for (u, id) in [(alice, ia), (bob, ib)] {
        assert_eq!(
            refused(w.authz_cmd(owner, AuthzCmd { cred: as_user(u), op: AuthzOp::Accept { id } })),
            AuthzErr::NoInvitation
        );
    }
    // An invitation lapses at 7 days with no command.
    let ic = invite(&mut w, owner, admin, Invitee::User(UserId(carol)), &[]);
    w.authz(owner).advance(graftyard_identity::INVITATION_TTL.as_millis() as u64 + 1);
    assert_eq!(
        refused(w.authz_cmd(owner, AuthzCmd { cred: as_user(carol), op: AuthzOp::Accept { id: ic } })),
        AuthzErr::NoInvitation
    );
    // A member is not invited again.
    let again = AuthzOp::Invite { invitee: Invitee::User(UserId(alice)), role: OrgRole::Member, teams: smallvec![] };
    assert_eq!(refused(w.authz_cmd(owner, AuthzCmd { cred: as_user(admin), op: again })), AuthzErr::AlreadyMember);
    // An email invitation: only the holder of the email on `Names` accepts.
    let email = Digest([5; 32]);
    let claim = NamesOp::ClaimEmail { email, account: carol };
    w.names_on(NameKey::Email(email), claim).unwrap();
    let ie = invite(&mut w, owner, admin, Invitee::Email(email), &[]);
    assert_eq!(
        refused(w.authz_cmd(owner, AuthzCmd { cred: as_user(bob), op: AuthzOp::Accept { id: ie } })),
        AuthzErr::Forbidden
    );
    w.authz_cmd(owner, AuthzCmd { cred: as_user(carol), op: AuthzOp::Accept { id: ie } }).unwrap();
    assert_eq!(w.authz(owner).fresh_leader().role(UserId(carol)), Some(OrgRole::Member));
}

/// The install's own `Authz` chain: the install account is cell block 0's, as the CLI's `install_account`.
fn install_account() -> AccountId {
    AccountId(DbId::pack(0, 0, 1).unwrap())
}

/// BF-36: a site admin switches `InstanceAuth` from Builtin to OIDC, and the next sign-in method is OIDC; a non-admin,
/// the operator, and any other owner's chain are refused.
#[test]
fn only_a_site_admin_writes_instance_auth_and_the_next_sign_in_follows_it() {
    let mut w = World::default();
    let install = install_account();
    let (admin, user) = (acct(0, 2, 1), acct(0, 2, 2));
    org(&mut w, install, admin);
    account(&mut w, user);
    w.authz_cmd(
        install,
        AuthzCmd { cred: OPS, op: AuthzOp::SetMember { user: UserId(user), role: Some(OrgRole::Member) } },
    )
    .unwrap();
    let before: AuthzFold = w.authz(install).fresh_leader();
    assert_eq!(before.instance_auth(), InstanceAuth::Builtin);
    assert!(before.instance_auth().admits(SignInMethod::Password));
    let oidc = InstanceAuth::Oidc(OidcConfig {
        issuer: Url("https://idp.example/".into()),
        client: "graftyard".into(),
        secret: vec![1, 2, 3].into(),
    });
    let set = |cred| AuthzCmd { cred, op: AuthzOp::SetInstanceAuth(oidc.clone()) };
    assert_eq!(refused(w.authz_cmd(install, set(as_user(user)))), AuthzErr::Forbidden, "a member, not an admin");
    assert_eq!(refused(w.authz_cmd(install, set(OPS))), AuthzErr::Forbidden, "the operator is not a site admin");
    w.authz_cmd(install, set(as_user(admin))).unwrap();
    let after: AuthzFold = w.authz(install).fresh_leader();
    assert_eq!(after.instance_auth().method(), SignInMethod::Oidc);
    assert!(!after.instance_auth().admits(SignInMethod::Password), "a password sign-in is refused");
    assert!(after.instance_auth().admits(SignInMethod::Link), "the bootstrap link still works");
    // Another owner's chain never holds it, even from its own admin.
    let other = acct(0, 9, 7);
    org(&mut w, other, admin);
    assert_eq!(refused(w.authz_cmd(other, set(as_user(admin)))), AuthzErr::NotInstall);
}

/// The `Account` row is read on every write: a web session issued before `sessions_not_before` is refused (sign out
/// everywhere), and a suspended account is refused whatever its credential, on the next entry after the change.
#[test]
fn every_write_reads_the_account_row_for_suspension_and_sign_out_everywhere() {
    use graftyard_identity::{Credential, CredentialRefusal, Principal, ScopeSet, WebSessionId};
    use graftyard_types::Ulid;
    let mut w = World::default();
    let (owner, admin) = (acct(0, 9, 1), acct(0, 2, 1));
    org(&mut w, owner, admin);
    let session = |at: u64| Principal {
        actor: graftyard_identity::Actor::User(UserId(admin)),
        scopes: ScopeSet(0),
        credential: Credential::Session(WebSessionId(Ulid::new(Timestamp(at), [3; 10]))),
        sso_orgs: Default::default(),
    };
    let team = |p: &Principal| AuthzCmd { cred: p.credential_shard(), op: AuthzOp::CreateTeam { name: "t".into() } };
    w.authz_cmd(owner, team(&session(5_000))).unwrap();
    // Sign out everywhere: every session issued before the new time is refused, a later one passes.
    let put = |suspended, nb| NamesOp::PutAccount { id: admin, suspended, sessions_not_before: Timestamp(nb) };
    w.names_cmd(ChainId::Names(account_shard(admin)), NamesCmd { cred: OPS, op: put(false, 6_000) }).unwrap();
    let r = refused(w.authz_cmd(owner, team(&session(5_000))));
    assert_eq!(r, AuthzErr::Unauthorized(CredentialRefusal::SignedOut));
    w.authz_cmd(owner, team(&session(6_000))).unwrap();
    // Suspension refuses the newer session too.
    w.names_cmd(ChainId::Names(account_shard(admin)), NamesCmd { cred: OPS, op: put(true, 6_000) }).unwrap();
    let r = refused(w.authz_cmd(owner, team(&session(7_000))));
    assert_eq!(r, AuthzErr::Unauthorized(CredentialRefusal::Suspended(admin)));
}

// ---- the `Identity` chain ----

use graftyard_identity::finish::{self, Step};
use graftyard_identity::{
    AccountCapture, CredentialRef, Fingerprint, IdentityCmd, IdentityErr, IdentityFold, IdentityOp, Login, ScopeSet,
    TokenKind, TokensCmd, TokensOp, WebSessionId, fingerprint_shard,
};

fn created(w: &mut World, u: AccountId, l: &str) {
    let c = AccountCapture { login: Login::parse(l).unwrap(), kind: AccountKind::User, created: Timestamp(1) };
    w.identity_cmd(u, IdentityCmd { cred: OPS, op: IdentityOp::Create(c) }).unwrap();
}

/// What the edge does for a new credential: commit it on `Tokens`, the commit point, then append the `Identity` side
/// right away. `kill` drops the edge after the first commit, as an edge that dies between them.
fn new_credential(w: &mut World, u: AccountId, c: CredentialRef, kill: bool) {
    let op = match c {
        CredentialRef::Token { hash, kind } => {
            TokensOp::Store { hash, kind, account: u, scopes: ScopeSet(1), expires: None }
        }
        CredentialRef::SshKey(fingerprint) => TokensOp::AddSshKey { fingerprint, account: u, repo: None },
    };
    w.tokens_cmd(TokensCmd { cred: as_user(u), op }).unwrap();
    if !kill {
        let add = IdentityOp::AddCredential { credential: c, title: "laptop".into() };
        w.identity_cmd(u, IdentityCmd { cred: as_user(u), op: add }).unwrap();
    }
}

fn listed(w: &mut World, u: AccountId) -> Vec<CredentialRef> {
    let f: IdentityFold = w.identity(u).fresh_leader();
    f.credentials.keys().copied().collect()
}

/// BF-35: a new SSH key and PAT are listed at once, with no `Audit` consumer, 100 of 100. The listing reads the
/// `Identity` fold through `fresh`, right after the edge's two commits.
#[test]
fn a_new_ssh_key_and_pat_are_listed_at_once_without_the_audit_consumer() {
    let mut w = World::default();
    let u = acct(0, 2, 1);
    account(&mut w, u);
    created(&mut w, u, "alice");
    let mut seen = 0;
    for i in 0..100u32 {
        let mut h = [0u8; 32];
        h[..4].copy_from_slice(&i.to_be_bytes());
        h[4] = 0xaa;
        let pat = CredentialRef::Token { hash: h, kind: TokenKind::Pat };
        let mut f = [0u8; 32];
        f[..4].copy_from_slice(&i.to_be_bytes());
        f[4] = 0xbb;
        let key = CredentialRef::SshKey(Fingerprint(f));
        new_credential(&mut w, u, key, false);
        new_credential(&mut w, u, pat, false);
        let l = listed(&mut w, u);
        if l.contains(&key) && l.contains(&pat) {
            seen += 1;
        }
    }
    assert_eq!(seen, 100, "listed at once");
}

/// The `Identity` side is refused for a credential that did not commit on `Tokens`, or belongs to someone else; an
/// edge that died between the commits leaves it to the `Audit` finisher, which lists it and later drops it once
/// revoked. A live credential is never dropped.
#[test]
fn audit_finishes_a_credential_nobody_listed_and_only_a_committed_one_is_listed() {
    let mut w = World::default();
    let (u, v) = (acct(0, 2, 1), acct(0, 2, 2));
    for (a, l) in [(u, "alice"), (v, "bob")] {
        account(&mut w, a);
        created(&mut w, a, l);
    }
    let ghost = CredentialRef::Token { hash: [9; 32], kind: TokenKind::Pat };
    let add = |c| IdentityCmd { cred: as_user(u), op: IdentityOp::AddCredential { credential: c, title: "x".into() } };
    assert_eq!(refused(w.identity_cmd(u, add(ghost))), IdentityErr::NotCommitted, "no Tokens row");
    let bobs = CredentialRef::SshKey(Fingerprint([4; 32]));
    new_credential(&mut w, v, bobs, false);
    assert_eq!(refused(w.identity_cmd(u, add(bobs))), IdentityErr::NotCommitted, "another account's key");
    // The edge dies after the Tokens commit: nothing lists the PAT until the finisher runs.
    let pat = CredentialRef::Token { hash: [7; 32], kind: TokenKind::Pat };
    new_credential(&mut w, u, pat, true);
    assert!(!listed(&mut w, u).contains(&pat));
    let finish = |w: &mut World| {
        let tokens = w.tokens(pat.shard()).fresh_leader();
        let id = w.identity(u).fresh_leader();
        finish::credential(u, pat, "Pat", &tokens, &id)
    };
    let step = finish(&mut w).expect("owed");
    assert!(matches!(step, Step::Identity(..)));
    w.step(step);
    assert!(listed(&mut w, u).contains(&pat));
    assert_eq!(finish(&mut w), None, "nothing owed twice");
    // A live credential is never dropped; once revoked, the finisher drops it.
    let drop = IdentityCmd { cred: OPS, op: IdentityOp::RemoveCredential { credential: pat } };
    assert_eq!(refused(w.identity_cmd(u, drop.clone())), IdentityErr::Forbidden);
    w.tokens_cmd(TokensCmd { cred: as_user(u), op: TokensOp::Revoke { hash: [7; 32] } }).unwrap();
    let step = finish(&mut w).expect("owed");
    w.step(step);
    assert!(!listed(&mut w, u).contains(&pat));
    assert_eq!(fingerprint_shard(&Fingerprint([4; 32])), bobs.shard());
}

/// BF-36's sign-in half: the next sign-in after the switch uses OIDC, a password sign-in is refused, the bootstrap
/// link still works, and a suspended account starts no session.
#[test]
fn a_sign_in_takes_only_the_method_instance_auth_names() {
    use graftyard_types::Ulid;
    let mut w = World::default();
    let install = install_account();
    let admin = acct(0, 2, 1);
    org(&mut w, install, admin);
    created(&mut w, admin, "admin");
    let sign_in = |n: u8, method| IdentityCmd {
        cred: OPS,
        op: IdentityOp::SignIn { id: WebSessionId(Ulid::new(Timestamp(9_000), [n; 10])), method, install },
    };
    w.identity_cmd(admin, sign_in(1, SignInMethod::Password)).unwrap();
    let oidc = InstanceAuth::Oidc(OidcConfig {
        issuer: Url("https://idp/".into()),
        client: "g".into(),
        secret: vec![1].into(),
    });
    w.authz_cmd(install, AuthzCmd { cred: as_user(admin), op: AuthzOp::SetInstanceAuth(oidc) }).unwrap();
    assert_eq!(refused(w.identity_cmd(admin, sign_in(2, SignInMethod::Password))), IdentityErr::WrongMethod);
    w.identity_cmd(admin, sign_in(3, SignInMethod::Oidc)).unwrap();
    w.identity_cmd(admin, sign_in(4, SignInMethod::Link)).unwrap();
    let f: IdentityFold = w.identity(admin).fresh_leader();
    assert_eq!(f.sessions.len(), 3);
    // A sign-in naming another owner's chain as the install's is refused.
    let other = IdentityCmd {
        cred: OPS,
        op: IdentityOp::SignIn {
            id: WebSessionId(Ulid::new(Timestamp(9_000), [5; 10])),
            method: SignInMethod::Oidc,
            install: acct(0, 9, 1),
        },
    };
    assert_eq!(refused(w.identity_cmd(admin, other)), IdentityErr::NotInstall);
    let put = NamesOp::PutAccount { id: admin, suspended: true, sessions_not_before: Timestamp(0) };
    w.names_cmd(ChainId::Names(account_shard(admin)), NamesCmd { cred: OPS, op: put }).unwrap();
    assert_eq!(refused(w.identity_cmd(admin, sign_in(6, SignInMethod::Oidc))), IdentityErr::Suspended);
}
