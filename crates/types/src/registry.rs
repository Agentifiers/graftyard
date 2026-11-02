//! `MemberRegistry` (`design/DESIGN.md`, Shape, "Release group", the registry paragraph).

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use smallvec::SmallVec;

use crate::chain::ChainId;
use crate::site::SiteId;

/// 64 shards by chain hash per site [guess].
pub const REGISTRY_SHARDS: u8 = 64;

/// One per (site, shard) on each site script. A member writes its row before it stores its first entry, and rewrites
/// it on each config change.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MemberRegistry {
    pub site: SiteId,
    pub shard: u8,
}

impl MemberRegistry {
    pub fn for_chain(site: SiteId, chain: ChainId) -> MemberRegistry {
        let h = Sha256::digest(chain.to_string().as_bytes());
        MemberRegistry { site, shard: h[0] % REGISTRY_SHARDS }
    }

    /// The registry's Durable Object name on its site's registry class.
    pub fn do_name(&self) -> String {
        format!("registry#{}", self.shard)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryRow {
    pub chain: ChainId,
    pub do_name: Box<str>,
    pub generation: u32,
    pub sites: SmallVec<[SiteId; 3]>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{DbId, RepoId};

    #[test]
    fn shard_is_stable_and_spreads() {
        let a = MemberRegistry::for_chain(SiteId(0), ChainId::Repo(RepoId(DbId(1))));
        assert_eq!(a, MemberRegistry::for_chain(SiteId(0), ChainId::Repo(RepoId(DbId(1)))));
        // Same chain, other site: same shard number, so `ReplaceMember` reads one shard per surviving site.
        assert_eq!(a.shard, MemberRegistry::for_chain(SiteId(2), ChainId::Repo(RepoId(DbId(1)))).shard);
        let mut seen = [0u32; REGISTRY_SHARDS as usize];
        for i in 0..6_400 {
            seen[MemberRegistry::for_chain(SiteId(0), ChainId::Repo(RepoId(DbId(i)))).shard as usize] += 1;
        }
        assert!(seen.iter().all(|&n| (50..=160).contains(&n)), "{seen:?}");
    }
}
