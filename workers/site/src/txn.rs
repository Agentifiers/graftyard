//! One input's statements in one `storage.transactionSync`, and BF-12's double-apply evidence on test installs.
//!
//! A request row is inserted with `ON CONFLICT (rid) DO NOTHING`, so an insert that wrote no row met a request id
//! already on disk: the double apply BF-12 counts. On test installs that insert's `rows_written` is read, and a `dup`
//! row is written in the same transaction only then. The check used to be a second statement per request
//! (`INSERT INTO dup ... WHERE EXISTS`), 13 to 19% of a loaded leader's deferred statements
//! (`tests/am1/results/tx-mix.md`, cut 1); now a request costs one statement and the evidence still survives a reset.
//! `GroupFold::apply` refuses the same double apply first, in memory (`RequestStore::put`), with a panic; this is the
//! check against what storage holds. `tests/writecuts/bed` runs it in local workerd.

#[cfg(any(test, feature = "test-faults"))]
use worker::SqlStorageValue;
use worker::js_sys;
use worker::wasm_bindgen::closure::Closure;
use worker::wasm_bindgen::{JsCast, JsValue};

/// A statement and its arguments (`crate::family::Stmt`), named here so `tests/writecuts/bed` can build this file alone.
pub type Stmt = (&'static str, Vec<worker::SqlStorageValue>);

/// The request row's insert (`host::SqlStore`, `Write::Apply`): `rid`, `idx`, `at`, `events`.
pub const REQUEST_INSERT: &str =
    "INSERT INTO request (rid, idx, at, events) VALUES (?, ?, ?, ?) ON CONFLICT (rid) DO NOTHING";
/// BF-12's evidence row (`crate::faults::SCHEMA`, `dup`), read by `/audit` as `dupRows`.
#[cfg(any(test, feature = "test-faults"))]
const DUP_INSERT: &str = "INSERT INTO dup (rid, idx) VALUES (?, ?)";

/// The `dup` row to write after a statement ran: only for a request insert that wrote nothing.
#[cfg(any(test, feature = "test-faults"))]
fn dup_of(q: &str, args: &[SqlStorageValue], written: usize) -> Option<Stmt> {
    (q == REQUEST_INSERT && written == 0).then(|| (DUP_INSERT, args.iter().take(2).cloned().collect()))
}

thread_local!(static GENERATION: std::cell::Cell<u64> = const { std::cell::Cell::new(0) });

/// How many transactions `commit_tx` has started in this isolate. Every fold tier's read memo is dropped once it
/// moves (`crate::tier`, "Read memo"), so no memo outlives a transaction, even one that ran outside its tier's
/// buffers: an install's table swap, or a flush whose `flushed` did not reach the fold.
pub fn generation() -> u64 {
    GENERATION.with(std::cell::Cell::get)
}

/// Runs the statements in one `storage.transactionSync`, which workers-rs 0.8.7 does not bind.
pub fn commit_tx(storage: &worker::Storage, stmts: Vec<Stmt>) -> std::result::Result<(), String> {
    if stmts.is_empty() {
        return Ok(());
    }
    // Counted before it runs, so a memo never outlives a transaction that failed partway.
    GENERATION.with(|g| g.set(g.get() + 1));
    let sql = storage.sql();
    let raw: &JsValue = storage.as_raw().as_ref();
    let f = js_sys::Reflect::get(raw, &JsValue::from_str("transactionSync"))
        .ok()
        .and_then(|f| f.dyn_into::<js_sys::Function>().ok())
        .ok_or("no transactionSync")?;
    let cb = Closure::once_into_js(move || -> std::result::Result<JsValue, JsValue> {
        let fail = |e: worker::Error| JsValue::from_str(&e.to_string());
        for (q, a) in stmts {
            #[cfg(feature = "test-faults")]
            let keep = (q == REQUEST_INSERT).then(|| a.clone());
            let _ran = sql.exec(q, a).map_err(fail)?;
            #[cfg(feature = "test-faults")]
            if let Some(a) = keep
                && let Some((dq, da)) = dup_of(q, &a, _ran.rows_written())
            {
                sql.exec(dq, da).map_err(fail)?;
            }
        }
        Ok(JsValue::UNDEFINED)
    });
    f.call1(raw, &cb).map(|_| ()).map_err(|e| format!("{e:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only a request insert that wrote no row is a double apply; its `dup` row names the request and the index.
    #[test]
    fn only_a_request_insert_that_wrote_nothing_is_a_double_apply() {
        let args = || {
            vec![
                SqlStorageValue::String("01ABC".into()),
                SqlStorageValue::Integer(7),
                SqlStorageValue::Integer(1_000),
                SqlStorageValue::Blob(vec![1, 2]),
            ]
        };
        let (q, a) = dup_of(REQUEST_INSERT, &args(), 0).expect("a conflict on the request id");
        assert_eq!(q, DUP_INSERT);
        assert_eq!(a, vec![SqlStorageValue::String("01ABC".into()), SqlStorageValue::Integer(7)]);
        // A row and its `request_at` index entry: written, so not a double apply.
        assert_eq!(dup_of(REQUEST_INSERT, &args(), 2), None);
        assert_eq!(dup_of(REQUEST_INSERT, &args(), 1), None);
        // Any other statement that writes nothing (a retention DELETE with nothing to drop) is not one either.
        assert_eq!(dup_of("DELETE FROM request WHERE at < ?", &[SqlStorageValue::Integer(5)], 0), None);
    }
}
