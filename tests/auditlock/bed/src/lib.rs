//! SQLITE_LOCKED bed (`tests/auditlock/bed/run.sh`): the archive alarm's two log walks on an idle leader, then the
//! fold audit's scratch `DROP TABLE`, in local workerd. `/walk?arm=main` reads the log as main 1155c957's
//! `SiteHost::view` and `SiteHost::rows` do (one lazy cursor each, stopped where `pack_bytes_above` and
//! `log_archive::due` stop); `arm=fix` reads it through this branch's `workers/site/src/logread.rs`. `/drop`, in a
//! later request as the audit's alarm is, drops and remakes a scratch table and answers `ok` or the error.
//! `/each?arm=` stops `each_row` after one row and drops in the same request.
use graftyard_git::log_archive::{SHORT_ROW, due, pack_bytes_above};
use graftyard_raft::{EntryBody, LogEntry, PREV_ZERO, Term, TierSet, codec};
use graftyard_types::{Index, Timestamp};
use worker::*;

#[path = "../../../../workers/site/src/logread.rs"]
#[allow(dead_code)]
mod logread;

const N: u64 = 200;
const BIG_SHORT: u64 = 1_000;
const BIG_LONG: u64 = 200;

fn int(v: &SqlStorageValue) -> u64 {
    match v {
        SqlStorageValue::Integer(i) => *i as u64,
        _ => 0,
    }
}
fn blob(v: &SqlStorageValue) -> Option<Vec<u8>> {
    match v {
        SqlStorageValue::Blob(b) => Some(b.clone()),
        _ => None,
    }
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
        let arm = url.query_pairs().find(|(k, _)| k == "arm").map(|(_, v)| v.to_string()).unwrap_or_default();
        let sql = self.state.storage().sql();
        sql.exec("CREATE TABLE IF NOT EXISTS log (idx INTEGER PRIMARY KEY, row BLOB NOT NULL)", None)?;
        sql.exec("CREATE TABLE IF NOT EXISTS i_m_comment (k0 INTEGER PRIMARY KEY)", None)?;
        let now = Date::now().as_millis();
        match url.path() {
            "/seed" => {
                for i in 1..=N {
                    let e: LogEntry<graftyard_git::repo::RepoEvent> = LogEntry {
                        term: Term(1),
                        index: Index(i),
                        prev: PREV_ZERO,
                        format: graftyard_git::repo::REPO_FORMAT_V1,
                        tiers: TierSet::CORE,
                        at: Timestamp(now - 1_000),
                        requests: Default::default(),
                        extend: None,
                        body: EntryBody::Events(vec![]),
                    };
                    sql.exec("INSERT OR REPLACE INTO log VALUES (?, ?)", vec![(i as i64).into(), SqlStorageValue::Blob(codec::to_bytes(&e))])?;
                }
                Response::ok("seeded")
            }
            "/walk" => {
                let now = Timestamp(now);
                let (d, unpromoted) = if arm == "main" {
                    let q = "SELECT length(row), CASE WHEN length(row) < ? THEN row END FROM log WHERE idx > ? ORDER BY idx";
                    let it = sql.exec(q, vec![(SHORT_ROW as i64).into(), 0i64.into()])?.raw().map_while(|r| {
                        let r = r.ok()?;
                        Some((int(&r[0]), blob(&r[1])))
                    });
                    let unpromoted = pack_bytes_above(it, 0);
                    let it = sql.exec("SELECT idx, row FROM log WHERE idx > ? ORDER BY idx", vec![0i64.into()])?.raw();
                    let rows = it.map_while(|r| {
                        let r = r.ok()?;
                        Some((Index(int(&r[0])), blob(&r[1])?))
                    });
                    (due(0, rows, now, true, Index(N), 2 << 30, 1 << 30).map_err(Error::RustError)?, unpromoted)
                } else {
                    let cols = format!("length(row), CASE WHEN length(row) < {SHORT_ROW} THEN row END");
                    let it = logread::log_after(&sql, &cols, 0).map_while(|r| {
                        let r = r.ok()?;
                        Some((int(&r[1]), blob(&r[2])))
                    });
                    let unpromoted = pack_bytes_above(it, 0);
                    let rows = logread::log_after(&sql, "row", 0).map_while(|r| {
                        let r = r.ok()?;
                        Some((Index(int(&r[0])), blob(&r[1])?))
                    });
                    (due(0, rows, now, true, Index(N), 2 << 30, 1 << 30).map_err(Error::RustError)?, unpromoted)
                };
                Response::ok(format!("due={} unpromoted={unpromoted}", d.is_some()))
            }
            "/each" => {
                let mut n = 0;
                let q = "SELECT idx FROM log ORDER BY idx";
                if arm == "main" {
                    for r in sql.exec(q, None)?.raw() {
                        r?;
                        n += 1;
                        break;
                    }
                } else {
                    logread::each_row(&sql, q, vec![], |_| {
                        n += 1;
                        Ok(false)
                    })?;
                }
                Response::ok(format!("read={n} drop={}", drop(&sql)))
            }
            // The page bound (`logread::PAGE_BYTES`): 1,000 rows of 1,500 B and then 200 of 300,000 B, as a leader's
            // log of short entries and then inline packs (B0-33 g3A and g4B); `/pages` walks them in the next request.
            "/seedbig" => {
                sql.exec("DELETE FROM log", None)?;
                for i in 1..=BIG_SHORT + BIG_LONG {
                    let n = if i <= BIG_SHORT { 1_500 } else { 300_000 };
                    sql.exec("INSERT INTO log VALUES (?, ?)", vec![(i as i64).into(), SqlStorageValue::Blob(vec![i as u8; n])])?;
                }
                Response::ok("seeded")
            }
            // Every row once, in order, and how much the isolate's wasm memory grew over the walk: main's pages, sized by
            // the page before, fetched the 184 long rows after the short ones in one page of about 55 MB.
            "/pages" => {
                let mem0 = core::arch::wasm32::memory_size::<0>() as u64 * 65_536;
                let (mut n, mut next, mut bytes) = (0u64, 1u64, 0u64);
                for r in logread::log_after(&sql, "row", 0) {
                    let r = r?;
                    if int(&r[0]) != next {
                        break;
                    }
                    bytes += blob(&r[1]).map_or(0, |b| b.len() as u64);
                    n += 1;
                    next += 1;
                }
                let grew = core::arch::wasm32::memory_size::<0>() as u64 * 65_536 - mem0;
                Response::ok(format!("rows={n} bytes={bytes} grew_mb={}", grew >> 20))
            }
            "/drop" => Response::ok(drop(&sql)),
            _ => Response::error("not found", 404),
        }
    }
}

fn drop(sql: &SqlStorage) -> String {
    match sql.exec("DROP TABLE IF EXISTS i_m_comment", None) {
        Ok(_) => match sql.exec("CREATE TABLE i_m_comment (k0 INTEGER PRIMARY KEY)", None) {
            Ok(_) => "ok".into(),
            Err(e) => e.to_string(),
        },
        Err(e) => e.to_string(),
    }
}

#[event(fetch)]
async fn fetch(req: Request, env: Env, _ctx: Context) -> Result<Response> {
    let url = req.url()?;
    let name = url.query_pairs().find(|(k, _)| k == "o").map(|(_, v)| v.to_string()).unwrap_or_default();
    env.durable_object("M")?.id_from_name(&name)?.get_stub()?.fetch_with_request(req).await
}
