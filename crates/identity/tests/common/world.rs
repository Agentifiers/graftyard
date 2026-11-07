//! A world of identity groups on real members: every `Names`, `Tokens`, and `Authz` chain a test touches, each three
//! members, with dependencies answered through `fresh` on their own groups, as the leader's host does once per entry.

use std::collections::BTreeMap;

use graftyard_identity::finish::Step;
use graftyard_identity::{
    AUTHZ_FORMAT, AUTHZ_FORMAT_V2, AUTHZ_FORMAT_V3, AuthzCmd, AuthzFold, IDENTITY_FORMAT, IdBlock, IdentityCmd,
    IdentityDep, IdentityFold, NAMES_FORMAT, NAMES_FORMAT_V2, NAMES_FORMAT_V3, NameKey, NamesCmd, NamesFold, NamesOp,
    TOKENS_FORMAT, TOKENS_FORMAT_V2, TOKENS_FORMAT_V3, TokensCmd, TokensFold, name_shard,
};
use graftyard_types::{AccountId, ChainId, CredentialShard, TokenShard};

use super::{A, Group};

pub const OPS: CredentialShard = CredentialShard::NONE;

/// The groups of one install that a test touches, each three real members, and the folds a leader's host reads
/// through `fresh` for `deps`.
#[derive(Default)]
pub struct World {
    pub names: BTreeMap<u16, Group<NamesCmd>>,
    pub tokens: BTreeMap<u16, Group<TokensCmd>>,
    pub authz: BTreeMap<i64, Group<AuthzCmd>>,
    pub identity: BTreeMap<i64, Group<IdentityCmd>>,
    next: u128,
}

pub fn names_chain(c: ChainId) -> u16 {
    match c {
        ChainId::Names(s) => s.0,
        _ => unreachable!(),
    }
}

impl World {
    pub fn names(&mut self, chain: ChainId) -> &mut Group<NamesCmd> {
        let s = names_chain(chain);
        self.names.entry(s).or_insert_with(|| {
            let shard = graftyard_types::NameShard(s);
            let mut g =
                Group::reading(chain, vec![NAMES_FORMAT, NAMES_FORMAT_V2, NAMES_FORMAT_V3], NamesFold::new(shard));
            g.elect(A);
            g.id_block(A, IdBlock::whole(0, 100 + u32::from(s)).unwrap()).unwrap();
            g.settle(&mut |c| panic!("no dependency expected, asked for {c}"));
            g
        })
    }

    pub fn tokens(&mut self, shard: TokenShard) -> &mut Group<TokensCmd> {
        self.tokens.entry(shard.0).or_insert_with(|| {
            let mut g = Group::reading(
                ChainId::Tokens(shard),
                vec![TOKENS_FORMAT, TOKENS_FORMAT_V2, TOKENS_FORMAT_V3],
                TokensFold::new(shard),
            );
            g.elect(A);
            g
        })
    }

    pub fn authz(&mut self, owner: AccountId) -> &mut Group<AuthzCmd> {
        self.authz.entry(owner.0.0).or_insert_with(|| {
            let mut g = Group::reading(
                ChainId::Authz(owner),
                vec![AUTHZ_FORMAT, AUTHZ_FORMAT_V2, AUTHZ_FORMAT_V3],
                AuthzFold::new(owner),
            );
            g.elect(A);
            g
        })
    }

    pub fn identity(&mut self, account: AccountId) -> &mut Group<IdentityCmd> {
        self.identity.entry(account.0.0).or_insert_with(|| {
            let mut g = Group::reading(ChainId::Identity(account), vec![IDENTITY_FORMAT], IdentityFold::new(account));
            g.elect(A);
            g
        })
    }

    pub fn identity_cmd(&mut self, account: AccountId, cmd: IdentityCmd) -> super::Outcome<IdentityCmd> {
        let id = self.id();
        self.identity(account);
        let mut g = self.identity.remove(&account.0.0).unwrap();
        let r = g.submit(id, cmd, &mut |c| self.read(c));
        self.identity.insert(account.0.0, g);
        r
    }

    /// What a leader's host does for `deps`: each chain read once through `fresh` on its own group.
    pub fn read(&mut self, c: ChainId) -> IdentityDep {
        match c {
            ChainId::Names(_) => IdentityDep::Names(self.names(c).fresh_leader()),
            ChainId::Tokens(s) => IdentityDep::Tokens(self.tokens(s).fresh_leader()),
            ChainId::Authz(o) => IdentityDep::Authz(self.authz(o).fresh_leader()),
            other => panic!("unexpected dep {other}"),
        }
    }

    pub fn id(&mut self) -> u128 {
        self.next += 1;
        self.next
    }

    /// Submits on the group of `chain` and settles, reading every dependency through `fresh` on its group.
    pub fn names_cmd(&mut self, chain: ChainId, cmd: NamesCmd) -> super::Outcome<NamesCmd> {
        let id = self.id();
        let mut g = self.names.remove(&names_chain(chain)).unwrap_or_else(|| {
            self.names(chain);
            self.names.remove(&names_chain(chain)).unwrap()
        });
        let r = g.submit(id, cmd, &mut |c| self.read(c));
        self.names.insert(names_chain(chain), g);
        r
    }

    pub fn names_on(&mut self, key: NameKey, op: NamesOp) -> super::Outcome<NamesCmd> {
        self.names_cmd(ChainId::Names(name_shard(&key)), NamesCmd { cred: OPS, op })
    }

    pub fn authz_cmd(&mut self, owner: AccountId, cmd: AuthzCmd) -> super::Outcome<AuthzCmd> {
        let id = self.id();
        self.authz(owner);
        let mut g = self.authz.remove(&owner.0.0).unwrap();
        let r = g.submit(id, cmd, &mut |c| self.read(c));
        self.authz.insert(owner.0.0, g);
        r
    }

    pub fn tokens_cmd(&mut self, cmd: TokensCmd) -> super::Outcome<TokensCmd> {
        let id = self.id();
        let shard = cmd.shard();
        self.tokens(shard);
        let mut g = self.tokens.remove(&shard.0).unwrap();
        let r = g.submit(id, cmd, &mut |c| self.read(c));
        self.tokens.insert(shard.0, g);
        r
    }

    pub fn step(&mut self, s: Step) {
        match s {
            Step::Names(chain, cmd) => {
                self.names_cmd(chain, cmd).expect("finisher step commits");
            }
            Step::Authz(ChainId::Authz(owner), cmd) => {
                self.authz_cmd(owner, *cmd).expect("finisher step commits");
            }
            Step::Identity(ChainId::Identity(a), cmd) => {
                self.identity_cmd(a, cmd).expect("finisher step commits");
            }
            Step::Tokens(_, cmd) => {
                self.tokens_cmd(*cmd).expect("finisher step commits");
            }
            Step::Authz(..) | Step::Identity(..) => unreachable!(),
        }
    }

    pub fn names_fold(&mut self, key: NameKey) -> NamesFold {
        self.names(ChainId::Names(name_shard(&key))).fresh_leader()
    }
}

pub trait FreshLeader<F> {
    fn fresh_leader(&mut self) -> F;
}

impl<C: graftyard_raft::Command> FreshLeader<C::Fold> for Group<C> {
    fn fresh_leader(&mut self) -> C::Fold {
        let s = self.leader().expect("a leader");
        self.fresh(s)
    }
}
