//! The site script's pack sweeper on a Cron Trigger (`design/DESIGN.md`, Stack, Cron Triggers: "pack sweeper
//! (including orphan multipart aborts)"): `graftyard_git::sweep_all` over every site's bucket, for the networks whose
//! home site is this one, with the `request` rows read from each repository group's leader through `fresh`.
//!
//! - `RequestSource` is the real `request` table: `/read` with `{"q": {"request": id}}` on the chain the pack's
//!   metadata names, which the leader answers from `GroupFold::request` at the same applied index as a `fresh` read,
//!   with the stored packs the row's events name.
//! - `home_site(n)`: a network id is its root repository's id in Milestone 1, so a network's home is its
//!   repository's owner record's `home`, found through `RepoOwner` on `Names` and the owner's `Placement` row.
//! - `next` is kept in this site's `Registry` object `sweep` (`GET`/`PUT /sweep`), with the run's report, so the next
//!   run continues there.

use std::collections::BTreeMap;

use graftyard_git::host::R2;
use graftyard_git::{CronBudget, Marker, PackRef, RequestRow, RequestSource, StoreError, sweep_all};
use graftyard_types::{ChainId, DbId, Index, NetworkId, SiteId, Timestamp};
use serde_json::{Value, json};
use worker::*;

use crate::{SiteEnv, now_ms};

/// The `Registry` object that holds this site's sweep cursor.
const CURSOR: &str = "sweep";

struct Rows<'a> {
    env: &'a Env,
    /// Each chain's owner record, read once per run: a network's packs all name its repository's chain.
    records: std::cell::RefCell<BTreeMap<ChainId, graftyard_types::PlacementRecord>>,
}

impl RequestSource for Rows<'_> {
    async fn request(&self, chain: ChainId, id: graftyard_types::RequestId) -> Result<Option<RequestRow>, StoreError> {
        let ChainId::Repo(repo) = chain else { return Err(StoreError(format!("{chain} is not a repository chain"))) };
        let cached = self.records.borrow().get(&chain).cloned();
        let record = match cached {
            Some(r) => r,
            None => {
                let owner = crate::member::repo_owner(self.env, repo).await.map_err(StoreError)?;
                let r = crate::member::owner_record(self.env, owner).await.map_err(StoreError)?;
                self.records.borrow_mut().insert(chain, r.clone());
                r
            }
        };
        let q = json!({ "q": { "request": id.to_string() } });
        let (v, _) = crate::lookup::call(self.env, chain, &record, "/read", &q).await.map_err(StoreError)?;
        row_of(chain, &v)
    }

    /// A probe read through the lookup: gone only when it ends no group, every member it reached not joined
    /// (`graftyard_lookup::is_no_group`). A chain whose record cannot be read, or any other failure, is not gone.
    async fn gone(&self, chain: ChainId) -> bool {
        let ChainId::Repo(repo) = chain else { return false };
        let cached = self.records.borrow().get(&chain).cloned();
        let record = match cached {
            Some(r) => r,
            None => {
                let Ok(owner) = crate::member::repo_owner(self.env, repo).await else { return false };
                let Ok(r) = crate::member::owner_record(self.env, owner).await else { return false };
                self.records.borrow_mut().insert(chain, r.clone());
                r
            }
        };
        let probe = json!({ "q": { "probe": true } });
        matches!(crate::lookup::call(self.env, chain, &record, "/read", &probe).await,
            Err(e) if graftyard_lookup::is_no_group(&e))
    }
}

/// A `/read` answer to `{"q": {"request": id}}`: `{"value": null}` for no row, or the row's index and packs.
fn row_of(chain: ChainId, v: &Value) -> Result<Option<RequestRow>, StoreError> {
    let bad = |what: &str| StoreError(format!("{chain}: request row answer {what}: {v}"));
    if v.get("error").is_some() || v.get("busy").is_some() {
        return Err(bad("is an error"));
    }
    let row = v.get("value").ok_or_else(|| bad("has no value"))?;
    if row.is_null() {
        return Ok(None);
    }
    let index = Index(row["index"].as_u64().ok_or_else(|| bad("has no index"))?);
    let packs: Vec<PackRef> = serde_json::from_value(row["packs"].clone()).map_err(|e| bad(&e.to_string()))?;
    let packs = packs
        .into_iter()
        .filter_map(|p| match p {
            PackRef::Stored { id, checksum, bytes, .. } => Some((id, Marker { chain, index, checksum, bytes })),
            PackRef::Inline(_) => None,
        })
        .collect();
    Ok(Some(RequestRow { index, packs }))
}

/// How long a network's cached home serves before a run reads it from `Names` again [guess]. A home moves only with
/// its owner's placement or a transfer, and a stale one at worst sweeps the network from two sites, which is
/// idempotent, or from neither until the newer reader's next refresh.
const HOME_FOR_MS: u64 = 24 * 3_600_000;
/// The most networks one run reads homes for, two `Names` reads each, so a run's reads stay well inside its
/// subrequests [guess].
const HOMES_PER_RUN: usize = 64;

/// A network's home site as a run last read it from `Names`, and when (ms).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Home {
    site: SiteId,
    at: u64,
}

/// The networks a run reads homes for: those with no cached home, in id order, then those whose home is
/// HOME_FOR_MS old, oldest first, at most HOMES_PER_RUN. Read every run, each 15-minute run woke the `Names`
/// members of every network's owner on an idle install (`tests/am1/results/lm1-memory.md`, idle rows).
fn due(cached: &BTreeMap<NetworkId, Home>, all: &[NetworkId], now: u64) -> Vec<NetworkId> {
    let missing = all.iter().filter(|n| !cached.contains_key(n)).copied();
    let mut stale: Vec<(u64, NetworkId)> = all
        .iter()
        .filter_map(|n| cached.get(n).filter(|h| now.saturating_sub(h.at) >= HOME_FOR_MS).map(|h| (h.at, *n)))
        .collect();
    stale.sort();
    missing.chain(stale.into_iter().map(|(_, n)| n)).take(HOMES_PER_RUN).collect()
}

/// Every network's home this run: the cached ones, with the `due` ones read again through `read`. Returns the homes,
/// the fresh reads to store, and the networks whose read failed, which keep their cached home if they had one.
async fn homes<F, Fut>(
    cached: &BTreeMap<NetworkId, Home>,
    all: &[NetworkId],
    now: u64,
    read: F,
) -> (BTreeMap<NetworkId, SiteId>, Vec<(NetworkId, Home)>, Vec<(i64, String)>)
where
    F: Fn(NetworkId) -> Fut,
    Fut: std::future::Future<Output = std::result::Result<SiteId, String>>,
{
    let mut home: BTreeMap<NetworkId, SiteId> =
        all.iter().filter_map(|n| cached.get(n).map(|h| (*n, h.site))).collect();
    let mut fresh = Vec::new();
    let mut unresolved = Vec::new();
    for n in due(cached, all, now) {
        match read(n).await {
            Ok(site) => {
                home.insert(n, site);
                fresh.push((n, Home { site, at: now }));
            }
            Err(e) => unresolved.push((n.0.0, e)),
        }
    }
    (home, fresh, unresolved)
}

/// A network's home: its root repository's owner record's `home`, through `RepoOwner` and `Placement` on `Names`.
async fn read_home(env: &Env, n: NetworkId) -> std::result::Result<SiteId, String> {
    let owner = crate::member::repo_owner(env, n.root()).await?;
    Ok(crate::member::owner_record(env, owner).await?.home)
}

/// Reads the cursor and the cached homes, or with `put` stores the next cursor and the run's report.
async fn cursor(
    env: &Env,
    site: &SiteEnv,
    put: Option<(Option<NetworkId>, &Value)>,
) -> Result<(Option<NetworkId>, BTreeMap<NetworkId, Home>)> {
    let stub = site.registry(env, site.id, CURSOR)?;
    let mut init = RequestInit::new();
    if let Some((next, last)) = put {
        init.with_method(Method::Put)
            .with_body(Some(json!({ "next": next.map(|n| n.0.0), "last": last }).to_string().into()));
    }
    let mut res = stub.fetch_with_request(Request::new_with_init("https://registry/sweep", &init)?).await?;
    let v: Value = res.json().await?;
    let homes = v["homes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|h| {
            let site = u8::try_from(h[1].as_u64()?).ok()?;
            Some((NetworkId(DbId(h[0].as_i64()?)), Home { site: SiteId(site), at: h[2].as_u64()? }))
        })
        .collect();
    Ok((v["next"].as_i64().map(|n| NetworkId(DbId(n))), homes))
}

/// Stores the homes a run read and drops those of networks no bucket holds any more, before the run sweeps, so a run
/// that dies mid-sweep does not read them again. Nothing is sent, and no row written, when both are empty.
async fn store_homes(env: &Env, site: &SiteEnv, fresh: &[(NetworkId, Home)], gone: &[NetworkId]) -> Result<()> {
    if fresh.is_empty() && gone.is_empty() {
        return Ok(());
    }
    let body = json!({
        "homes": fresh.iter().map(|(n, h)| json!([n.0.0, h.site.0, h.at])).collect::<Vec<_>>(),
        "gone": gone.iter().map(|n| n.0.0).collect::<Vec<_>>(),
    });
    let mut init = RequestInit::new();
    init.with_method(Method::Put).with_body(Some(body.to_string().into()));
    let stub = site.registry(env, site.id, CURSOR)?;
    stub.fetch_with_request(Request::new_with_init("https://registry/sweep_homes", &init)?).await?;
    Ok(())
}

/// One cron run. Errors are logged; the next run starts again from the stored cursor.
pub async fn run(env: &Env) -> Result<Value> {
    run_at(env, Timestamp(now_ms())).await
}

/// One run with the sweeper's clock at `now`: the cron's, and with `test-faults` BF-19's advanced clock
/// (`Registry`'s `/sweep_run`), the only fake in that run.
pub async fn run_at(env: &Env, now: Timestamp) -> Result<Value> {
    let site = SiteEnv::read(env)?;
    let raw: Vec<(SiteId, R2)> = site
        .sites
        .0
        .iter()
        .filter_map(|s| env.bucket(&format!("BUCKET_{}", s.id.0)).ok().map(|b| (s.id, R2(b))))
        .collect();
    let buckets: Vec<(SiteId, &R2)> = raw.iter().map(|(s, b)| (*s, b)).collect();
    // `mine` is synchronous, so every network's home is known first: from the cache, read again only when `due`. A
    // network whose home cannot be found this run is left to the next one.
    let all = graftyard_git::networks(&buckets).await.map_err(|e| Error::RustError(e.0))?;
    let (from, cached) = cursor(env, &site, None).await?;
    let (home, fresh, unresolved) = homes(&cached, &all, now.0, |n| read_home(env, n)).await;
    let gone: Vec<NetworkId> = cached.keys().filter(|n| all.binary_search(n).is_err()).copied().collect();
    store_homes(env, &site, &fresh, &gone).await?;
    let me = site.id;
    let rows = Rows { env, records: Default::default() };
    let r = sweep_all(&buckets, &rows, now, CronBudget::SITE_CRON, from, |n| home.get(&n) == Some(&me), now_ms)
        .await
        .map_err(|e| Error::RustError(e.0))?;
    let report = json!({
        "site": me.0,
        "from": from.map(|n| n.0.0),
        "next": r.next.map(|n| n.0.0),
        "now": now.0,
        "swept": r.swept.iter().map(|(n, s)| {
            let mut kinds: BTreeMap<&str, usize> = BTreeMap::new();
            for a in &s.plan.actions {
                *kinds.entry(match a {
                    graftyard_git::sweep::Action::PutMarker { .. } => "markers",
                    graftyard_git::sweep::Action::Fill { .. } => "fills",
                    graftyard_git::sweep::Action::Delete { .. } => "deletes",
                    graftyard_git::sweep::Action::Abort { .. } => "aborts",
                }).or_insert(0) += 1;
            }
            json!({
                "network": n.0.0,
                "planned": s.plan.actions.len(),
                "kinds": kinds,
                "alarms": s.plan.alarms.len(),
                "done": s.done,
                "failed": s.failed.len(),
                "failures": s.failed.iter().take(5).map(|(_, e)| e.clone()).collect::<Vec<_>>(),
            })
        }).collect::<Vec<_>>(),
        "failed": r.failed.iter().map(|(n, e)| json!({ "network": n.0.0, "error": e })).collect::<Vec<_>>(),
        "dead": r.dead.iter().map(|c| c.to_string()).collect::<Vec<_>>(),
        "walled": r.walled,
        "unresolved": unresolved.iter().map(|(n, e)| json!({ "network": n, "error": e })).collect::<Vec<_>>(),
        "homesRead": fresh.len() + unresolved.len(),
        "at": now_ms(),
    });
    cursor(env, &site, Some((r.next, &report))).await?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use graftyard_git::PackId;
    use graftyard_types::{Digest, SiteSet, Ulid};

    #[test]
    fn a_row_names_its_stored_packs_as_markers() {
        let chain = ChainId::Repo(graftyard_types::RepoId(DbId(7)));
        let id = PackId(Ulid(5 << 80));
        let stored = PackRef::Stored { id, checksum: Digest([3; 32]), bytes: 99, sites: SiteSet::default() };
        let v = json!({ "value": { "index": 12, "packs": [stored, PackRef::Inline(vec![1])] }, "index": 12 });
        let row = row_of(chain, &v).unwrap().unwrap();
        assert_eq!(row.index, Index(12));
        assert_eq!(row.packs, vec![(id, Marker { chain, index: Index(12), checksum: Digest([3; 32]), bytes: 99 })]);
        assert_eq!(row_of(chain, &json!({ "value": null, "index": 3 })).unwrap(), None);
        // A member that is not the leader, or a read past its deadline, is unavailable, never "no row": no row lets
        // the sweeper delete an old unmarked pack.
        assert!(row_of(chain, &json!({ "error": { "notLeader": {} } })).is_err());
        assert!(row_of(chain, &json!({ "busy": true })).is_err());
        assert!(row_of(chain, &json!({})).is_err());
    }

    type Run = (BTreeMap<NetworkId, SiteId>, Vec<(NetworkId, Home)>, Vec<i64>);

    /// One run's homes with a reader that answers site `n % 3`, or fails for network 13.
    fn run(cached: &BTreeMap<NetworkId, Home>, all: &[NetworkId], now: u64) -> Run {
        use futures_util::FutureExt;
        let read = |n: NetworkId| {
            std::future::ready(if n.0.0 == 13 { Err("no leader".to_string()) } else { Ok(SiteId((n.0.0 % 3) as u8)) })
        };
        let (home, fresh, unresolved) = homes(cached, all, now, read).now_or_never().expect("ready");
        (home, fresh, unresolved.into_iter().map(|(n, _)| n).collect())
    }

    fn nets(ids: impl IntoIterator<Item = i64>) -> Vec<NetworkId> {
        ids.into_iter().map(|n| NetworkId(DbId(n))).collect()
    }

    /// The idle-rows bug: every 15-minute run read each network's home from `Names`, two member reads a network,
    /// which woke and wrote rows on idle `Names` members all day. A run reads only networks it has no home for, or
    /// whose home is a day old; a run inside the day reads none and still sweeps with every home.
    #[test]
    fn a_run_reads_homes_only_for_new_or_day_old_networks() {
        const T: u64 = 1_800_000_000_000;
        let all = nets([4, 5, 7]);
        // First run: nothing cached, every network read once.
        let (home, fresh, _) = run(&BTreeMap::new(), &all, T);
        assert_eq!(fresh.len(), 3);
        assert_eq!(home, all.iter().map(|n| (*n, SiteId((n.0.0 % 3) as u8))).collect());
        let mut cached: BTreeMap<NetworkId, Home> = fresh.into_iter().collect();
        // A run 15 minutes later, and one just inside the day, read nothing and know every home.
        for later in [15 * 60_000, HOME_FOR_MS - 1] {
            let (home2, fresh2, unresolved2) = run(&cached, &all, T + later);
            assert!(fresh2.is_empty() && unresolved2.is_empty(), "read {fresh2:?} at +{later} ms");
            assert_eq!(home2, home);
        }
        // A new network is read alone.
        let more = nets([4, 5, 7, 8]);
        let (home3, fresh3, _) = run(&cached, &more, T + 60_000);
        assert_eq!(fresh3.iter().map(|(n, _)| n.0.0).collect::<Vec<_>>(), [8]);
        assert_eq!(home3.len(), 4);
        cached.extend(fresh3);
        // At the day, the old homes are read again, not the one read a minute after them.
        let (_, fresh4, _) = run(&cached, &more, T + HOME_FOR_MS);
        assert_eq!(fresh4.iter().map(|(n, _)| n.0.0).collect::<Vec<_>>(), [4, 5, 7]);
        assert!(fresh4.iter().all(|(_, h)| h.at == T + HOME_FOR_MS));
    }

    /// A failed read keeps a cached home and is tried again next run; a network never resolved has no home, so no
    /// site sweeps it. A run reads at most HOMES_PER_RUN, new networks before day-old ones.
    #[test]
    fn a_failed_read_keeps_the_cached_home_and_a_run_reads_at_most_its_cap() {
        const T: u64 = 1_800_000_000_000;
        let old = Home { site: SiteId(2), at: T - HOME_FOR_MS };
        let cached: BTreeMap<NetworkId, Home> = [(NetworkId(DbId(13)), old)].into();
        let (home, fresh, unresolved) = run(&cached, &nets([13, 14]), T);
        assert_eq!(unresolved, [13]);
        assert_eq!(home.get(&NetworkId(DbId(13))), Some(&SiteId(2)), "the cached home still sweeps");
        assert_eq!(fresh.iter().map(|(n, _)| n.0.0).collect::<Vec<_>>(), [14]);
        let (home, _, unresolved) = run(&BTreeMap::new(), &nets([13]), T);
        assert_eq!((home.len(), unresolved), (0, vec![13]));
        // 70 new networks and one day-old: the 64 lowest new ones, the stale one waits.
        let all = nets((100..170).chain([20]));
        let mut sorted = all.clone();
        sorted.sort();
        let cached: BTreeMap<NetworkId, Home> = [(NetworkId(DbId(20)), old)].into();
        let (_, fresh, _) = run(&cached, &sorted, T);
        assert_eq!(fresh.len(), HOMES_PER_RUN);
        assert_eq!(fresh.iter().map(|(n, _)| n.0.0).collect::<Vec<_>>(), (100..164).collect::<Vec<_>>());
    }
}
