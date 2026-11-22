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
pub mod catchup;
pub mod clone;
pub mod commit_fetch;
pub mod compose;
pub mod export;
pub mod feed;
pub mod group;
pub mod hooks;
pub mod inbox;
pub mod ingest;
pub mod init;
pub mod issue_ops;
pub mod issues;
pub mod key;
pub mod lease;
pub mod lifecycle;
pub mod log_archive;
pub mod ls_refs;
pub mod mem;
pub mod merge;
pub mod merge_cache;
pub mod merge_call;
pub mod pack;
pub mod pkt;
pub mod prsync;
pub mod purge;
pub mod queue;
pub mod queue_consumer;
pub mod refs;
pub mod repo;
pub mod run_checks;
pub mod search;
pub mod secrets;
pub mod sequencer;
pub mod shard;
pub mod smart;
pub mod split;
pub mod standin;
pub mod store;
pub mod sweep;
pub mod task;
pub mod tiers;
pub mod transfer;
pub mod watch;
pub mod workflow_job;

#[cfg(feature = "worker")]
pub mod host;

pub use bucket::{Bucket, ObjectInfo, PutNew, StoreError};
pub use ingest::{
    ChainLog, Head, Load, LogPacks, Oid, catch_up_plan, promotion_inputs, ref_changes, rehydrate_plan, tail_start,
};
pub use key::{ParseKeyError, R2Key};
pub use pack::{
    Archived, Compacted, DurablePack, INLINE_MAX, INLINE_MAX_HOT, Marker, ORPHAN_AFTER, ObjectMeta, PACK_COMMIT_WINDOW,
    PackId, PackIndex, PackRef, Promoted, REQUEST_RETENTION,
};
pub use shard::{
    Caller, CatchUpStep, CatchUps, CatchupAnswer, DISK_USE_PCT, DiskRepo, FANOUT_DOWN, FANOUT_DOWN_FOR, FANOUT_MAX,
    FANOUT_UP, FANOUT_UP_FOR, Fanout, GitCacheShard, HOT_POOL, INACTIVITY, ImageSlot, IngestCursor, KEEP_WARM_MS,
    LEDGER_BEAT, LEDGER_GAP, LOAD_STALE, LoadReports, NetworkLoad, OnCatchUp, POOL_COLD_SKIP_MS, PROMOTE_LAPSE,
    PROMOTE_NEXT_WAIT_S, PROMOTE_WAIT_S, PoolLease, PoolLedger, REGION_RETRIES, RENEW_EVERY, RegionCheck, Renewal,
    SHARDS, STATS_EVERY, Serve, TOUCH_EVERY, ValidateOn, WARM_FOR, caller, catchup_answer, chain_buckets,
    clone_cache_buckets, combine, container_regions, evict_plan, fanout, granted_k, image_for, keep_warm_due,
    may_start, object_name, on_catch_up, owner_share, pool_ask, pool_forward, pool_grants, pool_keep, pool_reserve,
    promote_lapse, promote_moves_on, promote_sites, promote_wait_s, push_validator, read_disk_first, read_on,
    region_check, renewal, serve, shards_for, stats_due, stats_line, touch_due, ui_missing, validate_on,
    validate_status, validator, warm_sites, warm_step, warming_now,
};
pub use store::{
    Copies, PART_SIZE, PackStore, Pipe, PipeReader, Place, PlaceError, PutOutcome, Stage, Streamed, TeeError,
    durable_sites, mark_committed, put_marker,
};
pub use sweep::{
    Action, Alarm, BucketView, CONSUMED_KEEP, CronBudget, Lookup, Plan, RequestRow, RequestSource, SweepAll,
    SweepReport, consumed_deletable, decide, networks, sweep, sweep_all,
};
