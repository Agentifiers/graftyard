//! AM1-WP2's identity rows on real `graftyard_raft::Member`s: `AgentApp` rows on `Authz(Genesis.account)` written only
//! by a site admin, and an org's `agents_enabled` and `agents_workspace`, which a later `SetPolicy` keeps.

mod common;

use common::world::{FreshLeader, OPS, World};
use common::{acct, record};
use graftyard_identity::glob::RefPattern;
use graftyard_identity::{
    AUTHZ_FORMAT_V2, AUTHZ_FORMAT_V3, AccountKind, AgentApp, AgentKind, AppId, AuthzCmd, AuthzErr, AuthzEvent,
    AuthzFold, AuthzOp, AuthzRead, IdBlock, NamesCmd, NamesOp, OrgPolicy, OrgRole, RepoPermission, UserId,
    WorkspaceMode, account_shard, install_account,
};
use graftyard_raft::{CommitError, Record, codec};
use graftyard_types::{AccountId, ChainId, CredentialShard, DbId, Timestamp};

fn refused<O: std::fmt::Debug, E: std::fmt::Debug>(r: Result<graftyard_raft::Acked<O>, CommitError<E>>) -> E {
    match r {
        Err(CommitError::Refused { err, .. }) => err,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

fn as_user(u: AccountId) -> CredentialShard {
    CredentialShard { account: Some(u), key: None }
}

/// An opened org with an id block and an admin whose `Account` row exists.
fn org(w: &mut World, owner: AccountId, admin: AccountId) {
    w.authz_cmd(owner, AuthzCmd { cred: OPS, op: AuthzOp::Open { kind: AccountKind::Org, enterprise: None } }).unwrap();
    let g = w.authz(owner);
    let l = g.leader().unwrap();
    g.id_block(l, IdBlock::whole(0, 40).unwrap()).unwrap();
    g.settle(&mut |c| panic!("{c}"));
    let op = AuthzOp::SetMember { user: UserId(admin), role: Some(OrgRole::Admin) };
    w.authz_cmd(owner, AuthzCmd { cred: OPS, op }).unwrap();
    let op = NamesOp::PutAccount { id: admin, suspended: false, sessions_not_before: Timestamp(0) };
    w.names_cmd(ChainId::Names(account_shard(admin)), NamesCmd { cred: OPS, op }).unwrap();
}

fn claude() -> AgentApp {
    AgentApp {
        app: AppId(DbId(3)),
        kind: AgentKind::ThirdParty,
        confine: Some(RefPattern::parse("refs/heads/claude/**").unwrap()),
    }
}

/// `PUT /agents/apps/{id}` and `DELETE`: only a site admin, only on the install's own chain, a re-send is a no-op,
/// and a `GraftyardRun` or `Mcp` row needs an App the install owns.
#[test]
fn agent_app_rows_live_only_on_the_install_chain_and_only_a_site_admin_writes_them() {
    let mut w = World::default();
    let install = install_account();
    let (admin, other_admin) = (acct(0, 2, 1), acct(0, 2, 2));
    org(&mut w, install, admin);
    let elsewhere = acct(0, 9, 7);
    org(&mut w, elsewhere, other_admin);
    let put = |cred, a: AgentApp| AuthzCmd { cred, op: AuthzOp::PutAgentApp(a) };
    assert_eq!(graftyard_raft::Command::format(&put(OPS, claude())), AUTHZ_FORMAT_V3);
    assert_eq!(
        refused(w.authz_cmd(install, put(as_user(other_admin), claude()))),
        AuthzErr::Forbidden,
        "not an admin of the install's org"
    );
    let a = w.authz_cmd(install, put(as_user(admin), claude())).unwrap();
    let f: AuthzFold = w.authz(install).fresh_leader();
    assert_eq!(f.agent_app(AppId(DbId(3))), Some(&claude()));
    // A re-send writes nothing.
    let again = w.authz_cmd(install, put(as_user(admin), claude())).unwrap();
    assert!(again.index > a.index);
    let evs = w.authz(install).committed_events(common::A);
    assert_eq!(evs.iter().filter(|e| matches!(e, AuthzEvent::AgentApp { .. })).count(), 1);
    // A Graftyard-run row for an App whose record is not on the install's chain.
    let run = AgentApp { app: AppId(DbId(9)), kind: AgentKind::GraftyardRun, confine: None };
    assert_eq!(refused(w.authz_cmd(install, put(as_user(admin), run))), AuthzErr::NotOwnApp);
    // Another owner's chain never holds one, even from its own admin.
    assert_eq!(refused(w.authz_cmd(elsewhere, put(as_user(other_admin), claude()))), AuthzErr::NotInstall);
    // Unflag: the row is gone at the next read, and a second delete is a no-op.
    let del = AuthzCmd { cred: as_user(admin), op: AuthzOp::DeleteAgentApp { app: AppId(DbId(3)) } };
    w.authz_cmd(install, del.clone()).unwrap();
    w.authz_cmd(install, del).unwrap();
    let f: AuthzFold = w.authz(install).fresh_leader();
    assert_eq!(f.agent_app(AppId(DbId(3))), None);
    let evs = w.authz(install).committed_events(common::A);
    assert_eq!(evs.iter().filter(|e| matches!(e, AuthzEvent::AgentApp { row: None, .. })).count(), 1);
}

/// `agents_enabled` defaults off and `agents_workspace` to `Fork`. `PATCH /orgs/{org}/agents` sets them, and a later
/// `SetPolicy` from a client that knows nothing of them keeps them.
#[test]
fn org_agent_settings_default_off_and_survive_a_policy_write() {
    let mut w = World::default();
    let (owner, admin) = (acct(0, 4, 1), acct(0, 4, 2));
    org(&mut w, owner, admin);
    let f: AuthzFold = w.authz(owner).fresh_leader();
    assert!(!f.policy().agents_enabled);
    assert_eq!(f.policy().agents_workspace, WorkspaceMode::Fork);
    let set = AuthzOp::SetOrgAgents { enabled: Some(true), workspace: Some(WorkspaceMode::InRepo) };
    w.authz_cmd(owner, AuthzCmd { cred: as_user(admin), op: set }).unwrap();
    let base = OrgPolicy { base_permission: Some(RepoPermission::Read), ..OrgPolicy::default() };
    let op = AuthzOp::SetPolicy(base);
    assert_eq!(graftyard_raft::Command::format(&AuthzCmd { cred: OPS, op: op.clone() }), AUTHZ_FORMAT_V2);
    w.authz_cmd(owner, AuthzCmd { cred: as_user(admin), op }).unwrap();
    let f: AuthzFold = w.authz(owner).fresh_leader();
    let p = f.policy();
    assert_eq!(
        (p.agents_enabled, p.agents_workspace, p.base_permission),
        (true, WorkspaceMode::InRepo, Some(RepoPermission::Read))
    );
    // The fold round-trips through the codec every dep read and stored blob uses.
    let back: AuthzFold = codec::from_bytes(&codec::to_bytes(&f)).unwrap();
    assert_eq!(back, f);
    // A user account has no org policy.
    let user = acct(0, 4, 9);
    w.authz_cmd(user, AuthzCmd { cred: OPS, op: AuthzOp::Open { kind: AccountKind::User, enterprise: None } }).unwrap();
    let set = AuthzOp::SetOrgAgents { enabled: Some(true), workspace: None };
    assert_eq!(refused(w.authz_cmd(user, AuthzCmd { cred: OPS, op: set })), AuthzErr::NotAnOrg);
}

/// A fold with no AM1-WP2 row is byte for byte what M3 wrote, and a `Policy` event's bytes are M3-WP3's, so entries and
/// stored blobs written before this package keep their digests. The pinned bytes were printed by `main` at 49d63f6.
#[test]
fn folds_and_policy_events_without_agent_rows_keep_their_m3_bytes() {
    let mut f = AuthzFold::new(acct(0, 4, 1));
    f.opened = Some(AccountKind::Org);
    graftyard_raft::Fold::apply(
        &mut f,
        &AuthzEvent::Policy(OrgPolicy { base_permission: Some(RepoPermission::Write), ..OrgPolicy::default() }),
    );
    let bytes = codec::to_bytes(&f);
    assert_eq!(hex(&bytes), M3_FOLD);
    let mut v = Vec::new();
    AuthzEvent::Policy(OrgPolicy { sso_enforced: true, ..OrgPolicy::default() }).encode(&mut v);
    assert_eq!(hex(&v), "060001");
    // An AM1-WP2 row grows a trailing part after the M3 bytes.
    let mut g = f.clone();
    graftyard_raft::Fold::apply(&mut g, &AuthzEvent::OrgAgents { enabled: true, workspace: WorkspaceMode::Fork });
    let b2 = codec::to_bytes(&g);
    assert_eq!(&b2[..bytes.len()], &bytes[..], "the M3 bytes come first");
    assert_eq!(codec::from_bytes::<AuthzFold>(&b2).unwrap(), g);
    let _ = record();
}

const M3_FOLD: &str = "0101828020010100000000000001000000000001020000000000";

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}
