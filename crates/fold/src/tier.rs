//! `apply` and `fold_digest` for one tier (`design/SHARED.md`, "Folds").
//!
//! Each tier keeps its own `Meta` row: `applied`, the term and digest of entry `applied`, the running row-hash sum, and
//! the index of a snapshot in progress. `apply` refuses an entry that does not follow (`index != applied + 1` or a wrong
//! `prev`), runs the tier's writes for the events the tier owns, and advances `applied` past every entry, owned or not,
//! so both tiers check the same digest chain.
//!
//! `fold_digest` is SHA-256 of the tier, `applied`, that entry's term and digest, and the sum mod 2^256 of SHA-256 of
//! every row (table, key, the index that wrote it, columns). A sum does not depend on the order rows were written or
//! are read, so members agree whatever their storage order, and `apply` keeps it current with one subtract and one add
//! per write. `refold_digest` recomputes the sum from the rows, which is what an audit and a snapshot writer check.

use graftyard_raft::{EntryBody, LogEntry, Record, Term, TierSet, digest};
use graftyard_types::{Digest, Index, SiteId, Tier, Timestamp};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::canon;
use crate::store::{Key, META, Read, Rows, Schema, Store, Stored, Val, undo_table};

/// The tier's own row.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Meta {
    pub applied: Index,
    pub term: Term,
    /// The time of entry `applied`.
    pub at: Timestamp,
    /// The digest of entry `applied`, which entry `applied + 1` names as `prev`. Zero before index 1.
    pub last: Digest,
    /// Sum mod 2^256 of every row's hash.
    pub sum: [u8; 32],
    /// The snapshot being written, while `snap_undo` copies are kept.
    pub snap: Option<SnapAt>,
    /// `sum` when the running snapshot began: the tier's digest at its `to`.
    pub snap_sum: [u8; 32],
}

/// Where a running snapshot stands: entry `to`, its term, and its digest, captured when the snapshot began.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapAt {
    pub to: Index,
    pub term: Term,
    pub entry: Digest,
    /// Entry `to`'s time, which the snapshot's `Archived.to_at` carries for retention.
    pub at: Timestamp,
}

pub fn meta(store: &dyn Store) -> Meta {
    match store.get(META, &[]) {
        Some(s) => match s.cols.first() {
            Some(Val::Blob(b)) => postcard::from_bytes(b).expect("the tier's own meta row"),
            _ => panic!("meta row has no blob"),
        },
        None => Meta {
            applied: Index(0),
            term: Term(0),
            at: Timestamp(0),
            last: graftyard_raft::PREV_ZERO,
            sum: [0; 32],
            snap: None,
            snap_sum: [0; 32],
        },
    }
}

pub(crate) fn save_meta(store: &mut dyn Store, m: &Meta) {
    let b = postcard::to_allocvec(m).expect("meta encodes");
    store.put(META, Vec::new(), Stored { written: m.applied, cols: vec![Val::Blob(b.into())] });
}

/// A tier's bit in an entry's `TierSet` tag.
pub fn tier_bit(t: Tier) -> TierSet {
    match t {
        Tier::Core => TierSet::CORE,
        Tier::Metadata => TierSet::METADATA,
    }
}

pub fn has(set: TierSet, t: Tier) -> bool {
    set.0 & tier_bit(t).0 != 0
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum FoldError {
    #[error("entry {got:?} where {want:?} was expected")]
    Index { got: Index, want: Index },
    #[error("entry {index:?} does not chain to the digest of entry {applied:?}")]
    Prev { index: Index, applied: Index },
    /// An event that writes a tier the entry's tag leaves out: the tier the tag skips would never apply it.
    #[error("entry {index:?} is tagged {tag:?} but event {event} needs {needs:?}")]
    Tag { index: Index, tag: TierSet, event: usize, needs: TierSet },
}

/// The chain-specific part of a tier fold: which tiers each event touches, and what it writes to one tier's rows. A
/// value holds what every member of the chain knows without the log, such as the chain's own repository id.
pub trait Tiered {
    type Event: Record;

    fn tier_of(ev: &Self::Event) -> TierSet;
    fn schema(tier: Tier) -> Schema;
    /// The rows `ev` writes in `tier`, given the tier as it stands. `at` is the entry's time; nothing else about time
    /// is available, so every member writes the same rows.
    fn write(&self, ev: &Self::Event, tier: Tier, at: Timestamp, rows: &mut dyn Rows);
    /// Runs in every tier for every entry, owned or not, before its events: time-based pruning.
    fn on_entry(&self, _tier: Tier, _at: Timestamp, _rows: &mut dyn Rows) {}
    /// An `EntryBody::IdBlock`, in every tier whatever the entry's tag: the leader tags them `CORE`
    /// (`graftyard_raft`), and each tier that assigns ids keeps its own cursor over the group's blocks.
    fn id_block(&self, _block: graftyard_types::IdBlock, _tier: Tier, _rows: &mut dyn Rows) {}
}

/// The rows `apply` writes through: it records which entry wrote each row, keeps the row-hash sum, and makes the
/// `snap_undo` copy of a row last written at or below a running snapshot's `to` the first time it changes.
pub struct Writer<'a> {
    store: &'a mut dyn Store,
    schema: Schema,
    index: Index,
    meta: Meta,
}

impl Writer<'_> {
    fn known(&self, table: &str) -> bool {
        self.schema.tables.iter().any(|t| t.name == table)
    }

    fn before_change(&mut self, table: &str, key: &[Val]) -> Option<Stored> {
        assert!(self.known(table), "table {table} is not in the {:?} tier", self.schema.tier);
        let old = self.store.get(table, key)?;
        sub(&mut self.meta.sum, &row_hash(table, key, &old));
        if let Some(s) = self.meta.snap
            && old.written <= s.to
        {
            let undo = undo_table(table);
            if self.store.get(&undo, key).is_none() {
                self.store.put(&undo, key.to_vec(), old.clone());
            }
        }
        Some(old)
    }
}

impl Read for Writer<'_> {
    fn get(&self, table: &str, key: &[Val]) -> Option<Vec<Val>> {
        assert!(self.known(table), "table {table} is not in the {:?} tier", self.schema.tier);
        self.store.get(table, key).map(|s| s.cols)
    }
    fn scan(&self, table: &str, from: &[Val], limit: usize) -> Vec<(Key, Vec<Val>)> {
        assert!(self.known(table), "table {table} is not in the {:?} tier", self.schema.tier);
        self.store.scan(table, from, limit).into_iter().map(|(k, s)| (k, s.cols)).collect()
    }
}

impl Rows for Writer<'_> {
    fn put(&mut self, table: &str, key: Key, cols: Vec<Val>) {
        self.before_change(table, &key);
        let row = Stored { written: self.index, cols };
        add(&mut self.meta.sum, &row_hash(table, &key, &row));
        self.store.put(table, key, row);
    }
    fn delete(&mut self, table: &str, key: &[Val]) {
        if self.before_change(table, key).is_some() {
            self.store.delete(table, key);
        }
    }
}

/// Reads a store's committed rows, for `decide`'s overlay and for tests.
impl<S: Store + ?Sized> Read for S {
    fn get(&self, table: &str, key: &[Val]) -> Option<Vec<Val>> {
        Store::get(self, table, key).map(|s| s.cols)
    }
    fn scan(&self, table: &str, from: &[Val], limit: usize) -> Vec<(Key, Vec<Val>)> {
        Store::scan(self, table, from, limit).into_iter().map(|(k, s)| (k, s.cols)).collect()
    }
}

/// The only writer of a tier (`design/SHARED.md`, "Folds"). Refuses an entry whose index is not `applied + 1` or
/// whose `prev` is not the digest of entry `applied`, and one whose tag leaves out a tier one of its events needs,
/// before it writes anything. Otherwise it runs `on_entry`, then, if the tag names the tier, the writes of each event
/// the tier owns, and advances `applied` to the entry.
pub fn apply<T: Tiered>(t: &T, store: &mut dyn Store, tier: Tier, e: &LogEntry<T::Event>) -> Result<(), FoldError> {
    let m = meta(store);
    let want = Index(m.applied.0 + 1);
    if e.index != want {
        return Err(FoldError::Index { got: e.index, want });
    }
    if e.prev != m.last {
        return Err(FoldError::Prev { index: e.index, applied: m.applied });
    }
    let mine = has(e.tiers, tier);
    if let EntryBody::Events(evs) = &e.body {
        for (i, ev) in evs.iter().enumerate() {
            let needs = T::tier_of(ev);
            if needs.0 & !e.tiers.0 != 0 {
                return Err(FoldError::Tag { index: e.index, tag: e.tiers, event: i, needs });
            }
        }
    }
    let mut w = Writer { store, schema: T::schema(tier), index: e.index, meta: m };
    t.on_entry(tier, e.at, &mut w);
    match &e.body {
        EntryBody::Events(evs) if mine => {
            for ev in evs.iter().filter(|ev| has(T::tier_of(ev), tier)) {
                t.write(ev, tier, e.at, &mut w);
            }
        }
        EntryBody::IdBlock(b) => t.id_block(*b, tier, &mut w),
        _ => {}
    }
    let mut m = w.meta;
    m.applied = e.index;
    m.term = e.term;
    m.at = e.at;
    m.last = digest(e);
    save_meta(store, &m);
    let d = digest_of(&TierDigest { tier, applied: m.applied, term: m.term, last: m.last }, &m.sum);
    store.put(
        DIGESTS,
        vec![Val::Int(e.index.0 as i64)],
        Stored { written: e.index, cols: vec![Val::Blob(d.0.to_vec().into())] },
    );
    if let Some(old) = e.index.0.checked_sub(DIGESTS_KEEP) {
        store.delete(DIGESTS, &[Val::Int(old as i64)]);
    }
    Ok(())
}

/// The tier's own table of its `fold_digest` after each of the last `DIGESTS_KEEP` entries it applied, so an audit
/// compares members at one index whatever batches they applied in. Outside the schema: not in the digest, not in a
/// snapshot.
pub const DIGESTS: &str = "_digest";
/// Entries whose digests a tier keeps [guess].
pub const DIGESTS_KEEP: u64 = 20_000;

/// The tier's `fold_digest` after entry `index`, while it is among the last `DIGESTS_KEEP`.
pub fn digest_at(store: &dyn Store, index: Index) -> Option<Digest> {
    match store.get(DIGESTS, &[Val::Int(index.0 as i64)])?.cols.first() {
        Some(Val::Blob(b)) => Some(Digest(b[..].try_into().ok()?)),
        _ => None,
    }
}

/// Test installs only: puts `rows` into `table` of a tier stamped `written`, at or below the tier's `applied`, and
/// keeps the row-hash sum, as `apply` would. The rows come from no entry, so a refold of the log does not reproduce
/// them; every member that seeds the same rows at the same `written` keeps the same `fold_digest`.
pub fn seed(store: &mut dyn Store, schema: &Schema, table: &str, written: Index, rows: Vec<(Key, Vec<Val>)>) {
    assert!(schema.tables.iter().any(|t| t.name == table), "table {table} is not in the {:?} tier", schema.tier);
    let mut m = meta(store);
    assert!(written <= m.applied && m.snap.is_none(), "seed at or below applied, with no snapshot running");
    for (key, cols) in rows {
        if let Some(old) = store.get(table, &key) {
            sub(&mut m.sum, &row_hash(table, &key, &old));
        }
        let row = Stored { written, cols };
        add(&mut m.sum, &row_hash(table, &key, &row));
        store.put(table, key, row);
    }
    save_meta(store, &m);
}

/// What `fold_digest` covers besides the rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TierDigest {
    pub tier: Tier,
    pub applied: Index,
    pub term: Term,
    pub last: Digest,
}

pub(crate) fn digest_of(d: &TierDigest, sum: &[u8; 32]) -> Digest {
    let mut h = Sha256::new();
    h.update(b"graftyard/fold/1");
    h.update(canon::to_vec(d));
    h.update(sum);
    Digest(h.finalize().into())
}

/// The tier's digest at `applied`, from the sum `apply` keeps.
pub fn fold_digest(store: &dyn Store, tier: Tier) -> Digest {
    let m = meta(store);
    digest_of(&TierDigest { tier, applied: m.applied, term: m.term, last: m.last }, &m.sum)
}

/// Adds one row's hash to a running sum, as a refold that reads a tier in pages does.
pub fn add_row(sum: &mut [u8; 32], table: &str, key: &[Val], row: &Stored) {
    add(sum, &row_hash(table, key, row));
}

/// A tier's digest with `meta`'s `applied`, term, and entry digest, and the row-hash sum `sum`.
pub fn digest_with(schema: &Schema, meta: &Meta, sum: &[u8; 32]) -> Digest {
    digest_of(&TierDigest { tier: schema.tier, applied: meta.applied, term: meta.term, last: meta.last }, sum)
}

/// The same digest with the sum recomputed from every row, as an audit or a restored snapshot checks it.
pub fn refold_digest(store: &dyn Store, schema: &Schema) -> Digest {
    let m = meta(store);
    let mut sum = [0u8; 32];
    for t in schema.tables {
        let mut from: Key = Vec::new();
        let mut first = true;
        loop {
            let batch = store.scan(t.name, &from, 512);
            let n = batch.len();
            for (k, row) in batch {
                if !first && k == from {
                    continue;
                }
                add(&mut sum, &row_hash(t.name, &k, &row));
                from = k;
            }
            first = false;
            if n < 512 {
                break;
            }
        }
    }
    digest_of(&TierDigest { tier: schema.tier, applied: m.applied, term: m.term, last: m.last }, &sum)
}

pub(crate) fn row_hash(table: &str, key: &[Val], row: &Stored) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(canon::to_vec(&(table, key, row.written.0, &row.cols)));
    h.finalize().into()
}

pub(crate) fn add(sum: &mut [u8; 32], x: &[u8; 32]) {
    let mut carry = 0u16;
    for i in (0..32).rev() {
        let v = u16::from(sum[i]) + u16::from(x[i]) + carry;
        sum[i] = v as u8;
        carry = v >> 8;
    }
}

fn sub(sum: &mut [u8; 32], x: &[u8; 32]) {
    let mut borrow = 0i16;
    for i in (0..32).rev() {
        let mut v = i16::from(sum[i]) - i16::from(x[i]) - borrow;
        borrow = 0;
        if v < 0 {
            v += 256;
            borrow = 1;
        }
        sum[i] = v as u8;
    }
}

/// One member's report for an audit: its digest of a tier at an index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mismatch {
    pub member: SiteId,
    pub got: Digest,
}

/// `fold_audit`'s compare-and-report core: the refold's digest at `index` against each member's digest of the same
/// tier at the same index. Any mismatch alarms.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuditReport {
    pub tier: Tier,
    pub index: Index,
    pub want: Digest,
    pub mismatches: Vec<Mismatch>,
}

impl AuditReport {
    pub fn alarm(&self) -> bool {
        !self.mismatches.is_empty()
    }
}

pub fn fold_audit_compare(tier: Tier, index: Index, refold: Digest, members: &[(SiteId, Digest)]) -> AuditReport {
    let mismatches =
        members.iter().filter(|(_, d)| *d != refold).map(|(s, d)| Mismatch { member: *s, got: *d }).collect();
    AuditReport { tier, index, want: refold, mismatches }
}
