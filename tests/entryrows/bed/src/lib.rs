//! Rows written per entry, counted by local workerd (`tests/entryrows/run.sh`). Each object applies one entry the way
//! a member's `host::SqlStore` does, and answers each statement's `rows_written()`, the counter Cloudflare bills.
//!
//! - `/repo?kind=K` builds a repository chain's two tiers with the member's own `workers/site/src/tier.rs` (its DDL,
//!   its `seal` statements, and the metadata tier's FTS5 rows), applies `crates/git/tests/fold_corpus.json`'s setup
//!   entry, then entry 2 holding fixture `K`, and counts entry 2: the `request` expiry delete, the request row, the
//!   `log` row, one `raft_state` row, and the tiers' sealed statements (fold rows and FTS5 rows apart).
//! - `/blob` does the same for a family whose fold is one blob row (`family::store_blob`: Actions, Identity, Tokens,
//!   Names, Authz, Ids), with a fold row already there, as on every entry after the first.
//! - `/deletes` counts the steady-state deletes an entry causes later: an expired `request` row, a `_digest` row past
//!   `DIGESTS_KEEP`, and a `log` row truncated after its archive.
//!
//! The member DDL below is copied from `workers/site/src/host.rs` `SCHEMA`; run.sh fails if any line is not there
//! verbatim.
use graftyard_fold::{Store, TierStore, Val};
use graftyard_git::repo::{RepoEvent, RepoFold};
use graftyard_raft::{EntryBody, Fold, Format, LogEntry, PREV_ZERO, Term, TierSet, codec, digest};
use graftyard_types::{AccountId, DbId, Index, RepoId, Timestamp};
use serde_json::{Value, json};
use worker::*;

mod family {
    pub type Stmt = (&'static str, Vec<worker::SqlStorageValue>);
}

mod host {
    pub fn text(v: &worker::SqlStorageValue) -> worker::Result<&str> {
        match v {
            worker::SqlStorageValue::String(s) => Ok(s),
            other => Err(worker::Error::RustError(format!("expected text, got {other:?}"))),
        }
    }
}

#[path = "../../../../workers/site/src/tier.rs"]
#[allow(dead_code, unexpected_cfgs)]
mod tier;

#[path = "../../../../workers/site/src/txn.rs"]
#[allow(dead_code)]
mod txn;

const SCHEMA: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS raft_state (id INTEGER PRIMARY KEY CHECK (id = 1), row BLOB NOT NULL)",
    "CREATE TABLE IF NOT EXISTS log (idx INTEGER PRIMARY KEY, term INTEGER NOT NULL, digest BLOB NOT NULL, row BLOB NOT NULL)",
    "CREATE TABLE IF NOT EXISTS fold (id INTEGER PRIMARY KEY CHECK (id = 1), row BLOB NOT NULL)",
    "CREATE TABLE IF NOT EXISTS request (rid TEXT PRIMARY KEY, idx INTEGER NOT NULL, at INTEGER NOT NULL, events BLOB NOT NULL)",
    "CREATE INDEX IF NOT EXISTS request_at ON request (at)",
];
const ARRIVAL: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS arrival (seq INTEGER PRIMARY KEY AUTOINCREMENT, rid TEXT NOT NULL, at INTEGER NOT NULL, role TEXT NOT NULL, fault TEXT, answer TEXT NOT NULL)",
    "CREATE INDEX IF NOT EXISTS arrival_at ON arrival (at)",
];
const CORPUS: &str = include_str!("../../../../crates/git/tests/fold_corpus.json");
/// The newest repository format (`crates/git/tests/fold_corpus.rs`, `NEWEST`).
const NEWEST: u16 = 25;

/// `fold_corpus.rs`'s fixture shorthand: `"@ref:x"` a ref name's bytes, `"@oid:ab"` 20 bytes, `"@d:ab"` 32 bytes.
fn expand(v: Value) -> Value {
    match v {
        Value::String(s) => {
            let bytes = |b: Vec<u8>| Value::Array(b.into_iter().map(Value::from).collect());
            let fill = |h: &str, n: usize| bytes(vec![u8::from_str_radix(h, 16).expect("a hex byte"); n]);
            if let Some(r) = s.strip_prefix("@ref:") {
                bytes(r.as_bytes().to_vec())
            } else if let Some(h) = s.strip_prefix("@oid:") {
                fill(h, 20)
            } else if let Some(h) = s.strip_prefix("@d:") {
                fill(h, 32)
            } else {
                Value::String(s)
            }
        }
        Value::Array(a) => Value::Array(a.into_iter().map(expand).collect()),
        Value::Object(o) => Value::Object(o.into_iter().map(|(k, v)| (k, expand(v))).collect()),
        v => v,
    }
}

fn entry(index: u64, prev: graftyard_types::Digest, events: Vec<RepoEvent>) -> LogEntry<RepoEvent> {
    let tiers = events.iter().fold(TierSet::NONE, |t, e| t.union(<RepoFold as Fold>::tiers(e)));
    LogEntry {
        term: Term(2),
        index: Index(index),
        prev,
        format: Format { family: 0x20, version: NEWEST },
        tiers,
        at: Timestamp(1_000 * index),
        requests: Default::default(),
        extend: None,
        body: EntryBody::Events(events),
    }
}

fn i(v: u64) -> SqlStorageValue {
    SqlStorageValue::Integer(v as i64)
}

/// Runs each statement, adding its rows written to its part.
struct Count<'a> {
    sql: &'a SqlStorage,
    parts: serde_json::Map<String, Value>,
}

impl Count<'_> {
    fn run(&mut self, part: &str, q: &str, args: Vec<SqlStorageValue>) -> Result<()> {
        let n = self.sql.exec(q, args)?.rows_written() as u64;
        let have = self.parts.get(part).and_then(Value::as_u64).unwrap_or(0);
        self.parts.insert(part.into(), json!(have + n));
        Ok(())
    }
}

/// The statements `host::SqlStore` writes for an applied entry besides the fold's own: the expiry delete, one request
/// row, the stored log row, and the member's state row.
fn host_rows(c: &mut Count, idx: u64, rid: &str, at: u64, row: Vec<u8>) -> Result<()> {
    c.run("request", "DELETE FROM request WHERE at < ?", vec![i(0)])?;
    c.run("request", txn::REQUEST_INSERT, vec![rid.into(), i(idx), i(at), vec![1u8; 40].into()])?;
    c.run(
        "log",
        "INSERT OR REPLACE INTO log (idx, term, digest, row) VALUES (?, ?, ?, ?)",
        vec![i(idx), i(2), vec![7u8; 32].into(), row.into()],
    )?;
    c.run("raft_state", "INSERT OR REPLACE INTO raft_state (id, row) VALUES (1, ?)", vec![vec![9u8; 60].into()])
}

/// Seals both tiers' buffered writes and runs them, fold rows and FTS5 rows apart.
fn seal(c: &mut Count, fold: &RepoFold) -> Result<()> {
    let mut stmts = Vec::new();
    tier::handle(&*fold.core).seal(&mut stmts);
    tier::handle(&*fold.meta).seal(&mut stmts);
    for (q, a) in stmts {
        let part = if q.contains("_fts") { "fts" } else { "fold" };
        c.run(part, q, a)?;
    }
    tier::handle(&*fold.core).flushed();
    tier::handle(&*fold.meta).flushed();
    Ok(())
}

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
        for s in SCHEMA {
            sql.exec(s, None)?;
        }
        let mut c = Count { sql: &sql, parts: Default::default() };
        match url.path() {
            "/repo" => {
                let corpus: Value = serde_json::from_str(CORPUS)?;
                let decode = |v: &Value| {
                    serde_json::from_value::<RepoEvent>(expand(v.clone())).map_err(|e| Error::RustError(e.to_string()))
                };
                let mut setup: Vec<Value> = corpus["setup"].as_array().unwrap().clone();
                // `kind=A,B` puts fixtures A and B in one entry, as a push entry carries `Refs`, `Provenance` and
                // `PathSet`. `setup=N` instead moves setup event N into the counted entry (`setup=4`: the
                // `Labels Defined` that K12 measured hosted as `DefineLabel`).
                let mut events = Vec::new();
                if let Ok(n) = q("setup").parse::<usize>() {
                    events.push(decode(&setup.remove(n))?);
                } else {
                    for kind in q("kind").split(',') {
                        let fixture = corpus["events"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .find(|v| v.as_object().and_then(|o| o.keys().next()).map(String::as_str) == Some(kind))
                            .ok_or_else(|| Error::RustError(format!("no fixture {kind}")))?;
                        events.push(decode(fixture)?);
                    }
                }
                let setup: Vec<RepoEvent> = setup.iter().map(decode).collect::<Result<_>>()?;
                // `wr=1`: every tier table as `WITHOUT ROWID`, made before `repo_tiers`, whose `IF NOT EXISTS` then
                // keeps them (the `without-rowid` lever, not built).
                if q("wr") == "1" {
                    for schema in [graftyard_git::tiers::CORE, graftyard_git::tiers::METADATA] {
                        for d in tier::ddl(&schema, "") {
                            if !d.contains("VIRTUAL") {
                                sql.exec(&format!("{d} WITHOUT ROWID"), None)?;
                            }
                        }
                    }
                }
                let (core, meta) = tier::repo_tiers(&sql);
                let mut fold =
                    RepoFold::with_tiers(Some(RepoId(DbId(7))), Some(AccountId(DbId(1))), Box::new(core), Box::new(meta));
                let first = entry(1, PREV_ZERO, setup);
                fold.apply_entry(&first);
                // The setup entry lands first and is not counted.
                let mut s = Count { sql: &sql, parts: Default::default() };
                seal(&mut s, &fold)?;
                host_rows(&mut s, 1, "r1", 1_000, codec::to_bytes(&first))?;
                let second = entry(2, digest(&first), events);
                fold.apply_entry(&second);
                seal(&mut c, &fold)?;
                host_rows(&mut c, 2, "r2", 2_000, codec::to_bytes(&second))?;
            }
            "/blob" => {
                let put = "INSERT OR REPLACE INTO fold (id, row) VALUES (1, ?)";
                sql.exec(put, vec![vec![3u8; 2_000].into()])?;
                sql.exec(txn::REQUEST_INSERT, vec!["r1".into(), i(1), i(1_000), vec![1u8; 40].into()])?;
                c.run("fold", put, vec![vec![4u8; 2_100].into()])?;
                host_rows(&mut c, 2, "r2", 2_000, vec![5u8; 300])?;
            }
            "/deletes" => {
                sql.exec(txn::REQUEST_INSERT, vec!["r1".into(), i(1), i(1_000), vec![1u8; 40].into()])?;
                sql.exec(
                    "INSERT OR REPLACE INTO log (idx, term, digest, row) VALUES (?, ?, ?, ?)",
                    vec![i(1), i(2), vec![7u8; 32].into(), vec![5u8; 300].into()],
                )?;
                let (mut core, _) = tier::repo_tiers(&sql);
                let key = vec![Val::Int(1)];
                let stored = graftyard_fold::Stored { written: Index(1), cols: vec![Val::Blob(vec![8u8; 32].into())] };
                core.put(graftyard_fold::DIGESTS, key.clone(), stored);
                let mut stmts = Vec::new();
                core.seal(&mut stmts);
                for (q, a) in stmts {
                    sql.exec(q, a)?;
                }
                core.flushed();
                c.run("request expired", "DELETE FROM request WHERE at < ?", vec![i(2_000)])?;
                core.delete(graftyard_fold::DIGESTS, &key);
                let mut stmts = Vec::new();
                core.seal(&mut stmts);
                for (q, a) in stmts {
                    c.run("_digest past DIGESTS_KEEP", q, a)?;
                }
                c.run("log truncated", "DELETE FROM log WHERE idx < ?", vec![i(2)])?;
                let _ = core.loaded();
            }
            // Test installs only: the `arrival` row the leader writes for each `/commit` (`workers/site/src/member.rs`),
            // in `workers/site/src/faults.rs`' DDL, so K12's hosted 19 can be split.
            "/arrival" => {
                sql.exec(ARRIVAL[0], None)?;
                sql.exec(ARRIVAL[1], None)?;
                let ins = "INSERT INTO arrival (rid, at, role, fault, answer) VALUES (?, ?, ?, ?, ?)";
                let args = |r: &str| vec![r.into(), i(1), "leader".into(), SqlStorageValue::Null, "ok".into()];
                sql.exec(ins, args("r0"))?;
                c.run("arrival", ins, args("r1"))?;
            }
            // Bytes per row on disk: `n` rows of `b`-byte values in a two-column-key table of `tier.rs`' shape, as built
            // or `WITHOUT ROWID` (`wr=1`), from the database size before and after.
            "/size" => {
                let (n, b): (i64, usize) = (q("n").parse().unwrap_or(2_000), q("b").parse().unwrap_or(800));
                let wr = if q("wr") == "1" { " WITHOUT ROWID" } else { "" };
                sql.exec(&format!("CREATE TABLE t (k0, k1, written INTEGER NOT NULL, v BLOB NOT NULL, PRIMARY KEY (k0, k1)){wr}"), None)?;
                let before = sql.database_size() as i64;
                for k in 0..n {
                    // Values that do not compress: a fold row's postcard is not run-length.
                    let v: Vec<u8> = (0..b).map(|j| ((k as usize * 31 + j * 17) % 251) as u8).collect();
                    let args = vec![i(7), i(k as u64), i(1), v.into()];
                    sql.exec("INSERT INTO t (k0, k1, written, v) VALUES (?, ?, ?, ?)", args)?;
                }
                c.parts.insert("bytesPerRow".into(), json!((sql.database_size() as i64 - before) / n));
            }
            _ => return Response::error("not found", 404),
        }
        Response::from_json(&Value::Object(c.parts))
    }
}

#[event(fetch)]
async fn fetch(req: Request, env: Env, _ctx: Context) -> Result<Response> {
    let url = req.url()?;
    let name = url.query_pairs().find(|(k, _)| k == "o").map(|(_, v)| v.to_string()).unwrap_or_default();
    env.durable_object("M")?.id_from_name(&name)?.get_stub()?.fetch_with_request(req).await
}
