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
use graftyard_types::{ChainId, DbId, Index, NetworkId, RepoId, SiteId, Timestamp};
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

/// Reads the cursor, or with `put` stores the next one and the run's report.
async fn cursor(env: &Env, site: &SiteEnv, put: Option<(Option<NetworkId>, &Value)>) -> Result<Option<NetworkId>> {
    let stub = site.registry(env, site.id, CURSOR)?;
    let mut init = RequestInit::new();
    if let Some((next, last)) = put {
        init.with_method(Method::Put)
            .with_body(Some(json!({ "next": next.map(|n| n.0.0), "last": last }).to_string().into()));
    }
    let mut res = stub.fetch_with_request(Request::new_with_init("https://registry/sweep", &init)?).await?;
    let v: Value = res.json().await?;
    Ok(v["next"].as_i64().map(|n| NetworkId(DbId(n))))
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
    // `mine` is synchronous, so every network's home is resolved first. A network whose home cannot be found this
    // run is left to the next one.
    let all = graftyard_git::networks(&buckets).await.map_err(|e| Error::RustError(e.0))?;
    let mut home: BTreeMap<NetworkId, SiteId> = BTreeMap::new();
    let mut unresolved = Vec::new();
    for n in all {
        let repo = RepoId(n.0);
        match crate::member::repo_owner(env, repo).await {
            Ok(owner) => match crate::member::owner_record(env, owner).await {
                Ok(r) => {
                    home.insert(n, r.home);
                }
                Err(e) => unresolved.push((n.0.0, e)),
            },
            Err(e) => unresolved.push((n.0.0, e)),
        }
    }
    let from = cursor(env, &site, None).await?;
    let me = site.id;
    let rows = Rows { env, records: Default::default() };
    let r = sweep_all(&buckets, &rows, now, CronBudget::SITE_CRON, from, |n| home.get(&n) == Some(&me))
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
        "unresolved": unresolved.iter().map(|(n, e)| json!({ "network": n, "error": e })).collect::<Vec<_>>(),
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
        let chain = ChainId::Repo(RepoId(DbId(7)));
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
}
