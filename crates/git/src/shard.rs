//! Git cache shards (`design/DESIGN.md`, Shape, "Git cache shards (derived)"): which shards serve a network, the start
//! cap, the start-time region check, the inactivity alarm, the image map, `IngestCursor`, and the LRU of bare
//! repositories on the shard's disk. Pure: the shard object in `host` feeds them its clock and the container's answers.

use std::time::Duration;

use arrayvec::ArrayVec;
use graftyard_types::{ChainId, Index, NetworkId, Region, SiteId, Timestamp};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use smallvec::SmallVec;

/// Shards per site at 50, 300, and 2,000 developers, set by B0-14 (`tests/m0/results/m0-wp6.md`: "the shard counts
/// stay 2, 5, and 25").
pub const SHARDS: [u16; 3] = [2, 5, 25];
/// Per site, counted in the start cap of `count + HOT_POOL`.
pub const HOT_POOL: u16 = 7;
/// At most this many shards serve one network on one site.
pub const FANOUT_MAX: u8 = 8;
/// The API's maximum inactivity timeout, which a warm shard holds.
pub const INACTIVITY: Duration = Duration::from_secs(6 * 3600);
/// The alarm renews the timeout this often.
pub const RENEW_EVERY: Duration = Duration::from_secs(3600);
/// A shard stays warm until this long after its last request.
pub const WARM_FOR: Duration = Duration::from_secs(24 * 3600);
/// Replacements of a container started outside its site's regions before the shard alarms.
pub const REGION_RETRIES: u8 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct GitCacheShard {
    pub site: SiteId,
    pub shard: u16,
}

impl GitCacheShard {
    /// The shard object's name on its site's shard class.
    pub fn do_name(&self) -> String {
        format!("git#{}#{}", self.site.0, self.shard)
    }
}

/// Rendezvous weight of (key, slot): the first 8 bytes of SHA-256 over both.
pub fn weight(key: &[u8], slot: u16) -> u64 {
    let mut h = Sha256::new();
    h.update(key);
    h.update(b"#");
    h.update(slot.to_be_bytes());
    u64::from_be_bytes(h.finalize()[..8].try_into().expect("8 bytes"))
}

fn by_weight(key: &[u8], slots: impl Iterator<Item = u16>) -> Vec<u16> {
    let mut v: Vec<(u64, u16)> = slots.map(|s| (weight(key, s), s)).collect();
    v.sort_by(|a, b| b.cmp(a));
    v.into_iter().map(|(_, s)| s).collect()
}

fn network_key(network: NetworkId) -> Vec<u8> {
    format!("network:{}", network.0.0).into_bytes()
}

/// The shards that serve `network` on `site`. The first is its rendezvous shard among `0..count`; the other `k - 1`
/// come from the site's hot pool, `count..count + HOT_POOL`, by rendezvous, so `count` never changes and no other
/// network moves when k does. `k` is clamped to `1..=min(FANOUT_MAX, 1 + HOT_POOL)`.
pub fn shards_for(network: NetworkId, site: SiteId, count: u16, k: u8) -> ArrayVec<GitCacheShard, 8> {
    assert!(count > 0, "a site has at least one shard");
    let key = network_key(network);
    let k = k.clamp(1, FANOUT_MAX.min(1 + HOT_POOL as u8)) as usize;
    let mut out = ArrayVec::new();
    out.push(GitCacheShard { site, shard: by_weight(&key, 0..count)[0] });
    for s in by_weight(&key, count..count + HOT_POOL).into_iter().take(k - 1) {
        out.push(GitCacheShard { site, shard: s });
    }
    out
}

// ---- Fan-out (`design/DESIGN.md`, Shape, "Agent load and agent safety", `NetworkLoad` and `fanout`) ----

/// What each warm site's first shard of a network reports each minute: the CPU of the most loaded of the site's k
/// shards (0.0 to 1.0 of its vCPU), and their queue depth.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct NetworkLoad {
    pub cpu_p99: f32,
    pub queue_depth: u32,
}

/// k steps up once the larger load stays above this for `FANOUT_UP_FOR` [guess].
pub const FANOUT_UP: f32 = 0.75;
pub const FANOUT_UP_FOR: Duration = Duration::from_secs(180);
/// k steps down once the larger load stays below this for `FANOUT_DOWN_FOR` [guess].
pub const FANOUT_DOWN: f32 = 0.4;
pub const FANOUT_DOWN_FOR: Duration = Duration::from_secs(3600);

/// The hysteresis `fanout` reads: since when the larger load has been above `FANOUT_UP`, or below `FANOUT_DOWN`. The
/// network group's leader keeps it beside k; it restarts at each step.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fanout {
    pub k: u8,
    pub high_since: Option<Timestamp>,
    pub low_since: Option<Timestamp>,
}

/// The k both warm sites hold, from the larger of their two loads, so the second warm site holds k shards when the
/// first is lost (`design/DESIGN.md`, Shape, `fanout`). Up one step past `FANOUT_UP` for `FANOUT_UP_FOR`, or to
/// `ceil(k x cpu / FANOUT_UP)` when that is more; down one step past `FANOUT_DOWN` for `FANOUT_DOWN_FOR`; at most
/// FANOUT_MAX and at least 1. Pure: no API call, no clock but `now`.
pub fn fanout(loads: [&NetworkLoad; 2], k: u8, now: Timestamp, f: &mut Fanout) -> u8 {
    let k = k.clamp(1, FANOUT_MAX);
    let cpu = loads[0].cpu_p99.max(loads[1].cpu_p99);
    f.high_since = if cpu > FANOUT_UP { Some(f.high_since.unwrap_or(now)) } else { None };
    f.low_since = if cpu < FANOUT_DOWN { Some(f.low_since.unwrap_or(now)) } else { None };
    let held = |since: Option<Timestamp>, d: Duration| since.is_some_and(|t| now.0 >= t.0 + d.as_millis() as u64);
    let next = if held(f.high_since, FANOUT_UP_FOR) {
        let want = (f32::from(k) * cpu / FANOUT_UP).ceil() as u8;
        want.max(k + 1).min(FANOUT_MAX)
    } else if held(f.low_since, FANOUT_DOWN_FOR) {
        (k - 1).max(1)
    } else {
        k
    };
    if next != k {
        f.high_since = None;
        f.low_since = None;
    }
    f.k = next;
    next
}

/// A load report older than this counts as idle: a warm site that stopped reporting holds no k up [guess, three of
/// its one-minute reports].
pub const LOAD_STALE: Duration = Duration::from_secs(180);

/// What a network group's leader keeps between `NetworkLoad` reports: each warm site's latest report and when it
/// came, and the hysteresis `fanout` reads. In memory only: a new leader starts from the committed k with no
/// hysteresis, so at worst a step waits one more window.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LoadReports {
    pub sites: ArrayVec<(SiteId, Timestamp, NetworkLoad), 2>,
    pub f: Fanout,
}

impl LoadReports {
    /// Takes `site`'s report at `now` and answers the k to commit when it differs from `k`, the committed one. Only
    /// the network's two warm sites count; a third site's report is ignored, and a stale one counts as idle.
    pub fn report(&mut self, warm: [SiteId; 2], site: SiteId, load: NetworkLoad, k: u8, now: Timestamp) -> Option<u8> {
        if !warm.contains(&site) {
            return None;
        }
        self.sites.retain(|(s, ..)| warm.contains(s) && *s != site);
        self.sites.push((site, now, load));
        let idle = NetworkLoad::default();
        let fresh = |s: SiteId| {
            self.sites
                .iter()
                .find(|(x, at, _)| *x == s && now.0 < at.0 + LOAD_STALE.as_millis() as u64)
                .map_or(idle, |(_, _, l)| *l)
        };
        let loads = [fresh(warm[0]), fresh(warm[1])];
        let next = fanout([&loads[0], &loads[1]], k, now, &mut self.f);
        (next != k.clamp(1, FANOUT_MAX)).then_some(next)
    }
}

/// The NetworkLoad of a site's k shards of one network: the most loaded one's CPU and their summed queue depth.
pub fn combine(loads: &[NetworkLoad]) -> NetworkLoad {
    NetworkLoad {
        cpu_p99: loads.iter().map(|l| l.cpu_p99).fold(0.0, f32::max),
        queue_depth: loads.iter().map(|l| l.queue_depth).sum(),
    }
}

/// The pool shards each network of `asks` holds on one site: `(network, owner, held, want)`, where `held` and `want`
/// are pool shards (k - 1). Each owner is held to `share(owner)`, its plan's share of the pool, and all to HOT_POOL.
/// Shards a network already holds are kept first, up to its owner's share, so another owner's step up never reduces
/// it; then the rest is granted in `asks` order. Returns the pool shards each network holds.
pub fn pool_grants(asks: &[(NetworkId, u64, u8, u8)], share: impl Fn(u64) -> u16) -> Vec<u8> {
    let mut left = HOT_POOL;
    let mut by_owner: std::collections::BTreeMap<u64, u16> = Default::default();
    let mut out: Vec<u8> = asks.iter().map(|a| a.2.min(a.3)).collect();
    for (i, a) in asks.iter().enumerate() {
        let used = by_owner.entry(a.1).or_default();
        let keep = u16::from(out[i]).min(share(a.1).saturating_sub(*used)).min(left);
        out[i] = keep as u8;
        *used += keep;
        left -= keep;
    }
    for (i, a) in asks.iter().enumerate() {
        let used = by_owner.entry(a.1).or_default();
        let more = u16::from(a.3.saturating_sub(out[i])).min(share(a.1).saturating_sub(*used)).min(left);
        out[i] += more as u8;
        *used += more;
        left -= more;
    }
    out
}

/// A hot network's clone-cache entry goes to both warm sites' buckets: the bucket bindings, `BUCKET_<site>`, in the
/// network's `warm_sites` order.
pub fn clone_cache_buckets(network: NetworkId, home: SiteId, sites: &[SiteId], hot: bool) -> Vec<SiteId> {
    let [a, b] = warm_sites(network, home, sites);
    if hot && a != b { vec![a, b] } else { vec![home] }
}

/// The warm sites of a network: its group's home, then the next site by rendezvous among the others.
pub fn warm_sites(network: NetworkId, home: SiteId, sites: &[SiteId]) -> [SiteId; 2] {
    let key = network_key(network);
    let next =
        sites.iter().filter(|s| **s != home).max_by_key(|s| weight(&key, u16::from(s.0))).copied().unwrap_or(home);
    [home, next]
}

/// The shard of the k that validates a push: rendezvous on the push's id among the caught-up ones.
pub fn validator(push: &[u8], caught_up: &[GitCacheShard]) -> Option<GitCacheShard> {
    caught_up.iter().max_by_key(|s| weight(push, s.shard)).copied()
}

/// The shard of `network`'s k on `site` that validates push `push`: rendezvous on the push id among
/// `shards_for(network, site, count, k)`, so one push always lands on the same shard and pushes spread evenly. The
/// caller falls back to the first shard when the chosen one is not caught up.
pub fn push_validator(network: NetworkId, site: SiteId, count: u16, k: u8, push: &[u8]) -> GitCacheShard {
    let ks = shards_for(network, site, count, k);
    validator(push, &ks).unwrap_or(ks[0])
}

/// The shard object a shard of `network` answers as. A network's first shard keeps the per-network name every site
/// script and edge has used since M1 (`git#<site>#net:<repo>`), so no network moves when fan-out ships; a hot-pool
/// shard is shared by the networks rendezvous gives it, `git#<site>#pool:<n>`, and keeps each one's repository and
/// `IngestCursor` side by side.
pub fn object_name(network: NetworkId, s: GitCacheShard, count: u16) -> String {
    if s.shard < count {
        format!("git#{}#net:{}", s.site.0, network.0.0)
    } else {
        format!("git#{}#pool:{}", s.site.0, s.shard)
    }
}

/// The policy has no `max_instances`, so a shard object starts its container only while its index is under the cap.
pub fn may_start(shard: u16, count: u16) -> bool {
    shard < count + HOT_POOL
}

/// The container regions of a site, as `CLOUDFLARE_REGION` names them. An `Oc` site lists OC and APAC.
pub fn container_regions(region: Region) -> &'static [&'static str] {
    match region {
        Region::Enam => &["ENAM"],
        Region::Wnam => &["WNAM"],
        Region::Weur => &["WEUR"],
        Region::Eeur => &["EEUR"],
        Region::Apac => &["APAC"],
        Region::Oc => &["OC", "APAC"],
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegionCheck {
    Keep,
    /// Destroy the container and start another.
    Replace,
    /// Out of region after `REGION_RETRIES` replacements: keep serving and alarm.
    Alarm,
}

/// At start the object reads `CLOUDFLARE_REGION` inside the container. `replaced` counts this start's replacements so
/// far. A missing variable is out of region.
pub fn region_check(site: Region, reported: Option<&str>, replaced: u8) -> RegionCheck {
    if reported.is_some_and(|r| container_regions(site).contains(&r)) {
        RegionCheck::Keep
    } else if replaced < REGION_RETRIES {
        RegionCheck::Replace
    } else {
        RegionCheck::Alarm
    }
}

/// What the shard's alarm does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Renewal {
    /// Renew the 6 h inactivity timeout and fire again at `next`.
    Renew { next: Timestamp },
    /// Past `WARM_FOR` since the last request: set no alarm, so the timeout lapses and the container stops.
    Lapse,
}

/// The shard's alarm at `now`, given its last request. Every request also calls this, so the first request after a
/// lapse arms the alarm again.
pub fn renewal(now: Timestamp, last_request: Timestamp) -> Renewal {
    let warm_until = last_request.0 + WARM_FOR.as_millis() as u64;
    if now.0 >= warm_until {
        Renewal::Lapse
    } else {
        Renewal::Renew { next: Timestamp((now.0 + RENEW_EVERY.as_millis() as u64).min(warm_until)) }
    }
}

/// Each site-script version's image map: its release's git image as `git` and the previous release's as `git_prev`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageSlot {
    Git,
    GitPrev,
}

impl ImageSlot {
    pub fn name(self) -> &'static str {
        match self {
            ImageSlot::Git => "git",
            ImageSlot::GitPrev => "git_prev",
        }
    }
}

/// Which image a shard starts. `day_step` is how far the site's `ContainerDay` has rolled (0 to 2, covering 10%,
/// 50%, and 100% of the site's shards in rendezvous order); `None` before the day. A committed `Rollback` returns
/// every shard to `git_prev`.
pub fn image_for(shard: GitCacheShard, count: u16, day_step: Option<u8>, rolled_back: bool) -> ImageSlot {
    if rolled_back {
        return ImageSlot::GitPrev;
    }
    let Some(step) = day_step else { return ImageSlot::GitPrev };
    let total = count + HOT_POOL;
    let pct = [10u32, 50, 100][usize::from(step.min(2))];
    let covered = (u32::from(total) * pct).div_ceil(100) as usize;
    let order = by_weight(format!("day#{}", shard.site.0).as_bytes(), 0..total);
    if order.iter().take(covered).any(|s| *s == shard.shard) { ImageSlot::Git } else { ImageSlot::GitPrev }
}

/// Per (shard, network), in the shard's own SQLite: the index through which the shard has read each member chain's
/// `Refs`, `Merged`, and `Promoted` entries and the network chain's `Compacted` entries.
///
/// The cursor describes the container's disk, which a container restart empties, so the shard keeps beside it the id
/// of the disk it describes (`disk`, a random token the container writes at first start) and treats a cursor for
/// another disk as empty. The design keeps no such id.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IngestCursor {
    pub network: NetworkId,
    pub chains: SmallVec<[(ChainId, Index); 4]>,
}

impl IngestCursor {
    pub fn new(network: NetworkId) -> IngestCursor {
        IngestCursor { network, chains: SmallVec::new() }
    }

    pub fn at(&self, chain: ChainId) -> Index {
        self.chains.iter().find(|(c, _)| *c == chain).map(|(_, i)| *i).unwrap_or(Index(0))
    }

    /// The entries the shard must read before it serves refs at `applied`: `None` when caught up.
    pub fn behind(&self, chain: ChainId, applied: Index) -> Option<std::ops::RangeInclusive<Index>> {
        let at = self.at(chain);
        (applied > at).then(|| Index(at.0 + 1)..=applied)
    }

    /// Advances only forward, after the packs of entries up to `to` are on the shard's disk.
    pub fn advance(&mut self, chain: ChainId, to: Index) {
        match self.chains.iter_mut().find(|(c, _)| *c == chain) {
            Some((_, i)) => *i = (*i).max(to),
            None => self.chains.push((chain, to)),
        }
    }
}

// ---- The LRU of bare repositories ----

/// Of the container's disk, the share the repositories may take; the rest is for spools, quarantines, and
/// index-pack's temporary files while one more repository is rehydrated [guess].
pub const DISK_USE_PCT: u64 = 80;

/// One repository on the shard's disk, as the container's `/repos` reports it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiskRepo {
    pub network: NetworkId,
    pub bytes: u64,
    /// The last request that read or wrote it, by the shard's clock.
    pub used: Timestamp,
}

/// The repositories to evict, least recently used first, so the rest fit under `DISK_USE_PCT` of `disk_bytes` with
/// room for `incoming` more bytes. `keep` (the network the current request is for) is never evicted, so a request
/// whose own repository is larger than the budget still runs and leaves only that one. Ties on `used` break by
/// network id, so two calls over the same listing agree.
pub fn evict_plan(repos: &[DiskRepo], disk_bytes: u64, incoming: u64, keep: NetworkId) -> Vec<NetworkId> {
    let budget = disk_bytes / 100 * DISK_USE_PCT;
    let mut total: u64 = repos.iter().map(|r| r.bytes).sum::<u64>().saturating_add(incoming);
    let mut order: Vec<&DiskRepo> = repos.iter().filter(|r| r.network != keep).collect();
    order.sort_by_key(|r| (r.used, r.network.0.0));
    let mut out = Vec::new();
    for r in order {
        if total <= budget {
            break;
        }
        total -= r.bytes;
        out.push(r.network);
    }
    out
}
