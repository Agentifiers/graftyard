//! Tokens 0x11.5 (`decisions/DECISIONS.md`, "`lifeboat-revoke`"): a developer's lifeboat tokens all live on
//! `lifeboat_shard(account)`, and each enroll's `StoreLifeboat` revokes every earlier one in its own entry, so a leaked
//! token stops working at the next enroll, on the live fold, on a member restored from its storage, on the fold blob,
//! and on a fold rebuilt from the shard's log.

mod common;

use common::{A, SITES, acct};
use graftyard_identity::{
    NamesCmd, NamesOp, OpaqueToken, TOKENS_FORMAT, TOKENS_FORMAT_V2, TOKENS_FORMAT_V3, TOKENS_FORMAT_V4, TokenKind,
    TokensCmd, TokensErr, TokensEvent, TokensFold, TokensOp, TokensRead, account_shard, draw_secret_on, lifeboat_shard,
};
use graftyard_raft::{CallError, CommitError, Fold, codec};
use graftyard_types::{AccountId, ChainId, CredentialShard, Timestamp};

use common::world::{FreshLeader, OPS, World};

fn rand() -> impl FnMut() -> u64 {
    let mut x = 0x2545_f491_4f6c_dd1du64;
    move || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    }
}

/// A `lifeboat:read` token drawn onto its account's shard, as the edge's route draws it.
fn token(r: &mut impl FnMut() -> u64, account: AccountId) -> OpaqueToken {
    let (s, _) = draw_secret_on(r, lifeboat_shard(account), 200_000).expect("lands");
    OpaqueToken::new(TokenKind::Pat, s).unwrap()
}

fn enroll(w: &mut World, t: &OpaqueToken, account: AccountId) {
    w.tokens_cmd(TokensCmd { cred: OPS, op: TokensOp::StoreLifeboat { hash: t.hash(), account } }).unwrap();
}

/// Two accounts whose lifeboat tokens share a shard, so a revocation that took every row of the shard shows.
fn neighbours() -> (AccountId, AccountId) {
    let a = acct(0, 2, 7);
    let b =
        (0..20_000u16).map(|i| acct(0, 3, i)).find(|b| lifeboat_shard(*b) == lifeboat_shard(a)).expect("a neighbour");
    (a, b)
}

#[test]
fn an_earlier_lifeboat_token_is_refused_after_a_second_enroll_and_after_a_rebuild() {
    let mut w = World::default();
    let mut r = rand();
    let (dev, other) = neighbours();
    let shard = lifeboat_shard(dev);
    let (first, kept) = (token(&mut r, dev), token(&mut r, other));
    enroll(&mut w, &first, dev);
    enroll(&mut w, &kept, other);
    assert_eq!(w.tokens(shard).fresh_leader().lifeboat(&first.hash()), Some(dev));
    let second = token(&mut r, dev);
    enroll(&mut w, &second, dev);
    let third = token(&mut r, dev);
    enroll(&mut w, &third, dev);

    let check = |f: &TokensFold, how: &str| {
        assert_eq!(f.lifeboat(&first.hash()), None, "{how}: the first token is refused");
        assert_eq!(f.lifeboat(&second.hash()), None, "{how}: the second token is refused");
        assert_eq!(f.lifeboat(&third.hash()), Some(dev), "{how}: the newest token reads");
        assert_eq!(f.lifeboat(&kept.hash()), Some(other), "{how}: another developer's token is untouched");
    };
    let g = w.tokens(shard);
    check(&g.fresh_leader(), "the leader's fold");
    // The second enroll's entry carries the revocation and the new row together.
    let evs = g.committed_events(A);
    let at = evs.iter().position(|e| *e == TokensEvent::LifeboatRead { hash: second.hash(), account: dev }).unwrap();
    assert_eq!(evs[at - 1], TokensEvent::LifeboatRevoked { hash: first.hash() });

    // Every member restored from its own storage.
    for s in SITES {
        g.crash(s);
        g.restart(s);
    }
    g.elect_after_promises(A);
    check(&g.fresh_leader(), "after a restore");

    // The fold's stored blob, and the fold rebuilt from the shard's log from entry 1.
    let f = g.fresh_leader();
    check(&codec::from_bytes::<TokensFold>(&codec::to_bytes(&f)).unwrap(), "the fold blob");
    let mut rebuilt = TokensFold::new(shard);
    for ev in g.committed_events(A) {
        rebuilt.apply(&ev);
    }
    check(&rebuilt, "the rebuilt fold");
}

#[test]
fn a_lifeboat_token_is_stored_only_on_its_accounts_shard_by_the_operator_in_format_5() {
    let mut w = World::default();
    let mut r = rand();
    let dev = acct(0, 2, 9);
    // A hash off the account's lifeboat shard could not see the earlier tokens, so it is refused.
    let off =
        std::iter::repeat_with(|| graftyard_identity::draw_secret(&mut r, graftyard_identity::Family::Na).unwrap().0)
            .map(|s| OpaqueToken::new(TokenKind::Pat, s).unwrap())
            .find(|t| t.shard() != lifeboat_shard(dev))
            .unwrap();
    let op = TokensOp::StoreLifeboat { hash: off.hash(), account: dev };
    assert!(matches!(
        w.tokens_cmd(TokensCmd { cred: OPS, op }),
        Err(CommitError::Refused { err: TokensErr::WrongShard { want }, .. }) if want == lifeboat_shard(dev)
    ));
    // Only the operator: the edge verified the developer, then commits as itself.
    let t = token(&mut r, dev);
    // The developer's own account row, so the refusal is the op's rule and not a missing credential.
    let put = NamesOp::PutAccount { id: dev, suspended: false, sessions_not_before: Timestamp(0) };
    w.names_cmd(ChainId::Names(account_shard(dev)), NamesCmd { cred: OPS, op: put }).unwrap();
    let me = CredentialShard { account: Some(dev), key: None };
    let op = TokensOp::StoreLifeboat { hash: t.hash(), account: dev };
    assert!(matches!(
        w.tokens_cmd(TokensCmd { cred: me, op: op.clone() }),
        Err(CommitError::Refused { err: TokensErr::Forbidden, .. })
    ));
    // A member of the release before reads no format 5.
    let shard = lifeboat_shard(dev);
    let mut old: common::Group<TokensCmd> = common::Group::reading(
        ChainId::Tokens(shard),
        vec![TOKENS_FORMAT, TOKENS_FORMAT_V2, TOKENS_FORMAT_V3, TOKENS_FORMAT_V4],
        TokensFold::new(shard),
    );
    old.elect(A);
    assert!(matches!(old.commit(A, 1, TokensCmd { cred: OPS, op }), Err(CallError::Unsupported(_))));
}
