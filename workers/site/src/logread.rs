//! Multi-row reads that leave no statement open. A cursor stopped before its last row keeps its statement, and a lock
//! on what it reads, until the isolate collects it; meanwhile every `DROP TABLE` in the member fails with
//! SQLITE_LOCKED: the fold audit's `reset_scratch`, and an install's table swap. In local workerd a cursor stopped after
//! one row still held the lock in the object's next request (`tests/auditlock/bed`). On graft-m1, 141 of 348 fold audit
//! refolds on 2026-11-16 ended that way: a leader's archive alarm walks its log every minute and stops early
//! (`tests/am1/results/b030-split.md`, "Corrected 2026-11-16 21:40Z").

use std::collections::VecDeque;

use worker::{Result, SqlStorage, SqlStorageValue};

pub type Row = Vec<SqlStorageValue>;

/// Rows of `q` one at a time while `f` returns true. A read that stops early, or whose `f` fails, steps through the
/// rest without keeping it, so the statement ends: bound `q` to what stopping early may cost.
pub fn each_row(
    sql: &SqlStorage,
    q: &str,
    args: Vec<SqlStorageValue>,
    mut f: impl FnMut(Row) -> Result<bool>,
) -> Result<()> {
    let mut it = sql.exec(q, args)?.raw();
    let mut out = Ok(());
    for r in it.by_ref() {
        match r.and_then(&mut f) {
            Ok(true) => {}
            Ok(false) => break,
            Err(e) => {
                out = Err(e);
                break;
            }
        }
    }
    for r in it {
        if r.is_err() {
            break;
        }
    }
    out
}

/// The most stored row bytes one page of `log_after` fetches, or one row when that row alone is longer (at most
/// `ENTRY_MAX`). Pages were sized by the rows of the page before, so a page of 512 or 1,024 short rows was followed by
/// as many rows of inline packs: on B0-33 g3A and g4B a leader's `/entries` rehydrate read and archive walk fetched
/// pages of about 55 to 70 MB, its isolate stood at 114.8 MiB, and g4B's reset in the middle of one
/// (`tests/am1/results/archive-memory.md`).
pub const PAGE_BYTES: u64 = 4 << 20;
/// Rows a page asks the lengths of: pages start at 8 and double up to this while they read under half of
/// `PAGE_BYTES`, so a walk of short rows takes few queries.
const PAGE_ROWS_MAX: u64 = 1_024;

thread_local! {
    /// The most row bytes one `log_after` page fetched in this isolate (`/stats` `memSplit` `logPageHigh`).
    static PAGE_HIGH: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// `PAGE_HIGH`, the isolate's largest `log_after` page so far.
pub fn page_high() -> u64 {
    PAGE_HIGH.with(|h| h.get())
}

/// Of rows' `(idx, length(row))` in index order, the last index of the longest prefix within `budget` bytes, at least
/// one row, and its bytes. `None` for no rows.
fn page_through(lens: &[(u64, u64)], budget: u64) -> Option<(u64, u64)> {
    let mut out: Option<(u64, u64)> = None;
    for &(idx, len) in lens {
        let bytes = out.map_or(0, |(_, b)| b) + len;
        if out.is_some() && bytes > budget {
            break;
        }
        out = Some((idx, bytes));
    }
    out
}

/// `idx` and then `cols` of the stored log's rows above `after`, in index order, a page at a time: each page first
/// reads only its rows' lengths, then fetches `cols` of as many as fit in `PAGE_BYTES` (`page_through`), and reads
/// that query to its end, so a walk that stops early leaves no statement open, and has read at most one page, about
/// `PAGE_BYTES`, past where it stopped. An error ends the walk.
pub fn log_after<'a>(sql: &'a SqlStorage, cols: &str, after: u64) -> impl Iterator<Item = Result<Row>> + use<'a> {
    const LENS: &str = "SELECT idx, length(row) FROM log WHERE idx > ? ORDER BY idx LIMIT ?";
    let q = format!("SELECT idx, {cols} FROM log WHERE idx > ? AND idx <= ? ORDER BY idx");
    let (mut after, mut page, mut buf, mut more) = (after, 8u64, VecDeque::<Row>::new(), true);
    let int = |v: &SqlStorageValue| match v {
        SqlStorageValue::Integer(i) => Some(*i as u64),
        _ => None,
    };
    std::iter::from_fn(move || {
        if buf.is_empty() && more {
            let args = vec![SqlStorageValue::Integer(after as i64), SqlStorageValue::Integer(page as i64)];
            let lens = sql
                .exec(LENS, args)
                .and_then(|c| c.raw().collect::<Result<Vec<Row>>>())
                .map(|rows| rows.iter().filter_map(|r| Some((int(&r[0])?, int(&r[1])?))).collect::<Vec<_>>());
            let lens = match lens {
                Ok(l) => l,
                Err(e) => {
                    more = false;
                    return Some(Err(e));
                }
            };
            let Some((through, bytes)) = page_through(&lens, PAGE_BYTES) else {
                more = false;
                return None;
            };
            let args = vec![SqlStorageValue::Integer(after as i64), SqlStorageValue::Integer(through as i64)];
            let rows = match sql.exec(&q, args).and_then(|c| c.raw().collect::<Result<Vec<Row>>>()) {
                Ok(rows) => rows,
                Err(e) => {
                    more = false;
                    return Some(Err(e));
                }
            };
            PAGE_HIGH.with(|h| h.set(h.get().max(bytes)));
            // More rows follow when the lengths filled their limit or the page took only part of them.
            more = lens.len() as u64 == page || lens.last().is_some_and(|l| l.0 > through);
            match rows.last().and_then(|r| int(&r[0])) {
                Some(i) => after = i,
                None => more = false,
            }
            if bytes < PAGE_BYTES / 2 && lens.last().is_some_and(|l| l.0 == through) {
                page = (page * 2).min(PAGE_ROWS_MAX);
            }
            buf.extend(rows);
        }
        buf.pop_front().map(Ok)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A page stops before the row that would pass the budget, so 1,000 short rows and then large ones give a page
    /// of the short rows and as many large ones as fit, not every row its count allows.
    #[test]
    fn a_page_holds_its_budget_not_its_row_count() {
        let mut lens: Vec<(u64, u64)> = (1..=1_000).map(|i| (i, 1_500)).collect();
        lens.extend((1_001..=1_024).map(|i| (i, 300_000)));
        // 1,500,000 short bytes leave room for 8 rows of 300,000 under 4 MiB (4,194,304), not 9.
        assert_eq!(page_through(&lens, PAGE_BYTES), Some((1_008, 3_900_000)));
        // Exactly at the budget is inside it.
        assert_eq!(page_through(&[(5, 2), (6, 3), (7, 1)], 5), Some((6, 5)));
    }

    /// One row longer than the budget is still a page, alone; no rows is no page.
    #[test]
    fn a_long_first_row_is_a_page_alone() {
        assert_eq!(page_through(&[(9, 1_500_000), (10, 10)], 1_000_000), Some((9, 1_500_000)));
        assert_eq!(page_through(&[], PAGE_BYTES), None);
    }
}
