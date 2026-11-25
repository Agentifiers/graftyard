//! The change feed's consumer (`design/DESIGN.md`, Shape, "Lifeboat feed"; `decisions/DECISIONS.md`, `export-host`).
//! The pure half is `graftyard_git::export`.
//!
//! - **Reports.** Each leader sends its commit index and record to the feed site's registry object `export`
//!   (`Host::report_commit`), at most once a round and only once it moved. The object keeps the highest per chain.
//! - **Rounds.** Only the site that binds the export bucket (`EXPORT`, site A) runs them. The cron kicks one, and each
//!   round that ran sends the next as a queue message due FEED_EVERY_MS after its own claim, at once if it ran longer
//!   (`export::next_round_delay_s`). The object's `export_round` claim collapses a kick and a message that meet into
//!   one round (`export::round_due`).
//! - **One round.** It reads `FeedState`, folds in any object written after it, plans the next batch from the reports,
//!   reads each chain's rows from a learner or follower (`graftyard_lookup::log_rows`, never the leader), six at a time,
//!   fills and checks the object, writes it create-only, and then writes the state. A round that dies between the two
//!   writes is recovered by the next one's fold.
//! - **Chains no leader reports.** A site day resets every member, and a group nothing calls elects no leader, so it
//!   never reports (m3fx49b: `release` and `ids`). Each round also probes, through the same non-leader source read
//!   with an empty range, the commit index of `Release`, `Ids`, and the chains of one sweep slot, reported or not (a
//!   report can be left behind by a leader reset before its next one): one `MemberRegistry` shard of one site (`export::sweep_slots`, the claim's counter), each chain on
//!   its first site (`export::swept`). A probe that passed the cursor plans the chain as a report would
//!   (`export::with_probes`), so every chain reaches the feed within one sweep, led or not.

use std::collections::BTreeMap;

use graftyard_git::export::{self, FeedState};
use graftyard_git::key::R2Key;
use graftyard_lookup::{Bindings, LogRows};
use graftyard_types::{ChainId, Index, MemberRegistry, PlacementRecord, RegistryRow, SiteId};
use serde_json::{Value, json};
use worker::*;

use crate::SiteEnv;

/// The registry object that holds the reports and the round claim, on the feed site.
const OBJECT: &str = "export";
/// How many source reads a round runs at once: a Worker's connections waiting for headers.
const READS_AT_ONCE: usize = 6;

// The registry object's SQL (`crate::registry`, `/export_report` and `/export_round`). The column is `idx`, not
// `commit`, which SQLite reserves: m3fx49's reports all failed on it. `tests::sql_prepares` runs each in SQLite.
pub(crate) const REPORTS_SCHEMA: &str =
    "CREATE TABLE IF NOT EXISTS export_report (chain TEXT PRIMARY KEY, idx INTEGER NOT NULL, record TEXT NOT NULL)";
pub(crate) const REPORT_HELD: &str = "SELECT idx FROM export_report WHERE chain = ?";
pub(crate) const REPORT_PUT: &str = "INSERT OR REPLACE INTO export_report (chain, idx, record) VALUES (?, ?, ?)";
pub(crate) const REPORTS_ALL: &str = "SELECT chain, idx, record FROM export_report";
// A retired chain's report, dropped by its purge or by a round that proved it retired (`decisions/DECISIONS.md`,
// "`feed-retired`"): no leader of it reports again, so a kept row would be planned and unread every round for good.
pub(crate) const REPORT_DROP: &str = "DELETE FROM export_report WHERE chain = ?";
// When a purge first found the feed behind a chain (`/export_purge_wait`), so its wait is capped across the purge's
// calls (`export::purge_wait`); dropped with the report.
pub(crate) const PURGE_WAIT_SCHEMA: &str =
    "CREATE TABLE IF NOT EXISTS export_purge_wait (chain TEXT PRIMARY KEY, since INTEGER NOT NULL)";
pub(crate) const PURGE_WAIT_START: &str = "INSERT OR IGNORE INTO export_purge_wait (chain, since) VALUES (?, ?)";
pub(crate) const PURGE_WAIT_GET: &str = "SELECT since FROM export_purge_wait WHERE chain = ?";
pub(crate) const PURGE_WAIT_DROP: &str = "DELETE FROM export_purge_wait WHERE chain = ?";
pub(crate) const ROUND_SCHEMA: &str =
    "CREATE TABLE IF NOT EXISTS export_round (k INTEGER PRIMARY KEY CHECK (k = 0), last INTEGER NOT NULL)";
pub(crate) const ROUND_LAST: &str = "SELECT last FROM export_round WHERE k = 0";
pub(crate) const ROUND_PUT: &str = "INSERT OR REPLACE INTO export_round (k, last) VALUES (0, ?)";
// The sweep's round counter, its own table so the `export_round` rows an install already holds keep their shape.
pub(crate) const SWEEP_SCHEMA: &str =
    "CREATE TABLE IF NOT EXISTS export_sweep (k INTEGER PRIMARY KEY CHECK (k = 0), n INTEGER NOT NULL)";
pub(crate) const SWEEP_N: &str = "SELECT n FROM export_sweep WHERE k = 0";
pub(crate) const SWEEP_PUT: &str = "INSERT OR REPLACE INTO export_sweep (k, n) VALUES (0, ?)";
// Test installs only: the feed hold (`/export_hold`). While a row is held, no round is due, so nothing new reaches the
// feed and a row can archive a repository the feed never carried (BF-53's R59 read, `decisions/DECISIONS.md`,
// "`lifeboat-turns`"). Its own table, so a product build's `export_round` claim reads no new column.
#[cfg(any(test, feature = "test-faults"))]
pub(crate) const HOLD_SCHEMA: &str =
    "CREATE TABLE IF NOT EXISTS export_hold (k INTEGER PRIMARY KEY CHECK (k = 0), held INTEGER NOT NULL)";
#[cfg(any(test, feature = "test-faults"))]
pub(crate) const HOLD_GET: &str = "SELECT held FROM export_hold WHERE k = 0";
#[cfg(any(test, feature = "test-faults"))]
pub(crate) const HOLD_PUT: &str = "INSERT OR REPLACE INTO export_hold (k, held) VALUES (0, ?)";

/// The next round's queue message, `{"exportFeed": true}`. A struct, not `json!`: the queue binding serializes its body
/// through `serde_wasm_bindgen`, which makes a `serde_json` map a JS `Map`, and the JSON content type sends a `Map` as
/// `{}`, which the consumer read as a `PrSync` batch missing `repo` (m3fx49b: no round but the cron's).
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct NextRound {
    export_feed: bool,
}

/// The feed site: the install's first site, which holds the export bucket.
fn feed_site(site: &SiteEnv) -> SiteId {
    site.sites.0.iter().map(|s| s.id).min().unwrap_or(SiteId(0))
}

/// A leader's report: its chain's commit index and the record its members are found under.
pub async fn report(env: &Env, chain: ChainId, commit: Index, record: &PlacementRecord) -> Result<()> {
    let site = SiteEnv::read(env)?;
    let stub = site.registry(env, feed_site(&site), OBJECT)?;
    let body = json!({ "chain": chain.to_string(), "commit": commit.0, "record": record }).to_string();
    let mut init = RequestInit::new();
    init.with_method(Method::Put).with_body(Some(body.into()));
    let res = stub.fetch_with_request(Request::new_with_init("https://registry/export_report", &init)?).await?;
    if res.status_code() != 200 {
        return Err(Error::RustError(format!("registry answered {}", res.status_code())));
    }
    Ok(())
}

/// The cron's kick or the queue's message: a round when one is due, then the next message. A site without the export
/// bucket does nothing.
pub async fn kick(env: &Env) {
    let Ok(bucket) = env.bucket("EXPORT") else { return };
    let (reports, sweep) = match claim(env).await {
        Ok(Some(r)) => r,
        Ok(None) => return,
        Err(e) => return console_error!("export feed: the round claim failed: {e}"),
    };
    let claimed = crate::now_ms();
    match Box::pin(round(env, &bucket, reports, sweep)).await {
        Ok(line) => console_log!("export feed {line}"),
        Err(e) => console_error!("export feed round failed: {e}"),
    }
    let next = MessageBuilder::new(NextRound { export_feed: true })
        .delay_seconds(export::next_round_delay_s(claimed, crate::now_ms()))
        .build();
    if let Err(e) = async { env.queue("PRSYNC")?.send(next).await }.await {
        console_error!("export feed: the next round's message failed, the cron restarts it: {e}");
    }
}

/// A round's reports, each a chain's commit index and the record its members are found under.
type Reports = Vec<(ChainId, Index, PlacementRecord)>;

/// The round claim: the reports and the sweep's round number when a round is due, `None` when another started within
/// FEED_EVERY_MS.
async fn claim(env: &Env) -> Result<Option<(Reports, u64)>> {
    let site = SiteEnv::read(env)?;
    let stub = site.registry(env, feed_site(&site), OBJECT)?;
    let mut init = RequestInit::new();
    init.with_method(Method::Post);
    let mut res = stub.fetch_with_request(Request::new_with_init("https://registry/export_round", &init)?).await?;
    let v: Value = res.json().await?;
    if v["due"] != json!(true) {
        return Ok(None);
    }
    let reports = v["reports"].as_array().cloned().unwrap_or_default();
    let reports = reports
        .into_iter()
        .filter_map(|r| {
            let chain = r["chain"].as_str()?.parse().ok()?;
            let record = serde_json::from_value(r["record"].clone()).ok()?;
            Some((chain, Index(r["commit"].as_u64()?), record))
        })
        .collect();
    Ok(Some((reports, v["sweep"].as_u64().unwrap_or(0))))
}

/// The chains round `sweep` probes, each with the record it is read under: `Release` and `Ids` from the genesis
/// (`release::system_record`), then the chains of the sweep's slot from that site's `MemberRegistry` shard, under
/// their rows' records. A record or a registry read that fails is logged and skipped; the next sweep asks again.
/// Drops `chain`'s report in the feed site's `export` object (`/export_report` DELETE), which a purge and a round that
/// proved the chain retired both send.
pub async fn drop_report(env: &Env, chain: ChainId) -> Result<()> {
    let site = SiteEnv::read(env)?;
    let stub = site.registry(env, feed_site(&site), OBJECT)?;
    let mut init = RequestInit::new();
    init.with_method(Method::Delete);
    let url = format!("https://registry/export_report?chain={chain}");
    let res = stub.fetch_with_request(Request::new_with_init(&url, &init)?).await?;
    if res.status_code() != 200 {
        return Err(Error::RustError(format!("registry answered {}", res.status_code())));
    }
    Ok(())
}

async fn probe_targets(env: &Env, site: &SiteEnv, sweep: u64) -> Vec<(ChainId, PlacementRecord)> {
    let mut out = Vec::new();
    for c in export::ROUND_PROBES {
        match crate::release::system_record(env, c).await {
            Ok(r) => out.push((c, r)),
            Err(e) => console_error!("export feed: no record for {c}: {e}"),
        }
    }
    let live: Vec<SiteId> = site.sites.0.iter().filter(|s| !s.spare).map(|s| s.id).collect();
    for (i, shard) in export::sweep_slots(sweep, export::SWEEP_SHARDS, live.len()) {
        let at = live[i];
        let read = async {
            let stub = site.registry(env, at, &MemberRegistry { site: at, shard }.do_name())?;
            let mut res = stub.fetch_with_str(&format!("https://registry/rows?site={}", at.0)).await?;
            res.json::<Vec<RegistryRow>>().await
        };
        match read.await {
            Ok(rows) => out.extend(export::swept(&rows, at).into_iter().filter_map(|r| Some((r.chain, r.record()?)))),
            Err(e) => console_error!("export feed: the sweep's registry#{shard} read on site {} failed: {e}", at.0),
        }
    }
    out
}

async fn get(bucket: &Bucket, key: &str) -> Result<Option<Vec<u8>>> {
    match bucket.get(key).execute().await? {
        Some(o) => Ok(Some(o.body().ok_or_else(|| Error::RustError(format!("{key}: no body")))?.bytes().await?)),
        None => Ok(None),
    }
}

/// One round, as `export::plan`, `fill`, and `check_object` decide it. Returns its log line.
async fn round(env: &Env, bucket: &Bucket, reports: Reports, sweep: u64) -> Result<String> {
    let site = SiteEnv::read(env)?;
    let b = Bindings { env, sites: &site.sites };
    let err = |e: String| Error::RustError(e);
    let mut state = match get(bucket, &R2Key::ExportState.to_string()).await? {
        Some(bytes) => FeedState::decode(&bytes).map_err(err)?,
        None => FeedState::default(),
    };
    // Objects a round wrote before it could write the state: folded in, oldest first.
    let mut listed = bucket.list().prefix("export/");
    if let Some(l) = state.last {
        listed = listed.start_after(l.key().to_string());
    }
    let mut later = Vec::new();
    for o in listed.execute().await?.objects() {
        if let Some(bytes) = get(bucket, &o.key()).await? {
            later.push(export::decode_object(&bytes).map_err(err)?.batch);
        }
    }
    let recovered = later.len();
    state.recover(&later);
    let reported: Vec<(ChainId, Index)> = reports.iter().map(|(c, i, _)| (*c, *i)).collect();
    let mut records: BTreeMap<ChainId, &PlacementRecord> = reports.iter().map(|(c, _, r)| (*c, r)).collect();
    // The probes: an empty range (`to` = the cursor) from a non-leader, for its commit. A chain with a report is probed
    // too: a leader that reported once and reset before its next report leaves the report behind its log for good.
    let targets = probe_targets(env, &site, sweep).await;
    let b = &b;
    let mut probes = Vec::new();
    for part in targets.chunks(READS_AT_ONCE) {
        let reads = part.iter().map(|(c, record)| {
            let at = state.cursors.get(*c).0;
            async move { (*c, graftyard_lookup::log_rows(b, *c, record, at + 1, at).await) }
        });
        for (c, got) in futures_util::future::join_all(reads).await {
            if let LogRows::Page(_, bytes) = got
                && let Ok(p) = export::decode_log_page(&bytes)
            {
                probes.push((c, p.commit));
            }
        }
    }
    for (c, r) in &targets {
        records.entry(*c).or_insert(r);
    }
    let probed = format!("reports={} probed={}/{}", reports.len(), probes.len(), targets.len());
    let commits = export::with_probes(&reported, &probes);
    let Some(batch) = export::plan(&state, &commits, crate::now_ms()) else {
        if recovered > 0 {
            bucket.put(R2Key::ExportState.to_string(), state.encode()).execute().await?;
        }
        return Ok(format!("idle {probed} recovered={recovered}"));
    };
    let mut fetched = BTreeMap::new();
    let mut unjoined = Vec::new();
    for part in batch.chains.chunks(READS_AT_ONCE) {
        let reads = part.iter().filter_map(|(c, to)| {
            let record = records.get(c)?;
            let from = state.cursors.get(*c).0 + 1;
            Some(async move { (*c, graftyard_lookup::log_rows(b, *c, record, from, to.0).await) })
        });
        for (c, got) in futures_util::future::join_all(reads).await {
            match got {
                LogRows::Page(_, bytes) => {
                    if let Some(f) = export::decode_log_page(&bytes).ok().and_then(|p| p.fetched()) {
                        fetched.insert(c, f);
                    }
                }
                LogRows::Unjoined => unjoined.push(c),
                LogRows::Missed => {}
            }
        }
    }
    // A planned chain every source answered not joined: its report is dropped only once the members prove the group
    // retired (`export::forgets`), with its unexported tail logged. A live chain whose sources fail keeps its report.
    for (c, to) in batch.chains.iter().filter(|(c, _)| unjoined.contains(c)) {
        let Some(record) = records.get(c) else { continue };
        let retired = Box::pin(graftyard_lookup::retired_group(b, *c, record)).await;
        if export::forgets(export::Unread::Unjoined, retired) {
            console_error!("{}", export::unexported_line(*c, Index(state.cursors.get(*c).0 + 1), *to));
            if let Err(e) = drop_report(env, *c).await {
                console_error!("export feed: the retired {c}'s report was not dropped, the next round asks again: {e}");
            }
        }
    }
    let read = fetched.len();
    let Some(object) = export::fill(&batch, &state.cursors, &fetched) else {
        return Ok(format!("planned={} read=0: every source failed, the next round asks again", batch.chains.len()));
    };
    export::check_object(&object, &state.cursors).map_err(err)?;
    let key = object.batch.key().to_string();
    let put = bucket
        .put(&key, export::encode_object(&object))
        .only_if(Conditional { etag_does_not_match: Some("*".into()), ..Default::default() })
        .execute()
        .await?;
    if put.is_none() {
        return Ok(format!("{key} exists: another round wrote it, the next round folds it in"));
    }
    state.advance(&object.batch);
    bucket.put(R2Key::ExportState.to_string(), state.encode()).execute().await?;
    let rows: usize = object.entries.iter().map(|(_, _, r)| r.len()).sum();
    Ok(format!(
        "{key} {probed} planned={} read={read} chains={} entries={rows} recovered={recovered}",
        batch.chains.len(),
        object.batch.chains.len()
    ))
}

#[cfg(test)]
mod tests {
    use rusqlite::{Connection, params};

    use super::{
        HOLD_GET, HOLD_PUT, HOLD_SCHEMA, PURGE_WAIT_DROP, PURGE_WAIT_GET, PURGE_WAIT_SCHEMA, PURGE_WAIT_START,
        REPORT_DROP, REPORT_HELD, REPORT_PUT, REPORTS_ALL, REPORTS_SCHEMA, ROUND_LAST, ROUND_PUT, ROUND_SCHEMA,
        SWEEP_N, SWEEP_PUT, SWEEP_SCHEMA,
    };

    /// Every statement the registry object runs for the feed prepares and round-trips in SQLite. No other native
    /// test runs this SQL, and a reserved word as a column fails only here or on a hosted install.
    #[test]
    fn sql_prepares() {
        let db = Connection::open_in_memory().unwrap();
        db.execute(REPORTS_SCHEMA, []).unwrap();
        db.execute(ROUND_SCHEMA, []).unwrap();
        db.execute(REPORT_PUT, params!["repo:65609", 10, "{}"]).unwrap();
        db.execute(REPORT_PUT, params!["repo:65609", 12, "{\"r\":1}"]).unwrap();
        db.execute(REPORT_PUT, params!["repo:65610", 3, "{}"]).unwrap();
        db.execute(REPORT_DROP, ["repo:65610"]).unwrap();
        let left: i64 =
            db.query_row("SELECT COUNT(*) FROM export_report WHERE chain = 'repo:65610'", [], |r| r.get(0)).unwrap();
        assert_eq!(left, 0, "the dropped report is gone");
        db.execute(PURGE_WAIT_SCHEMA, []).unwrap();
        db.execute(PURGE_WAIT_START, params!["repo:65610", 5]).unwrap();
        db.execute(PURGE_WAIT_START, params!["repo:65610", 9]).unwrap();
        let since: i64 = db.query_row(PURGE_WAIT_GET, ["repo:65610"], |r| r.get(0)).unwrap();
        assert_eq!(since, 5, "the first call's start stands");
        db.execute(PURGE_WAIT_DROP, ["repo:65610"]).unwrap();
        let held: i64 = db.query_row(REPORT_HELD, ["repo:65609"], |r| r.get(0)).unwrap();
        assert_eq!(held, 12);
        let all: Vec<(String, i64, String)> = db
            .prepare(REPORTS_ALL)
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(all, vec![("repo:65609".to_owned(), 12, "{\"r\":1}".to_owned())]);
        db.execute(ROUND_PUT, [5_i64]).unwrap();
        db.execute(ROUND_PUT, [7_i64]).unwrap();
        let last: i64 = db.query_row(ROUND_LAST, [], |r| r.get(0)).unwrap();
        assert_eq!(last, 7);
        db.execute(HOLD_SCHEMA, []).unwrap();
        db.execute(HOLD_PUT, [1_i64]).unwrap();
        db.execute(HOLD_PUT, [0_i64]).unwrap();
        let held: i64 = db.query_row(HOLD_GET, [], |r| r.get(0)).unwrap();
        assert_eq!(held, 0);
        db.execute(SWEEP_SCHEMA, []).unwrap();
        db.execute(SWEEP_PUT, [1_i64]).unwrap();
        db.execute(SWEEP_PUT, [2_i64]).unwrap();
        let n: i64 = db.query_row(SWEEP_N, [], |r| r.get(0)).unwrap();
        assert_eq!(n, 2);
    }
}
