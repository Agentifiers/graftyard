//! Who votes and what counts as a quorum under each `GroupConfig` (`design/DESIGN.md`, Shape, "The write path", the
//! quorum rule, and "Chains", `GroupConfig`).

use std::collections::BTreeSet;

use graftyard_types::{MemberKind, Placement, Region, SiteId};
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

/// The members whose acks made a quorum, and how many regions they span. Built only by the commit-advance path from
/// the acks the leader counted; `crate::outcome` puts it in `Acked`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuorumProof {
    sites: SmallVec<[SiteId; 3]>,
    regions: u8,
}

impl QuorumProof {
    pub(crate) fn counted(acked: &BTreeSet<SiteId>, regions: &[(SiteId, Region)]) -> QuorumProof {
        let mut seen: SmallVec<[Region; 3]> = SmallVec::new();
        for s in acked {
            if let Some((_, r)) = regions.iter().find(|(id, _)| id == s)
                && !seen.contains(r)
            {
                seen.push(*r);
            }
        }
        QuorumProof { sites: acked.iter().copied().collect(), regions: seen.len() as u8 }
    }

    pub fn sites(&self) -> &[SiteId] {
        &self.sites
    }

    /// At least 2 unless the config is `Degraded` or `Solo`.
    pub fn regions(&self) -> u8 {
        self.regions
    }
}
