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

/// The shards that serve `network` on `site`. The first is its rendezvous shard among `0..count`, so `count` never
/// changes and no other network moves when k does. The others are the pool shards the site's `PoolLedger` granted the
/// network: `pool` holds pool slots, `0..HOT_POOL`, and slot `p` is shard `count + p`. A granted slot belongs to one
/// network, so two hot networks never share a pool shard. At most `FANOUT_MAX - 1` slots are used, and a slot outside
/// the pool or named twice is skipped.
pub fn shards_for(network: NetworkId, site: SiteId, count: u16, pool: &[u16]) -> ArrayVec<GitCacheShard, 8> {
    assert!(count > 0, "a site has at least one shard");
    let key = network_key(network);
    let mut out = ArrayVec::new();
    out.push(GitCacheShard { site, shard: by_weight(&key, 0..count)[0] });
    for p in pool {
        let s = GitCacheShard { site, shard: count + p };
        if *p < HOT_POOL && out.len() < usize::from(FANOUT_MAX) && !out.contains(&s) {
            out.push(s);
        }
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

/// An owner's share of a site's pool: with no plan field, `HOT_POOL - 1`, so no owner holds the whole pool [guess,
/// B0-34's monorepo share].
pub fn owner_share(_owner: u64) -> u16 {
    HOT_POOL - 1
}

/// A pool ledger that did not run for longer than this was down or out of reach. While it holds a lease, its object's
/// alarm calls `PoolLedger::beat` every `LEDGER_BEAT`, so a gap past this is never just a quiet stretch with no
/// callers [guess, two missed beats].
pub const LEDGER_GAP: Duration = Duration::from_secs(90);
/// How often a ledger holding a lease beats: half of `LEDGER_GAP`.
pub const LEDGER_BEAT: Duration = Duration::from_secs(45);

/// One network's lease on a site's pool: the pool slots (`0..HOT_POOL`) it holds, its owner, and when its leader last
/// asked, by the ledger's clock.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PoolLease {
    pub network: NetworkId,
    pub owner: u64,
    pub slots: Vec<u16>,
    pub at: Timestamp,
}

/// A site's pool ledger, kept in the SQLite of the site's first pool shard (shard `count`): every live lease and when
/// the ledger last answered an ask. Pure: the shard object feeds it its clock and stores it after each ask.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PoolLedger {
    pub leases: Vec<PoolLease>,
    pub seen: Option<Timestamp>,
}

impl PoolLedger {
    /// Drops leases not renewed within LOAD_STALE of the ledger's own running time. The ledger runs at every call and
    /// at every `beat`, so a gap of more than LEDGER_GAP since it last ran means its object was down or out of reach,
    /// and that gap counts against no lease: an outage of the ledger never frees every network's slots at once. A
    /// network that cooled during the outage frees them within LOAD_STALE of the ledger's return. A lease nobody
    /// renews while the ledger runs lapses LOAD_STALE after its last renewal, however few calls come.
    fn settle(&mut self, now: Timestamp) {
        let gap = self.seen.map_or(0, |s| now.0.saturating_sub(s.0));
        if gap > LEDGER_GAP.as_millis() as u64 {
            for l in &mut self.leases {
                l.at = Timestamp((l.at.0 + gap).min(now.0));
            }
        }
        self.leases.retain(|l| now.0 < l.at.0 + LOAD_STALE.as_millis() as u64);
    }

    /// `network`'s ask for `want` pool shards (its wanted k - 1) at `now`, which also renews its lease. Every other
    /// live lease is kept whole, so another owner's ask never reduces it; `network` then keeps what it holds and grows
    /// toward `want` within its owner's `share` and the free slots (`pool_grants`, with this network last). New slots
    /// are the free ones in rendezvous order of the network. A network the ledger holds no lease for asks from zero,
    /// whatever its leader holds, so one whose lease expired never takes slots back from others. `want` 0 releases
    /// the lease. Returns the slots `network` now holds.
    pub fn ask(
        &mut self,
        network: NetworkId,
        owner: u64,
        want: u8,
        now: Timestamp,
        share: impl Fn(u64) -> u16,
    ) -> Vec<u16> {
        self.settle(now);
        self.seen = Some(now);
        let mine = self.leases.iter().position(|l| l.network == network).map(|i| self.leases.remove(i));
        let held = mine.as_ref().map_or(Vec::new(), |l| l.slots.clone());
        let mut asks: Vec<(NetworkId, u64, u8, u8)> =
            self.leases.iter().map(|l| (l.network, l.owner, l.slots.len() as u8, l.slots.len() as u8)).collect();
        asks.push((network, owner, held.len() as u8, want.min(FANOUT_MAX - 1)));
        let g = usize::from(*pool_grants(&asks, share).last().expect("one ask"));
        let mut slots: Vec<u16> = held.into_iter().take(g).collect();
        let taken: Vec<u16> =
            self.leases.iter().flat_map(|l| l.slots.iter().copied()).chain(slots.iter().copied()).collect();
        let free = by_weight(&network_key(network), 0..HOT_POOL).into_iter().filter(|s| !taken.contains(s));
        slots.extend(free.take(g - slots.len()));
        if !slots.is_empty() {
            self.leases.push(PoolLease { network, owner, slots: slots.clone(), at: now });
        }
        slots
    }

    /// The ledger object's heartbeat, from its alarm every `LEDGER_BEAT` while any lease is held (`beat_due`): the
    /// ledger runs with no call, drops what lapsed, and marks the time, so a quiet stretch is not read as an outage.
    /// Without it a ledger called less often than LEDGER_GAP shifted every lease at each call, and none ever lapsed:
    /// two networks torn down at k 4 held their owner's whole share for 95 minutes (LM-1's m21,
    /// `lm1-memory` round 10).
    pub fn beat(&mut self, now: Timestamp) {
        self.settle(now);
        self.seen = Some(now);
    }

    /// When the ledger's object must next `beat`: `LEDGER_BEAT` after `now` while it holds a lease, never otherwise.
    pub fn beat_due(&self, now: Timestamp) -> Option<Timestamp> {
        (!self.leases.is_empty()).then(|| Timestamp(now.0 + LEDGER_BEAT.as_millis() as u64))
    }

    /// The network's first shard on this site, once a minute while it serves pushes: renews `network`'s lease as it
    /// is, never growing it, and answers the slots it holds. So a network in use keeps its slots while its leader
    /// cannot reach this ledger, and one nobody pushes to stops renewing and expires.
    pub fn renew(&mut self, network: NetworkId, now: Timestamp) -> Vec<u16> {
        self.settle(now);
        self.seen = Some(now);
        match self.leases.iter_mut().find(|l| l.network == network) {
            Some(l) => {
                l.at = now;
                l.slots.clone()
            }
            None => Vec::new(),
        }
    }
}

/// The pool shards a network's leader asks both warm sites' ledgers for after a load report: the k `fanout` wants
/// (`step`, else the committed `k`) less the first shard. `None` when k is 1 and stays 1, so a cool network never
/// calls a ledger. An ask at the committed k renews the network's leases, and one below it releases slots.
pub fn pool_ask(k: u8, step: Option<u8>) -> Option<u8> {
    let want = step.unwrap_or(k).clamp(1, FANOUT_MAX);
    (want > 1 || k > 1).then_some(want - 1)
}

/// The k a network's leader commits after asking both warm sites' ledgers for `want` - 1 pool shards. A step up goes
/// to 1 plus the fewer pool shards either granted, so it never passes what both granted, and never below `k`. A step
/// down is `fanout`'s alone: a grant short of `k` - 1 (a new warm site's ledger whose pool other networks already
/// hold) keeps `k`, and that site serves with what it holds until slots free. `None` is a ledger that did not answer:
/// then k stays, with no step up and no step down, so a failure never takes a network's shards away.
pub fn granted_k(k: u8, want: u8, grants: [Option<u8>; 2]) -> u8 {
    let (k, want) = (k.clamp(1, FANOUT_MAX), want.clamp(1, FANOUT_MAX));
    match grants {
        [Some(a), Some(b)] if want > k => (1 + a.min(b)).clamp(k, want),
        [Some(_), Some(_)] => want,
        _ => k,
    }
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
/// `shards_for(network, site, count, pool)`, so one push always lands on the same shard and pushes spread evenly. The
/// caller falls back to the first shard when the chosen one is not caught up.
pub fn push_validator(network: NetworkId, site: SiteId, count: u16, pool: &[u16], push: &[u8]) -> GitCacheShard {
    let ks = shards_for(network, site, count, pool);
    validator(push, &ks).unwrap_or(ks[0])
}

/// The shard object a shard of `network` answers as. A network's first shard keeps the per-network name every site
/// script and edge has used since M1 (`git#<site>#net:<repo>`), so no network moves when fan-out ships; a hot-pool
/// shard, `git#<site>#pool:<n>`, serves the one network the site's `PoolLedger` leased it to, and keeps the repository
/// and `IngestCursor` of each network that held it before side by side.
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

/// Who calls a shard: a client's request or the work it caused (a push, a fetch, a merge, a test merge, an archive
/// promotion), or a new leadership's warm catch-up (`warm` on `/catchup`), which no client asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Caller {
    Client,
    Warm,
}

/// What the shard object does with a call, given whether its container runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Serve {
    /// Answer without starting the container: a warm call on a stopped shard. A network nobody uses has no shard to
    /// warm, and starting one would bill a container until `WARM_FOR` ran out (`tests/am1/results/idle-shards.md`).
    Skip,
    /// Serve, starting the container if needed. `touch` records the call as the shard's last request, which keeps it
    /// warm `WARM_FOR` longer; a warm call does not, so leaderships on an idle group never keep its shard running.
    Run { touch: bool },
}

pub fn serve(caller: Caller, running: bool) -> Serve {
    match caller {
        Caller::Client => Serve::Run { touch: true },
        Caller::Warm if running => Serve::Run { touch: false },
        Caller::Warm => Serve::Skip,
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

/// How long the archive's promotion waits on one site's shard before it tries the next: a shard still catching up
/// answers 503 `warming` past it and goes on in the background.
pub const PROMOTE_WAIT_S: u32 = 60;
/// The wait on a shard that is not the promotion's last try [guess]. A warm shard's catch-up took 0.1 to 3.0 s in
/// B0-30's rerun, and a new leader's first promotion waited out the full `PROMOTE_WAIT_S` on its first site's shard,
/// whose catch-up had not finished, before it tried the next, which promoted and validated in 0.8 s: 61.7 s in all
/// (`tests/am1/results/b030-split.md`, "B0-30 rerun").
pub const PROMOTE_NEXT_WAIT_S: u32 = 10;

/// How long a promotion waits on one site's shard: `PROMOTE_NEXT_WAIT_S` when another site's shard follows, so a
/// shard behind costs one short wait, and `PROMOTE_WAIT_S` on the last try, which waits for any catch-up to end.
pub fn promote_wait_s(next: bool) -> u32 {
    if next { PROMOTE_NEXT_WAIT_S } else { PROMOTE_WAIT_S }
}

/// The sites whose shards a promotion tries, in order: `me` (the leader's site, whose shard the alarm calls) first,
/// then the record's other Full sites in record order. A Witness holds no content, so it has no git cache. A shard
/// rehydrating after a reset answered every promotion 503 `warming` for 30 minutes and more, and archiving stopped
/// behind it (`tests/am1/results/hot-shards.md`).
pub fn promote_sites(record: &graftyard_types::PlacementRecord, me: SiteId) -> Vec<SiteId> {
    let full = record
        .sites
        .iter()
        .zip(record.name.kinds())
        .filter(|(_, k)| **k == graftyard_types::MemberKind::Full)
        .map(|(s, _)| *s);
    std::iter::once(me).chain(full.filter(|s| *s != me)).collect()
}

/// Whether a promotion that one shard answered with `status` goes on to the next site's shard: a 422 is the packs'
/// own answer (a base no ingested pack provides), the same on every shard, so it ends the step; anything else (503
/// `warming`, a 409 for a pack that shard never ingested, a failed call as 0) is that shard's.
pub fn promote_moves_on(status: u16) -> bool {
    status != 422
}

/// The status a shard answers a `/validate` (or `/promote`) that did not succeed. `container` is what the container
/// answered the call itself, `None` when the shard never got an answer from it: its catch-up failed, the container
/// was not running, or the call to it broke. Only the container's own 4xx is the pack's or the request's answer (a
/// pack `index-pack` refuses, a tip it does not connect, a malformed body), the same on every shard, which ends a
/// caller's tries and refuses the push, as 422. A 5xx or no answer is this shard's state, answered 503 so the
/// caller tries the next site's shard: at a home-shard kill, 23 pushes in flight were refused `unpacker error`
/// because a broken catch-up and a lost container call both answered 422 (`tests/am1/results/hot-shards-2.md`).
pub fn validate_status(container: Option<u16>) -> u16 {
    match container {
        Some(s) if (400..500).contains(&s) => 422,
        _ => 503,
    }
}

/// How often a leading member catches up every Full site's first shard of its network while it applies entries
/// [guess]. A shard that serves nothing falls behind by every push the home validated: at j9g's home-shard kill, site b's
/// was 1,100 entries behind and site c's 1,900, so each push's fallback catch-up outlasted the edge's waits (15 s, then
/// 60 s), and the 16 pushes in flight answered 503 after 137 s (`tests/am1/results/hot-shards-3.md`). 15 s of journey
/// 9's pushes is about 60 entries, a catch-up of a second or two.
pub const KEEP_WARM_MS: u64 = 15_000;

/// Whether a leading member catches up its network's shards now: the first time this leadership asks (`last` is
/// `None`), or `KEEP_WARM_MS` after the last time if it applied entries since. A group that applies nothing wakes no
/// shard: a `warm` call never starts a stopped one either (`serve`).
pub fn keep_warm_due(last: Option<(u64, Index)>, now: u64, applied: Index) -> bool {
    last.is_none_or(|(at, was)| applied > was && now >= at.saturating_add(KEEP_WARM_MS))
}

/// How long the network's first shard sends no push to a pool shard that answered `warming` [guess]: a pool shard
/// that holds no cursor starts a rehydrate, which takes minutes on a hot group, and under a carried-over k every push
/// meanwhile paid a call to it before the first shard validated it itself (`tests/am1/results/hot-shards-3.md`).
pub const POOL_COLD_SKIP_MS: u64 = 30_000;

/// Whether the network's first shard forwards a push to the pool shard `push_validator` picked: not while one that
/// answered `warming` at `cold_at` is inside `POOL_COLD_SKIP_MS` of it, so pushes validate on the first shard at once.
pub fn pool_forward(cold_at: Option<u64>, now: u64) -> bool {
    cold_at.is_none_or(|at| now >= at.saturating_add(POOL_COLD_SKIP_MS))
}

/// Whether a shard answers a call 503 `warming` at once rather than waiting out the call's `wait` on its catch-up.
/// `follows`: the caller has another site's shard to try after this one (`next=1`; never on its last try, which
/// waits, so a call never runs out of shards that would serve it). `rehydrating`: a rehydrate of the network is under
/// way on this shard, which takes tens of minutes on a hot group. `pool_cold`: a pool shard forwarded a push holds no
/// cursor for its disk. The edge learned "cold" per isolate, so after a home-shard kill each new isolate waited the
/// home's 15 s once, 11% of j9e's pushes (`tests/am1/results/hot-shards.md`); the shard's own mark is one for all.
pub fn warming_now(follows: bool, rehydrating: bool, pool_cold: bool) -> bool {
    pool_cold || (follows && rehydrating)
}
