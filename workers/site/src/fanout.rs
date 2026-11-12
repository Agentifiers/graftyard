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
        // A follower answers `notLeader`, as `/read` does, so the caller's lookup follows the leader. An error here
        // would be a 500, which lookup counts as a missed member and reports with `suspect` (hosted B0-34: 27 of 29
        // reports in its window ended so on the followers).
        if c.m.role() != Role::Leader {
            return Ok(not_leader(c.m.not_leader()));
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

/// What a member that does not lead answers `/fanout`: the `notLeader` shape `graftyard_lookup` follows.
fn not_leader(nl: graftyard_raft::NotLeader) -> Value {
    json!({ "error": crate::host::call_error_json(&graftyard_raft::CallError::NotLeader(nl)) })
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

    /// A `rows` read with `packed` answers the whole tiers as one JSON string the edge reads back as the same rows;
    /// without it, the object a member built before `packed` answered.
    #[test]
    fn a_packed_rows_read_answers_one_string_of_the_same_rows() {
        use graftyard_api::hosted::TierRows;
        let mut fold = RepoFold::new(RepoId(DbId(9)), AccountId(DbId(1)));
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
        let packed = Repo::read_json(&fold, &json!({ "rows": true, "ranges": true, "packed": true }));
        let plain = Repo::read_json(&fold, &json!({ "rows": true, "ranges": true }));
        assert!(packed.is_string() && plain.is_object());
        let rows = TierRows::from_value(&packed).unwrap();
        assert_eq!(rows, TierRows::from_value(&plain).unwrap());
        assert!(rows.complete && rows.core.iter().any(|(t, _, _)| t == "fanout"));
    }

    /// The packed dump is cached by the tiers' state: a second read at the same state answers the same string, and an
    /// applied entry, another repository at the same index, or a dropped cache answers fresh rows, never a stale dump.
    #[test]
    fn a_cached_dump_never_outlives_its_tiers() {
        use graftyard_api::hosted::TierRows;
        let q = json!({ "rows": true, "ranges": true, "packed": true });
        let entry = |i: u64, k: u8, prev: Digest| LogEntry {
            term: graftyard_raft::Term(1),
            index: Index(i),
            prev,
            format: REPO_FORMAT_FANOUT,
            tiers: TierSet::CORE,
            at: Timestamp(i),
            requests: SmallVec::new(),
            extend: None,
            body: EntryBody::Events(vec![RepoEvent::Fanout { k }]),
        };
        let k_of = |v: &serde_json::Value| {
            let rows = TierRows::from_value(v).unwrap();
            rows.core.iter().find(|(t, _, _)| t == "fanout").map(|(_, _, c)| format!("{c:?}"))
        };
        let one = |k| entry(1, k, Digest([0; 32]));
        let two = |k, after: &LogEntry<RepoEvent>| entry(2, k, graftyard_raft::digest(after));
        let mut a = RepoFold::new(RepoId(DbId(31)), AccountId(DbId(1)));
        a.apply_entry(&one(2));
        let first = Repo::read_json(&a, &q);
        assert_eq!(Repo::read_json(&a, &q), first);
        a.apply_entry(&two(5, &one(2)));
        let second = Repo::read_json(&a, &q);
        assert_ne!(k_of(&second), k_of(&first), "an applied entry is in the next read");
        assert_eq!(TierRows::from_value(&second).unwrap().index, 2);
        // Another repository's tiers at the same index are not served the first one's dump.
        let mut b = RepoFold::new(RepoId(DbId(32)), AccountId(DbId(1)));
        b.apply_entry(&one(7));
        b.apply_entry(&two(3, &one(7)));
        assert_ne!(k_of(&Repo::read_json(&b, &q)), k_of(&second));
        crate::family::forget_dumps();
        assert_eq!(Repo::read_json(&a, &q), second);
    }

    /// A rebuild of a repository's dump is let through at most once per DUMP_EVERY_MS, and a read whose cached dump is
    /// at the tiers' state is served at once, however soon after the last rebuild.
    #[test]
    fn dump_rebuilds_are_spaced_but_cache_hits_are_not() {
        use crate::family::{DUMP_EVERY_MS, LEARNER_DUMP_EVERY_MS, dump_wait};
        let q = json!({ "rows": true, "ranges": true, "packed": true });
        let entry = |i: u64, k: u8, prev: Digest| LogEntry {
            term: graftyard_raft::Term(1),
            index: Index(i),
            prev,
            format: REPO_FORMAT_FANOUT,
            tiers: TierSet::CORE,
            at: Timestamp(i),
            requests: SmallVec::new(),
            extend: None,
            body: EntryBody::Events(vec![RepoEvent::Fanout { k }]),
        };
        let first = entry(1, 2, Digest([0; 32]));
        let mut a = RepoFold::new(RepoId(DbId(41)), AccountId(DbId(1)));
        a.apply_entry(&first);
        assert_eq!(dump_wait(&a, 10_000, DUMP_EVERY_MS), None, "the first rebuild is due");
        Repo::read_json(&a, &q);
        assert_eq!(dump_wait(&a, 10_100, DUMP_EVERY_MS), None, "the cached dump is at the tiers' state");
        a.apply_entry(&entry(2, 5, graftyard_raft::digest(&first)));
        assert_eq!(dump_wait(&a, 10_300, DUMP_EVERY_MS), Some(DUMP_EVERY_MS - 300));
        assert_eq!(dump_wait(&a, 10_000 + DUMP_EVERY_MS, DUMP_EVERY_MS), None, "due again");
        assert_eq!(dump_wait(&a, 10_001 + DUMP_EVERY_MS, DUMP_EVERY_MS), Some(DUMP_EVERY_MS - 1), "and recorded");
        // A learner's spacing is its own: at LEARNER_DUMP_EVERY_MS after the last rebuild one is due again.
        let at = 10_000 + DUMP_EVERY_MS;
        a.apply_entry(&entry(3, 6, graftyard_raft::digest(&entry(2, 5, graftyard_raft::digest(&first)))));
        assert_eq!(dump_wait(&a, at + 100, LEARNER_DUMP_EVERY_MS), Some(LEARNER_DUMP_EVERY_MS - 100));
        assert_eq!(dump_wait(&a, at + LEARNER_DUMP_EVERY_MS, LEARNER_DUMP_EVERY_MS), None, "due at its own interval");
    }

    /// A packed dump is built once per state of the tiers and every read at that state, from any input, shares the
    /// same bytes, which are exactly `read_json`'s answer; an applied entry rebuilds it, another repository never gets
    /// it, and a narrow read is not a dump. A learner spaces its rebuilds by LEARNER_DUMP_EVERY_MS, a voter by
    /// DUMP_EVERY_MS.
    #[test]
    fn a_dump_is_built_once_per_state_and_shared_by_every_read() {
        use crate::family::{DUMP_EVERY_MS, LEARNER_DUMP_EVERY_MS, dump_builds, dump_bytes, dump_every};
        let q = json!({ "rows": true, "ranges": true, "packed": true });
        let entry = |i: u64, k: u8, prev: Digest| LogEntry {
            term: graftyard_raft::Term(1),
            index: Index(i),
            prev,
            format: REPO_FORMAT_FANOUT,
            tiers: TierSet::CORE,
            at: Timestamp(i),
            requests: SmallVec::new(),
            extend: None,
            body: EntryBody::Events(vec![RepoEvent::Fanout { k }]),
        };
        let first = entry(1, 2, Digest([0; 32]));
        let mut a = RepoFold::new(RepoId(DbId(51)), AccountId(DbId(1)));
        a.apply_entry(&first);
        let built = dump_builds();
        let one = dump_bytes(&a, &q).expect("a packed read is a dump");
        let again = dump_bytes(&a, &q).expect("dump");
        assert!(std::rc::Rc::ptr_eq(&one, &again), "one copy for every read at this state");
        assert_eq!(dump_builds(), built + 1, "built once");
        assert_eq!(serde_json::to_vec(&Repo::read_json(&a, &q)).unwrap(), one.to_vec(), "the bytes are the answer");
        a.apply_entry(&entry(2, 5, graftyard_raft::digest(&first)));
        let next = dump_bytes(&a, &q).expect("dump");
        assert_ne!(next, one, "an applied entry is in the next dump");
        assert_eq!(dump_builds(), built + 2);
        let mut b = RepoFold::new(RepoId(DbId(52)), AccountId(DbId(1)));
        b.apply_entry(&first);
        b.apply_entry(&entry(2, 7, graftyard_raft::digest(&first)));
        assert_ne!(dump_bytes(&b, &q).expect("dump"), next, "another repository at the same index");
        for narrow in [
            json!({ "rows": { "access": true } }),
            json!({ "rows": { "merge": 7 }, "ranges": true, "packed": true }),
            json!({ "rows": true }),
            json!({ "ranges": true }),
        ] {
            assert!(dump_bytes(&a, &narrow).is_none(), "{narrow}");
        }
        // Through the host, as a grant answers: two inputs' grants at one state get the same copy, not one each.
        let (crate::host::ReadBody::Json(x), crate::host::ReadBody::Json(y)) = (
            crate::host::fold_answer::<Repo>(&a, &q, 0, DUMP_EVERY_MS),
            crate::host::fold_answer::<Repo>(&a, &q, 1, LEARNER_DUMP_EVERY_MS),
        ) else {
            panic!("a cached dump is served")
        };
        assert!(std::rc::Rc::ptr_eq(&x, &y) && std::rc::Rc::ptr_eq(&x, &next), "one copy across grants");
        assert_eq!(dump_builds(), built + 3, "no build beyond the two of `a` and the one of `b`");
        assert_eq!(dump_every(graftyard_types::learner_addr(0)), LEARNER_DUMP_EVERY_MS);
        assert_eq!(dump_every(graftyard_types::learner_addr(15)), LEARNER_DUMP_EVERY_MS);
        assert_eq!(dump_every(graftyard_types::SiteId(2)), DUMP_EVERY_MS);
        assert!(LEARNER_DUMP_EVERY_MS < DUMP_EVERY_MS);
    }

    /// A follower serves by read-index only the reads the design names: whole-tier and merge rows once its metadata
    /// tier is loaded, and the core-only access and agent rows; refs, `ranges`, deps, and the alarm reads stay on the
    /// leader.
    #[test]
    fn a_follower_serves_only_the_designs_reads() {
        use crate::family::follower_query;
        let tier = json!({ "rows": true, "ranges": true, "packed": true });
        let merge = json!({ "rows": { "merge": 7 }, "ranges": true, "packed": true });
        for q in [&tier, &merge] {
            assert!(follower_query(q, true), "{q}");
            assert!(!follower_query(q, false), "a loading metadata tier sends {q} to the leader");
        }
        for q in [
            json!({ "rows": { "access": true }, "ranges": true }),
            json!({ "rows": { "agent": true }, "ranges": true }),
        ] {
            assert!(follower_query(&q, false), "{q} reads the core tier only");
        }
        for q in [
            json!({}),
            json!({ "ranges": true }),
            json!({ "refs": { "limit": 10 } }),
            json!({ "dep": true }),
            json!({ "prsync": [1] }),
            json!({ "mergequeue": "main" }),
            json!({ "tiers": true }),
            json!({ "settings": true }),
            json!({ "request": "01" }),
            json!({ "probe": true }),
        ] {
            assert!(!follower_query(&q, true), "{q} stays on the leader");
        }
    }

    /// A follower's answer to a shard's report names the leader in the shape lookup follows (`/error/notLeader`),
    /// not an error the lookup would count as a missed member.
    #[test]
    fn a_follower_answers_a_report_with_not_leader() {
        let nl = graftyard_raft::NotLeader {
            term: graftyard_raft::Term(7),
            leader: Some(graftyard_types::SiteId(2)),
            config: None,
        };
        let v = super::not_leader(nl);
        assert_eq!(v.pointer("/error/notLeader/leader"), Some(&json!(2)));
        assert_eq!(v.pointer("/error/notLeader/term"), Some(&json!(7)));
    }
}
