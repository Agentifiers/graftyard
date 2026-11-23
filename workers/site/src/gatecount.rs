//! M5-WP4's hosted gate inputs, counted in each member isolate and summed in this site's `Registry` objects, where
//! `graft-ops` reads them over its `REGISTRY_<n>` bindings (`design/DESIGN.md`, "Install operations", Site day;
//! "Commit and read paths", journey 5, "Automatic drain"; `graftyard_release::gate`).
//!
//! - **Gate counters.** Per script version (the `VERSION` version metadata binding), the members' applied entries
//!   with their apply lag, verify refusals and fold storage losses (apply and verify exceptions), AppendEntries
//!   answered and acked, and `STEP_DOWN` trips (`graftyard_release::gate::Counters`). Object `gate`, one per site.
//! - **`SiteHealth`.** Per target site, this site's member-to-member calls and those that missed `CALL_DEADLINE` or
//!   failed (`HealthLog`). Object `site_health`, one per site, so the sick site never judges itself.
//!
//! Each isolate flushes at most every `FLUSH_MS`, only what it counted. The objects keep one SQLite row per bucket
//! (a minute per version for the gate, `HEALTH_BUCKET_MS` per target for health), so an object reset loses nothing
//! and a site writes a few rows a minute per isolate, not per call.

use std::cell::RefCell;
use std::collections::BTreeMap;

use graftyard_release::gate::Counters;
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

fn flush_due(env: &Env) {
    let now = crate::now_ms();
    let taken = PENDING.with(|p| {
        let mut p = p.borrow_mut();
        if now < p.flushed + FLUSH_MS {
            return None;
        }
        p.flushed = now;
        let version = p
            .version
            .get_or_insert_with(|| env.get_binding::<WorkerVersionMetadata>("VERSION").ok().map(|v| v.id()))
            .clone();
        Some((version, std::mem::take(&mut p.gate), std::mem::take(&mut p.health)))
    });
    let Some((version, gate, health)) = taken else { return };
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
/// Gate rows are kept this long: a whole site day, each of its three gated steps starved for twice `STEP_DWELL`, then
/// `FIRST_DAY`, because `graft-ops` reads N's rows from the intent's start at 100% and on the first day.
const GATE_KEEP_MS: u64 = 6 * graftyard_release::ops::STEP_DWELL + graftyard_release::ops::FIRST_DAY;

/// The `Registry` object's routes: PUT adds one isolate's counts to the current bucket; GET sums buckets from
/// `since` (epoch ms), the gate's for `version`.
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
    sql.exec("CREATE TABLE IF NOT EXISTS site_health (bucket INTEGER NOT NULL, target INTEGER NOT NULL, calls INTEGER NOT NULL, missed INTEGER NOT NULL, PRIMARY KEY (bucket, target)) WITHOUT ROWID", None)?;
    if let Some(b) = put {
        let bucket = (now / HEALTH_BUCKET_MS * HEALTH_BUCKET_MS) as i64;
        for c in b["counts"].as_array().into_iter().flatten() {
            let n = |i: usize| c[i].as_i64().unwrap_or(0);
            sql.exec(
                "INSERT INTO site_health (bucket, target, calls, missed) VALUES (?, ?, ?, ?)
                 ON CONFLICT (bucket, target) DO UPDATE SET calls = calls + excluded.calls, missed = missed + excluded.missed",
                vec![bucket.into(), n(0).into(), n(1).into(), n(2).into()],
            )?;
        }
        let keep = now.saturating_sub(graftyard_release::gate::UNDRAIN_AFTER + HEALTH_BUCKET_MS) as i64;
        sql.exec("DELETE FROM site_health WHERE bucket < ?", vec![keep.into()])?;
        return Ok(json!({}));
    }
    let from = since / HEALTH_BUCKET_MS as i64 * HEALTH_BUCKET_MS as i64;
    let rows = crate::host::rows(
        sql,
        "SELECT target, SUM(calls), SUM(missed) FROM site_health WHERE bucket >= ? GROUP BY target",
        vec![from.into()],
    )?;
    let out: Vec<serde_json::Value> = rows
        .iter()
        .map(|r| Ok(json!([crate::host::int(&r[0])?, crate::host::int(&r[1])?, crate::host::int(&r[2])?])))
        .collect::<worker::Result<_>>()?;
    Ok(json!(out))
}

async fn put(env: &Env, site: &crate::SiteEnv, object: &str, body: &serde_json::Value) -> worker::Result<()> {
    let mut init = worker::RequestInit::new();
    init.with_method(worker::Method::Put).with_body(Some(body.to_string().into()));
    let req = worker::Request::new_with_init(&format!("https://registry/{object}"), &init)?;
    site.registry(env, site.id, object)?.fetch_with_request(req).await.map(|_| ())
}
