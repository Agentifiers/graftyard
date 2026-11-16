//! The site-script sweeper and third-bucket filler (`design/DESIGN.md`, Shape, "Packs", `sweep`; Stack, Cron Triggers;
//! journey 1, step 2).
//!
//! `decide` is pure: it sees one network's listings in every bucket of its placement, the answers of the chains'
//! `request` tables, and `now`, and returns the actions and alarms. Time enters only through `now`, compared with each
//! object's R2 upload time and its `PackId` time, so a test shortens a day by passing a later `now`. `sweep` lists,
//! asks the `RequestSource`, calls `decide`, and runs at most `budget` actions, so a cron run fits one invocation's
//! subrequest limit and the next run continues.
//!
//! Rules, per pack id:
//! - A pack with a `Committed` marker in any bucket is never deleted. A bucket that holds the pack but lacks the marker
//!   gets one, and a bucket of the placement that lacks a pack whose marker exists is filled from a bucket holding a
//!   copy of the marker's size and checksum.
//! - An unmarked pack is looked up in the `request` table of the chain its custom metadata names. A row whose events
//!   name this pack id proves the commit, so the marker is written from the row, at any age. A row that names other
//!   packs (a retried request whose first placement failed) or no row at all proves the pack never committed, but the
//!   pack is deleted only once it is at least `ORPHAN_AFTER` old by both its upload time and its `PackId` time, and
//!   younger than `REQUEST_RETENTION` by its `PackId` time, past which a missing row proves nothing: then it is kept
//!   and alarmed. An unmarked pack without readable metadata, or whose lookup failed, is kept.
//! - A marker whose pack no bucket holds is kept and alarmed.
//! - An upload journal older than `ORPHAN_AFTER` has its multipart upload aborted and is deleted.

use std::collections::{BTreeMap, BTreeSet};

use futures_util::{StreamExt, stream};

use graftyard_types::{ChainId, Index, NetworkId, RequestId, SiteId, Timestamp};

use crate::bucket::{Bucket, ObjectInfo, StoreError};
use crate::key::R2Key;
use crate::pack::{Marker, ORPHAN_AFTER, ObjectMeta, PackId, REQUEST_RETENTION};
use crate::store::{marker_from_meta, put_marker};

/// What the chain's `request` table says about one `RequestId`: the entry's index and the stored packs its events
/// name, with each one's checksum and size, which the repaired marker carries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequestRow {
    pub index: Index,
    pub packs: Vec<(PackId, Marker)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Lookup {
    Row(RequestRow),
    NoRow,
    /// The chain's leader could not answer. Nothing is decided for its objects this run.
    Unavailable,
}

/// The `request` table, read from the chain's leader through `fresh`. The Raft group that holds it is hosted by the
/// site script later (M1-WP2 and workers/site); until then tests use a stub.
#[allow(async_fn_in_trait)]
pub trait RequestSource {
    async fn request(&self, chain: ChainId, id: RequestId) -> Result<Option<RequestRow>, StoreError>;
}

/// One bucket's listing of one network.
#[derive(Clone, Debug)]
pub struct BucketView {
    pub site: SiteId,
    /// Under `R2Key::pack_prefix`: `.pack` and `.idx` objects.
    pub packs: Vec<ObjectInfo>,
    /// Under `R2Key::marker_prefix`.
    pub markers: Vec<ObjectInfo>,
    /// Under `R2Key::upload_prefix`, with the upload id as the object's `upload` metadata.
    pub uploads: Vec<ObjectInfo>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    PutMarker { site: SiteId, id: PackId, marker: Marker },
    Fill { to: SiteId, from: SiteId, id: PackId, marker: Marker },
    Delete { site: SiteId, key: String },
    Abort { site: SiteId, key: String, upload_id: String, journal: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Alarm {
    /// A marker names a pack no bucket holds with the marker's size.
    LostPack {
        id: PackId,
    },
    /// Unmarked, with no `request` row, and older than `REQUEST_RETENTION`.
    PastRetention {
        site: SiteId,
        id: PackId,
    },
    /// Unmarked and old enough to judge, but its metadata does not name a chain and request.
    NoMetadata {
        site: SiteId,
        key: String,
    },
    LookupFailed {
        chain: ChainId,
        request: RequestId,
    },
    /// A key under the network's prefixes that is not one of its kinds.
    Stray {
        site: SiteId,
        key: String,
    },
    /// Markers for one pack that disagree.
    MarkerConflict {
        id: PackId,
    },
    /// A committed pack's copy whose size or SHA-256 differs from its marker. Neither deleted nor spread.
    BadCopy {
        site: SiteId,
        id: PackId,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    pub actions: Vec<Action>,
    pub alarms: Vec<Alarm>,
    /// Unmarked packs kept because they are younger than `ORPHAN_AFTER`.
    pub young: usize,
}

fn ms(d: std::time::Duration) -> u64 {
    d.as_millis() as u64
}

/// A pack a roll consumed and its `.idx` are kept this long after the roll, before the sweeper may delete them
/// (`decisions/DECISIONS.md`, `roll-up`, "Deleting consumed packs": a shard mid-rehydrate is still loading them)
/// [guess].
pub const CONSUMED_KEEP: u64 = 7 * 86_400_000;

/// Whether the sweeper may delete the packs one roll consumed (`Compacted.consumed`), whose entry committed at
/// `compacted_index` and whose time is `rolled_at`: only once the roll is `CONSUMED_KEEP` old, every shard that could
/// answer is on the list (`shard_cursors`, `Some(Index(u64::MAX))` for a shard with no disk for the network), and every
/// answer holds a cursor at or past the roll's entry, so no shard still serves from the consumed packs. Not called yet:
/// the sweeper's deletion of consumed packs is G9's remainder.
pub fn consumed_deletable(
    rolled_at: Timestamp,
    now: Timestamp,
    shard_cursors: &[Option<Index>],
    compacted_index: Index,
) -> bool {
    now.0.saturating_sub(rolled_at.0) >= CONSUMED_KEEP
        && !shard_cursors.is_empty()
        && shard_cursors.iter().all(|c| c.is_some_and(|i| i >= compacted_index))
}

/// One pack id's objects in one bucket.
#[derive(Default)]
struct Held<'a> {
    pack: Option<&'a ObjectInfo>,
    idx: Option<&'a ObjectInfo>,
}

struct Index_<'a> {
    held: BTreeMap<PackId, BTreeMap<SiteId, Held<'a>>>,
    markers: BTreeMap<PackId, BTreeMap<SiteId, Option<Marker>>>,
    stray: Vec<(SiteId, String)>,
}

fn index<'a>(network: NetworkId, views: &'a [BucketView]) -> Index_<'a> {
    let mut ix = Index_ { held: BTreeMap::new(), markers: BTreeMap::new(), stray: Vec::new() };
    for v in views {
        for o in &v.packs {
            match o.key.parse::<R2Key>() {
                Ok(R2Key::Pack { network: n, id }) if n == network => {
                    ix.held.entry(id).or_default().entry(v.site).or_default().pack = Some(o)
                }
                Ok(R2Key::Idx { network: n, id }) if n == network => {
                    ix.held.entry(id).or_default().entry(v.site).or_default().idx = Some(o)
                }
                _ => ix.stray.push((v.site, o.key.clone())),
            }
        }
        for o in &v.markers {
            match o.key.parse::<R2Key>() {
                Ok(R2Key::Committed { of }) if matches!(*of, R2Key::Pack { network: n, .. } if n == network) => {
                    let R2Key::Pack { id, .. } = *of else { unreachable!() };
                    ix.markers.entry(id).or_default().insert(v.site, marker_from_meta(&o.meta));
                }
                _ => ix.stray.push((v.site, o.key.clone())),
            }
        }
    }
    ix
}

/// The (chain, request) pairs whose `request` rows `decide` needs: every unmarked pack with readable metadata.
pub fn lookups_needed(network: NetworkId, views: &[BucketView]) -> BTreeSet<(ChainId, RequestId)> {
    let ix = index(network, views);
    let mut out = BTreeSet::new();
    for (id, sites) in &ix.held {
        if ix.markers.contains_key(id) {
            continue;
        }
        for h in sites.values() {
            if let Some(m) =
                h.pack.and_then(|o| ObjectMeta::from_pairs(o.meta.iter().map(|(k, v)| (k.as_str(), v.as_str()))))
            {
                out.insert((m.chain, m.request));
            }
        }
    }
    out
}

pub fn decide(
    network: NetworkId,
    views: &[BucketView],
    lookups: &BTreeMap<(ChainId, RequestId), Lookup>,
    now: Timestamp,
) -> Plan {
    let ix = index(network, views);
    let mut plan = Plan::default();
    for (site, key) in &ix.stray {
        plan.alarms.push(Alarm::Stray { site: *site, key: key.clone() });
    }
    let sites: Vec<SiteId> = views.iter().map(|v| v.site).collect();
    let ids: BTreeSet<PackId> = ix.held.keys().chain(ix.markers.keys()).copied().collect();
    let empty = BTreeMap::new();
    for id in ids {
        let held = ix.held.get(&id).unwrap_or(&empty);
        let marker = match ix.markers.get(&id) {
            None => None,
            Some(ms) => {
                // A marker whose metadata does not parse still proves the commit; its body is unknown here.
                let bodies: BTreeSet<_> =
                    ms.values().flatten().map(|m| (m.chain, m.index, m.checksum, m.bytes)).collect();
                if bodies.len() > 1 {
                    plan.alarms.push(Alarm::MarkerConflict { id });
                }
                Some(ms.values().flatten().next().cloned())
            }
        };
        match marker {
            Some(body) => committed(&mut plan, id, body, held, ix.markers.get(&id).unwrap(), &sites),
            None => uncommitted(&mut plan, id, held, lookups, now),
        }
    }
    for v in views {
        for o in &v.uploads {
            let Ok(R2Key::Upload { id, network: n }) = o.key.parse::<R2Key>() else {
                plan.alarms.push(Alarm::Stray { site: v.site, key: o.key.clone() });
                continue;
            };
            let old = now.0.saturating_sub(o.uploaded.0.max(id.time().0)) >= ms(ORPHAN_AFTER);
            match o.meta("upload") {
                Some(upload_id) if old && n == network => plan.actions.push(Action::Abort {
                    site: v.site,
                    key: R2Key::Pack { network, id }.to_string(),
                    upload_id: upload_id.to_owned(),
                    journal: o.key.clone(),
                }),
                Some(_) => {}
                None => plan.alarms.push(Alarm::Stray { site: v.site, key: o.key.clone() }),
            }
        }
    }
    plan.actions.sort_by_key(phase);
    plan
}

/// Markers, then fills, then aborts, then deletes: the order `sweep` runs a plan in.
fn phase(a: &Action) -> u8 {
    match a {
        Action::PutMarker { .. } => 0,
        Action::Fill { .. } => 1,
        Action::Abort { .. } => 2,
        Action::Delete { .. } => 3,
    }
}

fn committed(
    plan: &mut Plan,
    id: PackId,
    body: Option<Marker>,
    held: &BTreeMap<SiteId, Held<'_>>,
    marked: &BTreeMap<SiteId, Option<Marker>>,
    sites: &[SiteId],
) {
    let Some(m) = body else { return };
    // A copy counts when its size matches the marker, and its SHA-256 too where R2 reports one.
    let good = |h: &Held<'_>| h.pack.is_some_and(|o| o.size == m.bytes && o.sha256.is_none_or(|d| d == m.checksum));
    let source = held.iter().find(|(_, h)| good(h)).map(|(s, _)| *s);
    let Some(source) = source else {
        plan.alarms.push(Alarm::LostPack { id });
        return;
    };
    for site in sites {
        match held.get(site).and_then(|h| h.pack) {
            // The fill puts the marker after the bytes.
            None => plan.actions.push(Action::Fill { to: *site, from: source, id, marker: m.clone() }),
            Some(_) if !held.get(site).is_some_and(good) => plan.alarms.push(Alarm::BadCopy { site: *site, id }),
            Some(_) if !marked.contains_key(site) => {
                plan.actions.push(Action::PutMarker { site: *site, id, marker: m.clone() })
            }
            Some(_) => {}
        }
    }
}

fn uncommitted(
    plan: &mut Plan,
    id: PackId,
    held: &BTreeMap<SiteId, Held<'_>>,
    lookups: &BTreeMap<(ChainId, RequestId), Lookup>,
    now: Timestamp,
) {
    let meta = held
        .values()
        .filter_map(|h| h.pack)
        .find_map(|o| ObjectMeta::from_pairs(o.meta.iter().map(|(k, v)| (k.as_str(), v.as_str()))));
    let youngest = held.values().flat_map(|h| [h.pack, h.idx]).flatten().map(|o| o.uploaded.0).max().unwrap_or(0);
    let age = now.0.saturating_sub(youngest.max(id.time().0));
    let id_age = now.0.saturating_sub(id.time().0);
    let row = meta.map(|m| (m, lookups.get(&(m.chain, m.request)).cloned().unwrap_or(Lookup::Unavailable)));
    if let Some((_, Lookup::Row(row))) = &row
        && let Some((_, marker)) = row.packs.iter().find(|(p, _)| *p == id)
    {
        // The commit is proven at any age; mark every bucket that holds the pack, and the filler copies the rest on
        // a later run.
        for (site, h) in held {
            if h.pack.is_some() {
                plan.actions.push(Action::PutMarker { site: *site, id, marker: marker.clone() });
            }
        }
        return;
    }
    if age < ms(ORPHAN_AFTER) {
        plan.young += 1;
        return;
    }
    let delete = |plan: &mut Plan| {
        for (site, h) in held {
            for o in [h.pack, h.idx].into_iter().flatten() {
                plan.actions.push(Action::Delete { site: *site, key: o.key.clone() });
            }
        }
    };
    match row {
        // Only a `.idx` is left: a pack deleted by an earlier run, or an index whose pack put never landed.
        None if held.values().all(|h| h.pack.is_none()) => delete(plan),
        None => {
            let (site, key) = held.iter().find_map(|(s, h)| h.pack.map(|o| (*s, o.key.clone()))).unwrap();
            plan.alarms.push(Alarm::NoMetadata { site, key });
        }
        Some((m, Lookup::Unavailable)) => plan.alarms.push(Alarm::LookupFailed { chain: m.chain, request: m.request }),
        Some(_) if id_age >= ms(REQUEST_RETENTION) => {
            for site in held.keys() {
                plan.alarms.push(Alarm::PastRetention { site: *site, id });
            }
        }
        Some((_, Lookup::NoRow | Lookup::Row(_))) => delete(plan),
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SweepReport {
    pub plan: Plan,
    /// Actions run this time, from the front of `plan.actions`.
    pub done: usize,
    pub failed: Vec<(Action, String)>,
}

/// R2 and `request` calls in flight at once during a sweep.
const CONCURRENCY: usize = 16;

/// One run for one network over the buckets of its placement: list, look up, decide, act on at most `budget`
/// actions, taken from the front of the plan, which `decide` orders markers, fills, aborts, deletes.
pub async fn sweep<B: Bucket, R: RequestSource>(
    network: NetworkId,
    buckets: &[(SiteId, &B)],
    requests: &R,
    now: Timestamp,
    budget: usize,
) -> Result<SweepReport, StoreError> {
    let mut views = Vec::new();
    for (site, b) in buckets {
        views.push(BucketView {
            site: *site,
            packs: b.list(&R2Key::pack_prefix(network)).await?,
            markers: b.list(&R2Key::marker_prefix(network)).await?,
            uploads: b.list(&R2Key::upload_prefix(network)).await?,
        });
    }
    let lookups: BTreeMap<(ChainId, RequestId), Lookup> = stream::iter(lookups_needed(network, &views))
        .map(|(chain, id)| async move {
            let l = match requests.request(chain, id).await {
                Ok(Some(row)) => Lookup::Row(row),
                Ok(None) => Lookup::NoRow,
                Err(_) => Lookup::Unavailable,
            };
            ((chain, id), l)
        })
        .buffer_unordered(CONCURRENCY)
        .collect()
        .await;
    let plan = decide(network, &views, &lookups, now);
    let bucket = |s: SiteId| buckets.iter().find(|(x, _)| *x == s).map(|(_, b)| *b).expect("planned site");
    let mut report = SweepReport { plan: Plan::default(), done: 0, failed: Vec::new() };
    let chosen: Vec<&Action> = plan.actions.iter().take(budget).collect();
    // Phases in order: markers, then fills, then aborts, then deletes. Each phase runs concurrently and ends before
    // the next starts, so a run that dies midway has marked everything it was going to before deleting anything.
    for p in 0..3 {
        let results: Vec<(Action, Result<(), StoreError>)> = stream::iter(chosen.iter().filter(|a| phase(a) == p))
            .map(|a| async move {
                let r = match a {
                    Action::PutMarker { site, id, marker } => put_marker(bucket(*site), network, *id, marker).await,
                    Action::Fill { to, from, id, marker } => {
                        fill(bucket(*from), bucket(*to), network, *id, marker).await
                    }
                    Action::Abort { site, key, upload_id, journal } => {
                        match bucket(*site).abort_upload(key, upload_id).await {
                            Ok(()) => bucket(*site).delete(std::slice::from_ref(journal)).await,
                            Err(e) => Err(e),
                        }
                    }
                    Action::Delete { .. } => unreachable!("deletes run in their own phase"),
                };
                ((*a).clone(), r)
            })
            .buffer_unordered(CONCURRENCY)
            .collect()
            .await;
        for (a, r) in results {
            match r {
                Ok(()) => report.done += 1,
                Err(StoreError(e)) => report.failed.push((a, e)),
            }
        }
    }
    let mut deletes: BTreeMap<SiteId, Vec<String>> = BTreeMap::new();
    for a in chosen.iter().filter(|a| phase(a) == 3) {
        if let Action::Delete { site, key } = a {
            deletes.entry(*site).or_default().push(key.clone());
        }
    }
    for (site, keys) in deletes {
        for chunk in keys.chunks(1000) {
            match bucket(site).delete(chunk).await {
                Ok(()) => report.done += chunk.len(),
                Err(StoreError(e)) => report.failed.push((Action::Delete { site, key: chunk[0].clone() }, e)),
            }
        }
    }
    report.plan = plan;
    Ok(report)
}

/// One Cron Trigger run's limits. A Worker invocation has a subrequest cap, so a run sweeps at most `networks`
/// networks and runs at most `actions` actions across them; the next run continues after `SweepAll::next`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CronBudget {
    pub networks: usize,
    pub actions: usize,
}

impl CronBudget {
    /// A network costs three listings per bucket plus its lookups and actions, so 20 networks over 3 buckets (180
    /// listings) and 600 actions stay under 1,000 subrequests with room for the lookups [guess, unmeasured].
    pub const SITE_CRON: CronBudget = CronBudget { networks: 20, actions: 600 };
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SweepAll {
    pub swept: Vec<(NetworkId, SweepReport)>,
    /// Networks whose sweep failed (a listing error), kept for the next run.
    pub failed: Vec<(NetworkId, String)>,
    /// The network the next run starts at, or `None` when this run reached the end of the list, so the next run
    /// starts again from the first.
    pub next: Option<NetworkId>,
}

/// The networks with anything under `pack/`, `committed/pack/`, or `upload/` in any of `buckets`, ascending.
pub async fn networks<B: Bucket>(buckets: &[(SiteId, &B)]) -> Result<Vec<NetworkId>, StoreError> {
    let mut out = BTreeSet::new();
    for (_, b) in buckets {
        for prefix in ["pack/", "committed/pack/", "upload/"] {
            for seg in b.segments(prefix).await? {
                if let Ok(n) = seg.parse::<i64>() {
                    out.insert(NetworkId(graftyard_types::DbId(n)));
                }
            }
        }
    }
    Ok(out.into_iter().collect())
}

/// The site script's sweeper cron (`design/DESIGN.md`, Stack, Cron Triggers: "pack sweeper (including orphan
/// multipart aborts)"): every network `mine` accepts, in id order from `from`, swept with `sweep` until `budget`
/// runs out. `mine` lets each site sweep only the networks it is home for, so three sites do not repeat one another;
/// the sweep is idempotent either way. The caller stores `next` and passes it as `from` on the next run.
pub async fn sweep_all<B: Bucket, R: RequestSource>(
    buckets: &[(SiteId, &B)],
    requests: &R,
    now: Timestamp,
    budget: CronBudget,
    from: Option<NetworkId>,
    mine: impl Fn(NetworkId) -> bool,
) -> Result<SweepAll, StoreError> {
    let all: Vec<NetworkId> = networks(buckets).await?.into_iter().filter(|n| mine(*n)).collect();
    let start = from.map_or(0, |f| all.partition_point(|n| n.0.0 < f.0.0));
    let mut out = SweepAll::default();
    let mut actions = budget.actions;
    for n in all.iter().skip(start) {
        if out.swept.len() + out.failed.len() >= budget.networks || actions == 0 {
            out.next = Some(*n);
            return Ok(out);
        }
        match sweep(*n, buckets, requests, now, actions).await {
            Ok(r) => {
                let ran = r.done + r.failed.len();
                // Cut by the budget, with planned actions left: the next run continues at this network. Failed
                // actions alone do not hold the cron on one network; the next full pass retries them.
                let rest = ran >= actions && r.plan.actions.len() > ran;
                actions = actions.saturating_sub(ran);
                out.swept.push((*n, r));
                if rest {
                    out.next = Some(*n);
                    return Ok(out);
                }
            }
            Err(StoreError(e)) => out.failed.push((*n, e)),
        }
    }
    Ok(out)
}

/// Copies a committed pack into `to`, create-only, with `to` refusing bytes whose size or SHA-256 is not the marker's,
/// and its stored `.idx` when the source holds one: a promotion puts the idx only beside the copies its placement
/// waited for, and without it a rehydrate that reads this bucket runs `index-pack` (`crate::catchup`). The idx is
/// derived, so its copy may fail. Both stream from bucket to bucket (`Bucket::copy_new`): read whole, 16 fills at once
/// of a 5 GB monorepo's packs held 328 MB, and site a's cron died on that network on every run (R22).
async fn fill<B: Bucket>(from: &B, to: &B, network: NetworkId, id: PackId, m: &Marker) -> Result<(), StoreError> {
    let key = R2Key::Pack { network, id }.to_string();
    to.copy_new(from, &key, Some((m.bytes, m.checksum)))
        .await?
        .ok_or_else(|| StoreError(format!("{key} vanished from the source")))?;
    let _ = to.copy_new(from, &R2Key::Idx { network, id }.to_string(), None).await;
    put_marker(to, network, id, m).await
}

#[cfg(test)]
mod tests {
    use futures_executor::block_on;
    use graftyard_types::{DbId, Digest, RepoId, Ulid};
    use sha2::{Digest as _, Sha256};

    use super::*;
    use crate::mem::MemBucket;

    const NET: NetworkId = NetworkId(DbId(7));
    const CHAIN: ChainId = ChainId::Repo(RepoId(DbId(7)));
    const H: u64 = 3_600_000;
    const T0: u64 = 1_800_000_000_000;

    fn pid(n: u8, at: u64) -> PackId {
        PackId(Ulid::new(Timestamp(at), [n; 10]))
    }

    fn rid(n: u8) -> RequestId {
        RequestId(Ulid::new(Timestamp(T0), [n; 10]))
    }

    fn body(n: u8) -> Vec<u8> {
        vec![n; 100 + n as usize]
    }

    fn marker(n: u8, index: u64) -> Marker {
        let b = body(n);
        Marker { chain: CHAIN, index: Index(index), checksum: Digest(Sha256::digest(&b).into()), bytes: b.len() as u64 }
    }

    fn put_pack(b: &MemBucket, id: PackId, n: u8, req: RequestId, at: u64) {
        let meta = ObjectMeta { chain: CHAIN, request: req }.to_pairs().to_vec();
        b.plant(&R2Key::Pack { network: NET, id }.to_string(), &body(n), Timestamp(at), &meta);
    }

    fn put_mark(b: &MemBucket, id: PackId, m: &Marker) {
        b.set_now(Timestamp(T0));
        block_on(put_marker(b, NET, id, m)).unwrap();
    }

    #[derive(Default)]
    struct Rows(BTreeMap<RequestId, RequestRow>, bool);

    impl RequestSource for Rows {
        async fn request(&self, _chain: ChainId, id: RequestId) -> Result<Option<RequestRow>, StoreError> {
            if self.1 {
                return Err(StoreError("leader down".into()));
            }
            Ok(self.0.get(&id).cloned())
        }
    }

    struct Bed {
        b: [MemBucket; 3],
    }

    impl Bed {
        fn new() -> Bed {
            Bed { b: Default::default() }
        }
        fn buckets(&self) -> Vec<(SiteId, &MemBucket)> {
            self.b.iter().enumerate().map(|(i, b)| (SiteId(i as u8), b)).collect()
        }
        fn run(&self, rows: &Rows, now: u64) -> SweepReport {
            block_on(sweep(NET, &self.buckets(), rows, Timestamp(now), usize::MAX)).unwrap()
        }
        fn has(&self, site: usize, key: &R2Key) -> bool {
            self.b[site].keys().contains(&key.to_string())
        }
    }

    fn pack_key(id: PackId) -> R2Key {
        R2Key::Pack { network: NET, id }
    }

    fn mark_key(id: PackId) -> R2Key {
        R2Key::committed(pack_key(id)).unwrap()
    }

    /// G9's remainder, not called yet: the sweeper's deletion of consumed packs waits for the keep and for every
    /// shard of the network to have the roll's entry, and neither a shard that could not answer (`None`) nor a missing
    /// list holds it open (`consumed_deletable`).
    #[test]
    fn consumed_packs_wait_out_the_keep_and_every_shard_past_the_roll() {
        let (rolled, at) = (Timestamp(T0), Index(90));
        // 7 days minus 1 ms: kept whatever the shards answer.
        assert!(!consumed_deletable(rolled, Timestamp(T0 + CONSUMED_KEEP - 1), &[Some(at)], at));
        // Exactly the keep, every shard at or past the roll's entry.
        assert!(consumed_deletable(rolled, Timestamp(T0 + CONSUMED_KEEP), &[Some(at), Some(Index(89 + 10))], at));
        // One shard still behind the roll's entry.
        assert!(!consumed_deletable(rolled, Timestamp(T0 + CONSUMED_KEEP), &[Some(at), Some(Index(89))], at));
        // A shard that could not answer holds the deletion.
        assert!(!consumed_deletable(rolled, Timestamp(T0 + CONSUMED_KEEP), &[Some(at), None], at));
        // No shard was asked at all.
        assert!(!consumed_deletable(rolled, Timestamp(T0 + CONSUMED_KEEP), &[], at));
        // A shard with no disk for the network is past every roll, as the caller passes it.
        assert!(consumed_deletable(rolled, Timestamp(T0 + CONSUMED_KEEP), &[Some(Index(u64::MAX))], at));
    }

    #[test]
    fn failed_placement_is_deleted_after_a_day_and_not_before() {
        let bed = Bed::new();
        let id = pid(1, T0);
        put_pack(&bed.b[0], id, 1, rid(1), T0);
        let rows = Rows::default();
        let r = bed.run(&rows, T0 + 23 * H);
        assert_eq!((r.plan.young, r.done), (1, 0));
        assert!(bed.has(0, &pack_key(id)));
        bed.run(&rows, T0 + 24 * H);
        assert!(!bed.has(0, &pack_key(id)));
    }

    #[test]
    fn committed_pack_in_one_bucket_with_a_marker_is_filled_never_deleted() {
        let bed = Bed::new();
        let id = pid(2, T0);
        put_pack(&bed.b[1], id, 2, rid(2), T0);
        put_mark(&bed.b[1], id, &marker(2, 40));
        let idx = R2Key::Idx { network: NET, id }.to_string();
        let range = [("from".to_owned(), "1".to_owned()), ("to".to_owned(), "40".to_owned())];
        bed.b[1].plant(&idx, b"the idx", Timestamp(T0), &range);
        let r = bed.run(&Rows::default(), T0 + 100 * H);
        assert!(r.failed.is_empty(), "{:?}", r.failed);
        for s in 0..3 {
            assert!(bed.has(s, &pack_key(id)) && bed.has(s, &mark_key(id)), "site {s}");
        }
        // Store to store: the sweeper read none of the bytes it copied, which on a Workers host would sit in the
        // isolate's memory, 16 fills at once (R22).
        assert_eq!(bed.b[1].bytes_read(), 0);
        // The fill copied the bytes and the metadata, and the stored idx with its range.
        assert_eq!(block_on(bed.b[0].get(&pack_key(id).to_string())).unwrap(), Some(body(2)));
        assert_eq!(block_on(bed.b[2].get(&idx)).unwrap().as_deref(), Some(&b"the idx"[..]));
        assert_eq!(block_on(bed.b[2].head(&idx)).unwrap().map(|o| o.meta), Some(range.to_vec()));
        // A second run has nothing to do.
        assert!(bed.run(&Rows::default(), T0 + 101 * H).plan.actions.is_empty());
    }

    #[test]
    fn young_unmarked_pack_with_no_row_is_kept() {
        let bed = Bed::new();
        let id = pid(3, T0);
        put_pack(&bed.b[0], id, 3, rid(3), T0);
        put_pack(&bed.b[2], id, 3, rid(3), T0 + 2 * H);
        // 24 h after the first copy but not after the second: the youngest copy sets the age.
        let r = bed.run(&Rows::default(), T0 + 25 * H);
        assert_eq!(r.plan.young, 1);
        assert!(bed.has(0, &pack_key(id)) && bed.has(2, &pack_key(id)));
    }

    #[test]
    fn pack_id_time_counts_as_well_as_upload_time() {
        // A pack whose id was minted after its upload time (clock skew) is judged by the later of the two.
        let bed = Bed::new();
        let id = pid(4, T0 + 10 * H);
        put_pack(&bed.b[0], id, 4, rid(4), T0);
        assert_eq!(bed.run(&Rows::default(), T0 + 30 * H).plan.young, 1);
        assert!(bed.has(0, &pack_key(id)));
    }

    #[test]
    fn missing_marker_is_repaired_from_the_request_row_at_any_age() {
        let bed = Bed::new();
        let id = pid(5, T0);
        put_pack(&bed.b[0], id, 5, rid(5), T0);
        put_pack(&bed.b[1], id, 5, rid(5), T0);
        let mut rows = Rows::default();
        rows.0.insert(rid(5), RequestRow { index: Index(9), packs: vec![(id, marker(5, 9))] });
        bed.run(&rows, T0 + H);
        assert!(bed.has(0, &mark_key(id)) && bed.has(1, &mark_key(id)));
        // The next run fills the third bucket from the repaired markers, and an old age deletes nothing.
        bed.run(&Rows::default(), T0 + 30 * H);
        assert!(bed.has(2, &pack_key(id)) && bed.has(2, &mark_key(id)));
        let info = block_on(bed.b[0].list(&R2Key::marker_prefix(NET))).unwrap();
        assert_eq!(marker_from_meta(&info[0].meta), Some(marker(5, 9)));
    }

    #[test]
    fn a_row_for_a_retried_request_that_names_another_pack_does_not_save_this_one() {
        // The first placement of request 6 failed with one copy; the retry placed pack `b` and committed.
        let bed = Bed::new();
        let (a, b) = (pid(6, T0), pid(16, T0));
        put_pack(&bed.b[0], a, 6, rid(6), T0);
        put_pack(&bed.b[0], b, 16, rid(6), T0);
        put_pack(&bed.b[1], b, 16, rid(6), T0);
        let mut rows = Rows::default();
        rows.0.insert(rid(6), RequestRow { index: Index(3), packs: vec![(b, marker(16, 3))] });
        bed.run(&rows, T0 + 25 * H);
        assert!(!bed.has(0, &pack_key(a)), "the failed first placement is an orphan");
        assert!(!bed.has(0, &mark_key(a)), "and never gets a marker");
        assert!(bed.has(0, &mark_key(b)) && bed.has(1, &mark_key(b)));
        // The filler copies `b` to the third bucket on the next run, from the markers alone.
        bed.run(&Rows::default(), T0 + 26 * H);
        assert!(bed.has(2, &pack_key(b)) && bed.has(2, &mark_key(b)));
    }

    #[test]
    fn past_retention_or_unknown_metadata_or_a_down_leader_keeps_the_object() {
        let bed = Bed::new();
        let old = pid(7, T0);
        put_pack(&bed.b[0], old, 7, rid(7), T0);
        let bare = pid(8, T0);
        bed.b[0].plant(&pack_key(bare).to_string(), &body(8), Timestamp(T0), &[]);
        let r = bed.run(&Rows::default(), T0 + 49 * H);
        assert!(r.plan.alarms.contains(&Alarm::PastRetention { site: SiteId(0), id: old }));
        assert!(r.plan.alarms.iter().any(|a| matches!(a, Alarm::NoMetadata { .. })));
        assert!(bed.has(0, &pack_key(old)) && bed.has(0, &pack_key(bare)));
        let down = Rows(BTreeMap::new(), true);
        let fresh = pid(9, T0);
        put_pack(&bed.b[1], fresh, 9, rid(9), T0);
        let r = bed.run(&down, T0 + 25 * H);
        assert!(r.plan.alarms.contains(&Alarm::LookupFailed { chain: CHAIN, request: rid(9) }));
        assert!(bed.has(1, &pack_key(fresh)));
    }

    #[test]
    fn marker_for_a_missing_object_alarms_and_keeps_the_marker() {
        let bed = Bed::new();
        let id = pid(10, T0);
        put_mark(&bed.b[0], id, &marker(10, 2));
        let r = bed.run(&Rows::default(), T0 + 100 * H);
        assert_eq!(r.plan.alarms, vec![Alarm::LostPack { id }]);
        assert!(r.plan.actions.is_empty());
        assert!(bed.has(0, &mark_key(id)));
        // A copy of the wrong size is not a source: still lost, and the bad copy is neither deleted nor spread.
        bed.b[1].plant(&pack_key(id).to_string(), b"short", Timestamp(T0), &[]);
        let r = bed.run(&Rows::default(), T0 + 100 * H);
        assert_eq!(r.plan.alarms, vec![Alarm::LostPack { id }]);
        assert!(bed.has(1, &pack_key(id)));
        // With a good copy elsewhere, the bad one is alarmed, not overwritten, and the empty bucket is filled.
        put_pack(&bed.b[2], id, 10, rid(10), T0);
        let r = bed.run(&Rows::default(), T0 + 100 * H);
        assert_eq!(r.plan.alarms, vec![Alarm::BadCopy { site: SiteId(1), id }]);
        assert!(bed.has(0, &pack_key(id)) && bed.has(2, &mark_key(id)));
        assert_eq!(block_on(bed.b[1].get(&pack_key(id).to_string())).unwrap().as_deref(), Some(&b"short"[..]));
    }

    #[test]
    fn a_multipart_copy_whose_bytes_differ_from_its_marker_is_not_spread() {
        // A multipart copy carries no SHA-256, so its size alone makes it a source; the fill must still check the
        // bytes against the marker's checksum before the bucket keeps them.
        use crate::bucket::{Multipart, PartBuf};
        let bed = Bed::new();
        let id = pid(15, T0);
        let (key, journal) = (pack_key(id).to_string(), R2Key::Upload { network: NET, id }.to_string());
        let mut part = crate::mem::MemPart::with_capacity(0);
        part.push(&vec![0xee; body(15).len()]);
        let up = block_on(bed.b[0].start_upload(&key, &journal, &[])).unwrap().unwrap();
        let etag = block_on(bed.b[0].put_part(&key, &up, 1, &part)).unwrap();
        block_on(bed.b[0].complete_upload(&key, &up, &journal, &[(1, etag)])).unwrap();
        put_mark(&bed.b[0], id, &marker(15, 3));
        let r = bed.run(&Rows::default(), T0 + 100 * H);
        assert_eq!(r.failed.len(), 2, "{:?}", r.plan.actions);
        assert!(!bed.has(1, &pack_key(id)) && !bed.has(2, &pack_key(id)));
        assert!(!bed.has(1, &mark_key(id)) && !bed.has(2, &mark_key(id)));
    }

    #[test]
    fn orphan_multipart_uploads_are_aborted_after_a_day() {
        let bed = Bed::new();
        let id = pid(11, T0);
        let key = pack_key(id).to_string();
        bed.b[0].open_upload(&key, "u-1");
        bed.b[0].plant(
            &R2Key::Upload { network: NET, id }.to_string(),
            b"",
            Timestamp(T0),
            &[("upload".into(), "u-1".into())],
        );
        bed.run(&Rows::default(), T0 + H);
        assert_eq!(bed.b[0].open_uploads().len(), 1);
        bed.run(&Rows::default(), T0 + 24 * H);
        assert!(bed.b[0].open_uploads().is_empty());
        assert!(bed.b[0].keys().is_empty(), "journal deleted");
    }

    #[test]
    fn stray_keys_and_idx_without_pack() {
        let bed = Bed::new();
        bed.b[0].plant("pack/7/not-an-id.pack", b"x", Timestamp(T0), &[]);
        bed.b[0].plant("pack/7/../8/x", b"x", Timestamp(T0), &[]);
        let id = pid(12, T0);
        bed.b[0].plant(&R2Key::Idx { network: NET, id }.to_string(), b"idx", Timestamp(T0), &[]);
        let r = bed.run(&Rows::default(), T0 + 25 * H);
        assert_eq!(r.plan.alarms.iter().filter(|a| matches!(a, Alarm::Stray { .. })).count(), 2);
        assert_eq!(bed.b[0].keys(), vec!["pack/7/../8/x".to_owned(), "pack/7/not-an-id.pack".to_owned()]);
    }

    fn net(n: i64) -> NetworkId {
        NetworkId(DbId(n))
    }

    /// An orphan (failed placement, a day old) in network `n` of every bucket.
    fn orphan(bed: &Bed, n: i64, k: u8) {
        let id = pid(k, T0 - 30 * H);
        for b in &bed.b {
            let meta = ObjectMeta { chain: ChainId::Repo(RepoId(DbId(n))), request: rid(k) }.to_pairs().to_vec();
            b.plant(&R2Key::Pack { network: net(n), id }.to_string(), &body(k), Timestamp(T0 - 30 * H), &meta);
        }
    }

    fn all(bed: &Bed, rows: &Rows, budget: CronBudget, from: Option<NetworkId>) -> SweepAll {
        block_on(sweep_all(&bed.buckets(), rows, Timestamp(T0), budget, from, |_| true)).unwrap()
    }

    #[test]
    fn sweep_all_finds_every_network_and_resumes_after_its_budget() {
        let bed = Bed::new();
        for n in [3, 11, 7, 8, 100] {
            orphan(&bed, n, n as u8);
        }
        // A network known only by an upload journal or a marker is found too.
        bed.b[1].plant(&R2Key::Upload { network: net(42), id: pid(9, T0) }.to_string(), b"{}", Timestamp(T0), &[]);
        assert_eq!(block_on(networks(&bed.buckets())).unwrap(), [3, 7, 8, 11, 42, 100].map(net));
        let rows = Rows::default();
        // Two networks per run, each exactly once, consecutive ids included: 3 7, 8 11, 42 100, then the end.
        let b = CronBudget { networks: 2, actions: 100 };
        let r1 = all(&bed, &rows, b, None);
        assert_eq!(r1.swept.iter().map(|(n, _)| *n).collect::<Vec<_>>(), [net(3), net(7)]);
        assert_eq!(r1.next, Some(net(8)));
        let r2 = all(&bed, &rows, b, r1.next);
        assert_eq!(r2.swept.iter().map(|(n, _)| *n).collect::<Vec<_>>(), [net(8), net(11)]);
        let r3 = all(&bed, &rows, b, r2.next);
        assert_eq!(r3.swept.iter().map(|(n, _)| *n).collect::<Vec<_>>(), [net(42), net(100)]);
        assert_eq!(r3.next, None, "the end of the list: the next run starts over");
        // Every orphan deleted in all three buckets: 5 networks, 3 copies.
        let deleted: usize = [&r1, &r2, &r3].iter().flat_map(|r| &r.swept).map(|(_, s)| s.done).sum();
        assert_eq!(deleted, 5 * 3);
        assert!(bed.b.iter().all(|b| b.keys().iter().all(|k| !k.starts_with("pack/"))));
    }

    #[test]
    fn sweep_all_stops_inside_a_network_when_actions_run_out_and_continues_there() {
        let bed = Bed::new();
        for k in 1..=5 {
            orphan(&bed, 8, k);
        }
        orphan(&bed, 9, 6);
        let rows = Rows::default();
        // 15 deletes in network 8; 10 actions per run.
        let r1 = all(&bed, &rows, CronBudget { networks: 10, actions: 10 }, None);
        assert_eq!(r1.swept.len(), 1);
        assert_eq!(r1.swept[0].1.done, 10);
        assert_eq!(r1.next, Some(net(8)), "network 8 again next run");
        let r2 = all(&bed, &rows, CronBudget { networks: 10, actions: 10 }, r1.next);
        assert_eq!(r2.swept.iter().map(|(n, s)| (*n, s.done)).collect::<Vec<_>>(), [(net(8), 5), (net(9), 3)]);
        assert_eq!(r2.next, None);
    }

    #[test]
    fn sweep_all_sweeps_only_the_networks_the_site_owns_and_never_deletes_committed_packs() {
        let bed = Bed::new();
        orphan(&bed, 20, 1);
        orphan(&bed, 21, 2);
        // A committed pack in network 20, a day old, in all three buckets, its marker in one only.
        let id = pid(4, T0 - 30 * H);
        let meta = ObjectMeta { chain: ChainId::Repo(RepoId(DbId(20))), request: rid(4) }.to_pairs().to_vec();
        for b in &bed.b {
            b.plant(&R2Key::Pack { network: net(20), id }.to_string(), &body(3), Timestamp(T0 - 30 * H), &meta);
        }
        bed.b[0].set_now(Timestamp(T0));
        block_on(put_marker(&bed.b[0], net(20), id, &marker(3, 9))).unwrap();
        let rows = Rows::default();
        let even = |n: NetworkId| n.0.0 % 2 == 0;
        let r = block_on(sweep_all(&bed.buckets(), &rows, Timestamp(T0), CronBudget::SITE_CRON, None, even)).unwrap();
        assert_eq!(r.swept.iter().map(|(n, _)| *n).collect::<Vec<_>>(), [net(20)]);
        let pk = R2Key::Pack { network: net(20), id }.to_string();
        let mk = R2Key::committed(R2Key::Pack { network: net(20), id }).unwrap().to_string();
        for b in &bed.b {
            assert!(b.keys().contains(&pk), "a committed pack is never deleted");
            assert!(b.keys().contains(&mk), "its marker is repaired in every bucket");
            assert_eq!(b.keys().iter().filter(|k| k.starts_with("pack/20/")).count(), 1, "the orphan is deleted");
            assert!(b.keys().iter().any(|k| k.starts_with("pack/21/")), "network 21 is another site's");
        }
    }

    #[test]
    fn markers_and_fills_run_before_deletes_and_budget_cuts_the_tail() {
        let bed = Bed::new();
        let committed = pid(13, T0);
        put_pack(&bed.b[0], committed, 13, rid(13), T0);
        put_pack(&bed.b[1], committed, 13, rid(13), T0);
        let orphan = pid(14, T0);
        put_pack(&bed.b[0], orphan, 14, rid(14), T0);
        let mut rows = Rows::default();
        rows.0.insert(rid(13), RequestRow { index: Index(1), packs: vec![(committed, marker(13, 1))] });
        let r = block_on(sweep(NET, &bed.buckets(), &rows, Timestamp(T0 + 25 * H), 2)).unwrap();
        assert_eq!(r.done, 2);
        assert!(bed.has(0, &mark_key(committed)) && bed.has(1, &mark_key(committed)));
        assert!(bed.has(0, &pack_key(orphan)), "budget spent on markers first");
    }
}
