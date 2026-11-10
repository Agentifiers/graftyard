//! What a shard must fetch to rebuild a network (`rehydrate`) or to catch up behind its `IngestCursor`
//! (`design/DESIGN.md`, Shape, "Packs", `rehydrate`, and "Git cache shards (derived)", `IngestCursor`). Pure: the
//! shard object reads markers from its bucket and entries from the chain's leader, and these functions say which
//! packs to load, in which order, and where the cursor ends.
#![allow(async_fn_in_trait)]

use std::collections::BTreeSet;

use graftyard_types::{ChainId, Index, NetworkId};
use serde::{Deserialize, Serialize};

use crate::bucket::StoreError;
use crate::group::RefsPage;
use crate::pack::{Marker, PackId, PackRef, Promoted};
use crate::shard::IngestCursor;

/// A git object id in hex (SHA-1 or SHA-256).
pub type Oid = String;

/// A chain's first log index and its leader's applied index, from one read (`ChainLog::head`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Head {
    pub first: Index,
    pub applied: Index,
}

/// What one committed entry of a member chain says about packs and refs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogPacks {
    pub index: Index,
    /// Packs of its `Refs` and `Merged` events, inline or stored.
    pub packs: Vec<PackRef>,
    pub promoted: Option<Promoted>,
    /// Stored packs a `Compacted` on the network chain consumed.
    pub consumed: Vec<PackId>,
    /// Every ref the entry sets (`Some`) or deletes (`None`), in order (`RepoEvent::ref_changes`). `None` from a member
    /// whose release does not send them, so a catch-up over such an entry rehydrates instead.
    #[serde(default)]
    pub refs: Option<Vec<(String, Option<Oid>)>>,
}

/// A member chain's leader, read through `fresh`. No read answers a chain's whole ref set at once: catch-up takes ref
/// changes from the entries it reads, and only `rehydrate` reads the refs, a page at a time (`crate::group::refs_read`).
pub trait ChainLog {
    /// The first index still in the leader's log, and its applied index. Entries below the first are archived and
    /// truncated, so a shard whose cursor is below it cannot catch up from the log and rehydrates instead.
    async fn head(&self, chain: ChainId) -> Result<Head, StoreError>;
    /// One page of the chain's refs after `after` (`RefsPage::next` of the page before), with the index it was read at.
    async fn refs_page(&self, chain: ChainId, after: Option<String>) -> Result<(RefsPage, Index), StoreError>;
    /// Committed entries with index in `from..=to`. Fails for an index the leader has archived and truncated. A read
    /// routed to a newly elected leader may instead return a short range; consumers check it before applying anything.
    async fn entries(&self, chain: ChainId, from: Index, to: Index) -> Result<Vec<LogPacks>, StoreError>;
    /// The chain's last committed `Promoted`, with the index of its entry.
    async fn last_promoted(&self, chain: ChainId) -> Result<Option<(Index, Promoted)>, StoreError>;
    /// Every stored pack the network chain's `Compacted` entries consumed.
    async fn consumed(&self, network: NetworkId) -> Result<BTreeSet<PackId>, StoreError>;
}

/// Requires one entry at every index of `from..=to`, in order, before any of the range is applied.
pub fn require_complete_entries(
    chain: ChainId,
    from: Index,
    to: Index,
    entries: &[LogPacks],
) -> Result<(), StoreError> {
    if entries.iter().map(|e| e.index.0).eq(from.0..=to.0) {
        return Ok(());
    }
    let received: Vec<u64> = entries.iter().map(|e| e.index.0).collect();
    Err(StoreError(format!("{chain}: incomplete log entries {}..={}; received indices {received:?}", from.0, to.0)))
}

/// Each chain's head, every read sent to its leader at once and awaited together, in `chains` order. A catch-up needs
/// them all before it can tell whether to rehydrate; read one after another, they cost a leader round trip per chain,
/// across regions once leadership moved (`tests/m2/results/m2-wp6.md`, "Merge latency").
pub async fn heads<L: ChainLog>(log: &L, chains: &[ChainId]) -> Vec<Result<Head, StoreError>> {
    futures_util::future::join_all(chains.iter().map(|c| log.head(*c))).await
}

/// The net ref changes of the entries in `after + 1 ..= through`, applied in index order: each ref changed there once,
/// with its last value, deletes first and then sets, each in name order. `None` when such an entry does not carry its
/// ref changes, so the caller rehydrates.
pub fn ref_changes(entries: &[LogPacks], after: Index, through: Index) -> Option<Vec<(String, Option<Oid>)>> {
    let mut es: Vec<&LogPacks> = entries.iter().filter(|e| e.index > after && e.index <= through).collect();
    es.sort_by_key(|e| e.index);
    let mut net: std::collections::BTreeMap<&str, Option<&Oid>> = std::collections::BTreeMap::new();
    for e in es {
        for (name, oid) in e.refs.as_ref()? {
            net.insert(name, oid.as_ref());
        }
    }
    let (dels, sets): (Vec<_>, Vec<_>) = net.into_iter().partition(|(_, o)| o.is_none());
    Some(dels.into_iter().chain(sets).map(|(n, o)| (n.to_owned(), o.cloned())).collect())
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
        LogPacks { index: Index(index), packs, promoted: None, consumed: vec![], refs: None }
    }

    /// A log whose reads end only once every read of one `heads` call has started.
    struct Gate {
        started: std::cell::Cell<usize>,
        want: usize,
    }

    impl Gate {
        async fn wait(&self) {
            self.started.set(self.started.get() + 1);
            std::future::poll_fn(|_| {
                if self.started.get() >= self.want { std::task::Poll::Ready(()) } else { std::task::Poll::Pending }
            })
            .await
        }
    }

    impl ChainLog for Gate {
        async fn head(&self, _: ChainId) -> Result<Head, StoreError> {
            self.wait().await;
            Ok(Head { first: Index(1), applied: Index(7) })
        }
        async fn refs_page(&self, _: ChainId, _: Option<String>) -> Result<(RefsPage, Index), StoreError> {
            unreachable!()
        }
        async fn entries(&self, _: ChainId, _: Index, _: Index) -> Result<Vec<LogPacks>, StoreError> {
            unreachable!()
        }
        async fn last_promoted(&self, _: ChainId) -> Result<Option<(Index, Promoted)>, StoreError> {
            unreachable!()
        }
        async fn consumed(&self, _: NetworkId) -> Result<BTreeSet<PackId>, StoreError> {
            unreachable!()
        }
    }

    /// A catch-up's reads of its chains' heads are all in flight together: here no read ends until both started, so
    /// reads made one after another never end.
    #[test]
    fn a_catch_up_reads_every_head_at_once() {
        let other = ChainId::Repo(RepoId(DbId(2)));
        let log = Gate { started: std::cell::Cell::new(0), want: 2 };
        let chains = [CHAIN, other];
        let mut f = std::pin::pin!(heads(&log, &chains));
        let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
        let mut out = None;
        for _ in 0..2 {
            if let std::task::Poll::Ready(v) = f.as_mut().poll(&mut cx) {
                out = Some(v);
                break;
            }
        }
        let out = out.expect("both reads were in flight together");
        assert!(out.iter().all(|h| *h.as_ref().unwrap() == Head { first: Index(1), applied: Index(7) }));
    }

    #[test]
    fn ref_changes_are_the_net_change_of_the_entries_in_range() {
        let oid = |c: char| c.to_string().repeat(40);
        let e = |i: u64, refs: Vec<(&str, Option<String>)>| LogPacks {
            refs: Some(refs.into_iter().map(|(n, o)| (n.to_owned(), o)).collect()),
            ..entry(i, vec![])
        };
        let entries = [
            e(3, vec![("refs/heads/old", Some(oid('1')))]),
            e(4, vec![("refs/heads/a", Some(oid('1'))), ("refs/heads/b", Some(oid('2')))]),
            e(5, vec![("refs/heads/a", Some(oid('3'))), ("refs/heads/b", None), ("refs/heads/c", None)]),
        ];
        // 3 is at or below the cursor; a's last value wins; b and c are deleted, before any set.
        assert_eq!(
            ref_changes(&entries, Index(3), Index(5)).unwrap(),
            vec![
                ("refs/heads/b".to_owned(), None),
                ("refs/heads/c".to_owned(), None),
                ("refs/heads/a".to_owned(), Some(oid('3')))
            ]
        );
        // An entry from a release that sends no ref changes: the caller rehydrates.
        let mut old = entries.to_vec();
        old[1].refs = None;
        assert_eq!(ref_changes(&old, Index(3), Index(5)), None);
        assert_eq!(ref_changes(&old, Index(4), Index(5)).map(|v| v.len()), Some(3));
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
        let tail = [LogPacks { index: Index(14), packs: vec![], promoted: Some(p), consumed: vec![], refs: None }];
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

    /// A newly elected leader can answer a short page (`ChainLog::entries`). Only an exact `from..=to` run passes:
    /// a catch-up that applied a short page and advanced to `to` skipped those entries' packs and refs for good
    /// (`tests/am1/results/b0-30.md`, blocker 12).
    #[test]
    fn only_a_complete_entries_range_passes() {
        let range = |ix: &[u64]| ix.iter().map(|i| entry(*i, vec![])).collect::<Vec<_>>();
        let ok = |ix: &[u64]| require_complete_entries(CHAIN, Index(5), Index(8), &range(ix)).is_ok();
        assert!(ok(&[5, 6, 7, 8]));
        assert!(!ok(&[]), "the lagging leader's empty page");
        assert!(!ok(&[5, 6]), "a page short at the end");
        assert!(!ok(&[6, 7, 8]), "a page short at the start");
        assert!(!ok(&[5, 7, 8]), "a hole");
        assert!(!ok(&[5, 6, 6, 7, 8]), "a repeat");
        assert!(!ok(&[5, 6, 8, 7]), "out of order");
        assert!(!ok(&[5, 6, 7, 8, 9]), "past the range");
        let e = require_complete_entries(CHAIN, Index(5), Index(8), &range(&[5, 6])).unwrap_err();
        assert!(e.0.contains("5..=8") && e.0.contains("[5, 6]"), "{}", e.0);
    }
}
