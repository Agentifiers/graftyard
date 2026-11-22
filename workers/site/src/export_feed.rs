//! The change feed's consumer (`design/DESIGN.md`, Shape, "Lifeboat feed"; `decisions/DECISIONS.md`, `export-host`).
//! The pure half is `graftyard_git::export`.
//!
//! - **Reports.** Each leader sends its commit index and record to the feed site's registry object `export`
//!   (`Host::report_commit`), at most once a round and only once it moved. The object keeps the highest per chain.
//! - **Rounds.** Only the site that binds the export bucket (`EXPORT`, site A) runs them. The cron kicks one, and each
//!   round that ran sends the next as a queue message delayed by FEED_EVERY_MS. The object's `export_round` claim
//!   collapses a kick and a message that meet into one round (`export::round_due`).
//! - **One round.** It reads `FeedState`, folds in any object written after it, plans the next batch from the reports,
//!   reads each chain's rows from a learner or follower (`graftyard_lookup::log_rows`, never the leader), six at a time,
//!   fills and checks the object, writes it create-only, and then writes the state. A round that dies between the two
//!   writes is recovered by the next one's fold.

use std::collections::BTreeMap;

use graftyard_git::export::{self, FeedState};
use graftyard_git::key::R2Key;
use graftyard_lookup::Bindings;
use graftyard_types::{ChainId, Index, PlacementRecord, SiteId};
use serde_json::{Value, json};
use worker::*;

use crate::SiteEnv;

/// The registry object that holds the reports and the round claim, on the feed site.
const OBJECT: &str = "export";
/// How many source reads a round runs at once: a Worker's connections waiting for headers.
const READS_AT_ONCE: usize = 6;

pub(crate) const REPORTS_SCHEMA: &str =
    "CREATE TABLE IF NOT EXISTS export_report (chain TEXT PRIMARY KEY, commit INTEGER NOT NULL, record TEXT NOT NULL)";

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
    let reports = match claim(env).await {
        Ok(Some(r)) => r,
        Ok(None) => return,
        Err(e) => return console_error!("export feed: the round claim failed: {e}"),
    };
    match Box::pin(round(env, &bucket, reports)).await {
        Ok(line) => console_log!("export feed {line}"),
        Err(e) => console_error!("export feed round failed: {e}"),
    }
    let next =
        MessageBuilder::new(json!({ "exportFeed": true })).delay_seconds((export::FEED_EVERY_MS / 1000) as u32).build();
    if let Err(e) = async { env.queue("PRSYNC")?.send(next).await }.await {
        console_error!("export feed: the next round's message failed, the cron restarts it: {e}");
    }
}

/// The round claim: the reports when a round is due, `None` when another started within FEED_EVERY_MS.
async fn claim(env: &Env) -> Result<Option<Vec<(ChainId, Index, PlacementRecord)>>> {
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
    Ok(Some(
        reports
            .into_iter()
            .filter_map(|r| {
                let chain = r["chain"].as_str()?.parse().ok()?;
                let record = serde_json::from_value(r["record"].clone()).ok()?;
                Some((chain, Index(r["commit"].as_u64()?), record))
            })
            .collect(),
    ))
}

async fn get(bucket: &Bucket, key: &str) -> Result<Option<Vec<u8>>> {
    match bucket.get(key).execute().await? {
        Some(o) => Ok(Some(o.body().ok_or_else(|| Error::RustError(format!("{key}: no body")))?.bytes().await?)),
        None => Ok(None),
    }
}

/// One round, as `export::plan`, `fill`, and `check_object` decide it. Returns its log line.
async fn round(env: &Env, bucket: &Bucket, reports: Vec<(ChainId, Index, PlacementRecord)>) -> Result<String> {
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
    let commits: Vec<(ChainId, Index)> = reports.iter().map(|(c, i, _)| (*c, *i)).collect();
    let records: BTreeMap<ChainId, &PlacementRecord> = reports.iter().map(|(c, _, r)| (*c, r)).collect();
    let Some(batch) = export::plan(&state, &commits, crate::now_ms()) else {
        if recovered > 0 {
            bucket.put(R2Key::ExportState.to_string(), state.encode()).execute().await?;
        }
        return Ok(format!("idle reports={} recovered={recovered}", reports.len()));
    };
    let mut fetched = BTreeMap::new();
    let b = &b;
    for part in batch.chains.chunks(READS_AT_ONCE) {
        let reads = part.iter().filter_map(|(c, to)| {
            let record = records.get(c)?;
            let from = state.cursors.get(*c).0 + 1;
            Some(async move { (*c, graftyard_lookup::log_rows(b, *c, record, from, to.0).await) })
        });
        for (c, got) in futures_util::future::join_all(reads).await {
            let page = got.and_then(|(_, bytes)| export::decode_log_page(&bytes).ok());
            if let Some(f) = page.and_then(|p| p.fetched()) {
                fetched.insert(c, f);
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
        "{key} planned={} read={read} chains={} entries={rows} recovered={recovered}",
        batch.chains.len(),
        object.batch.chains.len()
    ))
}
