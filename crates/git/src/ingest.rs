//! What a shard must fetch to rebuild a network (`rehydrate`) or to catch up behind its `IngestCursor`
//! (`design/DESIGN.md`, Shape, "Packs", `rehydrate`, and "Git cache shards (derived)", `IngestCursor`). Pure: the
//! shard object reads markers from its bucket and entries from the chain's leader, and these functions say which
//! packs to load, in which order, and where the cursor ends.
#![allow(async_fn_in_trait)]

use std::collections::BTreeSet;

use graftyard_types::{ChainId, Index, NetworkId};
use serde::{Deserialize, Serialize};

use crate::bucket::StoreError;
use crate::pack::{Marker, PackId, PackRef, Promoted};
use crate::shard::IngestCursor;

/// A git object id in hex (SHA-1 or SHA-256).
pub type Oid = String;

/// The refs a leader hands a shard, with the chain's applied index (`design/DESIGN.md`, Shape, `CoreRefs`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoreRefs {
    pub chain: ChainId,
    pub applied: Index,
    pub refs: Vec<(String, Oid)>,
}

/// What one committed entry of a member chain says about packs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogPacks {
    pub index: Index,
    /// Packs of its `Refs` and `Merged` events, inline or stored.
    pub packs: Vec<PackRef>,
    pub promoted: Option<Promoted>,
    /// Stored packs a `Compacted` on the network chain consumed.
    pub consumed: Vec<PackId>,
}

/// A member chain's leader, read through `fresh`. Hosted by the network's Raft group later; a stub until then.
pub trait ChainLog {
    async fn core_refs(&self, chain: ChainId) -> Result<CoreRefs, StoreError>;
    /// The first index still in the leader's log. Entries below it are archived and truncated, so a shard whose cursor
    /// is below it cannot catch up from the log and rehydrates instead.
    async fn first_index(&self, chain: ChainId) -> Result<Index, StoreError>;
    /// Committed entries with index in `from..=to`. Fails for an index the leader has archived and truncated.
    async fn entries(&self, chain: ChainId, from: Index, to: Index) -> Result<Vec<LogPacks>, StoreError>;
    /// The chain's last committed `Promoted`, with the index of its entry.
    async fn last_promoted(&self, chain: ChainId) -> Result<Option<(Index, Promoted)>, StoreError>;
    /// Every stored pack the network chain's `Compacted` entries consumed.
    async fn consumed(&self, network: NetworkId) -> Result<BTreeSet<PackId>, StoreError>;
}

/// One pack to load. `order` sorts the load: markers by the index that named them, then the log tail in index order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Load {
    Stored { id: PackId, bytes: u64, checksum: graftyard_types::Digest },
    Inline { index: Index, slot: usize, bytes: Vec<u8> },
}

impl Load {
    pub fn stored_id(&self) -> Option<PackId> {
        match self {
            Load::Stored { id, .. } => Some(*id),
            Load::Inline { .. } => None,
        }
    }
}

/// Rehydrate: every stored pack with a marker, minus the consumed ones, plus every pack the entries after the last
/// promoted range name. A stored pack in both appears once. "After its last `Promoted`" is read as after the promoted
/// range's `to`: inline packs between `to` and the `Promoted` entry itself are not covered by it, and they are still in
/// the log because `Archived` covers only the promoted range.
pub fn rehydrate_plan(markers: &[(PackId, Marker)], consumed: &BTreeSet<PackId>, tail: &[LogPacks]) -> Vec<Load> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    let mut marked: Vec<&(PackId, Marker)> = markers.iter().filter(|(id, _)| !consumed.contains(id)).collect();
    marked.sort_by_key(|(id, m)| (m.index, *id));
    for (id, m) in marked {
        if seen.insert(*id) {
            out.push(Load::Stored { id: *id, bytes: m.bytes, checksum: m.checksum });
        }
    }
    out.extend(tail_loads(tail, consumed, &mut seen));
    out
}

fn tail_loads(tail: &[LogPacks], consumed: &BTreeSet<PackId>, seen: &mut BTreeSet<PackId>) -> Vec<Load> {
    let mut entries: Vec<&LogPacks> = tail.iter().collect();
    entries.sort_by_key(|e| e.index);
    let mut out = Vec::new();
    for e in entries {
        let promoted = e.promoted.iter().map(|p| &p.pack);
        for (slot, p) in e.packs.iter().chain(promoted).enumerate() {
            match p {
                PackRef::Inline(b) => out.push(Load::Inline { index: e.index, slot, bytes: b.clone() }),
                PackRef::Stored { id, checksum, bytes, .. } => {
                    if !consumed.contains(id) && seen.insert(*id) {
                        out.push(Load::Stored { id: *id, bytes: *bytes, checksum: *checksum });
                    }
                }
            }
        }
    }
    out
}

/// The first index a rehydrate reads from the log.
pub fn tail_start(last_promoted: Option<&Promoted>) -> Index {
    Index(last_promoted.map_or(0, |p| p.to.0) + 1)
}

/// Catch-up behind the cursor: the packs entries `cursor + 1 ..= applied` name, in index order.
pub fn catch_up_plan(cursor: &IngestCursor, chain: ChainId, entries: &[LogPacks], applied: Index) -> Vec<Load> {
    let at = cursor.at(chain);
    let tail: Vec<LogPacks> = entries.iter().filter(|e| e.index > at && e.index <= applied).cloned().collect();
    tail_loads(&tail, &BTreeSet::new(), &mut BTreeSet::new())
}

/// The inline packs of `from..=to`, which the home shard packs into one stored pack before `Promoted` commits, and the
/// stored packs of the range, whose markers `Promoted` re-puts first.
pub fn promotion_inputs(entries: &[LogPacks], from: Index, to: Index) -> (Vec<(Index, Vec<u8>)>, Vec<PackRef>) {
    let mut inline = Vec::new();
    let mut stored = Vec::new();
    for e in entries.iter().filter(|e| e.index >= from && e.index <= to) {
        for p in &e.packs {
            match p {
                PackRef::Inline(b) => inline.push((e.index, b.clone())),
                s @ PackRef::Stored { .. } => stored.push(s.clone()),
            }
        }
    }
    (inline, stored)
}

#[cfg(test)]
mod tests {
    use graftyard_types::{DbId, Digest, RepoId, SiteSet, Ulid};

    use super::*;

    const CHAIN: ChainId = ChainId::Repo(RepoId(DbId(1)));

    fn stored(n: u128) -> PackRef {
        PackRef::Stored { id: PackId(Ulid(n)), checksum: Digest([n as u8; 32]), bytes: n as u64, sites: SiteSet(3) }
    }

    fn marker(n: u128, index: u64) -> (PackId, Marker) {
        (
            PackId(Ulid(n)),
            Marker { chain: CHAIN, index: Index(index), checksum: Digest([n as u8; 32]), bytes: n as u64 },
        )
    }

    fn entry(index: u64, packs: Vec<PackRef>) -> LogPacks {
        LogPacks { index: Index(index), packs, promoted: None, consumed: vec![] }
    }

    #[test]
    fn rehydrate_takes_marked_packs_and_the_tail_once_each() {
        // Packs 1 and 2 are marked; pack 3 committed at 12 but its marker write was lost; 2 also appears in the tail.
        let markers = [marker(2, 11), marker(1, 3)];
        let tail = [entry(11, vec![stored(2)]), entry(12, vec![PackRef::Inline(vec![9]), stored(3)])];
        let plan = rehydrate_plan(&markers, &BTreeSet::new(), &tail);
        let ids: Vec<Option<u128>> = plan.iter().map(|l| l.stored_id().map(|p| p.0.0)).collect();
        assert_eq!(ids, vec![Some(1), Some(2), None, Some(3)]);
        // Consumed packs are skipped wherever they appear.
        let consumed = BTreeSet::from([PackId(Ulid(2)), PackId(Ulid(3))]);
        let plan = rehydrate_plan(&markers, &consumed, &tail);
        assert_eq!(plan.iter().filter_map(Load::stored_id).map(|p| p.0.0).collect::<Vec<_>>(), vec![1]);
    }

    #[test]
    fn tail_starts_after_the_promoted_range_and_includes_the_promoted_pack() {
        let p = Promoted { from: Index(1), to: Index(10), pack: stored(50) };
        assert_eq!(tail_start(Some(&p)), Index(11));
        assert_eq!(tail_start(None), Index(1));
        // The promoted entry itself (at 14) names the promoted pack, which a rehydrate must load even if its marker
        // never landed.
        let tail = [LogPacks { index: Index(14), packs: vec![], promoted: Some(p), consumed: vec![] }];
        let plan = rehydrate_plan(&[], &BTreeSet::new(), &tail);
        assert_eq!(plan.iter().filter_map(Load::stored_id).map(|p| p.0.0).collect::<Vec<_>>(), vec![50]);
    }

    #[test]
    fn catch_up_reads_only_past_the_cursor_up_to_applied() {
        let mut c = IngestCursor::new(NetworkId(DbId(1)));
        c.advance(CHAIN, Index(5));
        c.advance(CHAIN, Index(3));
        assert_eq!(c.at(CHAIN), Index(5), "never moves back");
        assert_eq!(c.behind(CHAIN, Index(5)), None);
        assert_eq!(c.behind(CHAIN, Index(8)), Some(Index(6)..=Index(8)));
        let entries: Vec<LogPacks> = (4..=9).map(|i| entry(i, vec![PackRef::Inline(vec![i as u8])])).collect();
        let plan = catch_up_plan(&c, CHAIN, &entries, Index(8));
        let idx: Vec<u64> = plan
            .iter()
            .map(|l| match l {
                Load::Inline { index, .. } => index.0,
                Load::Stored { .. } => 0,
            })
            .collect();
        assert_eq!(idx, vec![6, 7, 8]);
    }

    #[test]
    fn promotion_inputs_split_inline_from_stored_inside_the_range() {
        let entries = [
            entry(1, vec![PackRef::Inline(vec![1])]),
            entry(2, vec![stored(7), PackRef::Inline(vec![2])]),
            entry(3, vec![PackRef::Inline(vec![3])]),
        ];
        let (inline, stored_) = promotion_inputs(&entries, Index(1), Index(2));
        assert_eq!(inline, vec![(Index(1), vec![1]), (Index(2), vec![2])]);
        assert_eq!(stored_, vec![stored(7)]);
    }
}
