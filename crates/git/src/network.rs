//! The network chain (`ChainId::Network`; `design/SHARED.md`, `NetworkEvent`; `build/BUILD-PLAN.md`, AM3-WP2a): a
//! core-tier-only group placed under the root repository owner's `Placement` record. It lists every fork of the
//! network with the repository it was forked from. Slice 2 adds `Takedown`. SHARED's `Compacted` stays on repository
//! chains. The lead approved `ForkAdded { repo, parent }` in place of SHARED's `ForkAdded(RepoId)`.

use std::collections::BTreeMap;

use graftyard_identity::IdentityDep;
use graftyard_identity::audit::NetworkRead;
use graftyard_raft::{Command, Ev, Fold, Format, Record, Scratch, TierSet};
use graftyard_types::{ChainId, CredentialShard, NetworkId, RepoId};
use serde::{Deserialize, Serialize};
use smallvec::{SmallVec, smallvec};

pub const NETWORK_FORMAT: Format = Format { family: 0x21, version: 1 };

/// Wire tags: postcard by position, JSON by name; add a variant only at the end.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum NetworkEvent {
    /// A fork joins the network, forked from `parent`, a member (the root or an earlier fork).
    ForkAdded { repo: RepoId, parent: RepoId },
}

impl Record for NetworkEvent {
    fn encode(&self, out: &mut Vec<u8>) {
        match self {
            NetworkEvent::ForkAdded { repo, parent } => {
                out.push(0);
                graftyard_fold::canon::write(out, &(repo, parent));
            }
        }
    }
}

/// The network chain's fold: its network, and each fork with the repository it was forked from.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkFold {
    pub network: Option<NetworkId>,
    pub forks: BTreeMap<RepoId, RepoId>,
}

impl NetworkFold {
    pub fn new(network: NetworkId) -> NetworkFold {
        NetworkFold { network: Some(network), forks: BTreeMap::new() }
    }

    /// The repository `repo` was forked from; `None` for the root and for a repository not in the network.
    pub fn parent(&self, repo: RepoId) -> Option<RepoId> {
        self.forks.get(&repo).copied()
    }

    /// Whether `repo` is in the network: the root or a listed fork.
    pub fn member(&self, repo: RepoId) -> bool {
        self.network.is_some_and(|n| n.root() == repo) || self.forks.contains_key(&repo)
    }

    /// The direct forks of `parent`, ascending.
    pub fn forks_of(&self, parent: RepoId) -> impl Iterator<Item = RepoId> + '_ {
        self.forks.iter().filter(move |(_, p)| **p == parent).map(|(r, _)| *r)
    }

    /// What the finisher reads of the chain.
    pub fn read(&self) -> NetworkRead {
        NetworkRead { forks: self.forks.clone() }
    }
}

/// The fold with the entry's earlier `ForkAdded`s on top.
pub struct NetworkScratch<'a> {
    fold: &'a NetworkFold,
    added: BTreeMap<RepoId, RepoId>,
}

impl NetworkScratch<'_> {
    pub fn network(&self) -> Option<NetworkId> {
        self.fold.network
    }

    pub fn parent(&self, repo: RepoId) -> Option<RepoId> {
        self.added.get(&repo).copied().or_else(|| self.fold.parent(repo))
    }

    pub fn member(&self, repo: RepoId) -> bool {
        self.added.contains_key(&repo) || self.fold.member(repo)
    }
}

impl Scratch for NetworkScratch<'_> {
    type Event = NetworkEvent;
    fn apply(&mut self, ev: &NetworkEvent) {
        let NetworkEvent::ForkAdded { repo, parent } = ev;
        self.added.insert(*repo, *parent);
    }
    fn check(&self, ev: &NetworkEvent) -> Result<(), Box<str>> {
        let NetworkEvent::ForkAdded { repo, parent } = ev;
        if self.fold.network.is_some_and(|n| n.root() == *repo) {
            return Err(format!("{repo:?} is the network's root").into());
        }
        if let Some(p) = self.parent(*repo).filter(|p| p != parent) {
            return Err(format!("{repo:?} is a fork of {p:?}, the event names {parent:?}").into());
        }
        if !self.member(*parent) {
            return Err(format!("{parent:?} is not in the network").into());
        }
        Ok(())
    }
}

impl Fold for NetworkFold {
    type Event = NetworkEvent;
    type Scratch<'a> = NetworkScratch<'a>;
    fn scratch(&self) -> NetworkScratch<'_> {
        NetworkScratch { fold: self, added: BTreeMap::new() }
    }
    fn apply(&mut self, ev: &NetworkEvent) {
        let NetworkEvent::ForkAdded { repo, parent } = ev;
        self.forks.insert(*repo, *parent);
    }
}

/// Wire tags: JSON by name (`crates/git/tests/wire_tags.golden`). Add a variant only at the end.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum NetworkOp {
    /// The finisher's `NetworkStep::ForkAdded`: list `repo` as a fork of `parent`, idempotent by repository id.
    ForkAdded { repo: RepoId, parent: RepoId },
}

/// A network chain command. Only the finisher sends it, with the operator's credential.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkCmd {
    pub cred: CredentialShard,
    pub op: NetworkOp,
}

/// Whether the command listed a fork; a re-sent one finds it listed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkOut {
    pub added: bool,
}

/// Wire tags: JSON by name (`crates/git/tests/wire_tags.golden`). Add a variant only at the end.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum NetworkErr {
    /// The credential is not the operator's.
    Forbidden,
    /// The parent is neither the root nor a listed fork.
    NotMember,
    /// The repository is already listed as a fork of another parent.
    Conflict,
    /// The fold names no network, or the repository is the network's root.
    Invalid,
}

impl Record for NetworkCmd {
    fn encode(&self, out: &mut Vec<u8>) {
        out.push(0);
        out.push(self.cred.is_operator() as u8);
        graftyard_fold::canon::write(out, &self.op);
    }
}

impl Command for NetworkCmd {
    type Fold = NetworkFold;
    type Dep = IdentityDep;
    type Out = NetworkOut;
    type Err = NetworkErr;

    fn format(&self) -> Format {
        NETWORK_FORMAT
    }
    fn tiers(&self) -> TierSet {
        TierSet::CORE
    }
    fn deps(&self, _fold: &NetworkFold) -> SmallVec<[ChainId; 3]> {
        SmallVec::new()
    }
    fn decide(
        &self,
        fold: &NetworkScratch<'_>,
        _deps: &[(ChainId, IdentityDep)],
    ) -> Result<(SmallVec<[Ev<Self>; 4]>, NetworkOut), NetworkErr> {
        if !self.cred.is_operator() {
            return Err(NetworkErr::Forbidden);
        }
        let network = fold.network().ok_or(NetworkErr::Invalid)?;
        let NetworkOp::ForkAdded { repo, parent } = self.op;
        if repo == network.root() {
            return Err(NetworkErr::Invalid);
        }
        match fold.parent(repo) {
            Some(p) if p == parent => return Ok((SmallVec::new(), NetworkOut { added: false })),
            Some(_) => return Err(NetworkErr::Conflict),
            None => {}
        }
        if !fold.member(parent) {
            return Err(NetworkErr::NotMember);
        }
        Ok((smallvec![NetworkEvent::ForkAdded { repo, parent }], NetworkOut { added: true }))
    }
    fn replay(&self, events: &[NetworkEvent]) -> NetworkOut {
        NetworkOut { added: !events.is_empty() }
    }
}
