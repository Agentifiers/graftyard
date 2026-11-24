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
    /// Whether this leadership's last ask held one slot past the k - 1 served (`pool_reserve`).
    pub reserved: bool,
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

    /// Whether the larger warm load is above `FANOUT_UP` now, so the leader reserves the next pool slot ahead of the
    /// step (`pool_reserve`).
    pub fn high(&self) -> bool {
        self.f.high_since.is_some()
    }

    /// A step `report` answered that the leader did not commit, because a slot it would serve from was not caught up
    /// (`warm_step`): the hysteresis goes back to `prior`, what it was before that report, so the high line keeps its
    /// start and the step commits at the first report after the slot answers caught up, not `FANOUT_UP_FOR` later.
    pub fn step_waited(&mut self, prior: Fanout) {
        self.f = prior;
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
    /// Slots a lease gave up (it lapsed, or its network asked for fewer) and no lease holds again, which the ledger
    /// object's heartbeat tears down (`beat`), so a pool shard runs only while a network holds it
    /// (`design/DESIGN.md`, Shape, `HOT_POOL`).
    #[serde(default)]
    pub idle: Vec<u16>,
}

fn held_all(l: &Option<PoolLease>) -> Vec<u16> {
    l.as_ref().map_or(Vec::new(), |l| l.slots.clone())
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
        let stale = LOAD_STALE.as_millis() as u64;
        let lapsed: Vec<u16> =
            self.leases.iter().filter(|l| now.0 >= l.at.0 + stale).flat_map(|l| l.slots.iter().copied()).collect();
        self.idle.extend(lapsed);
        self.leases.retain(|l| now.0 < l.at.0 + stale);
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
        self.idle.extend(held_all(&mine).into_iter().filter(|x| !slots.contains(x)));
        self.idle.retain(|x| !slots.contains(x));
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
    ///
    /// Answers the slots to tear down now: those a lease gave up (it lapsed, or its network asked for fewer) that no
    /// lease holds again, each once. The object calls `/teardown` on each (`workers/site/src/shard.rs`, `alarm`), so
    /// the pool runs only for networks hot in the last `LOAD_STALE`, inside what `scripts/cost.py` prices.
    pub fn beat(&mut self, now: Timestamp) -> Vec<u16> {
        self.settle(now);
        self.seen = Some(now);
        let held: Vec<u16> = self.leases.iter().flat_map(|l| l.slots.iter().copied()).collect();
        let mut out: Vec<u16> = std::mem::take(&mut self.idle).into_iter().filter(|x| !held.contains(x)).collect();
        out.sort_unstable();
        out.dedup();
        out
    }

    /// When the ledger's object must next `beat`: `LEDGER_BEAT` after `now` while it holds a lease or a slot to tear
    /// down, never otherwise.
    pub fn beat_due(&self, now: Timestamp) -> Option<Timestamp> {
        (!self.leases.is_empty() || !self.idle.is_empty()).then(|| Timestamp(now.0 + LEDGER_BEAT.as_millis() as u64))
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

/// The pool shards the leader asks both warm sites' ledgers for after a load report, with the step up reserved
/// ahead (`design/DESIGN.md`, Shape, `HOT_POOL`): while the larger warm load is above `FANOUT_UP` (`high`) and k is
/// below `FANOUT_MAX`, it asks for k pool shards, one past the k - 1 it serves from, from the first high report, so
/// the reserved slot is started and caught up (`Caller::PoolWarm`) during `FANOUT_UP_FOR` instead of after the step.
/// Otherwise it is `pool_ask`, except that a reservation this leadership holds (`reserved`) is given back at k - 1:
/// the first shard renews its whole lease each minute, so a slot no ask releases would never lapse. `None` when
/// nothing is held or wanted.
pub fn pool_reserve(k: u8, step: Option<u8>, high: bool, reserved: bool) -> Option<u8> {
    let k = k.clamp(1, FANOUT_MAX);
    let ask = pool_ask(k, step);
    match (high && k < FANOUT_MAX).then_some(k) {
        Some(r) => Some(ask.map_or(r, |a| a.max(r))),
        None if reserved => Some(ask.unwrap_or(k - 1)),
        None => ask,
    }
}

/// The k a step up may commit given which of the granted slots are caught up: `next` (`granted_k`'s answer) only when
/// it does not pass the committed `k` or every slot it would serve from on both warm sites is warm, else `k`. A step
/// to a cold slot would send pushes to a shard that answers `warming` for the length of a rehydrate (32 minutes for
/// B0-34's monorepo, `tests/am1/results/lm1-memory.md`, round 18b), so the step waits, keeping the high line, until
/// the reserved slots catch up. `warm[s]` is, per warm site, whether each slot in `slots[s]` (in grant order) answered
/// its last `PoolWarm` catch-up 200; the step serves from the first `next - 1` of them.
pub fn warm_step(k: u8, next: u8, slots: [&[u16]; 2], warm: [&[bool]; 2]) -> u8 {
    if next <= k {
        return next;
    }
    let n = usize::from(next - 1);
    let ready = |s: usize| slots[s].len() >= n && warm[s].len() >= n && warm[s][..n].iter().all(|w| *w);
    if ready(0) && ready(1) { next } else { k }
}

/// What a shard's `/catchup` does for its caller (`catchup_answer`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CatchupAnswer {
    /// Wait for a catch-up that starts after the call, within the call's `wait`, then answer 200 (else `warming`).
    WaitForCatchUp,
    /// Answer 200 at once; a catch-up starts in the background.
    Warm,
    /// Answer `warming` at once; a catch-up starts in the background.
    Cold,
}

/// The `/catchup` answer. A pool warm (`Caller::PoolWarm`, the network leader's `/catchup?warm=pool` on a slot it
/// holds, whose answer `warm_step` reads) asks whether the slot may be stepped to: warm once its container runs, holds a
/// cursor for its own disk, and no rehydrate of the network is under way, which is a rehydrate finished, at once and
/// whatever its catch-up takes. A slot behind the head is safe to step to: a forwarded push it lacks a base for comes
/// back to the first shard (`on_catch_up`). Every other caller waits for a catch-up as before. Waiting as well, a
/// rehydrated slot under B0-34's load answered cold to 34 of 59 warms (its catch-ups took 1 to 14 s against the warm's
/// 2 s), and k stepped to 3 at 888 s of 900 (LM-1 round 36, T-01a1ccdd's trace).
pub fn catchup_answer(caller: Caller, running: bool, cursor: bool, rehydrating: bool) -> CatchupAnswer {
    match caller {
        Caller::PoolWarm if running && cursor && !rehydrating => CatchupAnswer::Warm,
        Caller::PoolWarm => CatchupAnswer::Cold,
        _ => CatchupAnswer::WaitForCatchUp,
    }
}

/// The pool shards a network keeps on each warm site after its leader asked for `want`, both ledgers granted up to
/// `granted` (`granted_k`), and it committed `next` (`warm_step`): the k - 1 it serves, or with `reserve` one past
/// it up to `want`; and while a step waits on cold slots (`next` < `granted`), every slot that step would serve from,
/// so they stay leased and warm until it commits. Trimming a waiting step to `next` - 1 gave its slots back before
/// they were warmed, so it never stepped (LM-1 m30: a forced k 4 on a fresh network kept k 1).
pub fn pool_keep(want: u8, granted: u8, next: u8, reserve: bool) -> u8 {
    let served = if reserve { want.min(next) } else { next.saturating_sub(1) };
    if next < granted { served.max(granted - 1) } else { served }
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

/// The sites whose buckets hold a network's composed-miss chain (`crate::compose`): its warm sites among its group's
/// `record`, home first, once each. The builder writes the chain to these buckets, every shard installs from them, and
/// the edge checks a manifest's chain packs in them in this order (`crate::compose::check_in_time`). It takes the
/// record, not a list of sites, so no caller can pass the install's: on graft-m1, with four sites and the group on
/// three, main took the install's and put monorepo 1048606's chain in bucket d, a site with no shard of the group, and
/// none on b or c (`decisions/DECISIONS.md`, "`chain-record`").
pub fn chain_buckets(network: NetworkId, record: &graftyard_types::PlacementRecord) -> Vec<SiteId> {
    let [a, b] = warm_sites(network, record.home, &record.sites);
    if a == b { vec![a] } else { vec![a, b] }
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

/// How often a client's request writes the shard's `last_request` and its alarm (`touch_due`): at most once a minute,
/// against WARM_FOR's day, so the container stops at most this much earlier. Written on every request, a 434-push
/// burst made 434 pairs of storage writes on one network shard, whose forwards each waited p50 3,038 ms
/// (`tests/am1/results/lm1-rows-m3f.md`, "m3fr8 after the run"; that the writes set the wait is an inference).
pub const TOUCH_EVERY: Duration = Duration::from_secs(60);

/// Whether a client's request at `now` writes `last_request` and the alarm, given the stored `last_request`: when
/// none is stored, or it is TOUCH_EVERY old or older.
pub fn touch_due(now: Timestamp, last_request: Option<Timestamp>) -> bool {
    last_request.is_none_or(|l| now.0.saturating_sub(l.0) >= TOUCH_EVERY.as_millis() as u64)
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
    /// The network leader's warm of a pool shard its network holds (`warm=pool` on `/catchup`): it starts a stopped
    /// container, since a pool shard nobody started never becomes warm, but is not a request, so it keeps none warm
    /// past its lease; the ledger tears the slot down when the lease is given up (`PoolLedger::beat`).
    PoolWarm,
    /// A due range's archive promotion (`warm=start` on its `/promote`, `/validate` and `/idx`): it starts a stopped
    /// shard, since the log cannot archive past the range otherwise, but is no client's request, so it keeps none warm.
    /// A shard with no client in `WARM_FOR` lapses `PROMOTE_LAPSE` after its last such call (`promote_lapse`).
    Start,
}

/// How long a shard that only a due range's promotion runs stays up after that promotion's last call [guess]: long
/// enough for the drain run's next promotion, minutes apart, to find it running. Held as a `Caller::Client`, each such
/// start kept a quiet group's container a day: 131330's due range started five on idle `graft-m2b`
/// (`tests/am1/results/lm1-memory.md`, round 34).
pub const PROMOTE_LAPSE: Duration = Duration::from_secs(600);

/// The inactivity timeout to set once a call has been served, if it changes: `PROMOTE_LAPSE` after a `Caller::Start`
/// call on a shard whose last client request (`last_request`) is `WARM_FOR` old or older, or that never had one; a
/// client's later request touches the shard, which sets `INACTIVITY` again (`ShardHost::touch`).
pub fn promote_lapse(caller: Caller, now: Timestamp, last_request: Option<Timestamp>) -> Option<Duration> {
    let idle = last_request.is_none_or(|l| renewal(now, l) == Renewal::Lapse);
    (caller == Caller::Start && idle).then_some(PROMOTE_LAPSE)
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

/// The caller a shard call's path and `warm` query name: `warm=pool` on `/catchup` is the network leader's pool warm,
/// and any other `warm` is a warm call, a new leadership's `/catchup` or promote-ahead's `/promote` with its `/validate`
/// and `/idx`. Promote-ahead as a client started every stopped shard of an old group that woke with a tail above the
/// bound and held it a day: 19 containers on idle `graft-m2b` (`tests/am1/results/lm1-memory.md`, round 28).
pub fn caller(path: &str, warm: Option<&str>) -> Caller {
    match warm {
        Some("pool") if path == "/catchup" => Caller::PoolWarm,
        Some("start") => Caller::Start,
        Some(_) => Caller::Warm,
        None => Caller::Client,
    }
}

pub fn serve(caller: Caller, running: bool) -> Serve {
    match caller {
        Caller::Client => Serve::Run { touch: true },
        Caller::Warm if running => Serve::Run { touch: false },
        Caller::Warm => Serve::Skip,
        Caller::PoolWarm | Caller::Start => Serve::Run { touch: false },
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

/// Which image a shard object starts. `day_step` is how far the site's `ContainerDay` has rolled
/// (`graftyard_release::gate::shard_day`: 0 to 2, covering 10%, 50%, and 100% of the site's shards); `None` before
/// the day and after its rollback. Shards run one object per network (`object_name`), so a step covers the objects
/// whose weight on `day#<site>` falls in its share, by the object's id: each step covers every object of the one
/// before, and about its share of them [inference: the design's rendezvous order over `count + HOT_POOL` slots would
/// put every network's first shard, slot 0, in one step].
pub fn image_for(site: SiteId, object: &str, day_step: Option<u8>) -> ImageSlot {
    let Some(step) = day_step else { return ImageSlot::GitPrev };
    let pct = [10u64, 50, 100][usize::from(step.min(2))];
    let key = format!("day#{}#{object}", site.0);
    if weight(key.as_bytes(), 0) % 100 < pct { ImageSlot::Git } else { ImageSlot::GitPrev }
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
    /// The default branch this disk's HEAD was last pointed at (G26), so a catch-up names HEAD only when it changed.
    /// `None` on a cursor saved before G26, and the next catch-up names it once.
    #[serde(default)]
    pub head: Option<String>,
}

impl IngestCursor {
    pub fn new(network: NetworkId) -> IngestCursor {
        IngestCursor { network, chains: SmallVec::new(), head: None }
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
    /// Its packs' bytes, of `bytes`.
    pub pack_bytes: u64,
    /// The last client fetch (`info-refs` or `upload-pack`), client validate, and catch-up write (ingest or refs),
    /// `None` for none since the repository was made or before a container that records them.
    pub fetched: Option<Timestamp>,
    pub pushed: Option<Timestamp>,
    pub caught_up: Option<Timestamp>,
}

/// How often a running shard logs its `shard stats` line (`stats_line`) from its alarm.
pub const STATS_EVERY: Duration = Duration::from_secs(24 * 3600);

/// Whether the shard's alarm at `now` logs its `shard stats` line, given when it last did: when it never did, or
/// STATS_EVERY ago or longer.
pub fn stats_due(now: Timestamp, last: Option<Timestamp>) -> bool {
    last.is_none_or(|l| now.0.saturating_sub(l.0) >= STATS_EVERY.as_millis() as u64)
}

/// The daily `shard stats` line: every repository on the shard's disk with its bytes, its packs' bytes, and its last
/// use, fetch, push, and catch-up, and the totals of those fetched, pushed, and used in the day before `now`. Summed
/// over every shard of every site, the fetched day's bytes are the daily working set that `scripts/cost.py` guesses
/// (`decisions/OPEN-QUESTIONS.md`, G1); a network warm on two sites counts on each, as its shards do. Times are ms.
pub fn stats_line(disk: u64, repos: &[DiskRepo], now: Timestamp) -> serde_json::Value {
    let day = STATS_EVERY.as_millis() as u64;
    let within = |t: Option<Timestamp>| t.is_some_and(|t| now.0.saturating_sub(t.0) < day);
    let sum = |f: &dyn Fn(&DiskRepo) -> bool| -> (usize, u64) {
        repos.iter().filter(|r| f(r)).fold((0, 0), |(n, b), r| (n + 1, b + r.bytes))
    };
    let (held_n, held) = sum(&|_| true);
    let total = |(n, bytes): (usize, u64)| serde_json::json!({ "networks": n, "bytes": bytes });
    let t = |t: Option<Timestamp>| t.map(|t| t.0);
    serde_json::json!({
        "at": now.0,
        "disk": disk,
        "held": total((held_n, held)),
        "day": {
            "used": total(sum(&|r| within(Some(r.used)))),
            "fetched": total(sum(&|r| within(r.fetched))),
            "pushed": total(sum(&|r| within(r.pushed))),
            "caught_up": total(sum(&|r| within(r.caught_up))),
        },
        "repos": repos.iter().map(|r| serde_json::json!({
            "network": r.network.0.0,
            "bytes": r.bytes,
            "pack_bytes": r.pack_bytes,
            "used": r.used.0,
            "fetched": t(r.fetched),
            "pushed": t(r.pushed),
            "caught_up": t(r.caught_up),
        })).collect::<Vec<_>>(),
    })
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

/// One shard object's catch-ups of one network, numbered from 1 as they start (`workers/site/src/shard.rs`, `Jobs`):
/// whether one runs, the number of the latest started, and of the latest that ended without an error, and whether the
/// shard was torn down since a caller that may start its container last came (`tear_down`, `resume`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CatchUps {
    pub running: bool,
    pub started: u64,
    pub ok: u64,
    pub torn: bool,
}

/// What a caller that needs a catch-up numbered `from` or later does next: go on, start one, or wait for the running
/// one to end and ask again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CatchUpStep {
    Done,
    Start,
    Await,
    /// The shard was torn down: no catch-up starts for this caller, which answers as a stopped shard does.
    Stopped,
}

impl CatchUps {
    /// The number of the next catch-up to start: the first that reads the group's head after anything committed
    /// before this call. The one running now may have read it earlier.
    pub fn next(&self) -> u64 {
        self.started + 1
    }

    pub fn step(&self, from: u64) -> CatchUpStep {
        if self.ok >= from {
            CatchUpStep::Done
        } else if self.running {
            CatchUpStep::Await
        } else if self.torn {
            CatchUpStep::Stopped
        } else {
            CatchUpStep::Start
        }
    }

    /// Starts one unless one runs or the shard was torn down; true when it did.
    pub fn start(&mut self) -> bool {
        if self.running || self.torn {
            return false;
        }
        self.running = true;
        self.started += 1;
        true
    }

    pub fn end(&mut self, ok: bool) {
        self.running = false;
        if ok {
            self.ok = self.started;
        }
    }

    /// `/teardown`: no run starts and no running run takes another piece until `resume`. Each run puts the `warming`
    /// key and arms the object's alarm, and its first piece starts the container. On B0-33 sab, a keep-warm caller
    /// still waiting when the teardown's kill failed the running run started the next one, which armed the alarm the
    /// teardown had just deleted and started site c's container again (`tests/am1/results/sync-ab.md`, "Close-out").
    pub fn tear_down(&mut self) {
        self.torn = true;
    }

    /// A caller that may start the container (a client request, a pool warm, or a due range's promotion) came after a
    /// teardown, so catch-ups run again. A keep-warm call (`Caller::Warm`) never resumes one.
    pub fn resume(&mut self) {
        self.torn = false;
    }
}

/// Which disk a shard checks a push against (`crate::host::OnDisk`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValidateOn {
    /// After a catch-up that started after the push arrived.
    CaughtUp,
    /// As it stands, with a catch-up only when the container finds a base or ref it lacks.
    Disk,
}

/// A push validates on the disk as it stands once this isolate caught the running container up (`cursor`): its
/// disk then holds every committed pack up to some index and ref tips whose history is whole, so a pack that
/// resolves and connects there resolves and connects on any later disk. The archive's promotion (`derived`) and a
/// container this isolate never caught up catch up first (design/DESIGN.md, Shape, "Git cache shards (derived)").
pub fn validate_on(derived: bool, running: bool, cursor: bool) -> ValidateOn {
    if derived || !running || !cursor { ValidateOn::CaughtUp } else { ValidateOn::Disk }
}

/// Which disk a read that names only commits answers from (`decisions/DECISIONS.md`, "`disk-first-reads`"): the disk as
/// it stands once this isolate caught the running container up (`cursor`) and no rehydrate is rebuilding it, as
/// `validate_on` decides for a push. A commit's tree, blobs and merge with another commit are the same on every disk
/// that holds them, so a catch-up can only add a missing commit, never change an answer.
pub fn read_on(running: bool, cursor: bool, rehydrating: bool) -> ValidateOn {
    if rehydrating { ValidateOn::CaughtUp } else { validate_on(false, running, cursor) }
}

/// Whether the container's answer to a `/ui` read says a commit it names is not on its disk: a 404 "no commit", or
/// "no repository" for a container that holds none of the network (`containers/git/server/ui.go`, `dirAt`; `main.go`,
/// `netParam`). Any other 404 (a path that is not there at a commit that is) is the read's answer.
pub fn ui_missing(status: u16, body: &str) -> bool {
    status == 404 && (body.starts_with("no commit ") || body.starts_with("no repository "))
}

/// A read on the disk first (`read_on`), and again after a catch-up only when the disk lacked something it names.
/// `read(caught_up)` catches up first when `caught_up` is true, then answers the result and whether it found an object
/// missing. The read after the catch-up is the answer either way, so a commit still missing once caught up answers as
/// it always did, and a disk that holds every commit answers however the catch-up would have gone.
pub async fn read_disk_first<T, E, R, F>(on_disk: bool, read: R) -> Result<T, E>
where
    R: Fn(bool) -> F,
    F: std::future::Future<Output = Result<(T, bool), E>>,
{
    if on_disk {
        let (t, missing) = read(false).await?;
        if !missing {
            return Ok(t);
        }
    }
    Ok(read(true).await?.0)
}

/// What a shard does with a push that needs a catch-up first: one it does not validate on the disk as it stands
/// (`validate_on`), or one whose base or ref the container found missing there (`crate::host::Validated::MissingBase`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OnCatchUp {
    /// Wait for a catch-up that started after the push arrived, then check it (again).
    Wait,
    /// Answer 409 at once: the network's first shard, which forwarded the push, validates it itself.
    Back,
}

/// A pool shard (`pool`, a push the network's first shard forwarded) never waits for a catch-up: it sends the push
/// back. The first shard, or a pool shard asked directly, waits. A pool shard's catch-up runs behind the first
/// shard's, both ingesting the burst's own packs: on B0-32 m3fr11 two pool shards held 10 burst pushes 15 s for the
/// catch-up after the running one (each ingesting 37 to 495 packs, 4.4 to 21 s), answered `warming`, and the first
/// shard then validated each in 131 to 244 ms. Those 10 were the burst's whole validate tail: p99 15.8 s, against
/// 1.3 s for the rest (`tests/am1/results/lm1-rows-m3f.md`, "m3fr11 after the run").
pub fn on_catch_up(pool: bool) -> OnCatchUp {
    if pool { OnCatchUp::Back } else { OnCatchUp::Wait }
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

/// The `wait` the edge gives a client call's first try on a network's shards, usually the home's: past it the shard
/// answers 503 `warming` and goes on catching up in the background. Every later try, a hedge's included, waits longer
/// (the edge's `VALIDATE_WAIT_OTHER_S`, 60 s).
pub const FIRST_TRY_WAIT_S: u32 = 15;

/// How often a shard object tells its container that moved load arrived (`POST /maint/hold`): the container holds
/// maintenance `YIELD_HOLD` (60 s) from each call, so one call per 10 s keeps a hold with 50 s to spare.
pub const MAINT_HOLD_EVERY: Duration = Duration::from_secs(10);

/// Whether a call on a shard is moved load, which holds the container's maintenance (`decisions/DECISIONS.md`,
/// "`maint-yield`", "`moved-mark`"): a client's push or fetch (`/validate`, `/upload-pack`, `/info-refs`, or the
/// streamed probe's `/catchup`), not derived and not a warm call, reaching a shard on a site other than the record's
/// `home` with `moved` (the edge's `moved=1`). The edge sends it only to another site after a try on the home failed,
/// in this request or within `COLD_SHARD_MS` in that isolate, so such a call means the home's shard is down, cold or
/// failing and this site carries its work. A hedge that passed a slow home sends none, nor do the later first tries
/// its cold mark sends here: on B0-34 mt21 they held site b's maintenance from 4.7 minutes before the kill, on a live
/// home (`decisions/OPEN-QUESTIONS.md`, G41). A pool shard a moved first shard forwards to counts too, because the
/// forward keeps the URL.
pub fn maint_hold(path: &str, caller: Caller, derived: bool, me: SiteId, home: SiteId, moved: bool) -> bool {
    matches!(path, "/validate" | "/upload-pack" | "/info-refs" | "/catchup")
        && caller == Caller::Client
        && !derived
        && me != home
        && moved
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
