//! `Credential`, `Principal`, and the leader's per-entry credential check (`design/DESIGN.md`, Shape, "The write
//! path", `CredentialShard`; `design/SHARED.md`, "Identity and tokens").
//!
//! The leader reads each distinct credential shard once per entry, after the command set is frozen, so a revocation
//! or suspension binds every command decided after it. `crates/raft`'s `Command` has no `credential()` yet, so each
//! command lists its credential chains in `deps`, which the leader reads exactly that way: once per entry, through
//! `fresh`, after the batch froze. A credential row on the deciding group's own chain is read from the scratch view
//! instead, so a revocation earlier in the same entry binds the commands after it.

use graftyard_raft::Enc;
use graftyard_types::{AccountId, ChainId, CredentialKey, CredentialShard, NameShard, TokenShard};
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;

use crate::authz::AuthzFold;
use crate::enc;
use crate::names::{NamesFold, NamesRead, account_shard};
use crate::tokens::{ScopeSet, TokenRow, TokensFold, TokensRead, shard_of_hash};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct UserId(pub AccountId);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct OrgId(pub AccountId);

/// SHARED's `Credential`. Milestone 1 authenticates stored tokens only; `Stateless`, `Ssh`, `Session`, `RunnerJwt`,
/// and `RuntimeToken` come with Milestones 3 and 4.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Credential {
    Stored([u8; 32]),
}

/// SHARED's `Actor`, cut to what Milestone 1 authenticates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Actor {
    User(UserId),
    Anonymous,
}

/// The authenticated caller.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Principal {
    pub actor: Actor,
    pub scopes: ScopeSet,
    pub credential: Credential,
    pub sso_orgs: SmallVec<[OrgId; 2]>,
}

impl Principal {
    /// What the leader re-reads for this caller's commands: the actor's `Account` row and the token's shard row.
    pub fn credential_shard(&self) -> CredentialShard {
        let account = match self.actor {
            Actor::User(u) => Some(u.0),
            Actor::Anonymous => None,
        };
        let key = match self.credential {
            Credential::Stored(hash) => Some(CredentialKey::Token { shard: shard_of_hash(&hash), hash }),
        };
        CredentialShard { account, key }
    }
}

/// A fold of another chain, read once per entry through that chain's `fresh`. The design's `deps: &[Fresh]`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum IdentityDep {
    Names(NamesFold),
    Tokens(TokensFold),
    Authz(AuthzFold),
}

/// Why the leader refused a command with 401.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CredentialRefusal {
    NoAccount(AccountId),
    Suspended(AccountId),
    NoToken,
    Revoked,
    /// The token belongs to another account than the command's actor.
    WrongAccount,
    /// The token's key names a shard its hash does not map to.
    WrongShard,
    /// The host did not pass the chain's read: fail closed.
    NotRead(ChainId),
}

/// The deciding group's own credential rows, when the group is a `Names` or `Tokens` shard.
#[derive(Clone, Copy)]
pub enum Own<'a> {
    Names(NameShard, &'a dyn NamesRead),
    Tokens(TokenShard, &'a dyn TokensRead),
    Other,
}

impl Own<'_> {
    fn chain(&self) -> Option<ChainId> {
        match self {
            Own::Names(s, _) => Some(ChainId::Names(*s)),
            Own::Tokens(s, _) => Some(ChainId::Tokens(*s)),
            Own::Other => None,
        }
    }
}

fn account_chain(a: AccountId) -> ChainId {
    ChainId::Names(account_shard(a))
}

/// The chains a credential is read from, other than the deciding group's own, for `deps`.
pub fn credential_chains(cred: &CredentialShard, own: Option<ChainId>) -> SmallVec<[ChainId; 3]> {
    let mut out = SmallVec::new();
    let mut push = |c: ChainId| {
        if Some(c) != own && !out.contains(&c) {
            out.push(c);
        }
    };
    if let Some(a) = cred.account {
        push(account_chain(a));
    }
    if let Some(CredentialKey::Token { shard, .. }) = cred.key {
        push(ChainId::Tokens(shard));
    }
    out
}

fn dep(deps: &[(ChainId, IdentityDep)], chain: ChainId) -> Option<&IdentityDep> {
    deps.iter().find(|(c, _)| *c == chain).map(|(_, d)| d)
}

/// The `Account` row of `a`, from the own scratch view or the deps.
pub(crate) fn account_row(
    a: AccountId,
    own: Own<'_>,
    deps: &[(ChainId, IdentityDep)],
) -> Result<Option<crate::names::AccountRow>, CredentialRefusal> {
    let chain = account_chain(a);
    match own {
        Own::Names(s, r) if ChainId::Names(s) == chain => Ok(r.account(a)),
        _ => match dep(deps, chain) {
            Some(IdentityDep::Names(f)) => Ok(f.account(a)),
            _ => Err(CredentialRefusal::NotRead(chain)),
        },
    }
}

fn token_row(
    shard: TokenShard,
    hash: &[u8; 32],
    own: Own<'_>,
    deps: &[(ChainId, IdentityDep)],
) -> Result<Option<TokenRow>, CredentialRefusal> {
    let chain = ChainId::Tokens(shard);
    match own {
        Own::Tokens(s, r) if s == shard => Ok(r.token(hash)),
        _ => match dep(deps, chain) {
            Some(IdentityDep::Tokens(f)) => Ok(f.token(hash)),
            _ => Err(CredentialRefusal::NotRead(chain)),
        },
    }
}

/// The 401 rule: a missing, revoked, or suspended row refuses the command. A command that names neither row was
/// submitted by `graft-ops` or a consumer and passes. Expiry is not checked here: `decide` sees no clock, so the
/// edge checks `expires` when it authenticates (`tests/m1/results/m1-wp7.md`).
pub fn check_credential(
    cred: &CredentialShard,
    own: Own<'_>,
    deps: &[(ChainId, IdentityDep)],
) -> Result<(), CredentialRefusal> {
    debug_assert!(own.chain().is_none_or(|c| !deps.iter().any(|(d, _)| *d == c)), "own chain passed as a dep");
    if let Some(a) = cred.account {
        match account_row(a, own, deps)? {
            None => return Err(CredentialRefusal::NoAccount(a)),
            Some(r) if r.suspended => return Err(CredentialRefusal::Suspended(a)),
            Some(_) => {}
        }
    }
    if let Some(CredentialKey::Token { shard, hash }) = cred.key {
        if shard_of_hash(&hash) != shard {
            return Err(CredentialRefusal::WrongShard);
        }
        match token_row(shard, &hash, own, deps)? {
            None => return Err(CredentialRefusal::NoToken),
            Some(t) if t.revoked => return Err(CredentialRefusal::Revoked),
            Some(t) if Some(t.account) != cred.account => return Err(CredentialRefusal::WrongAccount),
            Some(_) => {}
        }
    }
    Ok(())
}

pub(crate) fn encode_cred(e: &mut Enc, c: &CredentialShard) {
    e.option(c.account.as_ref(), |e, a| enc::account(e, *a));
    e.option(c.key.as_ref(), |e, k| match k {
        CredentialKey::Token { shard, hash } => {
            e.u8(0);
            e.u16(shard.0);
            e.0.extend_from_slice(hash);
        }
    });
}
