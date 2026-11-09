//! Fold tiers (`build/BUILD-PLAN.md`, M2-WP1; `design/SHARED.md`, "Folds"; `design/DESIGN.md`, Shape, Packs):
//! the tier-generic part of `apply`, `fold_digest`, snapshots with `snap_undo`, restore, the snapshot writer's
//! choice and cadence, snapshot retention inside `RESTORE_WINDOW`, and the compare-and-report core of `fold_audit`.
//!
//! A tier is tables of rows behind `store::Store`, so the same `apply` and digest run on a Durable Object's SQLite in
//! a host and on `MemStore` in tests. Pure: no clock, no I/O, no randomness.

mod archive;
pub mod canon;
mod snapshot;
pub mod store;
mod tier;

pub use archive::{
    ARCHIVE_HEADER, ArchiveError, archive_header, check_archive_header, decode_archive_some, encode_archive_row,
};
pub use snapshot::{
    FrameCheck, LOG_KEEP, LOG_MAX_BYTES, RESTORE_WINDOW, RefoldError, Restore, RestoreCursor, Row, SNAPSHOT_BYTES,
    SNAPSHOT_EVERY, SnapshotCursor, SnapshotError, SnapshotFrame, SnapshotJob, SnapshotTrailer, archive_due,
    archive_due_walk, decode_frames, decode_some, encode_frame, encode_frames, end_snapshot, frames_header,
    refold_segment, restore, snapshot_due, snapshot_writer, snapshots_to_keep,
};
pub use store::{
    Key, META, MemStore, Overlay, Read, Rows, Schema, Store, Stored, TableDef, TierStore, Val, undo_table,
};
pub use tier::{
    AuditReport, DIGESTS, DIGESTS_KEEP, FoldError, Meta, Mismatch, SnapAt, TierDigest, Tiered, add_row, apply,
    digest_at, digest_with, fold_audit_compare, fold_digest, has, meta, refold_digest, seed, tier_bit, try_meta,
};
