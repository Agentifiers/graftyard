//! `fanout` hosted (`design/DESIGN.md`, Shape, `NetworkLoad`, `fanout`, and "Git cache shards (derived)";
//! `build/BUILD-PLAN.md`, AM1-WP1, "the network leader's per-minute `fanout` commit and the shard's `NetworkLoad`
//! report"):
//!
//! - **Report.** A warm site's first shard of a network (`git#<site>#net:<repo>`, the one the edge has always called)
//!   reads its k shards' `/load` once a minute, combines them (`graftyard_git::combine`: the most loaded one's CPU),
//!   and posts the result to the network group's leader at `/fanout` ([`report_from_shard`]).
//! - **Commit.** The leader folds each warm site's latest report into `LoadReports` and, when `fanout` steps k, records
//!   `RepoEvent::Fanout { k }` (format 32.15) in an entry of its own. One k serves both warm sites. The reports and
//!   the hysteresis live in the leader's isolate only, so a new leader starts from the committed k and at worst waits
//!   one more `FANOUT_UP_FOR` to step.
//! - **Use.** Every catch-up of the first shard reads k from the fold's `ranges`, and its `/validate` forwards a push
//!   to the shard `push_validator` picks among the k, by rendezvous on the push id. A pool shard (`git#<site>#pool:<n>`)
//!   catches up before it validates, like any shard, so it holds every commit the push could build on; if it cannot
//!   answer, the first shard validates the push itself.
//!
//! No API call or cron: the reports ride the shard's own alarm and the commit is the leader's own entry.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use graftyard_git::{self as git, LoadReports, NetworkLoad};
use graftyard_raft::Role;
use graftyard_types::{ChainId, NetworkId, PlacementRecord, SiteId, Timestamp};
use serde_json::{Value, json};
use worker::*;

use crate::family::Repo;
use crate::host::Host;
use crate::now_ms;

/// The shard reports this often [the design's "each minute"].
pub const REPORT_EVERY_MS: u64 = 60_000;

thread_local! {
    /// Per leading member (object name): the warm sites' latest reports and the hysteresis.
    static REPORTS: RefCell<HashMap<String, LoadReports>> = RefCell::new(HashMap::new());
}

/// The leader's `/fanout`: `{"site": n, "load": NetworkLoad}`. Answers the committed k, and `stepped` when this report
/// moved it.
pub async fn report(h: &Rc<Host<Repo>>, body: Value) -> Result<Value> {
    let site = SiteId(body["site"].as_u64().ok_or("site")? as u8);
    let load: NetworkLoad = serde_json::from_value(body["load"].clone()).map_err(|e| e.to_string())?;
    let (key, warm, k) = {
        let core = h.core.borrow();
        let c = core.as_ref().ok_or("member not joined")?;
        if c.m.role() != Role::Leader {
            return Err("not leader".into());
        }
        let ChainId::Repo(repo) = c.join.chain else { return Err("not a repository chain".into()) };
        let sites: Vec<SiteId> = c.join.record.sites.iter().copied().collect();
        let warm = git::warm_sites(NetworkId(repo.0), c.join.record.home, &sites);
        (c.join.me.do_name.to_string(), warm, c.m.group_fold().fold.fanout_k())
    };
    let now = Timestamp(now_ms());
    let next = REPORTS.with(|r| r.borrow_mut().entry(key).or_default().report(warm, site, load, k, now));
    let Some(next) = next else { return Ok(json!({ "k": k })) };
    commit(h, next).await?;
    Ok(json!({ "k": next, "stepped": true }))
}

/// Test installs: commit `k` now, with no load behind it.
#[cfg(feature = "test-faults")]
pub async fn force(h: &Rc<Host<Repo>>, k: u8) -> Result<Value> {
    let k = k.clamp(1, git::FANOUT_MAX);
    commit(h, k).await?;
    Ok(json!({ "k": k }))
}

/// Records `Fanout { k }` as the leader's own entry and waits for it to commit.
async fn commit(h: &Rc<Host<Repo>>, k: u8) -> Result<()> {
    let rid = graftyard_lookup::request_id();
    let rx = h.wait_recorded(rid);
    let events = vec![git::repo::RepoEvent::Fanout { k }];
    h.input(|m, t| m.record(t, rid, git::repo::REPO_FORMAT_FANOUT, events))?
        .map_err(|e| Error::RustError(crate::host::call_error_json(&e).to_string()))?;
    h.served();
    match crate::host::within(std::time::Duration::from_secs(10), rx).await {
        Some(Ok(Ok(_))) => Ok(()),
        Some(Ok(Err(nl))) => Err(format!("not leader: {}", crate::host::nl_json(&nl)).into()),
        Some(Err(_)) => Err("member reset".into()),
        None => Err("the fanout entry did not commit in 10 s".into()),
    }
}

/// The shard side: this site's combined `NetworkLoad` of `net`'s k shards, posted to the group's leader. `loads` are
/// the k shards' own reports, the first shard's first.
pub async fn report_from_shard(
    env: &Env,
    sites: &graftyard_types::SiteTable,
    me: SiteId,
    net: NetworkId,
    record: &PlacementRecord,
    loads: &[NetworkLoad],
) -> std::result::Result<u8, String> {
    let body = json!({ "site": me.0, "load": git::combine(loads) });
    let b = graftyard_lookup::Bindings { env, sites };
    let (v, _) =
        graftyard_lookup::call(&b, ChainId::Repo(graftyard_types::RepoId(net.0)), record, "/fanout", &body).await?;
    Ok(v["k"].as_u64().unwrap_or(1) as u8)
}

/// The object a shard of `net` answers as on this site, by `GitCacheShard::shard` (`graftyard_git::object_name`).
pub fn stub(env: &Env, sites: &graftyard_types::SiteTable, me: SiteId, name: &str) -> Result<Stub> {
    let region = sites.get(me).ok_or("this site is not in GRAFTYARD_SITES")?.region;
    env.durable_object(&format!("SHARD_{}", me.0))?.get_by_name_with_location_hint(name, region.hint())
}

#[cfg(test)]
mod tests {
    use graftyard_git::repo::{REPO_FORMAT_FANOUT, RepoEvent, RepoFold};
    use graftyard_raft::{EntryBody, Fold, LogEntry, TierSet};
    use graftyard_types::{AccountId, DbId, Digest, Index, RepoId, Timestamp};
    use serde_json::json;
    use smallvec::SmallVec;

    use crate::family::{Family, Repo};

    /// The `ranges` read a shard's catch-up makes carries the committed k, 1 before any `Fanout` entry, and this
    /// release reads 32.15, so a leader's own `Fanout` record is not refused `Unsupported`.
    #[test]
    fn the_ranges_read_answers_the_committed_k() {
        let mut fold = RepoFold::new(RepoId(DbId(9)), AccountId(DbId(1)));
        let q = json!({ "ranges": true });
        assert_eq!(Repo::read_json(&fold, &q)["fanout"], 1);
        fold.apply_entry(&LogEntry {
            term: graftyard_raft::Term(1),
            index: Index(1),
            prev: Digest([0; 32]),
            format: REPO_FORMAT_FANOUT,
            tiers: TierSet::CORE,
            at: Timestamp(1),
            requests: SmallVec::new(),
            extend: None,
            body: EntryBody::Events(vec![RepoEvent::Fanout { k: 4 }]),
        });
        assert_eq!(Repo::read_json(&fold, &q)["fanout"], 4);
        assert!(Repo::reads().contains(&REPO_FORMAT_FANOUT));
    }
}
