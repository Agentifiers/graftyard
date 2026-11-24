//! M6-WP1's site half (`design/DESIGN.md`, "Install operations", Site loss; `build/BUILD-PLAN.md`, M6-WP1): what the
//! Workflows script (`workers/workflows`) and `graft-ops` call on a surviving site's `Registry` objects. Nothing here is
//! on a request path, and nothing calls the Cloudflare API.
//!
//! - **`/all_rows`** on each registry shard: every row, whatever its `sites`. The coordinator unions them across the
//!   surviving sites and shards (`graftyard_types::replace_candidates`), because a row keeps the record its member
//!   joined under and a group that moved onto the lost site later names it nowhere.
//! - **`/refill`** on the object `refill`: one budgeted pass of `graftyard_git::refill_run` from the surviving buckets
//!   into the spare's, single-copy packs first, resumable by network.
//! - **The drained-site probe** on the object `site_health`. A replaced site holds no members, so nothing calls it and
//!   `SiteHealth` never reaches `MIN_CALLS` to read it recovered: `RestorePlacement` would stay refused. `graft-ops`
//!   names the lost and replaced sites on the `site_health` read it makes every run (`?probe=`), and the object then pings one
//!   probe member on each every `PROBE_EVERY_MS` from its own alarm, adding each answer to its own `SiteHealth` rows as
//!   a member call. It stops `PROBE_HOLD_MS` after the last read that named a site, so a stopped `graft-ops` stops it.
//! - **`deleteAll()` after 35 days.** The Workflows script's expiry instance, one per member a move removed, sleeps
//!   `DELETE_AFTER` from the move and then asks that member `/expire` (`crate::member`), naming the member that
//!   replaced it. The removed member wipes itself only if that member's committed config names it and not this one: a
//!   member on a lost site never commits the config that drops it, so its own config cannot say so. No sweep wakes a
//!   live member.

use graftyard_types::{RegistryRow, SiteId};
use serde_json::{Value, json};
use worker::*;

use crate::{SiteEnv, now_ms};

/// How often a surviving site probes a drained one [guess]: `MIN_CALLS` (300) in 10 minutes, so a recovered site
/// shows evidence within one `UNDRAIN_AFTER`, and 43,200 calls a day per prober.
pub const PROBE_EVERY_MS: u64 = 2_000;
/// The object a probe calls on the drained site: one per site, joined to no group. An unjoined member answers `/ping`
/// at once (`retire::Unjoined::Ping`), and a member class that fails (a lost site, a site fault) answers nothing or
/// 503. No group's storage is touched.
pub const PROBE_MEMBER: &str = "probe#0";
/// How long a removed member keeps its storage before `deleteAll()` (`design/DESIGN.md`, "Commit and read paths",
/// journey 5: "Removed members run `deleteAll()` after 35 days").
pub const DELETE_AFTER: u64 = 35 * 86_400_000;

/// Every row a registry shard holds, as `/all_rows` answers it.
pub fn all_rows(sql: &SqlStorage) -> Result<Vec<RegistryRow>> {
    #[derive(serde::Deserialize)]
    struct Row {
        row: String,
    }
    let rows: Vec<Row> = sql.exec("SELECT row FROM registry ORDER BY chain", None)?.to_array()?;
    rows.iter().map(|r| serde_json::from_str(&r.row).map_err(Error::from)).collect()
}

/// How long one `graft-ops` read keeps the probe running: three of its 5-minute runs.
pub const PROBE_HOLD_MS: u64 = 15 * 60_000;

/// The sites `?probe=` names, other than this one: a comma-separated list of site ids.
pub fn probe_sites(q: &str, me: SiteId) -> Vec<u8> {
    let mut v: Vec<u8> = q.split(',').filter_map(|x| x.trim().parse().ok()).filter(|s| *s != me.0).collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// One probe round: each site's probe member, timed against `CALL_DEADLINE` as a member call is. Answers
/// `[target, calls, missed]` rows in `gatecount::route`'s PUT shape.
pub async fn probe(env: &Env, sites: &[u8]) -> Result<Value> {
    let site = SiteEnv::read(env)?;
    let mut counts = Vec::new();
    for t in sites {
        let call = async { site.member(env, SiteId(*t), PROBE_MEMBER)?.fetch_with_str("https://member/ping").await };
        let ok = matches!(
            crate::host::within(graftyard_raft::CALL_DEADLINE, call).await,
            Some(Ok(r)) if r.status_code() == 200
        );
        counts.push(json!([t, 1, u64::from(!ok)]));
    }
    Ok(json!({ "counts": counts }))
}

/// Whether a member a move removed at `removed_at` (ms) may run `deleteAll()` at `now`.
pub fn expired(removed_at: u64, now: u64) -> bool {
    now.saturating_sub(removed_at) >= DELETE_AFTER
}

/// `/refill`'s body: `{"lost", "spare", "single", "from"}`, answered as the run's report.
pub async fn refill(env: &Env, body: Value) -> Result<Value> {
    let site = SiteEnv::read(env)?;
    let field = |k: &str| body[k].as_u64().and_then(|n| u8::try_from(n).ok()).map(SiteId);
    let (Some(lost), Some(spare)) = (field("lost"), field("spare")) else {
        return Err(Error::RustError("/refill needs lost and spare".into()));
    };
    let raw: Vec<(SiteId, graftyard_git::host::R2)> = site
        .sites
        .0
        .iter()
        .filter_map(|s| env.bucket(&format!("BUCKET_{}", s.id.0)).ok().map(|b| (s.id, graftyard_git::host::R2(b))))
        .collect();
    let buckets: Vec<(SiteId, &graftyard_git::host::R2)> = raw.iter().map(|(s, b)| (*s, b)).collect();
    let from = body["from"].as_i64().map(|n| graftyard_types::NetworkId(graftyard_types::DbId(n)));
    let single = body["single"].as_bool().unwrap_or(false);
    let now = graftyard_types::Timestamp(now_ms());
    let r = graftyard_git::refill_run(&buckets, lost, spare, now, single, from, graftyard_git::CronBudget::SITE_CRON)
        .await
        .map_err(|e| Error::RustError(e.0))?;
    Ok(json!({
        "done": r.done,
        "single": r.single,
        "failed": r.failed.iter().map(|(n, e)| json!([n.0.0, e])).collect::<Vec<_>>(),
        "next": r.next.map(|n| n.0.0),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `?probe=` names sites by id; this site, junk and repeats drop out.
    #[test]
    fn a_probe_never_calls_its_own_site() {
        assert_eq!(probe_sites("2,0,x,2,,1", SiteId(1)), vec![0, 2]);
        assert!(probe_sites("", SiteId(1)).is_empty());
    }

    /// A removed member keeps its storage 35 days from the move that removed it.
    #[test]
    fn a_removed_member_is_wiped_only_35_days_after_its_move() {
        let at = 1_800_000_000_000;
        assert!(!expired(at, at + DELETE_AFTER - 1));
        assert!(expired(at, at + DELETE_AFTER));
        assert!(!expired(at, at - 1), "a clock behind the move");
    }
}
