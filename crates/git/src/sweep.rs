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
    /// Whether `chain` has no group: the lookup reached its members and every one answered not joined, as a retired or
    /// purged group's do. A sweep writes no marker or copy for such a chain (`sweep`). Any other answer, an outage
    /// included, is `false`, and the sweep goes on as before.
    async fn gone(&self, _chain: ChainId) -> bool {
        false
    }
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
    /// The chain's group is gone (retired or purged): its markers and fills were dropped from the plan.
    Gone {
        chain: ChainId,
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

/// `ReplaceMember`'s fill of the spare's bucket (`design/DESIGN.md`, Failure modes, pack buckets): every committed pack
/// of every network that `spare` lacks, copied from a surviving bucket, never from `lost`, whose views are dropped.
/// Packs held in one surviving bucket alone come first across all networks, since a second loss would take them; then
/// the rest, network by network. Each pack is copied once per network, not once per group. Only fills to `spare`: the
/// cron's own sweep marks, deletes and aborts as before.
pub fn refill(networks: &[(NetworkId, Vec<BucketView>)], lost: SiteId, spare: SiteId, now: Timestamp) -> Fills {
    let (mut single, rest) = refill_parts(networks, lost, spare, now);
    single.extend(rest);
    single
}

/// Fills into a spare, each with its network.
pub type Fills = Vec<(NetworkId, Action)>;

/// `refill`'s two halves: the fills of packs one surviving bucket alone holds, then the rest.
pub fn refill_parts(
    networks: &[(NetworkId, Vec<BucketView>)],
    lost: SiteId,
    spare: SiteId,
    now: Timestamp,
) -> (Fills, Fills) {
    let (mut single, mut rest) = (Vec::new(), Vec::new());
    for (network, views) in networks {
        // The spare's own listing counts, so a refill that ran before copies nothing twice.
        let mut views: Vec<BucketView> = views.iter().filter(|v| v.site != lost).cloned().collect();
        if !views.iter().any(|v| v.site == spare) {
            views.push(BucketView { site: spare, packs: Vec::new(), markers: Vec::new(), uploads: Vec::new() });
        }
        let copies = |id: PackId| {
            let key = R2Key::Pack { network: *network, id }.to_string();
            views.iter().filter(|v| v.site != spare && v.packs.iter().any(|o| o.key == key)).count()
        };
        for a in decide(*network, &views, &BTreeMap::new(), now).actions {
            if let Action::Fill { to, id, .. } = &a
                && *to == spare
            {
                if copies(*id) == 1 { single.push((*network, a)) } else { rest.push((*network, a)) }
            }
        }
    }
    (single, rest)
}

/// One call of `ReplaceMember`'s refill (`workers/workflows`, through the site's `/refill` route): the networks of
/// `buckets` from `from` in id order, listed and filled into `spare` by `refill_parts` until `budget` runs out, the
/// single-copy half only when `single`. `next` is where the next call starts, `None` once every network was done. The
/// caller runs the single half over every network first, then the whole.
pub async fn refill_run<B: Bucket>(
    buckets: &[(SiteId, &B)],
    lost: SiteId,
    spare: SiteId,
    now: Timestamp,
    single: bool,
    from: Option<NetworkId>,
    budget: CronBudget,
) -> Result<RefillRun, StoreError> {
    let live: Vec<(SiteId, &B)> = buckets.iter().filter(|(s, _)| *s != lost).copied().collect();
    let all = networks(&live).await?;
    let start = from.map_or(0, |f| all.partition_point(|n| n.0.0 < f.0.0));
    let bucket = |s: SiteId| live.iter().find(|(x, _)| *x == s).map(|(_, b)| *b).expect("planned site");
    let mut out = RefillRun::default();
    let mut left = budget.actions;
    for (k, n) in all.iter().enumerate().skip(start) {
        if k - start >= budget.networks || left == 0 {
            out.next = Some(*n);
            return Ok(out);
        }
        let mut views = Vec::new();
        for (site, b) in &live {
            views.push(BucketView {
                site: *site,
                packs: b.list(&R2Key::pack_prefix(*n)).await?,
                markers: b.list(&R2Key::marker_prefix(*n)).await?,
                uploads: Vec::new(),
            });
        }
        let (mut fills, rest) = refill_parts(&[(*n, views)], lost, spare, now);
        out.single += fills.len();
        if !single {
            fills.extend(rest);
        }
        if fills.len() > left {
            // Cut mid-network: the next call lists it again and skips what this one filled.
            fills.truncate(left);
            out.next = Some(*n);
        }
        for (_, a) in &fills {
            let Action::Fill { to, from, id, marker } = a else { unreachable!("refill plans fills only") };
            match fill(bucket(*from), bucket(*to), *n, *id, marker).await {
                Ok(()) => out.done += 1,
                Err(StoreError(e)) => out.failed.push((*n, e)),
            }
        }
        left -= fills.len();
        if out.next.is_some() {
            return Ok(out);
        }
    }
    Ok(out)
}

/// What one `refill_run` call did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RefillRun {
    pub done: usize,
    /// Single-copy fills planned over the networks this call listed, whether or not it ran them.
    pub single: usize,
    pub failed: Vec<(NetworkId, String)>,
    pub next: Option<NetworkId>,
}

/// The chain a marker put or a fill writes for, from its marker.
fn write_chain(a: &Action) -> Option<ChainId> {
    match a {
        Action::PutMarker { marker, .. } | Action::Fill { marker, .. } => Some(marker.chain),
        Action::Delete { .. } | Action::Abort { .. } => None,
    }
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
    let mut plan = decide(network, &views, &lookups, now);
    // A retired or purged group's chain gets no marker and no copy: a purge that missed one bucket (graft-m1's site d,
    // 2026-11-20) had its packs copied back into the others, 1048577's at 08:15:37Z and 3473415's at 13:47Z.
    let mut gone: BTreeSet<ChainId> = BTreeSet::new();
    for chain in plan.actions.iter().filter_map(write_chain).collect::<BTreeSet<_>>() {
        if requests.gone(chain).await {
            gone.insert(chain);
        }
    }
    if !gone.is_empty() {
        plan.actions.retain(|a| write_chain(a).is_none_or(|c| !gone.contains(&c)));
        plan.alarms.extend(gone.into_iter().map(|chain| Alarm::Gone { chain }));
    }
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
/// networks and runs at most `actions` actions across them; the next run continues after `SweepAll::next`. Past
/// `wall_ms` of the run's own clock it starts no network and asks no `request` row, so it returns, and its caller
/// stores the cursor, inside the Cron Trigger's 15 minutes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CronBudget {
    pub networks: usize,
    pub actions: usize,
    pub wall_ms: u64,
}

impl CronBudget {
    /// A network costs three listings per bucket plus its lookups and actions, so 20 networks over 3 buckets (180
    /// listings) and 600 actions stay under 1,000 subrequests with room for the lookups [guess, unmeasured]. SWEEP_WALL
    /// is 8 of the Cron Trigger's 15 minutes [guess]: the network in flight at the wall still runs its actions (site a's
    /// fills of 131296 took about 270 s), and the fold audit runs after the sweep in the same invocation. A run cancelled
    /// at 15 minutes stored no cursor, so every later run died at the same network (`tests/am1/results/lm1-memory.md`,
    /// "Site a's sweep wall").
    pub const SITE_CRON: CronBudget = CronBudget { networks: 20, actions: 600, wall_ms: SWEEP_WALL };
}

/// See `CronBudget::SITE_CRON`.
pub const SWEEP_WALL: u64 = 8 * 60_000;

/// The `RequestSource` one `sweep_all` run asks through: a chain whose lookup failed is not asked again this run, and
/// past the run's wall no chain is asked, each answered as a failed lookup, which keeps the pack and alarms. A group no
/// member answers (on `graft-m2b`, a retired test group whose packs stay in the buckets) cost LOOKUP_FOR (20 s) per
/// pack, 16 at once, so its thousands of packs outlasted the cron; now the asks already in flight when the first fails,
/// at most `CONCURRENCY`, are its whole cost.
struct RunRows<'a, R, C> {
    inner: &'a R,
    clock: C,
    deadline: u64,
    dead: std::cell::RefCell<BTreeSet<ChainId>>,
}

impl<R: RequestSource, C: Fn() -> u64> RequestSource for RunRows<'_, R, C> {
    async fn request(&self, chain: ChainId, id: RequestId) -> Result<Option<RequestRow>, StoreError> {
        if self.dead.borrow().contains(&chain) {
            return Err(StoreError(format!("{chain}: a lookup failed earlier this run")));
        }
        if (self.clock)() >= self.deadline {
            return Err(StoreError("the run's wall has passed".into()));
        }
        let r = self.inner.request(chain, id).await;
        if r.is_err() {
            self.dead.borrow_mut().insert(chain);
        }
        r
    }

    /// Asked once per chain a network's plan writes for, even when its `request` lookup failed this run: a purged
    /// group's lookup fails that way, and it is the chain this check exists for. Past the run's wall: not gone.
    async fn gone(&self, chain: ChainId) -> bool {
        if (self.clock)() >= self.deadline {
            return false;
        }
        self.inner.gone(chain).await
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SweepAll {
    pub swept: Vec<(NetworkId, SweepReport)>,
    /// Networks whose sweep failed (a listing error), kept for the next run.
    pub failed: Vec<(NetworkId, String)>,
    /// The network the next run starts at, or `None` when this run reached the end of the list, so the next run
    /// starts again from the first.
    pub next: Option<NetworkId>,
    /// The chains whose lookup failed this run and were not asked again.
    pub dead: Vec<ChainId>,
    /// The run stopped at `CronBudget::wall_ms`.
    pub walled: bool,
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
/// the sweep is idempotent either way. The caller stores `next` and passes it as `from` on the next run. `clock` is the
/// run's real time in ms, for `budget.wall_ms`; `now` is the sweeper's clock for ages, which BF-19 advances.
pub async fn sweep_all<B: Bucket, R: RequestSource>(
    buckets: &[(SiteId, &B)],
    requests: &R,
    now: Timestamp,
    budget: CronBudget,
    from: Option<NetworkId>,
    mine: impl Fn(NetworkId) -> bool,
    clock: impl Fn() -> u64,
) -> Result<SweepAll, StoreError> {
    let deadline = clock().saturating_add(budget.wall_ms);
    let rows = RunRows { inner: requests, clock, deadline, dead: Default::default() };
    let mut out = sweep_networks(buckets, &rows, now, budget, from, mine).await?;
    out.dead = rows.dead.into_inner().into_iter().collect();
    Ok(out)
}

async fn sweep_networks<B: Bucket, R: RequestSource, C: Fn() -> u64>(
    buckets: &[(SiteId, &B)],
    rows: &RunRows<'_, R, C>,
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
        if (rows.clock)() >= rows.deadline {
            out.next = Some(*n);
            out.walled = true;
            return Ok(out);
        }
        match sweep(*n, buckets, rows, now, actions).await {
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
    struct Rows(BTreeMap<RequestId, RequestRow>, bool, BTreeSet<ChainId>);

    impl RequestSource for Rows {
        async fn request(&self, _chain: ChainId, id: RequestId) -> Result<Option<RequestRow>, StoreError> {
            if self.1 {
                return Err(StoreError("leader down".into()));
            }
            Ok(self.0.get(&id).cloned())
        }
        async fn gone(&self, chain: ChainId) -> bool {
            self.2.contains(&chain)
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

    /// M6-WP1: site 0 is lost and site 3 is its spare. Network 7 has a pack only in bucket 1 (its other copy was on
    /// 0) and one in 1 and 2; network 8 has one only in bucket 2 and one in 1 and 2. The single-copy packs of both
    /// networks are copied first, every fill is from a surviving bucket into the spare, each pack once, and a pack the
    /// spare already holds is not copied again.
    #[test]
    fn a_refill_copies_single_copy_packs_first_from_surviving_buckets_only() {
        const NET2: NetworkId = NetworkId(DbId(8));
        let b: [MemBucket; 4] = Default::default();
        let put = |b: &MemBucket, net: NetworkId, id: PackId, n: u8| {
            let meta = ObjectMeta { chain: CHAIN, request: rid(n) }.to_pairs().to_vec();
            b.plant(&R2Key::Pack { network: net, id }.to_string(), &body(n), Timestamp(T0), &meta);
            b.set_now(Timestamp(T0));
            block_on(put_marker(b, net, id, &marker(n, u64::from(n)))).unwrap();
        };
        let (one7, two7, one8, two8, held) = (pid(1, T0), pid(2, T0), pid(3, T0), pid(4, T0), pid(5, T0));
        for (id, n) in [(one7, 1), (two7, 2), (held, 5)] {
            put(&b[0], NET, id, n);
        }
        put(&b[1], NET, one7, 1);
        put(&b[1], NET, two7, 2);
        put(&b[2], NET, two7, 2);
        put(&b[0], NET2, one8, 3);
        put(&b[2], NET2, one8, 3);
        put(&b[1], NET2, two8, 4);
        put(&b[2], NET2, two8, 4);
        put(&b[1], NET, held, 5);
        put(&b[2], NET, held, 5);
        put(&b[3], NET, held, 5);
        let view = |net: NetworkId| -> Vec<BucketView> {
            b.iter()
                .enumerate()
                .map(|(i, b)| BucketView {
                    site: SiteId(i as u8),
                    packs: block_on(b.list(&R2Key::pack_prefix(net))).unwrap(),
                    markers: block_on(b.list(&R2Key::marker_prefix(net))).unwrap(),
                    uploads: Vec::new(),
                })
                .collect()
        };
        let plan = refill(&[(NET, view(NET)), (NET2, view(NET2))], SiteId(0), SiteId(3), Timestamp(T0 + H));
        let got: Vec<(i64, PackId, u8, u8)> = plan
            .iter()
            .map(|(n, a)| match a {
                Action::Fill { to, from, id, .. } => (n.0.0, *id, from.0, to.0),
                a => panic!("{a:?}"),
            })
            .collect();
        assert_eq!(got.len(), 4, "{got:?}");
        let firsts: BTreeSet<PackId> = got[..2].iter().map(|g| g.1).collect();
        assert_eq!(firsts, BTreeSet::from([one7, one8]), "single-copy packs first, across networks: {got:?}");
        assert_eq!((got[0].2, got[1].2), (if got[0].1 == one7 { (1, 2) } else { (2, 1) }));
        assert!(got.iter().all(|g| g.2 != 0 && g.3 == 3), "never from the lost bucket, only into the spare");
        assert!(!got.iter().any(|g| g.1 == held), "the spare already holds it");
    }

    /// `refill_run`, as the Workflow drives it: a single-copy pass over both networks with one fill a call copies the
    /// lone copies only, each call ending where the next starts; the full pass then copies the rest; a pass after
    /// that copies nothing.
    #[test]
    fn refill_runs_copy_single_copy_packs_first_and_resume_from_their_cursor() {
        const NET2: NetworkId = NetworkId(DbId(8));
        let b: [MemBucket; 4] = Default::default();
        let put = |b: &MemBucket, net: NetworkId, id: PackId, n: u8| {
            let meta = ObjectMeta { chain: CHAIN, request: rid(n) }.to_pairs().to_vec();
            b.plant(&R2Key::Pack { network: net, id }.to_string(), &body(n), Timestamp(T0), &meta);
            b.set_now(Timestamp(T0));
            block_on(put_marker(b, net, id, &marker(n, u64::from(n)))).unwrap();
        };
        let (one7, two7, one8) = (pid(1, T0), pid(2, T0), pid(3, T0));
        put(&b[1], NET, one7, 1);
        put(&b[1], NET, two7, 2);
        put(&b[2], NET, two7, 2);
        put(&b[2], NET2, one8, 3);
        put(&b[0], NET2, one8, 3);
        let buckets: Vec<(SiteId, &MemBucket)> = b.iter().enumerate().map(|(i, b)| (SiteId(i as u8), b)).collect();
        let one = CronBudget { networks: 20, actions: 1, wall_ms: 0 };
        let run = |single, from, budget| {
            block_on(refill_run(&buckets, SiteId(0), SiteId(3), Timestamp(T0 + H), single, from, budget)).unwrap()
        };
        let r = run(true, None, one);
        assert_eq!((r.done, r.next), (1, Some(NET2)), "{r:?}");
        let r = run(true, r.next, one);
        assert_eq!((r.done, r.next), (1, None), "{r:?}");
        let has = |id: PackId, n: NetworkId| b[3].keys().contains(&R2Key::Pack { network: n, id }.to_string());
        assert!(has(one7, NET) && has(one8, NET2) && !has(two7, NET), "only the lone copies so far");
        let r = run(false, None, CronBudget::SITE_CRON);
        assert_eq!((r.done, r.next, r.failed.len()), (1, None, 0), "{r:?}");
        assert!(has(two7, NET));
        assert_eq!(run(false, None, CronBudget::SITE_CRON).done, 0, "nothing left to copy");
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

    /// `retire-sticks`: a purge that missed one bucket (graft-m1's site d) left a committed pack and its marker there,
    /// and site a's sweep copied them back into a, b and c: 1048577 at 08:15:37Z and 3473415 at 13:47Z on 2026-11-20,
    /// both retired and purged. A chain whose group is gone gets no fill and no marker, and the sweep alarms instead;
    /// another network's chain in the same run is filled as before.
    #[test]
    fn a_gone_chain_s_packs_are_not_filled_or_marked() {
        let bed = Bed::new();
        let (id, unmarked) = (pid(2, T0), pid(3, T0));
        put_pack(&bed.b[1], id, 2, rid(2), T0);
        put_mark(&bed.b[1], id, &marker(2, 504));
        // A copy with no marker in another bucket, which a marker put would mark.
        put_pack(&bed.b[2], id, 2, rid(2), T0);
        put_pack(&bed.b[0], unmarked, 3, rid(3), T0);
        let mut rows = Rows::default();
        rows.0.insert(rid(3), RequestRow { index: Index(9), packs: vec![(unmarked, marker(3, 9))] });
        rows.2.insert(CHAIN);
        let before: Vec<Vec<String>> = bed.b.iter().map(MemBucket::keys).collect();
        let r = bed.run(&rows, T0 + 100 * H);
        assert_eq!((r.done, r.failed.len()), (0, 0), "{:?}", r.plan.actions);
        assert!(r.plan.actions.is_empty(), "{:?}", r.plan.actions);
        assert!(r.plan.alarms.contains(&Alarm::Gone { chain: CHAIN }), "{:?}", r.plan.alarms);
        let after: Vec<Vec<String>> = bed.b.iter().map(MemBucket::keys).collect();
        assert_eq!(before, after, "a gone chain's buckets changed");
        // The same buckets with the group alive: filled and marked, as before.
        let r = bed.run(&Rows(rows.0.clone(), false, BTreeSet::new()), T0 + 100 * H);
        assert!(r.failed.is_empty() && r.done > 0, "{r:?}");
        assert!((0..3).all(|s| bed.has(s, &pack_key(id)) && bed.has(s, &mark_key(id))));
        assert!(bed.has(0, &mark_key(unmarked)));
    }

    /// Through a cron run: the gone chain's `request` lookup fails first (its unmarked pack's), which puts it in the
    /// run's `dead` set, and its marked pack still gets no fill.
    #[test]
    fn a_gone_chain_whose_lookup_failed_this_run_is_still_not_filled() {
        struct Gone;
        impl RequestSource for Gone {
            async fn request(&self, chain: ChainId, _id: RequestId) -> Result<Option<RequestRow>, StoreError> {
                Err(StoreError(format!("{chain}: no group")))
            }
            async fn gone(&self, _chain: ChainId) -> bool {
                true
            }
        }
        let bed = Bed::new();
        let (id, unmarked) = (pid(2, T0), pid(3, T0));
        put_pack(&bed.b[1], id, 2, rid(2), T0);
        put_mark(&bed.b[1], id, &marker(2, 504));
        put_pack(&bed.b[0], unmarked, 3, rid(3), T0);
        let before: Vec<Vec<String>> = bed.b.iter().map(MemBucket::keys).collect();
        let all = block_on(sweep_all(
            &bed.buckets(),
            &Gone,
            Timestamp(T0 + 100 * H),
            CronBudget::SITE_CRON,
            None,
            |_| true,
            || 0,
        ))
        .unwrap();
        assert_eq!(all.dead, [CHAIN]);
        assert_eq!(before, bed.b.iter().map(MemBucket::keys).collect::<Vec<_>>(), "{:?}", all.swept);
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
        let down = Rows(BTreeMap::new(), true, BTreeSet::new());
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
        block_on(sweep_all(&bed.buckets(), rows, Timestamp(T0), budget, from, |_| true, || T0)).unwrap()
    }

    /// A `request` table whose chains in `down` never answer, each ask costing `cost` ms of a shared clock, as a
    /// lookup of a group no member answers costs LOOKUP_FOR.
    struct Slow {
        down: BTreeSet<ChainId>,
        cost: u64,
        clock: std::cell::Cell<u64>,
        asked: std::cell::RefCell<Vec<ChainId>>,
    }

    impl RequestSource for Slow {
        async fn request(&self, chain: ChainId, _id: RequestId) -> Result<Option<RequestRow>, StoreError> {
            self.clock.set(self.clock.get() + self.cost);
            self.asked.borrow_mut().push(chain);
            if self.down.contains(&chain) { Err(StoreError("no member answered".into())) } else { Ok(None) }
        }
    }

    fn slow_all(bed: &Bed, rows: &Slow, budget: CronBudget, from: Option<NetworkId>) -> SweepAll {
        block_on(sweep_all(&bed.buckets(), rows, Timestamp(T0), budget, from, |_| true, || rows.clock.get())).unwrap()
    }

    /// Site a's wedged cron: network 8's group no member answers, and each of its 30 orphans asked its lookup for
    /// LOOKUP_FOR, so the run passed the Cron Trigger's 15 minutes, was cancelled before it stored its cursor, and
    /// the next run started at network 8 again. A failed chain is asked once a run, its packs kept and alarmed, and the
    /// run goes on to network 9, whose orphan is deleted.
    #[test]
    fn a_chain_whose_lookup_failed_is_asked_once_a_run() {
        let bed = Bed::new();
        for k in 1..=30 {
            orphan(&bed, 8, k);
        }
        orphan(&bed, 9, 31);
        let dead = ChainId::Repo(RepoId(DbId(8)));
        let rows = Slow { down: [dead].into(), cost: 20_000, clock: T0.into(), asked: Default::default() };
        let r = slow_all(&bed, &rows, CronBudget::SITE_CRON, None);
        assert_eq!(rows.asked.borrow().iter().filter(|c| **c == dead).count(), 1);
        assert_eq!(r.dead, vec![dead]);
        assert_eq!(r.swept.iter().map(|(n, s)| (n.0.0, s.done)).collect::<Vec<_>>(), [(8, 0), (9, 3)]);
        assert_eq!(r.swept[0].1.plan.alarms.len(), 30, "every pack of the dead chain is kept and alarmed");
        assert!(bed.b.iter().all(|b| b.keys().iter().filter(|k| k.starts_with("pack/8/")).count() == 30));
        assert!(!r.walled && r.next.is_none());
    }

    /// Past `wall_ms` a run starts no network and asks no row, so it returns with the next network as its cursor; one
    /// that started inside the wall runs what it was answered. Each lookup here costs 3 minutes: network 1 is asked at
    /// 0, network 2 at 3, network 3's first orphan at 6 and its second not at all (9, past the 8-minute wall), so that
    /// orphan is kept and network 4 waits for the next run.
    #[test]
    fn a_run_stops_starting_networks_at_its_wall() {
        let bed = Bed::new();
        for (n, k) in [(1, 1), (2, 2), (3, 3), (3, 4), (4, 5)] {
            orphan(&bed, n, k);
        }
        let rows = Slow { down: BTreeSet::new(), cost: 3 * 60_000, clock: T0.into(), asked: Default::default() };
        let r = slow_all(&bed, &rows, CronBudget::SITE_CRON, None);
        assert_eq!(r.swept.iter().map(|(n, s)| (n.0.0, s.done)).collect::<Vec<_>>(), [(1, 3), (2, 3), (3, 3)]);
        assert!(r.walled);
        assert_eq!(r.next, Some(net(4)));
        assert_eq!(rows.asked.borrow().len(), 3);
        // The next run continues from the stored cursor.
        let rows = Slow { down: BTreeSet::new(), cost: 0, clock: T0.into(), asked: Default::default() };
        let r2 = slow_all(&bed, &rows, CronBudget::SITE_CRON, r.next);
        assert_eq!(r2.swept.iter().map(|(n, s)| (n.0.0, s.done)).collect::<Vec<_>>(), [(4, 3)]);
        // At the wall exactly, a network's lookups are not asked: its orphan is kept for the next run.
        let bed = Bed::new();
        orphan(&bed, 1, 1);
        orphan(&bed, 1, 2);
        let rows = Slow { down: BTreeSet::new(), cost: SWEEP_WALL, clock: T0.into(), asked: Default::default() };
        let r = slow_all(&bed, &rows, CronBudget::SITE_CRON, None);
        assert_eq!(rows.asked.borrow().len(), 1, "the second lookup is past the wall");
        assert_eq!(r.swept[0].1.done, 3, "the answered orphan is deleted");
        assert!(bed.b.iter().all(|b| b.keys().iter().filter(|k| k.starts_with("pack/1/")).count() == 1));
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
        let b = CronBudget { networks: 2, actions: 100, wall_ms: SWEEP_WALL };
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
        let r1 = all(&bed, &rows, CronBudget { networks: 10, actions: 10, wall_ms: SWEEP_WALL }, None);
        assert_eq!(r1.swept.len(), 1);
        assert_eq!(r1.swept[0].1.done, 10);
        assert_eq!(r1.next, Some(net(8)), "network 8 again next run");
        let r2 = all(&bed, &rows, CronBudget { networks: 10, actions: 10, wall_ms: SWEEP_WALL }, r1.next);
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
        let r = block_on(sweep_all(&bed.buckets(), &rows, Timestamp(T0), CronBudget::SITE_CRON, None, even, || T0))
            .unwrap();
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
