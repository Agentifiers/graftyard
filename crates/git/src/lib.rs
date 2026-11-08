//! The pack store and git cache shards (`build/BUILD-PLAN.md`, M1-WP4 and M1-WP5; `design/DESIGN.md`, Shape,
//! "Packs" and "Git cache shards (derived)").
//!
//! Everything that decides is pure and tested here: R2 keys, the two-of-three rule, the sweeper's decisions, the
//! rehydrate and catch-up plans, rendezvous placement, the start cap, the region check, and the inactivity renewal.
//! The `worker` feature adds the Workers host: an R2 `Bucket` and the shard object's container calls, which a site
//! script hosts.

pub mod agent;
pub mod blob;
pub mod bucket;
pub mod clone;
pub mod group;
pub mod ingest;
pub mod issue_ops;
pub mod issues;
pub mod key;
pub mod lease;
pub mod mem;
pub mod merge;
pub mod merge_cache;
pub mod merge_call;
pub mod pack;
pub mod pkt;
pub mod prsync;
pub mod queue;
pub mod queue_consumer;
pub mod refs;
pub mod repo;
pub mod shard;
pub mod smart;
pub mod standin;
pub mod store;
pub mod sweep;
pub mod task;
pub mod tiers;
pub mod transfer;

#[cfg(feature = "worker")]
pub mod host;

pub use bucket::{Bucket, ObjectInfo, PutNew, StoreError};
pub use ingest::{
    ChainLog, CoreRefs, Load, LogPacks, Oid, catch_up_plan, promotion_inputs, rehydrate_plan, tail_start,
};
pub use key::{ParseKeyError, R2Key};
pub use pack::{
    Archived, DurablePack, INLINE_MAX, INLINE_MAX_HOT, Marker, ORPHAN_AFTER, ObjectMeta, PACK_COMMIT_WINDOW, PackId,
    PackIndex, PackRef, Promoted, REQUEST_RETENTION,
};
pub use shard::{
    DISK_USE_PCT, DiskRepo, FANOUT_DOWN, FANOUT_DOWN_FOR, FANOUT_MAX, FANOUT_UP, FANOUT_UP_FOR, Fanout, GitCacheShard,
    HOT_POOL, INACTIVITY, ImageSlot, IngestCursor, NetworkLoad, REGION_RETRIES, RENEW_EVERY, RegionCheck, Renewal,
    SHARDS, WARM_FOR, clone_cache_buckets, container_regions, evict_plan, fanout, image_for, may_start, pool_grants,
    region_check, renewal, shards_for, validator, warm_sites,
};
pub use store::{
    Copies, PART_SIZE, PackStore, Place, PlaceError, PutOutcome, Stage, TeeError, durable_sites, mark_committed,
    put_marker,
};
pub use sweep::{
    Action, Alarm, BucketView, CronBudget, Lookup, Plan, RequestRow, RequestSource, SweepAll, SweepReport, decide,
    networks, sweep, sweep_all,
};
