//! The identity commands on real `graftyard_raft::Member`s: BF-13's shape for login claims, ids across a leader
//! change in the middle of a block, and BF-15's revocation rule through `deps_read`.

mod common;

use std::collections::BTreeSet;

use common::{A, B, C, Group, SITES, acct, key, login};
use graftyard_identity::{
    AUTHZ_FORMAT, AccountKind, AuthzCmd, AuthzErr, AuthzFold, AuthzOp, AuthzOut, CredentialRefusal, IdBlock,
    IdentityDep, LoginRow, NAMES_FORMAT, NameKey, NamesCmd, NamesErr, NamesEvent, NamesFold, NamesOp, NamesOut,
    NamesRead, OpaqueToken, RepoName, ScopeSet, TokenKind, TokensCmd, TokensFold, TokensOp, account_shard, name_shard,
};
use graftyard_raft::{CommitError, Role};
use graftyard_types::{AccountId, ChainId, CredentialKey, CredentialShard, DbId, Timestamp};

const OPS: CredentialShard = CredentialShard::NONE;

fn claim(l: &str) -> NamesCmd {
    NamesCmd { cred: OPS, op: NamesOp::ClaimLogin { login: login(l), kind: AccountKind::User, account: None } }
}

fn block(b: u32) -> IdBlock {
    IdBlock::whole(0, b).unwrap()
}

fn no_deps(c: ChainId) -> IdentityDep {
    panic!("no dependency expected, asked for {c}")
}

fn names_group(l: &str) -> Group<NamesCmd> {
    let shard = name_shard(&NameKey::Login(key(l)));
    let mut g = Group::new(ChainId::Names(shard), NAMES_FORMAT, NamesFold::new(shard));
    g.elect(A);
    g
}

/// BF-13 for logins: two claims of one login go into one entry, the second refused only by the first, and the leader
/// is lost before the entry commits. Neither ticket resolves from the lost entry, and the re-sent claims decide
/// again on the new leader.
#[test]
fn a_refusal_resting_on_an_uncommitted_claim_is_never_returned() {
    let mut g = names_group("octocat");
    g.id_block(A, block(3)).unwrap();
    g.settle(&mut no_deps);
    // A hold entry keeps one entry in flight, so the three claims batch into the next entry: the next id block,
    // which the claims will assign from.
    g.id_block(A, block(4)).unwrap();
    let batch = g.m(A).log().len() as u64 + 1;
    let t0 = g.commit(A, 10, claim("octocat")).unwrap().ticket;
    let t1 = g.commit(A, 11, claim("OctoCat")).unwrap().ticket;
    let t2 = g.commit(A, 12, claim("OCTOCAT")).unwrap().ticket;
    assert_eq!(g.m(A).log().len() as u64, batch - 1, "the claims wait for the entry in flight");
    // Deliver everything but the claims' entry: the hold commits, the claims' entry stays on the leader alone.
    g.deliver(|e| !Group::<NamesCmd>::carries(e, batch));
    assert!(g.block_committed(block(4)), "the hold entry committed");
    assert_eq!(g.m(A).log().len() as u64, batch, "the claims' entry is appended on the leader");
    let graftyard_raft::EntryBody::Events(evs) = &g.m(A).log()[batch as usize - 1].body else { panic!() };
    assert_eq!(evs.len(), 1, "only the first claim produced events; the others were refused by it");
    assert!(
        g.result(t0).is_none() && g.result(t1).is_none() && g.result(t2).is_none(),
        "nothing resolves before commit"
    );
    // The leader is lost.
    g.crash(A);
    g.elect_after_promises(B);
    g.settle(&mut no_deps);
    // No ticket of the lost leadership ended in a refusal or an ack: each ended `NotLeader` or not at all.
    for t in [t0, t1, t2] {
        for r in g.results(t) {
            assert!(matches!(r, Err(CommitError::NotLeader(_))), "{t:?}: {r:?}");
        }
    }
    // The lost entry is gone from every live member.
    assert!(
        g.m(B).log().len() as u64 >= batch
            && !matches!(&g.m(B).log()[batch as usize - 1].body, graftyard_raft::EntryBody::Events(e) if !e.is_empty())
    );
    // The edge re-sends with the same request ids to the new leader, which decides them again.
    let n0 = g.commit(B, 10, claim("octocat")).unwrap().ticket;
    let n1 = g.commit(B, 11, claim("OctoCat")).unwrap().ticket;
    g.settle(&mut no_deps);
    g.restart(A);
    g.settle(&mut no_deps);
    let a = acct(0, 4, 0);
    assert_eq!(g.result(n0).unwrap().as_ref().map(|k| k.out.clone()), Ok(NamesOut::Account(a)));
    assert!(matches!(
        g.result(n1).unwrap(),
        Err(CommitError::Refused { err: NamesErr::Taken { by }, .. }) if *by == a
    ));
    for s in SITES {
        assert_eq!(
            g.m(s).group_fold().fold.login(&key("octocat")),
            Some(LoginRow::Claimed { account: a, kind: AccountKind::User }),
            "{s:?}"
        );
    }
}

/// Signups assign ids from a block; the leader changes mid-block with assigned-but-uncommitted ids in its tail.
/// The new leader resumes at the next free id after the committed ones; every committed id is unique, and no id
/// is committed twice even counting the lost tail.
#[test]
fn db_ids_are_never_reused_across_a_leader_change_mid_block() {
    let mut g = names_group("seed");
    let shard = match g.chain {
        ChainId::Names(s) => s,
        _ => unreachable!(),
    };
    // Logins that live on this shard.
    let logins: Vec<String> = (0..200_000)
        .map(|i| format!("u{i}"))
        .filter(|l| name_shard(&NameKey::Login(key(l))) == shard)
        .take(40)
        .collect();
    assert_eq!(logins.len(), 40);
    // A small block at the top of its range, so the change also crosses offset 2^16 - 1 and a block boundary.
    let top = IdBlock::new(DbId::pack(0, 21, u16::MAX - 11).unwrap(), 12).unwrap();
    g.id_block(A, top).unwrap();
    g.settle(&mut no_deps);
    let mut id = 100u128;
    for l in &logins[..5] {
        id += 1;
        assert!(g.submit(id, claim(l), &mut no_deps).is_ok());
    }
    // Five more on A, appended but never sent: assigned in A's log only.
    let tail_from = g.m(A).log().len() as u64 + 1;
    for l in &logins[5..10] {
        id += 1;
        g.commit(A, id, claim(l)).unwrap();
    }
    g.deliver(|e| !Group::<NamesCmd>::carries(e, tail_from));
    let lost_ids: Vec<AccountId> = g.m(A).log()[tail_from as usize - 1..]
        .iter()
        .flat_map(|e| match &e.body {
            graftyard_raft::EntryBody::Events(evs) => evs.clone(),
            _ => vec![],
        })
        .filter_map(|ev| match ev {
            NamesEvent::LoginClaimed { account, .. } => Some(account),
            _ => None,
        })
        .collect();
    assert!(!lost_ids.is_empty(), "the lost tail assigned ids");
    g.crash(A);
    g.elect_after_promises(C);
    g.restart(A);
    // C resumes: it has seven ids left in the top block, then needs a new one.
    let mut refused_for_block = 0;
    for l in &logins[10..] {
        id += 1;
        match g.submit(id, claim(l), &mut no_deps) {
            Ok(_) => {}
            Err(CommitError::Refused { err: NamesErr::NeedIdBlock, .. }) => {
                refused_for_block += 1;
                let leader = g.leader().unwrap();
                g.id_block(leader, block(22)).unwrap();
                g.settle(&mut no_deps);
                assert!(g.block_committed(block(22)));
                id += 1;
                assert!(g.submit(id, claim(l), &mut no_deps).is_ok());
            }
            Err(e) => panic!("{l}: {e:?}"),
        }
    }
    assert_eq!(refused_for_block, 1, "exactly one block boundary");
    // The committed claims (read on the leader: a follower learns the last commit index only from the next append)
    // hold distinct ids.
    let leader = g.leader().unwrap();
    let ids: Vec<AccountId> = g
        .committed_events(leader)
        .into_iter()
        .filter_map(|ev| match ev {
            NamesEvent::LoginClaimed { account, .. } => Some(account),
            _ => None,
        })
        .collect();
    let unique: BTreeSet<_> = ids.iter().copied().collect();
    assert_eq!(unique.len(), ids.len(), "an id was committed twice: {ids:?}");
    assert_eq!(ids.len(), 35, "five before, thirty after the change");
    // A's lost tail assigned ids that were never acknowledged; the new leader assigned them again to other logins,
    // and no committed id repeats.
    assert!(lost_ids.iter().all(|a| ids.contains(a)), "the lost ids were reassigned, so the block has no gap");
    assert!(ids.iter().any(|a| a.0.offset() == u16::MAX), "the block's last offset was used");
    assert!(ids.iter().any(|a| (a.0.block(), a.0.offset()) == (22, 0)), "the next block started at offset 0");
    // The ids the lost tail held were reassigned (gaps allowed, reuse never means: no id twice in the committed log).
    let lost_set: BTreeSet<_> = lost_ids.iter().copied().collect();
    assert!(lost_set.iter().all(|a| ids.iter().filter(|b| *b == a).count() <= 1));
    // Recording block 22 again is refused at once. Once one more entry commits, every member's cursor agrees.
    assert!(g.id_block(leader, block(22)).is_err());
    g.id_block(leader, block(23)).unwrap();
    g.settle(&mut no_deps);
    assert!(g.submit(9_999, claim(&logins[0]), &mut no_deps).is_err(), "already claimed");
    for s in SITES {
        assert_eq!(g.m(s).group_fold().fold.ids, g.m(leader).group_fold().fold.ids, "{s:?}");
    }
}

// ---- BF-15 through `deps_read` ----

struct Cred {
    user: AccountId,
    tok: OpaqueToken,
    names: NamesFold,
    tokens: TokensFold,
}

impl Cred {
    fn new() -> Cred {
        let user = acct(0, 2, 5);
        let tok = OpaqueToken::new(TokenKind::Pat, *b"IqIMNOZH6zOwIEB4T9A2g4EHMy8Ji4").unwrap();
        let mut names = NamesFold::new(account_shard(user));
        common::entry(
            &mut names,
            &[NamesCmd {
                cred: OPS,
                op: NamesOp::PutAccount { id: user, suspended: false, sessions_not_before: Timestamp(0) },
            }],
            &[],
        );
        let mut tokens = TokensFold::new(tok.shard());
        common::entry(
            &mut tokens,
            &[TokensCmd {
                cred: OPS,
                op: TokensOp::Store {
                    hash: tok.hash(),
                    kind: TokenKind::Pat,
                    account: user,
                    scopes: ScopeSet(1),
                    expires: None,
                },
            }],
            &[],
        );
        Cred { user, tok, names, tokens }
    }

    fn shard(&self) -> CredentialShard {
        CredentialShard {
            account: Some(self.user),
            key: Some(CredentialKey::Token { shard: self.tok.shard(), hash: self.tok.hash() }),
        }
    }

    fn read(&self, c: ChainId) -> IdentityDep {
        match c {
            ChainId::Names(_) => IdentityDep::Names(self.names.clone()),
            ChainId::Tokens(_) => IdentityDep::Tokens(self.tokens.clone()),
            other => panic!("unexpected dep {other}"),
        }
    }
}

fn revoke_or_suspend(c: &mut Cred, suspend: bool) {
    if suspend {
        common::entry(
            &mut c.names,
            &[NamesCmd {
                cred: OPS,
                op: NamesOp::PutAccount { id: c.user, suspended: true, sessions_not_before: Timestamp(0) },
            }],
            &[],
        );
    } else {
        common::entry(&mut c.tokens, &[TokensCmd { cred: OPS, op: TokensOp::Revoke { hash: c.tok.hash() } }], &[]);
    }
}

/// 1,000 commands queued on an owner's `Authz` leader under one credential. The credential is revoked (or the
/// account suspended) while they wait; the leader reads the credential chains once per entry after the batch froze,
/// so every command decided afterwards is refused with 401.
fn queued_then_revoked(suspend: bool) {
    let mut cred = Cred::new();
    let owner = cred.user;
    let mut g: Group<AuthzCmd> = Group::new(ChainId::Authz(owner), AUTHZ_FORMAT, AuthzFold::new(owner));
    g.elect(A);
    let ops = |op| AuthzCmd { cred: OPS, op };
    g.commit(A, 1, ops(AuthzOp::Open { kind: AccountKind::User, enterprise: None })).unwrap();
    g.settle(&mut no_deps);
    g.id_block(A, IdBlock::whole(0, 30).unwrap()).unwrap();
    g.settle(&mut no_deps);
    // One command decided before the revocation succeeds.
    let create = |i: usize| AuthzCmd {
        cred: cred.shard(),
        op: AuthzOp::CreateRepo { name: RepoName::parse(&format!("r{i}")).unwrap() },
    };
    let t = g.commit(A, 3, create(0)).unwrap().ticket;
    g.settle(&mut |c| cred.read(c));
    assert!(matches!(g.result(t), Some(Ok(a)) if matches!(a.out, AuthzOut::Repo { .. })));
    // The next command's entry waits on its dependency read; 999 more queue behind it.
    let mut tickets = vec![g.commit(A, 1_000, create(1)).unwrap().ticket];
    assert_eq!(g.deps.len(), 1, "the leader asked for the credential chains once for the entry");
    for i in 2..=1_000 {
        tickets.push(g.commit(A, 1_000 + i as u128, create(i)).unwrap().ticket);
    }
    // Revoked while they wait.
    revoke_or_suspend(&mut cred, suspend);
    let mut dep_reads = 0;
    let mut read = |c: ChainId| {
        dep_reads += 1;
        cred.read(c)
    };
    g.settle(&mut read);
    let want = if suspend { CredentialRefusal::Suspended(owner) } else { CredentialRefusal::Revoked };
    let mut refused = 0;
    for t in &tickets {
        match g.result(*t) {
            Some(Err(CommitError::Refused { err: AuthzErr::Unauthorized(r), .. })) if *r == want => refused += 1,
            other => panic!("{t:?}: {other:?}"),
        }
    }
    assert_eq!(refused, 1_000);
    // Reads were per entry, not per command: two chains for each entry.
    let entries = g.m(A).log().len();
    eprintln!("{dep_reads} dependency reads for 1,000 commands in {entries} log entries");
    assert!(dep_reads <= 2 * entries, "reads are per entry, two chains each");
    assert!(dep_reads < 20, "{dep_reads}");
    assert_eq!(g.fresh(A).names.len(), 1, "only the first repository exists");
    assert_eq!(g.m(B).role(), Role::Follower);
}

#[test]
fn bf15_revoked_token_with_1000_commands_queued() {
    queued_then_revoked(false);
}

#[test]
fn bf15_suspended_account_with_1000_commands_queued() {
    queued_then_revoked(true);
}

/// A follower whose fold says the login is taken refuses an entry that claims it for another account, so a leader
/// bug cannot commit two owners for one login.
#[test]
fn a_follower_refuses_a_second_owner_for_one_login() {
    let mut g = names_group("octocat");
    g.id_block(A, block(4)).unwrap();
    g.settle(&mut no_deps);
    g.commit(A, 2, claim("octocat")).unwrap();
    g.settle(&mut no_deps);
    // A follower applies an entry once the next append carries its commit index.
    g.id_block(A, block(5)).unwrap();
    g.settle(&mut no_deps);
    let fold = g.m(B).group_fold().fold.clone();
    let mut s = graftyard_raft::Fold::scratch(&fold);
    let bad = NamesEvent::LoginClaimed { key: key("OCTOCAT"), account: acct(0, 4, 9), kind: AccountKind::User };
    assert!(graftyard_raft::Scratch::check(&s, &bad).is_err());
    graftyard_raft::Scratch::apply(&mut s, &NamesEvent::FormerName { key: key("octocat"), account: acct(0, 4, 0) });
    assert!(graftyard_raft::Scratch::check(&s, &bad).is_ok(), "after a release the key is free again");
}
