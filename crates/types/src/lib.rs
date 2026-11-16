//! Shape types shared by every Graftyard script (`design/DESIGN.md`, "Shape"; `design/SHARED.md`).

pub mod chain;
pub mod credential;
pub mod ids;
pub mod log;
pub mod placement;
pub mod registry;
pub mod site;

pub use chain::{ChainId, NameShard, TokenShard};
pub use credential::{CredentialKey, CredentialShard};
pub use ids::{AccountId, DbId, IdBlock, NetworkId, ReleaseId, RepoId, Timestamp};
pub use log::{Digest, Index, ParseUlidError, RequestId, Tier, Ulid};
pub use placement::{
    GroupLoad, LEARNER_ADDR, LEARNER_AT, LEARNER_READS, MAX_LEARNERS, MIN_PAIR_RTT, Member, MemberKind, NEAR_RECHECK,
    NEAR_RTT, PLACE_TRIES, Placement, PlacementChecks, PlacementError, PlacementName, PlacementRecord, Rtt,
    SystemChain, learner_addr, learner_slot, nearest_try, place_learners,
};
pub use registry::{MemberRegistry, REGISTRY_SHARDS, RegistryRow, RowPut};
pub use site::{Jurisdiction, Region, Site, SiteId, SiteLink, SiteSet, SiteTable};
