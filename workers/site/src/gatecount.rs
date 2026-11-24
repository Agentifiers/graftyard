//! M5-WP4's hosted gate inputs, counted in each member isolate and summed in this site's `Registry` objects, where
//! `graft-ops` reads them over its `REGISTRY_<n>` bindings (`design/DESIGN.md`, "Install operations", Site day;
//! "Commit and read paths", journey 5, "Automatic drain"; `graftyard_release::gate`).
//!
//! - **Gate counters.** Per script version (the `VERSION` version metadata binding), the members' applied entries
//!   with their apply lag, verify refusals and fold storage losses (apply and verify exceptions), AppendEntries
//!   answered and acked, and `STEP_DOWN` trips (`graftyard_release::gate::Counters`). Object `gate`, one per site.
//! - **`GATE_ERRORS`'s input.** Per script key (`edge`, `mcp`, `front`), `graft-front`'s answers and 5xx by the
//!   version that answered, thrown calls, and answers with no version (`graftyard_release::gate::ScriptErrors`).
//!   Object `errors`, on each key's home site (`gate::errors_sites`), or the next one while the home does not answer.
//! - **`SiteHealth`.** Per target site, this site's member-to-member calls and those that missed `CALL_DEADLINE` or
//!   failed (`HealthLog`). Object `site_health`, one per site, so the sick site never judges itself. It keeps 10 s
//!   buckets for `UNDRAIN_AFTER` and per-minute rollups for `HEALTH_KEEP_MS`, which `ZERO_ACKS` and `RECOVER_AFTER` read.
//!
//! Each isolate flushes at most every `FLUSH_MS`, only what it counted. The objects keep one SQLite row per bucket
//! (a minute per version for the gate, `HEALTH_BUCKET_MS` per target for health), so an object reset loses nothing
//! and a site writes a few rows a minute per isolate, not per call.

use std::cell::RefCell;
use std::collections::BTreeMap;

use graftyard_release::gate::{Counters, ScriptErrors};
use serde_json::json;
use worker::{Env, WorkerVersionMetadata};

/// How often an isolate sends what it counted [guess]: well inside a 5-minute drain window and a step's dwell.
pub const FLUSH_MS: u64 = 30_000;

#[derive(Default)]
struct Pending {
    gate: Counters,
    /// Per target site: calls and missed.
    health: BTreeMap<u8, (u64, u64)>,
    flushed: u64,
    version: Option<Option<String>>,
}

thread_local! {
    static PENDING: RefCell<Pending> = RefCell::new(Pending::default());
}

/// Counts into this isolate's gate counters.
pub fn count(env: &Env, f: impl FnOnce(&mut Counters)) {
    PENDING.with(|p| f(&mut p.borrow_mut().gate));
    flush_due(env);
}

/// One member call to `target`'s site at its first outcome: answered in time, or missed and failed.
pub fn call(env: &Env, target: u8, ok: bool) {
    PENDING.with(|p| {
        let mut p = p.borrow_mut();
        let c = p.health.entry(target).or_default();
        c.0 += 1;
        c.1 += u64::from(!ok);
    });
    flush_due(env);
}

/// This isolate's script version (the `VERSION` binding), read once.
pub fn version(env: &Env) -> Option<String> {
    PENDING.with(|p| {
        p.borrow_mut()
            .version
            .get_or_insert_with(|| env.get_binding::<WorkerVersionMetadata>("VERSION").ok().map(|v| v.id()))
            .clone()
    })
}

fn flush_due(env: &Env) {
    let now = crate::now_ms();
    let taken = PENDING.with(|p| {
        let mut p = p.borrow_mut();
        if now < p.flushed + FLUSH_MS {
            return None;
        }
        p.flushed = now;
        Some((std::mem::take(&mut p.gate), std::mem::take(&mut p.health)))
    });
    let version = version(env);
    let Some((gate, health)) = taken else { return };
    let env = env.clone();
    worker::wasm_bindgen_futures::spawn_local(async move {
        let Ok(site) = crate::SiteEnv::read(&env) else { return };
        // A build with no `VERSION` binding cannot say which version counted, so it sends no gate counters.
        if let Some(v) = version.filter(|_| !gate.is_empty()) {
            let _ = put(&env, &site, "gate", &json!({ "version": v, "counters": gate })).await;
        }
        if !health.is_empty() {
            let counts: Vec<_> = health.iter().map(|(t, (c, m))| json!([t, c, m])).collect();
            let _ = put(&env, &site, "site_health", &json!({ "counts": counts })).await;
        }
    });
}

/// Health buckets' width.
pub const HEALTH_BUCKET_MS: u64 = 10_000;
/// The per-minute rollups (`site_health_min`) are kept this long: `ZERO_ACKS` and `RECOVER_AFTER`, both 24 h, plus an
/// hour so a read that starts late still sees its whole window (M6-WP1, `graftyard_release::gate::loss`). One row per
/// target a minute, so about 4,300 rows for three targets.
pub const HEALTH_KEEP_MS: u64 = graftyard_release::ZERO_ACKS + 3_600_000;
/// Gate rows are kept this long: a whole site day, each of its three gated steps starved for twice `STEP_DWELL`, then
/// `FIRST_DAY`, because `graft-ops` reads N's rows from the intent's start at 100% and on the first day.
const GATE_KEEP_MS: u64 = 6 * graftyard_release::ops::STEP_DWELL + graftyard_release::ops::FIRST_DAY;

/// The `Registry` object's routes: PUT adds one isolate's counts to the current bucket; GET sums buckets from
/// `since` (epoch ms), the gate's for `version`, the errors' for `key`.
pub fn route(
    sql: &worker::SqlStorage,
    object: &str,
    put: Option<serde_json::Value>,
    q: &dyn Fn(&str) -> Option<String>,
) -> worker::Result<serde_json::Value> {
    let now = crate::now_ms();
    let since: i64 = q("since").and_then(|v| v.parse().ok()).unwrap_or(0);
    if object == "gate" {
        sql.exec("CREATE TABLE IF NOT EXISTS gate (minute INTEGER NOT NULL, version TEXT NOT NULL, c TEXT NOT NULL, PRIMARY KEY (minute, version)) WITHOUT ROWID", None)?;
        if let Some(b) = put {
            let version = b["version"].as_str().ok_or_else(|| worker::Error::RustError("version".into()))?.to_owned();
            let add: Counters = serde_json::from_value(b["counters"].clone())?;
            let minute = (now / 60_000 * 60_000) as i64;
            let rows = crate::host::rows(
                sql,
                "SELECT c FROM gate WHERE minute = ? AND version = ?",
                vec![minute.into(), version.clone().into()],
            )?;
            let mut c: Counters = match rows.first() {
                Some(r) => serde_json::from_str(crate::host::text(&r[0])?)?,
                None => Counters::default(),
            };
            c.add(&add);
            sql.exec(
                "INSERT OR REPLACE INTO gate (minute, version, c) VALUES (?, ?, ?)",
                vec![minute.into(), version.into(), serde_json::to_string(&c)?.into()],
            )?;
            sql.exec("DELETE FROM gate WHERE minute < ?", vec![(now.saturating_sub(GATE_KEEP_MS) as i64).into()])?;
            return Ok(json!({}));
        }
        let version = q("version").unwrap_or_default();
        let mut sum = Counters::default();
        for r in crate::host::rows(
            sql,
            "SELECT c FROM gate WHERE version = ? AND minute >= ?",
            vec![version.into(), (since / 60_000 * 60_000).into()],
        )? {
            sum.add(&serde_json::from_str(crate::host::text(&r[0])?)?);
        }
        return Ok(serde_json::to_value(sum)?);
    }
    if object == "errors" {
        sql.exec("CREATE TABLE IF NOT EXISTS errors (minute INTEGER NOT NULL, key TEXT NOT NULL, c TEXT NOT NULL, PRIMARY KEY (minute, key)) WITHOUT ROWID", None)?;
        let sum = |key: String, from: i64| -> worker::Result<ScriptErrors> {
            let mut sum = ScriptErrors::default();
            for r in crate::host::rows(
                sql,
                "SELECT c FROM errors WHERE key = ? AND minute >= ?",
                vec![key.into(), (from / 60_000 * 60_000).into()],
            )? {
                sum.add(&serde_json::from_str(crate::host::text(&r[0])?)?);
            }
            Ok(sum)
        };
        if let Some(b) = put {
            let key = b["key"].as_str().ok_or_else(|| worker::Error::RustError("key".into()))?.to_owned();
            let add: ScriptErrors = serde_json::from_value(b["counts"].clone())?;
            let minute = (now / 60_000 * 60_000) as i64;
            let mut c = sum(key.clone(), minute)?;
            c.add(&add);
            sql.exec(
                "INSERT OR REPLACE INTO errors (minute, key, c) VALUES (?, ?, ?)",
                vec![minute.into(), key.into(), serde_json::to_string(&c)?.into()],
            )?;
            sql.exec("DELETE FROM errors WHERE minute < ?", vec![(now.saturating_sub(GATE_KEEP_MS) as i64).into()])?;
            return Ok(json!({}));
        }
        return Ok(serde_json::to_value(sum(q("key").unwrap_or_default(), since)?)?);
    }
    sql.exec("CREATE TABLE IF NOT EXISTS site_health (bucket INTEGER NOT NULL, target INTEGER NOT NULL, calls INTEGER NOT NULL, missed INTEGER NOT NULL, PRIMARY KEY (bucket, target)) WITHOUT ROWID", None)?;
    sql.exec("CREATE TABLE IF NOT EXISTS site_health_min (minute INTEGER NOT NULL, target INTEGER NOT NULL, calls INTEGER NOT NULL, missed INTEGER NOT NULL, PRIMARY KEY (minute, target)) WITHOUT ROWID", None)?;
    if let Some(b) = put {
        let bucket = (now / HEALTH_BUCKET_MS * HEALTH_BUCKET_MS) as i64;
        let minute = (now / 60_000 * 60_000) as i64;
        for c in b["counts"].as_array().into_iter().flatten() {
            let n = |i: usize| c[i].as_i64().unwrap_or(0);
            for (table, key, at) in [("site_health", "bucket", bucket), ("site_health_min", "minute", minute)] {
                sql.exec(
                    &format!(
                        "INSERT INTO {table} ({key}, target, calls, missed) VALUES (?, ?, ?, ?)
                         ON CONFLICT ({key}, target) DO UPDATE SET calls = calls + excluded.calls, missed = missed + excluded.missed"
                    ),
                    vec![at.into(), n(0).into(), n(1).into(), n(2).into()],
                )?;
            }
        }
        let keep = now.saturating_sub(graftyard_release::gate::UNDRAIN_AFTER + HEALTH_BUCKET_MS) as i64;
        sql.exec("DELETE FROM site_health WHERE bucket < ?", vec![keep.into()])?;
        sql.exec(
            "DELETE FROM site_health_min WHERE minute < ?",
            vec![(now.saturating_sub(HEALTH_KEEP_MS) as i64).into()],
        )?;
        return Ok(json!({}));
    }
    let (table, key, width) = health_table(now, since);
    let from = since / width * width;
    let rows = crate::host::rows(
        sql,
        &format!("SELECT target, SUM(calls), SUM(missed) FROM {table} WHERE {key} >= ? GROUP BY target"),
        vec![from.into()],
    )?;
    let out: Vec<serde_json::Value> = rows
        .iter()
        .map(|r| Ok(json!([crate::host::int(&r[0])?, crate::host::int(&r[1])?, crate::host::int(&r[2])?])))
        .collect::<worker::Result<_>>()?;
    Ok(json!(out))
}

/// The table a `site_health` read sums: the 10 s buckets for a window they still hold (`UNDRAIN_AFTER`), the
/// per-minute rollups for a longer one, `ZERO_ACKS` and `RECOVER_AFTER`'s. With its key column and width (ms).
fn health_table(now: u64, since: i64) -> (&'static str, &'static str, i64) {
    if now.saturating_sub(since.max(0) as u64) > graftyard_release::gate::UNDRAIN_AFTER {
        ("site_health_min", "minute", 60_000)
    } else {
        ("site_health", "bucket", HEALTH_BUCKET_MS as i64)
    }
}

async fn put(env: &Env, site: &crate::SiteEnv, object: &str, body: &serde_json::Value) -> worker::Result<()> {
    let mut init = worker::RequestInit::new();
    init.with_method(worker::Method::Put).with_body(Some(body.to_string().into()));
    let req = worker::Request::new_with_init(&format!("https://registry/{object}"), &init)?;
    site.registry(env, site.id, object)?.fetch_with_request(req).await.map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use graftyard_release::gate::{RECOVER_AFTER, UNDRAIN_AFTER};

    /// The 10 s buckets hold `UNDRAIN_AFTER` and no more, so a `ZERO_ACKS` or recovery read from them would sum 15
    /// minutes and call it 24 h. A window up to `UNDRAIN_AFTER` reads the buckets; anything longer, the rollups.
    #[test]
    fn a_day_long_health_read_sums_the_minute_rollups() {
        let now = 1_800_000_000_000u64;
        let at = |ago: u64| (now - ago) as i64;
        assert_eq!(health_table(now, at(5 * 60_000)).0, "site_health");
        assert_eq!(health_table(now, at(UNDRAIN_AFTER)).0, "site_health");
        assert_eq!(health_table(now, at(UNDRAIN_AFTER + 1)).0, "site_health_min");
        assert_eq!(health_table(now, at(RECOVER_AFTER)).0, "site_health_min");
        assert!(HEALTH_KEEP_MS > graftyard_release::ZERO_ACKS.max(RECOVER_AFTER), "the rollups outlive both windows");
    }
}
