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

/// About how many bytes one page of `log_after` reads: pages start at 8 rows (at most 12 MB of `ENTRY_MAX` rows) and
/// grow to `PAGE_ROWS_MAX` while they read under half of this, so a walk of short rows takes few queries.
pub const PAGE_BYTES: u64 = 4 << 20;
const PAGE_ROWS_MAX: u64 = 1_024;

/// `idx` and then `cols` of the stored log's rows above `after`, in index order, each page's query read to its end
/// (`PAGE_BYTES`): a walk that stops early leaves no statement open, and has read at most one page past where it
/// stopped. An error ends the walk.
pub fn log_after<'a>(sql: &'a SqlStorage, cols: &str, after: u64) -> impl Iterator<Item = Result<Row>> + use<'a> {
    let q = format!("SELECT idx, {cols} FROM log WHERE idx > ? ORDER BY idx LIMIT ?");
    let (mut after, mut page, mut buf, mut more) = (after, 8u64, VecDeque::<Row>::new(), true);
    std::iter::from_fn(move || {
        if buf.is_empty() && more {
            let args = vec![SqlStorageValue::Integer(after as i64), SqlStorageValue::Integer(page as i64)];
            let rows = match sql.exec(&q, args).and_then(|c| c.raw().collect::<Result<Vec<Row>>>()) {
                Ok(rows) => rows,
                Err(e) => {
                    more = false;
                    return Some(Err(e));
                }
            };
            more = rows.len() as u64 == page;
            match rows.last().map(|r| &r[0]) {
                Some(SqlStorageValue::Integer(i)) => after = *i as u64,
                Some(_) => more = false,
                None => {}
            }
            let bytes: u64 = rows.iter().flatten().map(size).sum();
            page = if bytes > PAGE_BYTES {
                (page / 2).max(1)
            } else if bytes < PAGE_BYTES / 2 {
                (page * 2).min(PAGE_ROWS_MAX)
            } else {
                page
            };
            buf.extend(rows);
        }
        buf.pop_front().map(Ok)
    })
}

fn size(v: &SqlStorageValue) -> u64 {
    match v {
        SqlStorageValue::Blob(b) => b.len() as u64,
        SqlStorageValue::String(s) => s.len() as u64,
        _ => 8,
    }
}
