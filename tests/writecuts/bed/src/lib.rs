//! BF-12's double-apply evidence in local workerd (`tests/writecuts/bed/run.sh`), through the member's own
//! `workers/site/src/txn.rs`: `/apply?rid=&idx=` runs one input's transaction holding a request insert, as
//! `host::SqlStore` builds it; `/dup` lists the `dup` rows. run.sh restarts workerd on its persisted state, as a member reset loses its isolate.
//! A first apply writes no `dup` row, a second apply of the same request id writes one naming the second index, and
//! the row is still there after the reset.
use worker::*;

#[path = "../../../../workers/site/src/txn.rs"]
mod txn;

#[durable_object]
pub struct M {
    state: State,
}

impl DurableObject for M {
    fn new(state: State, _env: Env) -> Self {
        Self { state }
    }

    async fn fetch(&self, req: Request) -> Result<Response> {
        let url = req.url()?;
        let q = |k: &str| url.query_pairs().find(|(n, _)| n == k).map(|(_, v)| v.to_string()).unwrap_or_default();
        let sql = self.state.storage().sql();
        sql.exec("CREATE TABLE IF NOT EXISTS request (rid TEXT PRIMARY KEY, idx INTEGER NOT NULL, at INTEGER NOT NULL, events BLOB NOT NULL)", None)?;
        sql.exec("CREATE INDEX IF NOT EXISTS request_at ON request (at)", None)?;
        sql.exec("CREATE TABLE IF NOT EXISTS dup (rid TEXT NOT NULL, idx INTEGER NOT NULL)", None)?;
        match url.path() {
            "/apply" => {
                let idx: i64 = q("idx").parse().unwrap_or(1);
                let stmts: Vec<txn::Stmt> = vec![
                    ("DELETE FROM request WHERE at < ?", vec![SqlStorageValue::Integer(0)]),
                    (
                        txn::REQUEST_INSERT,
                        vec![q("rid").into(), SqlStorageValue::Integer(idx), SqlStorageValue::Integer(1_000), vec![1u8, 2].into()],
                    ),
                ];
                txn::commit_tx(&self.state.storage(), stmts).map_err(Error::RustError)?;
                Response::ok("applied")
            }
            "/dup" => {
                let rows: Vec<String> = sql
                    .exec("SELECT rid, idx FROM dup ORDER BY idx", None)?
                    .raw()
                    .filter_map(|r| r.ok())
                    .map(|r| match (&r[0], &r[1]) {
                        (SqlStorageValue::String(s), SqlStorageValue::Integer(i)) => format!("{s}@{i}"),
                        _ => "?".into(),
                    })
                    .collect();
                Response::ok(rows.join(","))
            }
            _ => Response::error("not found", 404),
        }
    }
}

#[event(fetch)]
async fn fetch(req: Request, env: Env, _ctx: Context) -> Result<Response> {
    let url = req.url()?;
    let name = url.query_pairs().find(|(k, _)| k == "o").map(|(_, v)| v.to_string()).unwrap_or_default();
    env.durable_object("M")?.id_from_name(&name)?.get_stub()?.fetch_with_request(req).await
}
