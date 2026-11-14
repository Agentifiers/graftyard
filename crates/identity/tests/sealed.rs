//! M4-WP3's rows on an owner's `Authz` chain (BF-59) on real `graftyard_raft::Member`s: the owner's `SealKey`s and its
//! org Actions secrets and variables.

mod common;

use common::acct;
use common::world::{FreshLeader, OPS, World};
use graftyard_identity::sealed::{ActionsName, SealKey, SecretScope, SecretVisibility, Wrapped};
use graftyard_identity::{
    AUTHZ_FORMAT_V12, AccountKind, AuthzCmd, AuthzErr, AuthzFold, AuthzOp, IdBlock, NamesCmd, NamesOp, OrgRole, UserId,
    account_shard,
};
use graftyard_raft::{CommitError, codec};
use graftyard_types::{AccountId, ChainId, CredentialShard, DbId, Index, RepoId, Timestamp};
use smallvec::smallvec;

fn refused<O: std::fmt::Debug, E: std::fmt::Debug>(r: Result<graftyard_raft::Acked<O>, CommitError<E>>) -> E {
    match r {
        Err(CommitError::Refused { err, .. }) => err,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

fn as_user(u: AccountId) -> CredentialShard {
    CredentialShard { account: Some(u), key: None }
}

fn account(w: &mut World, id: AccountId) {
    let op = NamesOp::PutAccount { id, suspended: false, sessions_not_before: Timestamp(0) };
    w.names_cmd(ChainId::Names(account_shard(id)), NamesCmd { cred: OPS, op }).unwrap();
}

fn open(w: &mut World, owner: AccountId, kind: AccountKind, admin: Option<AccountId>) {
    w.authz_cmd(owner, AuthzCmd { cred: OPS, op: AuthzOp::Open { kind, enterprise: None } }).unwrap();
    let g = w.authz(owner);
    let l = g.leader().unwrap();
    g.id_block(l, IdBlock::whole(0, 40).unwrap()).unwrap();
    g.settle(&mut |c| panic!("{c}"));
    if let Some(admin) = admin {
        let op = AuthzOp::SetMember { user: UserId(admin), role: Some(OrgRole::Admin) };
        w.authz_cmd(owner, AuthzCmd { cred: OPS, op }).unwrap();
        let op = NamesOp::PutAccount { id: admin, suspended: false, sessions_not_before: Timestamp(0) };
        w.names_cmd(ChainId::Names(account_shard(admin)), NamesCmd { cred: OPS, op }).unwrap();
    }
}

fn key(id: &str, created: u64) -> SealKey {
    SealKey {
        key_id: id.into(),
        public: [created as u8; 32],
        private: Wrapped { slot: 0, nonce: [1; 12], ciphertext: vec![2; 48].into() },
        created: Timestamp(created),
        retired: None,
    }
}

fn sealed() -> Box<[u8]> {
    vec![7u8; 60].into()
}

/// A key is minted once per id and the newest unretired one is current; a retired key seals no new org secret; a key
/// drops only once retired and named by no org secret; org rows need an org and an org scope; a variable's create
/// refuses an existing name and its update a missing one; only an owner admin or the operator writes.
#[test]
fn seal_keys_rotate_and_org_rows_follow_their_rules() {
    let mut w = World::default();
    let (org, admin, member) = (acct(0, 5, 1), acct(0, 5, 2), acct(0, 5, 3));
    open(&mut w, org, AccountKind::Org, Some(admin));
    account(&mut w, member);
    let cmd = |cred, op| AuthzCmd { cred, op };
    let put_key = |k: SealKey| AuthzOp::PutSealKey { key: k };
    assert_eq!(graftyard_raft::Command::format(&cmd(OPS, put_key(key("1", 10)))), AUTHZ_FORMAT_V12);
    assert_eq!(refused(w.authz_cmd(org, cmd(as_user(member), put_key(key("1", 10))))), AuthzErr::Forbidden);
    w.authz_cmd(org, cmd(OPS, put_key(key("1", 10)))).unwrap();
    // A re-send is a no-op; another key under the same id is refused; the same key rewrapped under another slot
    // (the wrapping key's rotation) replaces its private half and nothing else.
    w.authz_cmd(org, cmd(OPS, put_key(key("1", 10)))).unwrap();
    assert_eq!(refused(w.authz_cmd(org, cmd(OPS, put_key(key("1", 11))))), AuthzErr::BadRecord);
    let rewrapped =
        |k: SealKey| SealKey { private: Wrapped { slot: 1, nonce: [5; 12], ciphertext: vec![6; 48].into() }, ..k };
    let swapped = SealKey { public: [99; 32], ..rewrapped(key("1", 10)) };
    assert_eq!(refused(w.authz_cmd(org, cmd(OPS, put_key(swapped)))), AuthzErr::BadRecord);
    w.authz_cmd(org, cmd(OPS, put_key(rewrapped(key("1", 10))))).unwrap();
    let f: AuthzFold = w.authz(org).fresh_leader();
    assert_eq!(f.actions().seal_keys["1"], rewrapped(key("1", 10)));
    let name = ActionsName::parse("deploy_token").unwrap();
    let all = SecretScope::Org { visibility: SecretVisibility::All, repos: smallvec![] };
    let put = |scope: SecretScope, key_id: &str| AuthzOp::PutOrgSecret {
        name: name.clone(),
        scope,
        sealed: sealed(),
        key_id: key_id.into(),
    };
    assert_eq!(refused(w.authz_cmd(org, cmd(as_user(admin), put(SecretScope::Repo, "1")))), AuthzErr::BadRecord);
    let picked = SecretScope::Org { visibility: SecretVisibility::All, repos: smallvec![RepoId(DbId(4))] };
    assert_eq!(refused(w.authz_cmd(org, cmd(as_user(admin), put(picked, "1")))), AuthzErr::BadRecord);
    assert_eq!(refused(w.authz_cmd(org, cmd(as_user(admin), put(all.clone(), "9")))), AuthzErr::BadRecord);
    w.authz_cmd(org, cmd(as_user(admin), put(all.clone(), "1"))).unwrap();
    // Rotation: a newer key becomes current, the old one is retired and seals nothing new, and it drops only once no
    // org secret names it.
    w.authz_cmd(org, cmd(OPS, put_key(key("2", 20)))).unwrap();
    let f: AuthzFold = w.authz(org).fresh_leader();
    assert_eq!(f.actions().current_key().map(|k| &*k.key_id), Some("2"));
    let retire = |id: &str, drop| AuthzOp::RetireSealKey { key_id: id.into(), drop };
    assert_eq!(refused(w.authz_cmd(org, cmd(OPS, retire("1", true)))), AuthzErr::BadRecord, "not retired yet");
    w.authz_cmd(org, cmd(OPS, retire("1", false))).unwrap();
    assert_eq!(refused(w.authz_cmd(org, cmd(as_user(admin), put(all.clone(), "1")))), AuthzErr::BadRecord);
    assert_eq!(refused(w.authz_cmd(org, cmd(OPS, retire("1", true)))), AuthzErr::BadRecord, "a secret names it");
    w.authz_cmd(org, cmd(as_user(admin), put(all.clone(), "2"))).unwrap();
    w.authz_cmd(org, cmd(OPS, retire("1", true))).unwrap();
    let f: AuthzFold = w.authz(org).fresh_leader();
    assert_eq!(f.actions().seal_keys.keys().map(|k| &**k).collect::<Vec<_>>(), vec!["2"]);
    assert_eq!(&*f.actions().secrets[&name].1.key_id, "2");
    // Variables.
    let var = |value: &str, create| AuthzOp::PutOrgVariable {
        name: name.clone(),
        scope: all.clone(),
        value: value.into(),
        create,
    };
    assert_eq!(refused(w.authz_cmd(org, cmd(as_user(admin), var("a", false)))), AuthzErr::NoRecord);
    w.authz_cmd(org, cmd(as_user(admin), var("a", true))).unwrap();
    assert_eq!(refused(w.authz_cmd(org, cmd(as_user(admin), var("b", true)))), AuthzErr::Taken);
    w.authz_cmd(org, cmd(as_user(admin), var("b", false))).unwrap();
    let del = AuthzOp::DeleteOrgSecret { name: name.clone() };
    w.authz_cmd(org, cmd(as_user(admin), del.clone())).unwrap();
    assert_eq!(refused(w.authz_cmd(org, cmd(as_user(admin), del))), AuthzErr::NoRecord);
    let f: AuthzFold = w.authz(org).fresh_leader();
    assert!(f.actions().secrets.is_empty());
    assert_eq!(&*f.actions().variables[&name].1.value, "b");
    let back: AuthzFold = codec::from_bytes(&codec::to_bytes(&f)).unwrap();
    assert_eq!(back, f);
    // A user account holds keys (its repositories' secrets seal to them) but no org rows.
    let user = acct(0, 5, 9);
    open(&mut w, user, AccountKind::User, None);
    account(&mut w, user);
    w.authz_cmd(user, cmd(as_user(user), put_key(key("3", 30)))).unwrap();
    assert_eq!(refused(w.authz_cmd(user, cmd(as_user(user), put(all, "3")))), AuthzErr::NotAnOrg);
}

/// A fold with no Actions row keeps the bytes it had, and its `Audit` cursor still round-trips beside Actions rows.
#[test]
fn a_fold_without_actions_rows_keeps_its_bytes() {
    let mut f = AuthzFold::new(acct(0, 5, 1));
    f.opened = Some(AccountKind::Org);
    f.more.more.more.cursor = Index(5);
    let before = codec::to_bytes(&f);
    let mut with = f.clone();
    with.more.more.more.more.seal_keys.insert("1".into(), key("1", 10));
    let back: AuthzFold = codec::from_bytes(&codec::to_bytes(&with)).unwrap();
    assert_eq!(back, with);
    assert_eq!(back.audit_cursor(), Index(5));
    with.more.more.more.more = Default::default();
    assert_eq!(codec::to_bytes(&with), before);
}
