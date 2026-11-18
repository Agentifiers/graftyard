//! The fold audit's streamed refold against a restore into a store (`decisions/DECISIONS.md`, "`refold-stream`"):
//! from one snapshot through every later entry, the two give the same `fold_digest`, the same `refold_digest` and
//! the same rows in every table, after each entry.

use graftyard_fold::{
    Changes, Key, META, MemStaged, MemStore, Schema, SnapView, SnapshotFrame, Store, Stored, apply, fold_digest,
    refold_digest, restore, stage,
};
use graftyard_git::repo::RepoEvent;
use graftyard_git::tiers::RepoTiers;
use graftyard_raft::LogEntry;
use graftyard_types::{ChainId, Index};

fn all(s: &dyn Store, schema: &Schema) -> Vec<(&'static str, Key, Stored)> {
    let mut out = Vec::new();
    for t in schema.tables {
        for (k, v) in s.scan(t.name, &[], usize::MAX) {
            out.push((t.name, k, v));
        }
    }
    out
}

/// Refolds `schema`'s tier from `frames` (its snapshot at `from`) through `entries` both ways, staged in pages of
/// `page_bytes` and read through a cache of one page, so every read past the current page goes to the staged rows,
/// and compares them after each entry. Returns the most pages one entry read.
pub fn equal_at_every_index(
    t: &RepoTiers,
    schema: Schema,
    chain: ChainId,
    from: Index,
    frames: &[SnapshotFrame],
    entries: &[LogEntry<RepoEvent>],
    page_bytes: usize,
) -> usize {
    let mut want = MemStore::default();
    restore(&mut want, &schema, frames, (chain, from)).expect("the snapshot restores");
    let (pages, meta) = stage(schema, (chain, from), frames.iter().cloned(), page_bytes).expect("the snapshot stages");
    let mut staged = MemStaged::default();
    staged.stage(pages);
    staged.save(Changes::from([((META.to_owned(), Vec::new()), Some(meta))]));
    let mut most = 0;
    for e in entries {
        apply(t, &mut want, schema.tier, e).expect("the entry applies");
        let mut v = SnapView::new(&staged, 1);
        apply(t, &mut v, schema.tier, e).expect("the entry applies to the view");
        most = most.max(v.reset_touched());
        let c = v.take();
        staged.save(c);
        let v = SnapView::new(&staged, 1);
        let at = e.index;
        assert_eq!(fold_digest(&v, schema.tier), fold_digest(&want, schema.tier), "fold_digest at {at:?}");
        assert_eq!(refold_digest(&v, &schema), refold_digest(&want, &schema), "refold_digest at {at:?}");
        assert_eq!(all(&v, &schema), all(&want, &schema), "rows at {at:?}");
        assert_eq!(fold_digest(&v, schema.tier), refold_digest(&v, &schema), "the view's sum is its rows' at {at:?}");
    }
    most
}
