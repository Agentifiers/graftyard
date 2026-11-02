//! What each command decides, entry by entry, as a leader decides and a follower checks.

mod common;

use common::{acct, entry, key, login, record};
use graftyard_identity::{
    AccountKind, AuthzCmd, AuthzErr, AuthzFold, AuthzOp, AuthzOut, AuthzRead, CredentialRefusal, IdBlock, IdentityDep,
    LoginRow, NameKey, NamesCmd, NamesErr, NamesEvent, NamesFold, NamesOp, NamesOut, NamesRead, OpaqueToken, OrgRole,
    RESERVED, RepoEntry, RepoName, RepoPermission, ScopeSet, TokenKind, TokensCmd, TokensErr, TokensFold, TokensOp,
    UserId, account_shard, name_shard,
};
use graftyard_raft::{Command, Fold, Scratch};
use graftyard_types::{
    AccountId, ChainId, CredentialKey, CredentialShard, DbId, NameShard, PlacementName, PlacementRecord, SiteId,
    Timestamp, TokenShard,
};
use smallvec::smallvec;

const OPS: CredentialShard = CredentialShard::NONE;

fn names_cmd(op: NamesOp) -> NamesCmd {
    NamesCmd { cred: OPS, op }
}

fn claim(l: &str) -> NamesCmd {
    names_cmd(NamesOp::ClaimLogin { login: login(l), kind: AccountKind::User, account: None })
}

/// The `Names` shard a login lives on, with a block of ids recorded.
fn shard_for(l: &str, block: u32) -> NamesFold {
    let mut f = NamesFold::new(name_shard(&NameKey::Login(key(l))));
    f.apply_id_block(IdBlock::whole(0, block).unwrap());
    f
}

// ---- Names: login claims ----

#[test]
fn two_claims_of_one_login_in_one_entry_the_second_refused_by_the_first() {
    let mut f = shard_for("octocat", 3);
    // Two edges claim the same login, in two spellings; both land in one entry.
    let r = entry(&mut f, &[claim("octocat"), claim("OctoCat")], &[]);
    let first = acct(0, 3, 0);
    assert_eq!(r[0], Ok(NamesOut::Account(first)));
    assert_eq!(r[1], Err(NamesErr::Taken { by: first }), "refused only because of the earlier command");
    // The refusal rests on the first command alone: on the fold before the entry, the second would have won.
    let before = shard_for("octocat", 3);
    assert!(claim("OctoCat").decide(&before.scratch(), &[]).is_ok());
    // Only one claim committed, and the refused one used no id.
    assert_eq!(f.login(&key("octocat")), Some(LoginRow::Claimed { account: first, kind: AccountKind::User }));
    assert_eq!(f.ids.peek(), Some(acct(0, 3, 1).0));
}

#[test]
fn different_logins_in_one_entry_take_consecutive_ids() {
    // Two logins on the same shard: search for a pair.
    let base = "user";
    let s0 = name_shard(&NameKey::Login(key("user0")));
    let other = (1..100_000).map(|i| format!("{base}{i}")).find(|l| name_shard(&NameKey::Login(key(l))) == s0).unwrap();
    let mut f = NamesFold::new(s0);
    f.apply_id_block(IdBlock::whole(0, 9).unwrap());
    let r = entry(&mut f, &[claim("user0"), claim(&other)], &[]);
    assert_eq!(r, [Ok(NamesOut::Account(acct(0, 9, 0))), Ok(NamesOut::Account(acct(0, 9, 1)))]);
}

#[test]
fn a_claim_on_the_wrong_shard_is_refused() {
    let mut f = shard_for("octocat", 1);
    let want = name_shard(&NameKey::Login(key("hubot")));
    assert_ne!(f.shard, Some(want));
    assert_eq!(entry(&mut f, &[claim("hubot")], &[]), [Err(NamesErr::WrongShard { want })]);
}

#[test]
fn case_folding_collides_and_keeps_the_owners_spelling_out_of_the_key() {
    let mut f = shard_for("Mona-Lisa", 1);
    assert_eq!(entry(&mut f, &[claim("Mona-Lisa")], &[]), [Ok(NamesOut::Account(acct(0, 1, 0)))]);
    for spelling in ["mona-lisa", "MONA-LISA", "mOnA-LiSa"] {
        assert_eq!(key(spelling), key("Mona-Lisa"));
        assert_eq!(entry(&mut f, &[claim(spelling)], &[]), [Err(NamesErr::Taken { by: acct(0, 1, 0) })], "{spelling}");
    }
    // Lowercasing is ASCII only: a non-ASCII login never parses, so no Unicode fold can collide.
    assert!(graftyard_identity::Login::parse("mönа").is_err());
    assert!(graftyard_identity::Login::parse("İnci").is_err());
}

#[test]
fn login_syntax_is_exactly_shareds_rule() {
    use graftyard_identity::Login;
    for ok in ["a", "A1", "a-b", "a-b-c", &"x".repeat(39), "0"] {
        assert!(Login::parse(ok).is_ok(), "{ok}");
    }
    for bad in ["", "-a", "a-", "a--b", &"x".repeat(40), "a_b", "a.b", "a b", "ä"] {
        assert!(Login::parse(bad).is_err(), "{bad}");
    }
}

#[test]
fn reserved_names_are_refused_in_any_case_and_followers_refuse_them_too() {
    for r in RESERVED.iter().filter(|r| graftyard_identity::Login::parse(r).is_ok()) {
        for spelling in [r.to_string(), r.to_ascii_uppercase()] {
            let mut f = shard_for(&spelling, 1);
            assert_eq!(entry(&mut f, &[claim(&spelling)], &[]), [Err(NamesErr::Reserved)], "{spelling}");
        }
    }
    // `_services`, `_t`, `_codeload`, `_registry` do not parse as logins at all.
    for r in RESERVED.iter().filter(|r| r.starts_with('_')) {
        assert!(graftyard_identity::Login::parse(r).is_err(), "{r}");
    }
    // A leader that let one through is refused by the follower's check.
    let f = shard_for("api", 1);
    let ev = NamesEvent::LoginClaimed { key: key("API"), account: acct(0, 1, 0), kind: AccountKind::User };
    assert!(f.scratch().check(&ev).is_err());
}

#[test]
fn a_released_login_is_a_former_name_until_someone_claims_it() {
    let mut f = shard_for("octocat", 1);
    let a = acct(0, 1, 0);
    entry(&mut f, &[claim("octocat")], &[]);
    // Someone else cannot release it.
    let other = acct(0, 7, 7);
    let rel = |account| names_cmd(NamesOp::ReleaseLogin { key: key("octocat"), account });
    assert_eq!(entry(&mut f, &[rel(other)], &[]), [Err(NamesErr::NotHolder)]);
    assert_eq!(entry(&mut f, &[rel(a)], &[]), [Ok(NamesOut::Account(a))]);
    assert_eq!(f.login(&key("octocat")), Some(LoginRow::Former { account: a }));
    // Releasing twice is refused.
    assert_eq!(entry(&mut f, &[rel(a)], &[]), [Err(NamesErr::NotHolder)]);
    // A new account claims it, in another spelling, and gets a new id: the old one is never reused.
    let r = entry(&mut f, &[claim("OctoCat")], &[]);
    let b = acct(0, 1, 1);
    assert_eq!(r, [Ok(NamesOut::Account(b))]);
    assert_eq!(f.login(&key("octocat")), Some(LoginRow::Claimed { account: b, kind: AccountKind::User }));
    // Release and reclaim in one entry: the claim sees the release.
    let r = entry(&mut f, &[rel(b), claim("octocat")], &[]);
    assert_eq!(r, [Ok(NamesOut::Account(b)), Ok(NamesOut::Account(acct(0, 1, 2)))]);
}

#[test]
fn the_original_owner_can_take_its_former_name_back_by_account() {
    let mut f = shard_for("octocat", 1);
    let a = acct(0, 1, 0);
    entry(&mut f, &[claim("octocat")], &[]);
    entry(&mut f, &[names_cmd(NamesOp::ReleaseLogin { key: key("octocat"), account: a })], &[]);
    let back = names_cmd(NamesOp::ClaimLogin { login: login("octocat"), kind: AccountKind::User, account: Some(a) });
    assert_eq!(entry(&mut f, std::slice::from_ref(&back), &[]), [Ok(NamesOut::Account(a))]);
    // Claiming it again while it holds it is `Taken`, even by the holder: a duplicate goes through the request row.
    assert_eq!(entry(&mut f, &[back], &[]), [Err(NamesErr::Taken { by: a })]);
}

#[test]
fn a_signup_with_no_block_asks_for_one_and_a_block_never_rewinds() {
    let mut f = NamesFold::new(name_shard(&NameKey::Login(key("octocat"))));
    assert_eq!(entry(&mut f, &[claim("octocat")], &[]), [Err(NamesErr::NeedIdBlock)]);
    let b5 = IdBlock::whole(0, 5).unwrap();
    f.apply_id_block(b5);
    assert_eq!(f.ids.peek(), DbId::pack(0, 5, 0));
    // A block that does not follow changes nothing (the follower's `verify` refuses it before it applies).
    let mut s = f.scratch();
    s.apply_id_block(IdBlock::whole(0, 4).unwrap());
    assert_eq!(s.ids().peek(), DbId::pack(0, 5, 0));
    f.apply_id_block(b5);
    f.apply_id_block(IdBlock::whole(0, 4).unwrap());
    assert_eq!(f.ids.peek(), DbId::pack(0, 5, 0), "a block never rewinds the cursor");
    assert_eq!(entry(&mut f, &[claim("octocat")], &[]), [Ok(NamesOut::Account(acct(0, 5, 0)))]);
}

// ---- Account rows and credentials ----

fn token_cred(account: AccountId, tok: &OpaqueToken) -> CredentialShard {
    CredentialShard { account: Some(account), key: Some(CredentialKey::Token { shard: tok.shard(), hash: tok.hash() }) }
}

fn pat(secret: &[u8; 30]) -> OpaqueToken {
    OpaqueToken::new(TokenKind::Pat, *secret).unwrap()
}

/// A user, its `Account` row on its account shard, and a stored token on the token's shard.
struct World {
    user: AccountId,
    tok: OpaqueToken,
    names: NamesFold,
    tokens: TokensFold,
}

impl World {
    fn new() -> World {
        let user = acct(0, 2, 5);
        let tok = pat(b"IqIMNOZH6zOwIEB4T9A2g4EHMy8Ji4");
        let mut names = NamesFold::new(account_shard(user));
        let put = names_cmd(NamesOp::PutAccount { id: user, suspended: false, sessions_not_before: Timestamp(0) });
        assert_eq!(entry(&mut names, &[put], &[]), [Ok(NamesOut::Done)]);
        let mut tokens = TokensFold::new(tok.shard());
        let store = TokensCmd {
            cred: OPS,
            op: TokensOp::Store {
                hash: tok.hash(),
                kind: TokenKind::Pat,
                account: user,
                scopes: ScopeSet(1),
                expires: None,
            },
        };
        assert_eq!(entry(&mut tokens, std::slice::from_ref(&store), &[]), [Ok(())]);
        assert_eq!(entry(&mut tokens, &[store], &[]), [Err(TokensErr::Exists)], "a hash is stored once");
        World { user, tok, names, tokens }
    }

    fn cred(&self) -> CredentialShard {
        token_cred(self.user, &self.tok)
    }

    fn deps(&self) -> Vec<(ChainId, IdentityDep)> {
        vec![
            (ChainId::Names(account_shard(self.user)), IdentityDep::Names(self.names.clone())),
            (ChainId::Tokens(self.tok.shard()), IdentityDep::Tokens(self.tokens.clone())),
        ]
    }

    fn revoke(&mut self) {
        let r = TokensCmd { cred: OPS, op: TokensOp::Revoke { hash: self.tok.hash() } };
        assert_eq!(entry(&mut self.tokens, &[r], &[]), [Ok(())]);
    }

    fn suspend(&mut self) {
        let put = names_cmd(NamesOp::PutAccount { id: self.user, suspended: true, sessions_not_before: Timestamp(0) });
        assert_eq!(entry(&mut self.names, &[put], &[]), [Ok(NamesOut::Done)]);
    }
}

/// An owner's `Authz` chain, a user account that owns it.
fn user_authz(owner: AccountId) -> AuthzFold {
    let mut a = AuthzFold::new(owner);
    let open = AuthzCmd { cred: OPS, op: AuthzOp::Open { kind: AccountKind::User, enterprise: None } };
    assert_eq!(entry(&mut a, &[open], &[]), [Ok(AuthzOut::Done)]);
    a.apply_id_block(IdBlock::whole(0, 77).unwrap());
    a
}

fn create(cred: CredentialShard, name: &str) -> AuthzCmd {
    AuthzCmd { cred, op: AuthzOp::CreateRepo { name: RepoName::parse(name).unwrap() } }
}

#[test]
fn commands_lists_the_credential_chains_in_deps_once() {
    let w = World::new();
    let c = create(w.cred(), "r");
    let deps = c.deps(&user_authz(w.user));
    assert_eq!(deps.as_slice(), [ChainId::Names(account_shard(w.user)), ChainId::Tokens(w.tok.shard())]);
    // On the token's own shard, the token row is read from the scratch view, not as a dep.
    let t = TokensCmd { cred: w.cred(), op: TokensOp::Revoke { hash: w.tok.hash() } };
    assert_eq!(t.deps(&w.tokens).as_slice(), [ChainId::Names(account_shard(w.user))]);
    assert!(OPS == CredentialShard::NONE && create(OPS, "r").deps(&AuthzFold::default()).is_empty());
}

#[test]
fn a_revoked_token_refuses_every_command_decided_afterwards() {
    let mut w = World::new();
    let mut authz = user_authz(w.user);
    // 1,000 commands queued before the revocation, decided in entries after it (BF-15's rule).
    let queued: Vec<AuthzCmd> = (0..1_000).map(|i| create(w.cred(), &format!("repo-{i}"))).collect();
    assert!(matches!(entry(&mut authz, &queued[..1], &w.deps())[0], Ok(AuthzOut::Repo { .. })));
    w.revoke();
    let deps = w.deps();
    let mut refused = 0;
    for batch in queued[1..].chunks(37) {
        for r in entry(&mut authz, batch, &deps) {
            assert_eq!(r, Err(AuthzErr::Unauthorized(CredentialRefusal::Revoked)));
            refused += 1;
        }
    }
    assert_eq!(refused, 999);
    assert_eq!(authz.names.len(), 1);
}

#[test]
fn a_suspended_account_refuses_every_command_decided_afterwards() {
    let mut w = World::new();
    let mut authz = user_authz(w.user);
    w.suspend();
    let deps = w.deps();
    let queued: Vec<AuthzCmd> = (0..1_000).map(|i| create(w.cred(), &format!("repo-{i}"))).collect();
    for r in entry(&mut authz, &queued, &deps) {
        assert_eq!(r, Err(AuthzErr::Unauthorized(CredentialRefusal::Suspended(w.user))));
    }
    // A missing dep fails closed: the host did not read the chain.
    let r = entry(&mut authz, &queued[..1], &deps[..1]);
    assert_eq!(r, [Err(AuthzErr::Unauthorized(CredentialRefusal::Suspended(w.user)))]);
    let r = entry(&mut authz, &queued[..1], &[]);
    assert_eq!(r, [Err(AuthzErr::Unauthorized(CredentialRefusal::NotRead(ChainId::Names(account_shard(w.user)))))]);
}

#[test]
fn a_revocation_earlier_in_the_same_entry_binds_the_commands_after_it() {
    let w = World::new();
    let mut tokens = w.tokens.clone();
    let names_dep = vec![(ChainId::Names(account_shard(w.user)), IdentityDep::Names(w.names.clone()))];
    // On the token's own shard: the user revokes its own token, then a second command under it.
    let other = pat(b"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAA");
    let revoke = TokensCmd { cred: w.cred(), op: TokensOp::Revoke { hash: w.tok.hash() } };
    let mut cmds = vec![revoke];
    if other.shard() == w.tok.shard() {
        cmds.push(TokensCmd {
            cred: w.cred(),
            op: TokensOp::Store {
                hash: other.hash(),
                kind: TokenKind::Pat,
                account: w.user,
                scopes: ScopeSet(0),
                expires: None,
            },
        });
    } else {
        cmds.push(TokensCmd { cred: w.cred(), op: TokensOp::Revoke { hash: w.tok.hash() } });
    }
    let r = entry(&mut tokens, &cmds, &names_dep);
    assert_eq!(r, [Ok(()), Err(TokensErr::Unauthorized(CredentialRefusal::Revoked))]);
    // Same on the account's `Names` shard: suspension, then a command under the suspended account.
    let mut names = w.names.clone();
    let tok_dep = vec![(ChainId::Tokens(w.tok.shard()), IdentityDep::Tokens(w.tokens.clone()))];
    let suspend = names_cmd(NamesOp::PutAccount { id: w.user, suspended: true, sessions_not_before: Timestamp(0) });
    let own = NamesCmd {
        cred: w.cred(),
        op: NamesOp::PutAccount { id: w.user, suspended: false, sessions_not_before: Timestamp(9) },
    };
    let r = entry(&mut names, &[suspend, own], &tok_dep);
    assert_eq!(r, [Ok(NamesOut::Done), Err(NamesErr::Unauthorized(CredentialRefusal::Suspended(w.user)))]);
}

#[test]
fn credential_refusals_cover_missing_wrong_account_and_wrong_shard() {
    let w = World::new();
    let mut authz = user_authz(w.user);
    let stranger = pat(b"BBBBBBBBBBBBBBBBBBBBBBBBBBBBBB");
    let mut deps = w.deps();
    deps.push((ChainId::Tokens(stranger.shard()), IdentityDep::Tokens(TokensFold::new(stranger.shard()))));
    let r = entry(&mut authz, &[create(token_cred(w.user, &stranger), "x")], &deps);
    assert_eq!(r, [Err(AuthzErr::Unauthorized(CredentialRefusal::NoToken))]);
    // The token belongs to `w.user`; a command naming another account under it is refused.
    let other = acct(0, 2, 6);
    let mut names = w.names.clone();
    let put = names_cmd(NamesOp::PutAccount { id: other, suspended: false, sessions_not_before: Timestamp(0) });
    if account_shard(other) == account_shard(w.user) {
        entry(&mut names, &[put], &[]);
    }
    let cred = CredentialShard { account: Some(other), ..w.cred() };
    let r = AuthzCmd { cred, op: AuthzOp::CreateRepo { name: RepoName::parse("y").unwrap() } };
    let mut deps2 = vec![(ChainId::Tokens(w.tok.shard()), IdentityDep::Tokens(w.tokens.clone()))];
    let mut other_names = NamesFold::new(account_shard(other));
    entry(
        &mut other_names,
        &[names_cmd(NamesOp::PutAccount { id: other, suspended: false, sessions_not_before: Timestamp(0) })],
        &[],
    );
    deps2.push((ChainId::Names(account_shard(other)), IdentityDep::Names(other_names)));
    assert_eq!(entry(&mut authz, &[r], &deps2), [Err(AuthzErr::Unauthorized(CredentialRefusal::WrongAccount))]);
    // A key that names a shard its hash does not map to.
    let bad = CredentialShard {
        account: Some(w.user),
        key: Some(CredentialKey::Token { shard: TokenShard((w.tok.shard().0 + 1) % 3072), hash: w.tok.hash() }),
    };
    assert_eq!(
        entry(&mut authz, &[create(bad, "z")], &w.deps()),
        [Err(AuthzErr::Unauthorized(CredentialRefusal::WrongShard))]
    );
}

// ---- Owner placement records ----

fn tri(sites: [u8; 3], home: u8) -> (PlacementName, smallvec::SmallVec<[SiteId; 3]>, SiteId) {
    (PlacementName::Tri, sites.iter().map(|s| SiteId(*s)).collect(), SiteId(home))
}

fn mv(owner: AccountId, sites: [u8; 3], home: u8, at: u64) -> NamesCmd {
    let (name, sites, home) = tri(sites, home);
    names_cmd(NamesOp::MovePlacement { owner, name, sites, home, at: Timestamp(at) })
}

#[test]
fn an_owner_move_bumps_generation_and_keeps_moving_from_until_finished() {
    let owner = acct(0, 3, 3);
    let mut f = NamesFold::new(account_shard(owner));
    let init = names_cmd(NamesOp::InitPlacement { owner, record: record() });
    entry(&mut f, std::slice::from_ref(&init), &[]);
    assert_eq!(entry(&mut f, &[init], &[]), [Err(NamesErr::RecordExists)]);
    let r1 = f.placement(owner).unwrap().clone();
    assert_eq!((r1.generation, r1.moving_from.is_none()), (1, true));

    let r = entry(&mut f, &[mv(owner, [0, 1, 3], 0, 50)], &[]);
    let NamesOut::Placement(r2) = r[0].clone().unwrap() else { panic!() };
    assert_eq!(r2.generation, 2);
    assert_eq!(r2.moving_from.as_deref(), Some(&r1));
    assert_eq!(f.placement(owner), Some(&r2));
    let gens: Vec<u32> = r2.lookup_order().map(|r| r.generation).collect();
    assert_eq!(gens, [2, 1]);

    // The same record again is refused; a second move during the move nests and keeps both older records.
    assert_eq!(entry(&mut f, &[mv(owner, [0, 1, 3], 0, 60)], &[]), [Err(NamesErr::Unchanged)]);
    let r = entry(&mut f, &[mv(owner, [0, 4, 3], 0, 70)], &[]);
    let NamesOut::Placement(r3) = r[0].clone().unwrap() else { panic!() };
    let gens: Vec<u32> = r3.lookup_order().map(|r| r.generation).collect();
    assert_eq!(gens, [3, 2, 1]);

    // The first move's Workflow finishing late may not clear the second's `moving_from`.
    let finish = |generation| names_cmd(NamesOp::FinishMove { owner, generation });
    assert_eq!(entry(&mut f, &[finish(2)], &[]), [Err(NamesErr::StaleGeneration { current: 3 })]);
    assert!(f.placement(owner).unwrap().moving_from.is_some());
    let r = entry(&mut f, &[finish(3)], &[]);
    let NamesOut::Placement(done) = r[0].clone().unwrap() else { panic!() };
    assert_eq!(done, PlacementRecord { moving_from: None, ..r3.clone() });
    assert_eq!(entry(&mut f, &[finish(3)], &[]), [Err(NamesErr::StaleGeneration { current: 3 })]);
}

#[test]
fn followers_refuse_a_placement_that_drops_moving_from_or_skips_a_generation() {
    let owner = acct(0, 3, 3);
    let mut f = NamesFold::new(account_shard(owner));
    entry(&mut f, &[names_cmd(NamesOp::InitPlacement { owner, record: record() })], &[]);
    let r1 = record();
    let s = f.scratch();
    let (name, sites, home) = tri([0, 1, 3], 0);
    let good =
        PlacementRecord { name, sites, home, generation: 2, moving_from: Some(Box::new(r1.clone())), at: Timestamp(5) };
    assert!(s.check(&NamesEvent::Placement { owner, record: good.clone() }).is_ok());
    let dropped = PlacementRecord { moving_from: None, ..good.clone() };
    assert!(s.check(&NamesEvent::Placement { owner, record: dropped }).is_err());
    let skipped = PlacementRecord { generation: 3, ..good.clone() };
    assert!(s.check(&NamesEvent::Placement { owner, record: skipped }).is_err());
    let same_gen = PlacementRecord { generation: 1, ..good };
    assert!(s.check(&NamesEvent::Placement { owner, record: same_gen }).is_err());
}

#[test]
fn a_move_to_a_malformed_record_is_refused() {
    let owner = acct(0, 3, 3);
    let mut f = NamesFold::new(account_shard(owner));
    entry(&mut f, &[names_cmd(NamesOp::InitPlacement { owner, record: record() })], &[]);
    let bad = names_cmd(NamesOp::MovePlacement {
        owner,
        name: PlacementName::Tri,
        sites: smallvec![SiteId(0), SiteId(0), SiteId(2)],
        home: SiteId(0),
        at: Timestamp(9),
    });
    assert!(matches!(entry(&mut f, &[bad], &[])[0], Err(NamesErr::BadRecord(_))));
    assert_eq!(f.placement(owner).unwrap().generation, 1);
}

#[test]
fn a_user_may_not_place_suspend_or_record_blocks() {
    let w = World::new();
    let mut names = w.names.clone();
    let tok_dep = vec![(ChainId::Tokens(w.tok.shard()), IdentityDep::Tokens(w.tokens.clone()))];
    for op in [
        NamesOp::PutAccount { id: w.user, suspended: true, sessions_not_before: Timestamp(0) },
        NamesOp::InitPlacement { owner: w.user, record: record() },
    ] {
        let r = entry(&mut names, &[NamesCmd { cred: w.cred(), op }], &tok_dep);
        assert_eq!(r, [Err(NamesErr::Forbidden)]);
    }
    // Its own sign-out-everywhere is allowed.
    let ok = NamesOp::PutAccount { id: w.user, suspended: false, sessions_not_before: Timestamp(99) };
    assert_eq!(entry(&mut names, &[NamesCmd { cred: w.cred(), op: ok }], &tok_dep), [Ok(NamesOut::Done)]);
}

#[test]
fn repo_owner_rows_live_on_the_repos_shard() {
    let repo = graftyard_types::RepoId(DbId::pack(0, 77, 0).unwrap());
    let owner = acct(0, 2, 5);
    let mut f = NamesFold::new(name_shard(&NameKey::Repo(repo)));
    let c = names_cmd(NamesOp::RepoOwner { repo, owner });
    assert_eq!(entry(&mut f, std::slice::from_ref(&c), &[]), [Ok(NamesOut::Done)]);
    assert_eq!(f.repo_owner(repo), Some(owner));
    let mut wrong = NamesFold::new(NameShard((name_shard(&NameKey::Repo(repo)).0 + 1) % 1024));
    assert!(matches!(entry(&mut wrong, &[c], &[])[0], Err(NamesErr::WrongShard { .. })));
}

// ---- Authz: admin grants and RepoName ----

#[test]
fn repo_names_are_unique_case_folded_and_renames_leave_aliases() {
    let w = World::new();
    let mut a = user_authz(w.user);
    let d = w.deps();
    let r = entry(&mut a, &[create(w.cred(), "Hello-World"), create(w.cred(), "hello-world.git")], &d);
    let repo = graftyard_types::RepoId(DbId::pack(0, 77, 0).unwrap());
    assert_eq!(r[0], Ok(AuthzOut::Repo { repo, epoch: graftyard_identity::NameEpoch(1) }));
    assert_eq!(r[1], Err(AuthzErr::Taken), "refused by the earlier command of the same entry");
    let k = RepoName::parse("HELLO-WORLD").unwrap().key();
    assert_eq!(a.resolve(&k), Some((repo, true)));

    let rename = |from: &str, to: &str| AuthzCmd {
        cred: w.cred(),
        op: AuthzOp::RenameRepo { from: RepoName::parse(from).unwrap().key(), to: RepoName::parse(to).unwrap() },
    };
    assert_eq!(entry(&mut a, &[rename("hello-world", "HELLO-world")], &d), [Err(AuthzErr::SameName)]);
    assert!(matches!(entry(&mut a, &[rename("hello-world", "spoon")], &d)[0], Ok(AuthzOut::Repo { .. })));
    assert_eq!(a.resolve(&RepoName::parse("spoon").unwrap().key()), Some((repo, true)));
    assert_eq!(a.resolve(&k), Some((repo, false)), "the old name is an alias (301)");
    // A new repository may take the alias's key; the alias then ends.
    let r = entry(&mut a, &[create(w.cred(), "hello-world")], &d);
    let AuthzOut::Repo { repo: second, .. } = r[0].clone().unwrap() else { panic!() };
    assert_ne!(second, repo);
    assert_eq!(a.resolve(&k), Some((second, true)));
    // A deleted name is held.
    let del = AuthzCmd { cred: w.cred(), op: AuthzOp::DeleteRepo { key: k.clone(), retain_until: Timestamp(9) } };
    assert_eq!(entry(&mut a, &[del], &d), [Ok(AuthzOut::Done)]);
    assert_eq!(a.resolve(&k), None);
    assert_eq!(entry(&mut a, &[create(w.cred(), "hello-world")], &d), [Err(AuthzErr::Taken)]);
    // A reservation (a transfer's commit point) is held too, and a follower refuses a `Live` over it.
    let rk = RepoName::parse("incoming").unwrap().key();
    let res = graftyard_identity::AuthzEvent::RepoName {
        key: rk.clone(),
        entry: Some(RepoEntry::Reserved { repo: graftyard_types::RepoId(DbId(5)), from: acct(0, 9, 9) }),
    };
    a.apply(&res);
    assert_eq!(entry(&mut a, &[create(w.cred(), "incoming")], &d), [Err(AuthzErr::Taken)]);
}

#[test]
fn repo_name_syntax() {
    for ok in ["a", "A.b_c-d", ".github", "x.git", &"x".repeat(100), &format!("{}.git", "y".repeat(100))] {
        assert!(RepoName::parse(ok).is_ok(), "{ok}");
    }
    assert_eq!(RepoName::parse("spoon.git").unwrap().as_str(), "spoon");
    for bad in ["", ".", "..", "..git", ".git", "a/b", "a b", &"x".repeat(101), "ü"] {
        assert!(RepoName::parse(bad).is_err(), "{bad}");
    }
}

#[test]
fn only_admins_create_repositories_and_org_admin_grants_bind_by_entry() {
    let w = World::new();
    let org = acct(0, 8, 1);
    let mut a = AuthzFold::new(org);
    let ops = |op| AuthzCmd { cred: OPS, op };
    entry(&mut a, &[ops(AuthzOp::Open { kind: AccountKind::Org, enterprise: None })], &[]);
    a.apply_id_block(IdBlock::whole(0, 12).unwrap());
    let d = w.deps();
    // Not a member: refused.
    assert_eq!(entry(&mut a, &[create(w.cred(), "r")], &d), [Err(AuthzErr::Forbidden)]);
    // A member, not an admin: refused. The admin grant and the create in one entry: the create sees it.
    let user = UserId(w.user);
    entry(&mut a, &[ops(AuthzOp::SetMember { user, role: Some(OrgRole::Member) })], &[]);
    assert_eq!(entry(&mut a, &[create(w.cred(), "r")], &d), [Err(AuthzErr::Forbidden)]);
    let r = entry(&mut a, &[ops(AuthzOp::SetMember { user, role: Some(OrgRole::Admin) }), create(w.cred(), "r")], &d);
    assert!(matches!(r[1], Ok(AuthzOut::Repo { .. })));
    assert!(a.is_admin(user));
    // An admin grants a repository permission; removal of admin binds the next command in the same entry.
    let repo = a.resolve(&RepoName::parse("r").unwrap().key()).unwrap().0;
    let grant = AuthzCmd {
        cred: w.cred(),
        op: AuthzOp::SetGrant { repo, user: UserId(acct(0, 1, 1)), permission: Some(RepoPermission::Write) },
    };
    let demote = ops(AuthzOp::SetMember { user, role: None });
    let r = entry(&mut a, &[grant.clone(), demote, grant], &d);
    assert_eq!(r[0], Ok(AuthzOut::Done));
    assert_eq!(r[2], Err(AuthzErr::Forbidden));
    assert_eq!(a.grant(repo, UserId(acct(0, 1, 1))), Some(RepoPermission::Write));
    // Only operators open chains; membership needs an org.
    let open = AuthzCmd { cred: w.cred(), op: AuthzOp::Open { kind: AccountKind::Org, enterprise: None } };
    assert_eq!(entry(&mut a, &[open], &d), [Err(AuthzErr::Forbidden)]);
    let mut u = user_authz(w.user);
    let m = ops(AuthzOp::SetMember { user, role: Some(OrgRole::Admin) });
    assert_eq!(entry(&mut u, &[m], &[]), [Err(AuthzErr::NotAnOrg)]);
}

#[test]
fn a_repository_id_is_never_reused_across_blocks() {
    let w = World::new();
    let mut a = user_authz(w.user);
    let d = w.deps();
    let mut seen = std::collections::BTreeSet::new();
    // Run the block out: the last id is offset 2^16 - 1, then the group asks for a new block.
    a.ids = {
        let mut c = graftyard_identity::IdCursor::default();
        c.apply_block(IdBlock::new(DbId::pack(0, 77, u16::MAX - 2).unwrap(), 3).unwrap());
        c
    };
    for i in 0..3 {
        let r = entry(&mut a, &[create(w.cred(), &format!("r{i}"))], &d);
        let AuthzOut::Repo { repo, .. } = r[0].clone().unwrap() else { panic!() };
        assert!(seen.insert(repo));
    }
    assert_eq!(seen.last().unwrap().0.offset(), u16::MAX);
    assert_eq!(entry(&mut a, &[create(w.cred(), "r3")], &d), [Err(AuthzErr::NeedIdBlock)]);
    a.apply_id_block(IdBlock::whole(0, 78).unwrap());
    let r = entry(&mut a, &[create(w.cred(), "r3")], &d);
    let AuthzOut::Repo { repo, .. } = r[0].clone().unwrap() else { panic!() };
    assert_eq!((repo.0.block(), repo.0.offset()), (78, 0));
}
