//! Who votes and what counts as a quorum under each `GroupConfig` (`design/DESIGN.md`, Shape, "The write path", the
//! quorum rule, and "Chains", `GroupConfig`).

use std::collections::BTreeSet;

use graftyard_types::{Index, MemberKind, Placement, Region, SiteId, Timestamp};
use smallvec::SmallVec;

use crate::types::GroupConfig;

/// Every voter of the config: both halves of a `Joint`, and not the `lost` site of a `Degraded`.
pub fn voters(c: &GroupConfig) -> BTreeSet<SiteId> {
    match c {
        GroupConfig::Normal(p) => p.voters().iter().map(|m| m.site).collect(),
        GroupConfig::Degraded { placement, lost } => {
            placement.voters().iter().map(|m| m.site).filter(|s| s != lost).collect()
        }
        GroupConfig::Joint { old, new } => old.voters().iter().chain(new.voters()).map(|m| m.site).collect(),
    }
}

pub fn is_voter(c: &GroupConfig, site: SiteId) -> bool {
    voters(c).contains(&site)
}

fn majority(p: &Placement, yes: &BTreeSet<SiteId>, lost: Option<SiteId>) -> bool {
    let n = p.voters().len();
    let got = p.voters().iter().filter(|m| Some(m.site) != lost && yes.contains(&m.site)).count();
    got > n / 2
}

/// A vote, a lease, or a read-index round: a majority of every voter set, witnesses included, because a witness
/// votes. Under `Joint` that is a majority of both configs.
pub fn election(c: &GroupConfig, yes: &BTreeSet<SiteId>) -> bool {
    match c {
        GroupConfig::Normal(p) => majority(p, yes, None),
        GroupConfig::Degraded { placement, lost } => majority(placement, yes, Some(*lost)),
        GroupConfig::Joint { old, new } => majority(old, yes, None) && majority(new, yes, None),
    }
}

fn stored(p: &Placement, acked: &BTreeSet<SiteId>, lost: Option<SiteId>) -> bool {
    let full: Vec<SiteId> = p.voters().iter().filter(|m| m.kind == MemberKind::Full).map(|m| m.site).collect();
    if lost.is_some() {
        // Degraded: the surviving Full member and the witness, which is every voter but the lost one.
        return p.voters().iter().filter(|m| Some(m.site) != lost).all(|m| acked.contains(&m.site));
    }
    if full.len() == p.voters().len() {
        majority(p, acked, None)
    } else {
        // Normal, 2 Full + W: both Full members; the witness only votes.
        full.iter().all(|s| acked.contains(s))
    }
}

/// The quorum rule for resolving tickets: Normal all Full, a majority (leader + 1 follower of 3); Normal 2 Full + W,
/// both Full members; Degraded, the surviving Full member and the witness; Solo, the member alone; Joint, the rule of
/// each half.
pub fn commit(c: &GroupConfig, acked: &BTreeSet<SiteId>) -> bool {
    match c {
        GroupConfig::Normal(p) => stored(p, acked, None),
        GroupConfig::Degraded { placement, lost } => stored(placement, acked, Some(*lost)),
        GroupConfig::Joint { old, new } => stored(old, acked, None) && stored(new, acked, None),
    }
}

/// The members whose acks made a quorum, and how many regions they span, with the leader's evidence for them: the
/// index the quorum committed, the leader's clock when it committed it, and for each counted site the index the
/// leader knew durable there and when it learned that (for itself, when its own write's `SelfAck` came back). Built
/// only by the commit-advance path from the acks the leader counted; `crate::outcome` puts it in `Acked`.
///
/// The evidence lets a checker prove each acknowledgment from member clocks alone: every counted site holds
/// `index` durably, the counted sites make a quorum, each ack was known at or before `at`, and the host sent the reply
/// at or after `at`, all on the answering leader's clock.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuorumProof {
    sites: SmallVec<[SiteId; 3]>,
    regions: u8,
    leader: SiteId,
    index: Index,
    entry_at: Timestamp,
    at: Timestamp,
    acks: SmallVec<[DurableAck; 3]>,
}

/// One site's durable copy as the leader counted it: the highest index it knew the site stored, and the leader's
/// clock when it learned that.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DurableAck {
    pub site: SiteId,
    pub durable: Index,
    pub at: Timestamp,
}

impl QuorumProof {
    pub(crate) fn counted(
        leader: SiteId,
        index: Index,
        entry_at: Timestamp,
        at: Timestamp,
        acks: &[DurableAck],
        regions: &[(SiteId, Region)],
    ) -> QuorumProof {
        let mut acks: SmallVec<[DurableAck; 3]> = acks.iter().copied().collect();
        acks.sort_by_key(|a| a.site);
        let mut seen: SmallVec<[Region; 3]> = SmallVec::new();
        for a in &acks {
            if let Some((_, r)) = regions.iter().find(|(id, _)| *id == a.site)
                && !seen.contains(r)
            {
                seen.push(*r);
            }
        }
        QuorumProof {
            sites: acks.iter().map(|a| a.site).collect(),
            regions: seen.len() as u8,
            leader,
            index,
            entry_at,
            at,
            acks,
        }
    }

    pub fn sites(&self) -> &[SiteId] {
        &self.sites
    }

    /// At least 2 unless the config is `Degraded` or `Solo`.
    pub fn regions(&self) -> u8 {
        self.regions
    }

    /// The leader that counted the acks, on whose clock every time here is.
    pub fn leader(&self) -> SiteId {
        self.leader
    }

    /// The commit index this proof is for. A duplicate answered from its `request` row carries the leader's latest
    /// proof, whose index is at or above the row's.
    pub fn index(&self) -> Index {
        self.index
    }

    /// The `at` of the entry at `index`: the leader's clock when it built the entry. No follower can have stored the
    /// entry earlier, so each follower's ack comes at least one round trip after it.
    pub fn entry_at(&self) -> Timestamp {
        self.entry_at
    }

    /// The leader's clock when its commit reached `index`.
    pub fn at(&self) -> Timestamp {
        self.at
    }

    /// Each counted site's durable copy, by site.
    pub fn acks(&self) -> &[DurableAck] {
        &self.acks
    }
}
