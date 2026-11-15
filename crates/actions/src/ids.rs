//! Actions ids (SHARED, "Actions runs and jobs") and `actions_shard` (DESIGN, Shape, "Identity").

use graftyard_types::{DbId, RepoId};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct RunId(pub DbId);
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct JobId(pub DbId);
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct RunnerId(pub DbId);

/// SHARED's `RunnerSessionId(Ulid)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct RunnerSessionId(pub u128);

/// Actions shards per owner (DESIGN, `actions_shard`).
pub const ACTIONS_SHARDS: u8 = 16;
/// `Owner`-scope runners register on this shard (DESIGN).
pub const OWNER_SCOPE_SHARD: u8 = 0;

/// `hash(repo) % 16`: every run of one repository on one shard, so `/actions/runs` is one query. The hash is SHA-256
/// of the id's 8 big-endian bytes, first byte [inference: DESIGN names no hash].
pub fn actions_shard(repo: RepoId) -> u8 {
    Sha256::digest(repo.0.0.to_be_bytes())[0] % ACTIONS_SHARDS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shards_spread_over_all_16_and_stay_put() {
        let mut seen = [0u32; 16];
        for i in 1..=1_600 {
            seen[actions_shard(RepoId(DbId(i))) as usize] += 1;
        }
        assert!(seen.iter().all(|n| (50..=150).contains(n)), "{seen:?}");
        assert_eq!(actions_shard(RepoId(DbId(65_536))), actions_shard(RepoId(DbId(65_536))));
    }
}
