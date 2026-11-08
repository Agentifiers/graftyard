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

/// BF-3's key half: one SSH key added from two edges at once gets one owner, whether both commands land in one entry
/// or in two, 100 keys each way; a follower's check refuses a second owner's row.
#[test]
fn one_ssh_key_added_from_two_edges_at_once_gets_one_owner() {
    use graftyard_identity::{TokensErr, TokensEvent, TokensRead};
    use graftyard_raft::{Fold, Scratch};
    let mut w = World::default();
    let (u, v) = (acct(0, 2, 1), acct(0, 2, 2));
    account(&mut w, u);
    account(&mut w, v);
    let mut owners = 0;
    for i in 0..200u32 {
        let mut f = [0u8; 32];
        f[..4].copy_from_slice(&i.to_be_bytes());
        f[4] = 0xcc;
        let fp = Fingerprint(f);
        let add =
            |a| TokensCmd { cred: as_user(a), op: TokensOp::AddSshKey { fingerprint: fp, account: a, repo: None } };
        let shard = fingerprint_shard(&fp);
        w.tokens(shard);
        let mut g = w.tokens.remove(&shard.0).unwrap();
        let l = g.leader().unwrap();
        let (x, y) = (w.id(), w.id());
        let a = g.commit(l, x, add(u)).unwrap();
        // Odd keys: the first edge's command commits before the second arrives, so they land in two entries.
        if i % 2 == 1 {
            g.settle(&mut |c| w.read(c));
        }
        let b = g.commit(l, y, add(v)).unwrap();
        g.settle(&mut |c| w.read(c));
        let (ra, rb) = (g.result(a.ticket).unwrap().clone(), g.result(b.ticket).unwrap().clone());
        assert!(ra.is_ok(), "{ra:?}");
        assert_eq!(refused(rb), TokensErr::KeyTaken { by: u });
        let fold = g.fresh_leader();
        assert_eq!(fold.ssh_key(&fp).map(|r| r.account), Some(u));
        let second = TokensEvent::SshKey { fingerprint: fp, account: v, repo: None };
        assert!(fold.scratch().check(&second).is_err(), "a follower refuses a second owner");
        for s in common::SITES {
            let n = g
                .committed_events(s)
                .iter()
                .filter(|e| matches!(e, TokensEvent::SshKey { fingerprint, .. } if *fingerprint == fp))
                .count();
            assert_eq!(n, 1, "one row on {s:?}");
        }
        w.tokens.insert(shard.0, g);
        owners += 1;
    }
    assert_eq!(owners, 200);
}

// ---- BF-30: transfer and enterprise change ----

use graftyard_identity::finish::{Transfer, TransferStep, TransferView};
use graftyard_identity::{NamesRead, RepoName};
use graftyard_types::{PlacementName, PlacementRecord, SiteId};

fn placed(w: &mut World, owner: AccountId, sites: [u8; 3]) -> PlacementRecord {
    let record = PlacementRecord {
        name: PlacementName::Tri,
        sites: sites.iter().map(|s| SiteId(*s)).collect(),
        home: SiteId(sites[0]),
        generation: 1,
        moving_from: None,
        at: Timestamp(1),
    };
    let op = NamesOp::InitPlacement { owner, record: record.clone() };
    w.names_cmd(ChainId::Names(account_shard(owner)), NamesCmd { cred: OPS, op }).unwrap();
    record
}

fn placement(w: &mut World, owner: AccountId) -> PlacementRecord {
    w.names(ChainId::Names(account_shard(owner))).fresh_leader().placement(owner).cloned().expect("a record")
}

/// BF-30: a repository moves from org `alpha` (enterprise E1) to org `beta` (enterprise E2), on another placement or
/// the same one, by the finisher's plan with every step submitted twice, as a racing edge and the finisher would.
/// After every step, every route that resolves (the old name, the new name, and `/repositories/{id}` through
/// `RepoOwner`) names a record the repository's group is in or was in, whose members answer with the group's config.
/// The repository chain's `Settings` is modelled here by its owner and enterprise; `crates/git/tests/issues.rs` checks
/// that its `deps` follow them.
#[test]
fn a_transfer_keeps_every_route_on_the_group_and_names_the_new_enterprise() {
    for other_place in [true, false] {
        let mut w = World::default();
        let (alpha, beta, admin) = (acct(0, 9, 1), acct(0, 9, 2), acct(0, 2, 1));
        let (e1, e2, e3) = (acct(0, 9, 5), acct(0, 9, 6), acct(0, 9, 7));
        org(&mut w, alpha, admin);
        org(&mut w, beta, admin);
        let p_alpha = placed(&mut w, alpha, [0, 1, 2]);
        let p_beta = placed(&mut w, beta, if other_place { [3, 4, 5] } else { [0, 1, 2] });
        for (o, e) in [(alpha, e1), (beta, e2)] {
            w.authz_cmd(o, AuthzCmd { cred: OPS, op: AuthzOp::SetEnterprise { enterprise: Some(e) } }).unwrap();
        }
        let name = RepoName::parse("tool").unwrap();
        let create = AuthzCmd { cred: as_user(admin), op: AuthzOp::CreateRepo { name: name.clone() } };
        let AuthzOut::Repo { repo, .. } = w.authz_cmd(alpha, create).unwrap().out else { panic!() };
        w.names_on(NameKey::Repo(repo), NamesOp::RepoOwner { repo, owner: alpha }).unwrap();
        // The repository chain: its `Settings` owner and enterprise, and the records its group has been in.
        let (mut owner, mut enterprise) = (alpha, Some(e1));
        let mut group = vec![p_alpha.clone()];
        // The commit point. A user who does not administer the source owner is refused.
        let outsider = acct(0, 2, 9);
        account(&mut w, outsider);
        w.authz_cmd(
            beta,
            AuthzCmd { cred: OPS, op: AuthzOp::SetMember { user: UserId(outsider), role: Some(OrgRole::Admin) } },
        )
        .unwrap();
        let reserve =
            |a| AuthzCmd { cred: as_user(a), op: AuthzOp::ReserveTransfer { name: name.clone(), repo, from: alpha } };
        assert_eq!(refused(w.authz_cmd(beta, reserve(outsider))), AuthzErr::Forbidden);
        w.authz_cmd(beta, reserve(admin)).unwrap();
        let t = Transfer { repo, from: alpha, from_key: name.key(), to: beta, to_key: name.key() };
        let routes = |w: &mut World, group: &[PlacementRecord]| -> usize {
            let (fa, fb): (AuthzFold, AuthzFold) = (w.authz(alpha).fresh_leader(), w.authz(beta).fresh_leader());
            let ro = w.names_fold(NameKey::Repo(repo)).repo_owner(repo);
            let mut n = 0;
            for (o, f, key) in [(alpha, &fa, &t.from_key), (beta, &fb, &t.to_key)] {
                if f.resolve(key).map(|(r, _)| r) == Some(repo) {
                    let rec = placement(w, o);
                    assert!(group.contains(&rec), "the name on {o:?} routes to {rec:?}, outside {group:?}");
                    n += 1;
                }
            }
            let rec = placement(w, ro.expect("a RepoOwner row"));
            assert!(group.contains(&rec), "/repositories/{{id}} routes to {rec:?}, outside {group:?}");
            n
        };
        let mut steps = Vec::new();
        for _ in 0..12 {
            assert!(routes(&mut w, &group) >= 1, "some name resolves throughout");
            let (fa, fb): (AuthzFold, AuthzFold) = (w.authz(alpha).fresh_leader(), w.authz(beta).fresh_leader());
            let ro = w.names_fold(NameKey::Repo(repo)).repo_owner(repo);
            let v = TransferView {
                from_authz: &fa,
                to_authz: &fb,
                from_placement: &p_alpha,
                to_placement: &p_beta,
                group_at: group.last().unwrap(),
                settings_owner: owner,
                repo_owner: ro,
            };
            let Some(step) = graftyard_identity::finish::transfer(&t, &v) else { break };
            steps.push(std::mem::discriminant(&step));
            match step {
                TransferStep::Move { to } => group.push(to),
                TransferStep::Settings { owner: o, enterprise: e } => (owner, enterprise) = (o, e),
                TransferStep::Step(s) => {
                    w.step(s.clone());
                    w.step(s);
                }
            }
        }
        assert_eq!(steps.len(), if other_place { 5 } else { 4 }, "move, Settings, Live, Alias, RepoOwner");
        assert_eq!(routes(&mut w, &group), 2, "both names resolve at the end");
        let (fa, fb): (AuthzFold, AuthzFold) = (w.authz(alpha).fresh_leader(), w.authz(beta).fresh_leader());
        assert_eq!(fa.resolve(&t.from_key), Some((repo, false)), "the old name redirects");
        assert_eq!(fb.resolve(&t.to_key), Some((repo, true)));
        assert_eq!((owner, enterprise), (beta, Some(e2)), "Settings names the new owner and its enterprise");
        // An org's enterprise change: only the operator submits it; its repositories then get `Settings`.
        let change = |cred| AuthzCmd { cred, op: AuthzOp::SetEnterprise { enterprise: Some(e3) } };
        assert_eq!(refused(w.authz_cmd(beta, change(as_user(admin)))), AuthzErr::Forbidden);
        w.authz_cmd(beta, change(OPS)).unwrap();
        let fb: AuthzFold = w.authz(beta).fresh_leader();
        assert_eq!(fb.enterprise(), Some(e3));
    }
}

// ---- the edge's writes: signup, sign-in, a new credential, and rename ----

/// What the edge sends, step by step, on real groups: a signup's claim then its `AccountLogin`, `Account`, and
/// `Created`; a sign-in; a new PAT's `Tokens` row then its `Identity` side; a rename's claim, release, and new
/// `AccountLogin`. The login the edge renders for the account id is read from `account_shard` alone, at every point
/// after the signup's steps, with no `LOGINS` table. Each step re-sent is a no-op, as a racing finisher's would be.
#[test]
fn the_edges_writes_keep_the_account_login_and_identity_in_step() {
    use graftyard_identity::{NamesOut, edge};
    use graftyard_types::Ulid;
    let mut w = World::default();
    let install = install_account();
    let admin = acct(0, 2, 1);
    org(&mut w, install, admin);
    let mona = Login::parse("Mona-Lisa").unwrap();
    let claim = NamesOp::ClaimLogin { login: mona.clone(), kind: AccountKind::User, account: None };
    let NamesOut::Account(a) = w.names_on(NameKey::Login(mona.key()), claim).unwrap().out else { panic!() };
    let rendered = |w: &mut World| w.names(ChainId::Names(account_shard(a))).fresh_leader().account_login(a).cloned();
    assert_eq!(rendered(&mut w), None, "before the edge's follow-ups only the finisher could render it");
    let default = common::record();
    for s in edge::after_signup(a, &mona, AccountKind::User, Timestamp(1), &default) {
        w.step(s.clone());
        // A re-sent `InitPlacement` is refused `RecordExists`, which is done; every other step is a no-op.
        if let graftyard_identity::finish::Step::Names(
            chain,
            cmd @ NamesCmd { op: NamesOp::InitPlacement { .. }, .. },
        ) = s
        {
            assert_eq!(refused(w.names_cmd(chain, cmd)), graftyard_identity::NamesErr::RecordExists);
            continue;
        }
        w.step(s);
    }
    assert_eq!(rendered(&mut w), Some(mona.clone()));
    // The new owner's chains have a record, the install account's, before anything writes to them.
    let placed = w.names(ChainId::Names(account_shard(a))).fresh_leader().placement(a).cloned();
    assert_eq!(placed, Some(default.clone()), "a new owner copies the install account's record");
    let session = WebSessionId(Ulid::new(Timestamp(9_000), [1; 10]));
    w.step(edge::sign_in(a, session, SignInMethod::Password, install));
    let f: IdentityFold = w.identity(a).fresh_leader();
    assert!(f.sessions.contains_key(&session));
    // A PAT: the `Tokens` row, then the `Identity` side right after it.
    let pat = CredentialRef::Token { hash: [0x51; 32], kind: TokenKind::Pat };
    let store =
        TokensOp::Store { hash: [0x51; 32], kind: TokenKind::Pat, account: a, scopes: ScopeSet(1), expires: None };
    w.tokens_cmd(TokensCmd { cred: as_user(a), op: store }).unwrap();
    w.step(edge::after_credential(a, as_user(a), pat, "laptop"));
    assert_eq!(listed(&mut w, a), [pat]);
    // Rename to "mona": every step, then the rendered login follows.
    let new = Login::parse("mona").unwrap();
    for s in edge::rename(a, as_user(a), &mona, &new) {
        w.step(s);
    }
    assert_eq!(rendered(&mut w), Some(new.clone()));
    // The rename's last step replayed late with the old login is refused: the old login is a former name.
    let late = NamesCmd { cred: as_user(a), op: NamesOp::SetAccountLogin { account: a, login: mona } };
    assert_eq!(refused(w.names_cmd(ChainId::Names(account_shard(a)), late)), graftyard_identity::NamesErr::NotHolder);
    assert_eq!(rendered(&mut w), Some(new));
}

/// An OpenSSH `ssh-ed25519` public key line for a 32-byte key.
fn ed25519_line(k: [u8; 32]) -> String {
    use base64::Engine as _;
    let mut blob = Vec::new();
    for part in [&b"ssh-ed25519"[..], &k[..]] {
        blob.extend_from_slice(&(part.len() as u32).to_be_bytes());
        blob.extend_from_slice(part);
    }
    format!("ssh-ed25519 {}", base64::engine::general_purpose::STANDARD.encode(blob))
}

/// BF-35's key text (format 0x14.2): `AddKey` lists a committed key with its text at once, refuses text whose blob
/// does not hash to the fingerprint and a key with no live `Tokens` row, and a fold with no key text keeps 0x14.1's
/// bytes.
#[test]
fn add_key_lists_the_key_text_only_for_its_own_committed_fingerprint() {
    use graftyard_raft::codec;
    let mut w = World::default();
    let u = acct(0, 2, 1);
    account(&mut w, u);
    created(&mut w, u, "alice");
    let before = codec::to_bytes(&w.identity(u).fresh_leader());
    let plain = IdentityFold { more: Default::default(), ..codec::from_bytes::<IdentityFold>(&before).unwrap() };
    assert_eq!(codec::to_bytes(&plain), before, "no key text: 0x14.1's bytes");
    let (line, other) = (ed25519_line([5; 32]), ed25519_line([6; 32]));
    let fp = graftyard_identity::key_fingerprint(&line).expect("an ed25519 key");
    let add = |key: &str| IdentityCmd {
        cred: as_user(u),
        op: IdentityOp::AddKey { fingerprint: fp, key: key.into(), title: "laptop".into() },
    };
    assert_eq!(refused(w.identity_cmd(u, add(&line))), IdentityErr::NotCommitted, "no Tokens row yet");
    let op = TokensOp::AddSshKey { fingerprint: fp, account: u, repo: None };
    w.tokens_cmd(TokensCmd { cred: as_user(u), op }).unwrap();
    assert_eq!(refused(w.identity_cmd(u, add(&other))), IdentityErr::KeyMismatch, "another key's text");
    assert_eq!(refused(w.identity_cmd(u, add("ssh-ed25519 !!"))), IdentityErr::KeyMismatch, "not a key");
    w.identity_cmd(u, add(&line)).unwrap();
    w.identity_cmd(u, add(&line)).unwrap();
    let f: IdentityFold = w.identity(u).fresh_leader();
    assert!(f.credentials.contains_key(&CredentialRef::SshKey(fp)));
    assert_eq!(f.more.key_text.get(&fp).map(AsRef::as_ref), Some(line.as_str()));
    assert_eq!(codec::from_bytes::<IdentityFold>(&codec::to_bytes(&f)).unwrap(), f);
    assert_eq!(graftyard_raft::Command::format(&add(&line)), graftyard_identity::IDENTITY_FORMAT_V2);
}
