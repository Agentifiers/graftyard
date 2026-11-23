//! Test-faults-only hosted SQLite costs for decisions/DECISIONS.md "`seal-batch`" and
//! tests/am1/results/seal-bytes-design.md section 3. Arm (a) runs k one-row statements a transaction, arm (b) one
//! k-row statement a transaction (split at SQLite's 100-bound-parameter limit), and arm (c) one-row `log`-shaped
//! transactions of a given KB. Each arm writes its own `probe_<arm>` scratch table (`probe_ab` for arms a and b, and
//! `probe_c` for arm c), created and dropped in the same request, so nothing persists. It never touches fold tables,
//! so it does not move the read memo except through crate::txn::generation: commit_tx counts a generation, which only
//! drops memos, harmlessly.

use std::cell::RefCell;
use std::collections::HashMap;

use serde_json::{Value, json};
use worker::SqlStorageValue;

const AB_CREATE: &str =
    "CREATE TABLE IF NOT EXISTS probe_ab (k0 BLOB PRIMARY KEY, written INTEGER NOT NULL, v BLOB NOT NULL)";
const AB_DROP: &str = "DROP TABLE IF EXISTS probe_ab";
const AB_ONE: &str = "INSERT OR REPLACE INTO probe_ab (k0, written, v) VALUES (?, ?, ?)";
const C_CREATE: &str = "CREATE TABLE IF NOT EXISTS probe_c (idx INTEGER PRIMARY KEY, term INTEGER NOT NULL, digest BLOB NOT NULL, row BLOB NOT NULL)";
const C_DROP: &str = "DROP TABLE IF EXISTS probe_c";
const C_ONE: &str = "INSERT OR REPLACE INTO probe_c (idx, term, digest, row) VALUES (?, ?, ?, ?)";

fn batch_sizes(k: usize, per: usize) -> Vec<usize> {
    assert!(per > 0);
    (0..k).step_by(per).map(|start| per.min(k - start)).collect()
}

fn multi_row_sql(rows: usize) -> String {
    format!("INSERT OR REPLACE INTO probe_ab (k0, written, v) VALUES {}", vec!["(?, ?, ?)"; rows].join(", "))
}

/// One leaked statement per distinct batch size, as crate::txn::Stmt requires static SQL text.
fn batch_sql(rows: usize) -> &'static str {
    thread_local!(static SQL: RefCell<HashMap<usize, &'static str>> = RefCell::new(HashMap::new()));
    SQL.with(|sql| *sql.borrow_mut().entry(rows).or_insert_with(|| Box::leak(multi_row_sql(rows).into_boxed_str())))
}

fn usize_arg(body: &Value, key: &str, default: usize, max: usize) -> usize {
    body.get(key).and_then(Value::as_u64).map_or(default, |v| v.min(max as u64) as usize)
}

/// Runs one SQLite cost arm on this repository member's own storage.
pub fn run(
    h: &std::rc::Rc<crate::host::Host<crate::family::Repo>>,
    body: &serde_json::Value,
) -> worker::Result<serde_json::Value> {
    let arm = body.get("arm").and_then(Value::as_str).unwrap_or_default();
    let (drop, create) = match arm {
        "a" | "b" => (AB_DROP, AB_CREATE),
        "c" => (C_DROP, C_CREATE),
        _ => return Err(worker::Error::RustError("arm must be a, b or c".into())),
    };
    let n = usize_arg(body, "n", 50, 400);
    let k = usize_arg(body, "k", 60, 400);
    let kb = usize_arg(body, "kb", 128, 1024);
    let vbytes = usize_arg(body, "vbytes", 60, 4096);

    let storage = h.state.storage();
    let sql = storage.sql();
    sql.exec(drop, None)?;
    sql.exec(create, None)?;

    let answer = (|| -> worker::Result<Value> {
        let value = vec![0; vbytes];
        let row = if arm == "c" { (0..kb * 1024).map(|i| (i * 131 + 7) as u8).collect::<Vec<_>>() } else { Vec::new() };
        let mut statements = 0;
        let mut rows = 0;
        let mut bytes = 0;
        let mut tx_ms = 0;
        let loop_start = crate::now_ms();
        for tx in 0..n {
            // Build every statement, including blob clones, before timing this transaction.
            let mut stmts: Vec<crate::txn::Stmt> = Vec::new();
            match arm {
                "a" => {
                    for r in 0..k {
                        let number = tx * k + r;
                        let mut key = vec![0; 16];
                        key[..8].copy_from_slice(&(number as u64).to_le_bytes());
                        stmts.push((
                            AB_ONE,
                            vec![
                                SqlStorageValue::Blob(key),
                                SqlStorageValue::Integer(number as i64),
                                SqlStorageValue::Blob(value.clone()),
                            ],
                        ));
                    }
                }
                "b" => {
                    let mut first = 0;
                    for count in batch_sizes(k, 33) {
                        let mut args = Vec::with_capacity(count * 3);
                        for r in first..first + count {
                            let number = tx * k + r;
                            let mut key = vec![0; 16];
                            key[..8].copy_from_slice(&(number as u64).to_le_bytes());
                            args.extend([
                                SqlStorageValue::Blob(key),
                                SqlStorageValue::Integer(number as i64),
                                SqlStorageValue::Blob(value.clone()),
                            ]);
                        }
                        stmts.push((batch_sql(count), args));
                        first += count;
                    }
                }
                "c" => stmts.push((
                    C_ONE,
                    vec![
                        SqlStorageValue::Integer((tx + 1) as i64),
                        SqlStorageValue::Integer(1),
                        SqlStorageValue::Blob(vec![0; 32]),
                        SqlStorageValue::Blob(row.clone()),
                    ],
                )),
                _ => unreachable!(),
            }
            let statement_count = stmts.len();
            // Date.now in a Worker advances only on I/O, so txMs may read 0. Workers Logs' per-invocation cpuTimeMs
            // supplies CPU; these counts let its reader attribute that CPU to statements, rows, transactions and bytes.
            let start = crate::now_ms();
            crate::txn::commit_tx(&storage, stmts).map_err(worker::Error::RustError)?;
            tx_ms += crate::now_ms().saturating_sub(start);
            statements += statement_count;
            match arm {
                "a" | "b" => {
                    rows += k;
                    bytes += k * vbytes;
                }
                "c" => {
                    rows += 1;
                    bytes += kb * 1024;
                }
                _ => unreachable!(),
            }
        }
        let ms = crate::now_ms().saturating_sub(loop_start);
        Ok(json!({
            "arm": arm,
            "n": n,
            "k": (arm != "c").then_some(k),
            "kb": (arm == "c").then_some(kb),
            "vbytes": vbytes,
            "statements": statements,
            "rows": rows,
            "bytes": bytes,
            "txMs": tx_ms,
            "ms": ms,
        }))
    })();
    // Attempt cleanup even when a transaction failed; preserve that transaction's error if both operations fail.
    let dropped = sql.exec(drop, None);
    let answer = answer?;
    dropped?;
    Ok(answer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batches_split_at_the_binding_limit() {
        assert_eq!(batch_sizes(60, 33), [33, 27]);
        assert_eq!(batch_sizes(66, 33), [33, 33]);
        assert_eq!(batch_sizes(1, 33), [1]);
        assert_eq!(batch_sizes(0, 33), Vec::<usize>::new());
    }

    #[test]
    fn multi_row_statements_have_three_placeholders_a_row() {
        assert_eq!(multi_row_sql(1), AB_ONE);
        assert_eq!(multi_row_sql(2), "INSERT OR REPLACE INTO probe_ab (k0, written, v) VALUES (?, ?, ?), (?, ?, ?)");
        for rows in 1..=3 {
            assert_eq!(multi_row_sql(rows).matches('?').count(), 3 * rows);
        }
    }
}
