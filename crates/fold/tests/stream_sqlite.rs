//! `refold-stream`'s SQL in SQLite itself (`decisions/DECISIONS.md`, "`refold-stream`"): pages staged with
//! `PAGE_PUT` and found with `PAGE_FIND`'s `last >= ? ORDER BY last LIMIT 1`, which compares BLOBs as SQLite does, and
//! changes saved with `SAVED_PUT` and read back with `SAVED_GET` and `SAVED_SCAN`. A view over those tables reads what
//! one map holds, for keys of every kind the tier schemas use, and `AUDIT_CLEAR` leaves both tables empty.

use std::collections::BTreeMap;

use graftyard_fold::{
    AUDIT_CLEAR, Key, OVERLAY_DDL, PAGE_DDL, PAGE_FIND, PAGE_PUT, PAGE_ROWS, PageAt, Pager, Row, SAVED_GET, SAVED_PUT,
    SAVED_SCAN, SnapView, Staged, Store, Stored, Val, key_bytes,
};
use graftyard_types::Index;
use rusqlite::{Connection, params};

struct Sql(Connection, i64);

impl Staged for Sql {
    fn page(&self, table: &str, from: &[u8]) -> Option<PageAt> {
        self.0
            .query_row(PAGE_FIND, params![self.1, table, from], |r| Ok(PageAt { first: r.get(0)?, last: r.get(1)? }))
            .ok()
    }
    fn rows(&self, table: &str, last: &[u8]) -> Vec<Row> {
        let b: Vec<u8> = self.0.query_row(PAGE_ROWS, params![self.1, table, last], |r| r.get(0)).unwrap();
        postcard::from_bytes(&b).unwrap()
    }
    fn saved(&self, table: &str, key: &[Val]) -> Option<Option<Stored>> {
        let b: Vec<u8> = self.0.query_row(SAVED_GET, params![self.1, table, key_bytes(key)], |r| r.get(0)).ok()?;
        Some(postcard::from_bytes::<(Key, Option<Stored>)>(&b).unwrap().1)
    }
    fn saved_scan(&self, table: &str, from: &[Val], limit: usize) -> Vec<(Key, Option<Stored>)> {
        let mut q = self.0.prepare(SAVED_SCAN).unwrap();
        let limit = limit.min(i64::MAX as usize) as i64;
        q.query_map(params![self.1, table, key_bytes(from), limit], |r| r.get::<_, Vec<u8>>(0))
            .unwrap()
            .map(|b| postcard::from_bytes(&b.unwrap()).unwrap())
            .collect()
    }
}

struct Rng(u64);
impl Rng {
    fn next(&mut self, n: u64) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 % n
    }
}

/// Key values with the cases an order-preserving encoding gets wrong: integers across the sign boundary and at both
/// ends, empty text and blobs, values that prefix each other, and the escape byte inside them.
fn val(r: &mut Rng) -> Val {
    const INTS: [i64; 11] = [i64::MIN, i64::MIN + 1, -256, -2, -1, 0, 1, 2, 255, i64::MAX - 1, i64::MAX];
    const TEXT: [&str; 7] = ["", "a", "a\0", "a\0b", "ab", "b", "refs/heads/main"];
    const BLOB: [&[u8]; 7] = [b"", b"\0", b"\0\0", b"\0\xff", b"\x01", b"\xff", b"\xff\0"];
    match r.next(4) {
        0 => Val::Int(INTS[r.next(INTS.len() as u64) as usize]),
        1 => Val::Int(r.next(u64::MAX) as i64),
        2 => Val::Text(TEXT[r.next(TEXT.len() as u64) as usize].into()),
        _ => Val::Blob(BLOB[r.next(BLOB.len() as u64) as usize].into()),
    }
}

/// A table's keys have one shape: `n` columns, as each tier table's key does. Composite keys share leading columns
/// often, so prefixes and ties on the first column are common.
fn key(r: &mut Rng, n: usize, firsts: &[Val]) -> Key {
    let mut k: Key = (0..n).map(|_| val(r)).collect();
    if n > 0 && r.next(2) == 0 {
        k[0] = firsts[r.next(firsts.len() as u64) as usize].clone();
    }
    k
}

#[test]
fn a_view_over_sqlite_pages_reads_what_one_map_holds_and_clears() {
    let mut r = Rng(0x2545_f491_4f6c_dd1d);
    for round in 0..60 {
        let c = Connection::open_in_memory().unwrap();
        c.execute(PAGE_DDL, []).unwrap();
        c.execute(OVERLAY_DDL, []).unwrap();
        let tables = [("one", 0usize), ("ints", 1), ("pair", 2), ("triple", 3)];
        let firsts: Vec<Val> = (0..4).map(|_| val(&mut r)).collect();
        let mut want: BTreeMap<&str, BTreeMap<Key, Stored>> = BTreeMap::new();
        let mut pager = Pager::new(1 + r.next(300) as usize);
        for (t, n) in tables {
            let m = want.entry(t).or_default();
            for _ in 0..if n == 0 { 1 } else { r.next(80) } {
                m.insert(key(&mut r, n, &firsts), Stored { written: Index(r.next(9)), cols: vec![Val::Int(1)] });
            }
            for (k, v) in m.iter() {
                pager.put(t, k.clone(), v.clone());
            }
        }
        for p in pager.take() {
            c.execute(PAGE_PUT, params![1, &*p.table, p.last, p.first, p.rows]).unwrap();
        }
        // Saved changes from earlier batches, then this batch's own in the view.
        for _ in 0..r.next(25) {
            let (t, n) = tables[1 + r.next(3) as usize];
            let k = key(&mut r, n, &firsts);
            let row = (r.next(3) != 0).then(|| Stored { written: Index(20), cols: vec![Val::Int(2)] });
            let b = postcard::to_allocvec(&(&k, &row)).unwrap();
            c.execute(SAVED_PUT, params![1, t, key_bytes(&k), b]).unwrap();
            match row {
                Some(v) => want.get_mut(t).unwrap().insert(k, v),
                None => want.get_mut(t).unwrap().remove(&k),
            };
        }
        let staged = Sql(c, 1);
        let mut v = SnapView::new(&staged, 1);
        for _ in 0..r.next(15) {
            let (t, n) = tables[1 + r.next(3) as usize];
            let k = key(&mut r, n, &firsts);
            if r.next(3) == 0 {
                v.delete(t, &k);
                want.get_mut(t).unwrap().remove(&k);
            } else {
                let row = Stored { written: Index(30), cols: vec![Val::Int(3)] };
                v.put(t, k.clone(), row.clone());
                want.get_mut(t).unwrap().insert(k, row);
            }
        }
        for (t, n) in tables {
            let m = &want[t];
            for k in m.keys() {
                assert_eq!(v.get(t, k), m.get(k).cloned(), "round {round} {t} get {k:?}");
            }
            for _ in 0..30 {
                let k = key(&mut r, n, &firsts);
                let from = &k[..r.next(n as u64 + 1) as usize];
                assert_eq!(v.get(t, &k), m.get(&k).cloned(), "round {round} {t} get {k:?}");
                let limit = 1 + r.next(10) as usize;
                let expect: Vec<(Key, Stored)> =
                    m.range(from.to_vec()..).take(limit).map(|(a, b)| (a.clone(), b.clone())).collect();
                assert_eq!(v.scan(t, from, limit), expect, "round {round} {t} scan from {from:?} limit {limit}");
            }
        }
        drop(v);
        for q in AUDIT_CLEAR {
            staged.0.execute(q, []).unwrap();
        }
        for t in ["audit_page", "audit_overlay"] {
            let n: i64 = staged.0.query_row(&format!("SELECT count(*) FROM {t}"), [], |r| r.get(0)).unwrap();
            assert_eq!(n, 0, "{t} after AUDIT_CLEAR");
        }
    }
}
