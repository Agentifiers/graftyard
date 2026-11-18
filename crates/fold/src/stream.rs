//! The fold audit's refold without scratch tier tables (`decisions/DECISIONS.md`, "`refold-stream`"; `design/SHARED.md`,
//! "Folds", `fold_audit`).
//!
//! - **Staging.** A tier snapshot's frames pass through `Restore`, which checks their order, count and digest against
//!   the trailer, into a `Pager` that regroups each table's rows into pages of at most `PAGE_BYTES`. A host keeps each
//!   page as one row keyed by its table and its last key (`PAGE_DDL`); `key_bytes` encodes keys so that comparing the
//!   bytes orders them as `Key` does, so `PAGE_FIND`'s `last >= ?` finds the one page that can hold a key.
//! - **Apply.** `SnapView` is a `Store` over the staged pages: the batch's own changes in memory first, then the
//!   changes saved by earlier batches (`Staged::saved`), then the pages, through a cache of `CACHE_PAGES` decoded
//!   pages. `apply` and `Writer` run on it unchanged, so the digest is the one a restored tier gives: the snapshot's
//!   checked sum plus each entry's subtract-old, add-new. Every read is synchronous and complete, so fold code never
//!   sees a row missing that the tier holds. Writes to the digest ring and `snap_undo` tables are dropped: an audit
//!   reads neither.
//! - **Bound.** Memory holds the cache, the batch's entries and its changes; the pages and saved changes stay in SQLite.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::rc::Rc;

use graftyard_types::{ChainId, Digest, Index};

use crate::snapshot::{RefoldError, Restore, Row, SnapshotError, SnapshotFrame};
use crate::store::{Key, META, Schema, Store, Stored, Val};
use crate::tier::{DIGESTS, Tiered};

/// A staged page's encoded rows at most [guess]: about 4,000 metadata rows at 65 B. Fewer pages cost fewer rows written
/// (`scripts/cost.py`), larger ones more memory per cached page.
pub const PAGE_BYTES: usize = 256 << 10;
/// Decoded pages one `SnapView` keeps [guess]: about 6 MiB decoded at `PAGE_BYTES`.
pub const CACHE_PAGES: usize = 8;

/// The staged pages: one row per page of a tier's table, keyed by its last key in `key_bytes` order.
pub const PAGE_DDL: &str = "CREATE TABLE IF NOT EXISTS audit_page (tier INTEGER NOT NULL, tab TEXT NOT NULL, last BLOB NOT NULL, first BLOB NOT NULL, rows BLOB NOT NULL, PRIMARY KEY (tier, tab, last)) WITHOUT ROWID";
/// The changes of entries applied by earlier batches, by tier, table and `key_bytes`: `row` is the postcard of
/// `(Key, Option<Stored>)`, `None` for a deleted row.
pub const OVERLAY_DDL: &str = "CREATE TABLE IF NOT EXISTS audit_overlay (tier INTEGER NOT NULL, tab TEXT NOT NULL, k BLOB NOT NULL, row BLOB NOT NULL, PRIMARY KEY (tier, tab, k)) WITHOUT ROWID";
/// The page of a table that can hold keys from `?3` on: the one with the smallest last key at or after it.
pub const PAGE_FIND: &str =
    "SELECT first, last FROM audit_page WHERE tier = ? AND tab = ? AND last >= ? ORDER BY last LIMIT 1";
pub const PAGE_ROWS: &str = "SELECT rows FROM audit_page WHERE tier = ? AND tab = ? AND last = ?";
pub const PAGE_PUT: &str = "INSERT OR REPLACE INTO audit_page (tier, tab, last, first, rows) VALUES (?, ?, ?, ?, ?)";
pub const SAVED_GET: &str = "SELECT row FROM audit_overlay WHERE tier = ? AND tab = ? AND k = ?";
pub const SAVED_SCAN: &str = "SELECT row FROM audit_overlay WHERE tier = ? AND tab = ? AND k >= ? ORDER BY k LIMIT ?";
pub const SAVED_PUT: &str = "INSERT OR REPLACE INTO audit_overlay (tier, tab, k, row) VALUES (?, ?, ?, ?)";
/// What ends an audit, or clears one an object reset left behind: no staged page or saved change outlives it.
pub const AUDIT_CLEAR: [&str; 2] = ["DELETE FROM audit_page", "DELETE FROM audit_overlay"];

/// The first byte of every `key_bytes`, so no key encodes to an empty blob.
const KEY_FORMAT: u8 = 1;

/// A key's bytes, ordered as `Key` is: `KEY_FORMAT`, then each value a tag in `Val`'s variant order, then an integer as eight bytes
/// big-endian with its sign bit flipped, or text and blob bytes with each `0x00` written `0x00 0xff` and ended by
/// `0x00 0x00`. Each value ends itself, so a key that prefixes another encodes to a prefix of its bytes and sorts first,
/// as SQLite compares two BLOBs: `memcmp`, then length.
pub fn key_bytes(key: &[Val]) -> Vec<u8> {
    let mut out = vec![KEY_FORMAT];
    for v in key {
        match v {
            Val::Null => out.push(1),
            Val::Int(i) => {
                out.push(2);
                out.extend_from_slice(&((*i as u64) ^ (1 << 63)).to_be_bytes());
            }
            Val::Text(s) => {
                out.push(3);
                escaped(s.as_bytes(), &mut out);
            }
            Val::Blob(b) => {
                out.push(4);
                escaped(b, &mut out);
            }
        }
    }
    out
}

fn escaped(b: &[u8], out: &mut Vec<u8>) {
    for &c in b {
        out.push(c);
        if c == 0 {
            out.push(0xff);
        }
    }
    out.extend_from_slice(&[0, 0]);
}

/// The smallest byte string after `b`, so the page after one whose last key is `b` is the first with `last >= after(b)`.
fn after(b: &[u8]) -> Vec<u8> {
    let mut v = b.to_vec();
    v.push(0);
    v
}

/// One staged page: a table's rows from `first` to `last`, as the postcard of `Vec<Row>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Page {
    pub table: Box<str>,
    pub first: Vec<u8>,
    pub last: Vec<u8>,
    pub rows: Vec<u8>,
}

/// The `Store` a `Restore` writes into while it stages a snapshot: rows become pages, and the meta row is kept. It
/// holds nothing else, so `Restore::begin` finds it empty.
#[derive(Debug)]
pub struct Pager {
    page_bytes: usize,
    table: Option<Box<str>>,
    first: Vec<u8>,
    last: Vec<u8>,
    count: usize,
    body: Vec<u8>,
    pages: Vec<Page>,
    meta: Option<Stored>,
}

impl Pager {
    pub fn new(page_bytes: usize) -> Pager {
        Pager {
            page_bytes,
            table: None,
            first: Vec::new(),
            last: Vec::new(),
            count: 0,
            body: Vec::new(),
            pages: Vec::new(),
            meta: None,
        }
    }

    fn flush(&mut self) {
        let Some(table) = self.table.take() else { return };
        // A postcard `Vec` is its length as a varint, then its elements.
        let mut rows = Vec::with_capacity(self.body.len() + 10);
        let mut n = self.count as u64;
        loop {
            let b = (n & 0x7f) as u8;
            n >>= 7;
            rows.push(if n == 0 { b } else { b | 0x80 });
            if n == 0 {
                break;
            }
        }
        rows.append(&mut self.body);
        self.pages.push(Page {
            table,
            first: std::mem::take(&mut self.first),
            last: std::mem::take(&mut self.last),
            rows,
        });
        self.count = 0;
    }

    /// The pages filled so far, the last one ended short: a host stages them with the batch that read their rows.
    pub fn take(&mut self) -> Vec<Page> {
        self.flush();
        std::mem::take(&mut self.pages)
    }

    /// The tier's meta row, once `Restore::finish_rows` wrote it.
    pub fn meta(&self) -> Option<&Stored> {
        self.meta.as_ref()
    }
}

impl Store for Pager {
    fn get(&self, _table: &str, _key: &[Val]) -> Option<Stored> {
        None
    }
    fn put(&mut self, table: &str, key: Key, row: Stored) {
        if table == META {
            self.meta = Some(row);
            return;
        }
        let k = key_bytes(&key);
        let r = postcard::to_allocvec(&Row { key, written: row.written, cols: row.cols }).expect("a row encodes");
        if self.table.as_deref() != Some(table) || self.body.len() + r.len() > self.page_bytes {
            self.flush();
        }
        if self.table.is_none() {
            self.table = Some(table.into());
            self.first = k.clone();
        }
        self.last = k;
        self.count += 1;
        self.body.extend_from_slice(&r);
    }
    fn delete(&mut self, _table: &str, _key: &[Val]) {}
    fn scan(&self, _table: &str, _from: &[Val], _limit: usize) -> Vec<(Key, Stored)> {
        Vec::new()
    }
}

/// A staged page's place: its first and last key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageAt {
    pub first: Vec<u8>,
    pub last: Vec<u8>,
}

/// What a host stages for one tier: its pages and the changes earlier batches saved. Reads cannot fail: a host whose
/// storage refuses one records it, answers nothing, and fails the batch before it commits anything.
pub trait Staged {
    /// The page of `table` with the smallest last key at or after `from` (`PAGE_FIND`).
    fn page(&self, table: &str, from: &[u8]) -> Option<PageAt>;
    /// The rows of the page of `table` whose last key is `last`.
    fn rows(&self, table: &str, last: &[u8]) -> Vec<Row>;
    /// A saved change: `Some(None)` for a deleted row, `None` for a row no batch changed.
    fn saved(&self, table: &str, key: &[Val]) -> Option<Option<Stored>>;
    /// Saved changes with key at or after `from`, in key order, at most `limit`.
    fn saved_scan(&self, table: &str, from: &[Val], limit: usize) -> Vec<(Key, Option<Stored>)>;
}

/// A batch's changes: by table and key, `None` for a deleted row.
pub type Changes = BTreeMap<(String, Key), Option<Stored>>;

/// Changes from a key, at most a count, in key order.
type Upper<'a> = &'a dyn Fn(&[Val], usize) -> Vec<(Key, Option<Stored>)>;
/// Rows from a key, at most a count, in key order.
type Lower<'a> = &'a dyn Fn(&[Val], usize) -> Vec<(Key, Stored)>;
/// A cached page: its table, its place, its decoded rows.
type Cached = (String, PageAt, Rc<Vec<Row>>);

/// `upper`'s entries (`None` deletes) over `lower`'s rows, from `from`, cut at `limit`. Upper entries are taken until
/// `limit` of them are visible, since any later one sorts after `limit` visible rows, and enough lower rows that the
/// taken deletes cannot leave fewer than `limit` (`crate::store::Overlay::scan`).
fn merge(upper: Upper<'_>, lower: Lower<'_>, from: &[Val], limit: usize) -> Vec<(Key, Stored)> {
    if limit == 0 {
        return Vec::new();
    }
    let mut n = limit;
    let mine = loop {
        let got = upper(from, n);
        let full = got.len() == n;
        let (mut visible, mut take) = (0, Vec::new());
        for (k, v) in got {
            if visible == limit {
                break;
            }
            visible += usize::from(v.is_some());
            take.push((k, v));
        }
        if visible == limit || !full {
            break take;
        }
        n = n.saturating_mul(2);
    };
    let deletes = mine.iter().filter(|(_, v)| v.is_none()).count();
    let mut merged: BTreeMap<Key, Stored> = lower(from, limit.saturating_add(deletes)).into_iter().collect();
    for (k, v) in mine {
        match v {
            Some(v) => merged.insert(k, v),
            None => merged.remove(&k),
        };
    }
    merged.into_iter().take(limit).collect()
}

/// A tier as of a staged snapshot plus the entries applied on it: see the module doc.
pub struct SnapView<'a> {
    base: &'a dyn Staged,
    mem: Changes,
    /// Decoded pages, least recently used first: table, first and last key, rows.
    cache: RefCell<VecDeque<Cached>>,
    cache_pages: usize,
    /// Pages read from the host, and the distinct pages read since `reset_touched`.
    loads: Cell<u64>,
    touched: RefCell<BTreeSet<(String, Vec<u8>)>>,
}

impl<'a> SnapView<'a> {
    pub fn new(base: &'a dyn Staged, cache_pages: usize) -> SnapView<'a> {
        SnapView {
            base,
            mem: Changes::new(),
            cache: RefCell::new(VecDeque::new()),
            cache_pages: cache_pages.max(1),
            loads: Cell::new(0),
            touched: RefCell::new(BTreeSet::new()),
        }
    }

    /// The changes since the view was made, for the host to save with its batch.
    pub fn take(&mut self) -> Changes {
        std::mem::take(&mut self.mem)
    }

    /// Changes held in memory: a host saves them before they pass its batch's bound.
    pub fn changed(&self) -> usize {
        self.mem.len()
    }

    pub fn loads(&self) -> u64 {
        self.loads.get()
    }

    /// The distinct pages read since the last call, which it resets.
    pub fn reset_touched(&self) -> usize {
        std::mem::take(&mut *self.touched.borrow_mut()).len()
    }

    /// A cached page of `table` that holds `kb` between its first and last key.
    fn cached(&self, table: &str, kb: &[u8]) -> Option<(PageAt, Rc<Vec<Row>>)> {
        let mut c = self.cache.borrow_mut();
        let at = c.iter().position(|(t, p, _)| t == table && p.first.as_slice() <= kb && kb <= p.last.as_slice())?;
        let hit = c.remove(at).expect("found");
        let out = (hit.1.clone(), hit.2.clone());
        c.push_back(hit);
        self.touched.borrow_mut().insert((table.to_owned(), out.0.last.clone()));
        Some(out)
    }

    /// The page of `table` that can hold keys from `kb` on, and its rows.
    fn page_from(&self, table: &str, kb: &[u8]) -> Option<(PageAt, Rc<Vec<Row>>)> {
        if let Some(hit) = self.cached(table, kb) {
            return Some(hit);
        }
        let p = self.base.page(table, kb)?;
        self.touched.borrow_mut().insert((table.to_owned(), p.last.clone()));
        let mut c = self.cache.borrow_mut();
        if let Some(at) = c.iter().position(|(t, q, _)| t == table && q.last == p.last) {
            let hit = c.remove(at).expect("found");
            let rows = hit.2.clone();
            c.push_back(hit);
            return Some((p, rows));
        }
        self.loads.set(self.loads.get() + 1);
        let rows = Rc::new(self.base.rows(table, &p.last));
        if c.len() == self.cache_pages {
            c.pop_front();
        }
        c.push_back((table.to_owned(), p.clone(), rows.clone()));
        Some((p, rows))
    }

    fn page_get(&self, table: &str, key: &[Val]) -> Option<Stored> {
        let kb = key_bytes(key);
        let (p, rows) = self.page_from(table, &kb)?;
        if p.first > kb {
            return None;
        }
        let at = rows.binary_search_by(|r| r.key.as_slice().cmp(key)).ok()?;
        Some(Stored { written: rows[at].written, cols: rows[at].cols.clone() })
    }

    fn page_scan(&self, table: &str, from: &[Val], limit: usize) -> Vec<(Key, Stored)> {
        let mut out = Vec::new();
        let mut at = key_bytes(from);
        while out.len() < limit {
            let Some((p, rows)) = self.page_from(table, &at) else { break };
            let start = rows.partition_point(|r| r.key.as_slice() < from);
            for r in &rows[start..] {
                if out.len() == limit {
                    break;
                }
                out.push((r.key.clone(), Stored { written: r.written, cols: r.cols.clone() }));
            }
            at = after(&p.last);
        }
        out
    }

    /// The tier before this view's changes: saved changes over the pages.
    fn lower_scan(&self, table: &str, from: &[Val], limit: usize) -> Vec<(Key, Stored)> {
        merge(&|f, n| self.base.saved_scan(table, f, n), &|f, n| self.page_scan(table, f, n), from, limit)
    }

    fn dropped(table: &str) -> bool {
        table == DIGESTS || table.ends_with("~undo")
    }
}

impl Store for SnapView<'_> {
    fn get(&self, table: &str, key: &[Val]) -> Option<Stored> {
        if let Some(v) = self.mem.get(&(table.to_owned(), key.to_vec())) {
            return v.clone();
        }
        match self.base.saved(table, key) {
            Some(v) => v,
            None => self.page_get(table, key),
        }
    }
    fn put(&mut self, table: &str, key: Key, row: Stored) {
        if !Self::dropped(table) {
            self.mem.insert((table.to_owned(), key), Some(row));
        }
    }
    fn delete(&mut self, table: &str, key: &[Val]) {
        if !Self::dropped(table) {
            self.mem.insert((table.to_owned(), key.to_vec()), None);
        }
    }
    fn scan(&self, table: &str, from: &[Val], limit: usize) -> Vec<(Key, Stored)> {
        let mem = |f: &[Val], n: usize| -> Vec<(Key, Option<Stored>)> {
            self.mem
                .range((table.to_owned(), f.to_vec())..)
                .take_while(|((t, _), _)| t == table)
                .take(n)
                .map(|((_, k), v)| (k.clone(), v.clone()))
                .collect()
        };
        merge(&mem, &|f, n| self.lower_scan(table, f, n), from, limit)
    }
}

type PageBody = (Vec<u8>, Vec<u8>);

/// An in-memory `Staged`, for tests and for `refold_segment`.
#[derive(Clone, Debug, Default)]
pub struct MemStaged {
    /// By table and last key: the first key and the rows' bytes.
    pages: BTreeMap<(String, Vec<u8>), PageBody>,
    saved: BTreeMap<(String, Key), Option<Stored>>,
}

impl MemStaged {
    pub fn stage(&mut self, pages: Vec<Page>) {
        for p in pages {
            self.pages.insert((p.table.into(), p.last), (p.first, p.rows));
        }
    }

    pub fn save(&mut self, changes: Changes) {
        self.saved.extend(changes);
    }

    pub fn pages(&self) -> usize {
        self.pages.len()
    }
}

impl Staged for MemStaged {
    fn page(&self, table: &str, from: &[u8]) -> Option<PageAt> {
        let ((t, last), (first, _)) = self.pages.range((table.to_owned(), from.to_vec())..).next()?;
        (t == table).then(|| PageAt { first: first.clone(), last: last.clone() })
    }
    fn rows(&self, table: &str, last: &[u8]) -> Vec<Row> {
        self.pages
            .get(&(table.to_owned(), last.to_vec()))
            .map(|(_, b)| postcard::from_bytes(b).expect("a page this test staged"))
            .unwrap_or_default()
    }
    fn saved(&self, table: &str, key: &[Val]) -> Option<Option<Stored>> {
        self.saved.get(&(table.to_owned(), key.to_vec())).cloned()
    }
    fn saved_scan(&self, table: &str, from: &[Val], limit: usize) -> Vec<(Key, Option<Stored>)> {
        self.saved
            .range((table.to_owned(), from.to_vec())..)
            .take_while(|((t, _), _)| t == table)
            .take(limit)
            .map(|((_, k), v)| (k.clone(), v.clone()))
            .collect()
    }
}

/// Stages a snapshot's frames as a host's audit does: checked by `Restore` against the trailer and `want`, regrouped
/// into pages of at most `page_bytes`. The pages and the tier's meta row at the snapshot.
pub fn stage(
    schema: Schema,
    want: (ChainId, Index),
    frames: impl IntoIterator<Item = SnapshotFrame>,
    page_bytes: usize,
) -> Result<(Vec<Page>, Stored), SnapshotError> {
    let mut pager = Pager::new(page_bytes);
    let mut r = Restore::begin(&pager, schema, want)?;
    for f in frames {
        r.frame(&mut pager, &f)?;
    }
    r.finish_rows(&mut pager)?;
    let meta = pager.meta.clone().expect("finish_rows writes the meta row");
    Ok((pager.take(), meta))
}

/// A fold audit's refold of one tier (F48, "`refold-stream`"): the snapshot at `from` staged, then `entries` applied
/// in order through a `SnapView`, each entry's changes saved before the next as a host's batch saves them. Returns the
/// tier's `fold_digest` after the last entry, which the audit compares with each member's `digest_at` of the same
/// index, or with the next snapshot's trailer when the entries end at its `to`.
pub fn refold_segment<T: Tiered>(
    t: &T,
    schema: Schema,
    from: (ChainId, Index),
    frames: impl IntoIterator<Item = SnapshotFrame>,
    entries: impl IntoIterator<Item = graftyard_raft::LogEntry<T::Event>>,
) -> Result<Digest, RefoldError> {
    let (pages, meta) = stage(schema, from, frames, PAGE_BYTES)?;
    let mut staged = MemStaged::default();
    staged.stage(pages);
    staged.save(Changes::from([((META.to_owned(), Vec::new()), Some(meta))]));
    for e in entries {
        let mut v = SnapView::new(&staged, CACHE_PAGES);
        crate::tier::apply(t, &mut v, schema.tier, &e)?;
        let c = v.take();
        staged.save(c);
    }
    Ok(crate::tier::fold_digest(&SnapView::new(&staged, CACHE_PAGES), schema.tier))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Rng(u64);
    impl Rng {
        fn next(&mut self, n: u64) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0 % n
        }
    }

    /// Values of every kind the tier schemas key on, with the cases an encoding gets wrong: integers across the sign
    /// boundary and at the ends, empty text and blobs, prefixes of each other, and the escape byte inside them.
    fn val(r: &mut Rng) -> Val {
        const INTS: [i64; 11] = [i64::MIN, i64::MIN + 1, -256, -2, -1, 0, 1, 2, 255, i64::MAX - 1, i64::MAX];
        const TEXT: [&str; 8] = ["", "a", "a\0", "a\0b", "ab", "b", "\0", "refs/heads/main"];
        const BLOB: [&[u8]; 8] = [b"", b"\0", b"\0\0", b"\0\xff", b"\x01", b"\xff", b"\xff\0", b"\xff\xff"];
        match r.next(4) {
            0 => Val::Int(INTS[r.next(INTS.len() as u64) as usize]),
            1 => Val::Int(r.next(u64::MAX) as i64),
            2 => Val::Text(TEXT[r.next(TEXT.len() as u64) as usize].into()),
            _ => Val::Blob(BLOB[r.next(BLOB.len() as u64) as usize].into()),
        }
    }

    fn key(r: &mut Rng) -> Key {
        (0..r.next(4)).map(|_| val(r)).collect()
    }

    /// `key_bytes` orders keys as `Key` does for every pair, including a key against its own prefixes and keys whose
    /// columns differ in kind, so a page lookup by `last >= key_bytes(k)` finds the page `Key` order puts `k` in.
    #[test]
    fn key_bytes_order_keys_as_keys_order() {
        let mut r = Rng(0x9e37_79b9_7f4a_7c15);
        let keys: Vec<Key> = (0..600).map(|_| key(&mut r)).collect();
        for a in &keys {
            for p in 0..=a.len() {
                assert_eq!(key_bytes(&a[..p]).cmp(&key_bytes(a)), a[..p].cmp(a), "{a:?} against its prefix {p}");
            }
            for b in &keys {
                assert_eq!(key_bytes(a).cmp(&key_bytes(b)), a.cmp(b), "{a:?} against {b:?}");
            }
        }
    }

    /// Over random tables split into pages of random sizes, with random saved and batch changes, every `get` and every
    /// `scan` from a random key with a random limit equals the same rows held in one map, whatever the cache's size.
    #[test]
    fn a_view_reads_what_one_map_holds() {
        let mut r = Rng(7);
        for round in 0..200 {
            let mut want: BTreeMap<Key, Stored> = BTreeMap::new();
            for _ in 0..r.next(120) {
                want.insert(key(&mut r), Stored { written: Index(r.next(9)), cols: vec![Val::Int(r.next(99) as i64)] });
            }
            let mut pager = Pager::new(1 + r.next(400) as usize);
            for (k, v) in &want {
                pager.put("t", k.clone(), v.clone());
            }
            pager.put("u", vec![Val::Int(1)], Stored { written: Index(1), cols: vec![] });
            let mut staged = MemStaged::default();
            staged.stage(pager.take());
            let change = |r: &mut Rng, want: &mut BTreeMap<Key, Stored>| {
                let k = if r.next(2) == 0 && !want.is_empty() {
                    want.keys().nth(r.next(want.len() as u64) as usize).unwrap().clone()
                } else {
                    key(r)
                };
                let v = (r.next(3) != 0).then(|| Stored { written: Index(10), cols: vec![Val::Int(-1)] });
                match &v {
                    Some(v) => want.insert(k.clone(), v.clone()),
                    None => want.remove(&k),
                };
                (("t".to_owned(), k), v)
            };
            let saved: Changes = (0..r.next(30)).map(|_| change(&mut r, &mut want)).collect();
            staged.save(saved);
            let mut v = SnapView::new(&staged, 1 + r.next(3) as usize);
            for _ in 0..r.next(30) {
                let ((t, k), x) = change(&mut r, &mut want);
                match x {
                    Some(x) => v.put(&t, k, x),
                    None => v.delete(&t, &k),
                }
            }
            for _ in 0..40 {
                let k = key(&mut r);
                assert_eq!(v.get("t", &k), want.get(&k).cloned(), "round {round} get {k:?}");
                let limit = 1 + r.next(12) as usize;
                let got = v.scan("t", &k, limit);
                let expect: Vec<(Key, Stored)> =
                    want.range(k.clone()..).take(limit).map(|(a, b)| (a.clone(), b.clone())).collect();
                assert_eq!(got, expect, "round {round} scan from {k:?} limit {limit}");
            }
            assert_eq!(v.scan("u", &[], 5).len(), 1, "another table's pages stay apart");
        }
    }

    /// A page's bytes decode as the rows put into it, and no page passes its byte bound unless one row alone does.
    #[test]
    fn pages_hold_their_rows_within_the_bound() {
        let mut pager = Pager::new(100);
        let rows: Vec<Row> =
            (0..50).map(|i| Row { key: vec![Val::Int(i)], written: Index(1), cols: vec![Val::Int(i * 7)] }).collect();
        for r in &rows {
            pager.put("t", r.key.clone(), Stored { written: r.written, cols: r.cols.clone() });
        }
        let pages = pager.take();
        assert!(pages.len() > 1 && pages.iter().all(|p| p.rows.len() <= 101));
        let back: Vec<Row> = pages.iter().flat_map(|p| postcard::from_bytes::<Vec<Row>>(&p.rows).unwrap()).collect();
        assert_eq!(back, rows);
    }
}
