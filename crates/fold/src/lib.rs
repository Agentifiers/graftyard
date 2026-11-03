//! Fold tiers (`build/BUILD-PLAN.md`, M2-WP1; `design/SHARED.md`, "Folds"; `design/DESIGN.md`, Shape, Packs):
//! the tier-generic part of `apply`, `fold_digest`, snapshots with `snap_undo`, restore, the snapshot writer's
//! choice and cadence, snapshot retention inside `RESTORE_WINDOW`, and the compare-and-report core of `fold_audit`.
//!
//! A tier is tables of rows behind `store::Store`, so the same `apply` and digest run on a Durable Object's SQLite in
//! a host and on `MemStore` in tests. Pure: no clock, no I/O, no randomness.

pub mod canon;
mod snapshot;
pub mod store;
mod tier;

pub use snapshot::{
    RESTORE_WINDOW, Row, SNAPSHOT_BYTES, SNAPSHOT_EVERY, SnapshotError, SnapshotFrame, SnapshotJob, SnapshotTrailer,
    decode_frames, encode_frames, end_snapshot, restore, snapshot_due, snapshot_writer, snapshots_to_keep,
};
pub use store::{Key, MemStore, Overlay, Read, Rows, Schema, Store, Stored, TableDef, Val};
pub use tier::{
    AuditReport, FoldError, Meta, Mismatch, SnapAt, TierDigest, Tiered, apply, fold_audit_compare, fold_digest, has,
    meta, refold_digest, tier_bit,
};
