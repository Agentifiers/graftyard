//! AM1-WP1's in-memory rerun of B0-34 (`fanout` from `NetworkLoad` with the per-owner hot pool) and B0-35 (lazy test
//! merges under `TEST_MERGE_BUDGET`), on the pure halves the hosts call (`build/BUILD-PLAN.md`, M0-WP9). The git CPU
//! is a model: a shard's CPU is the network's offered vCPU over its k shards' 4 vCPU, and merge costs follow M0's
//! torvalds/linux measurements [measured, `tests/m0/results/m0-wp9.md`, B0-34: p50 5 to 45 ms, p99 at most 338 ms].

use graftyard_git::issues::IssueNumber;
use graftyard_git::prsync::{
    Budgets, MERGE_EST_MS, MergeAnswer, MergeBudget, MergeCause, TEST_MERGE_BUDGET, budgeted, instance_vcpu,
};
use graftyard_git::{
    FANOUT_DOWN_FOR, FANOUT_MAX, FANOUT_UP_FOR, Fanout, HOT_POOL, NetworkLoad, clone_cache_buckets, fanout, pool_grants,
};
use std::collections::BTreeMap;

/// The tiers' shard at 2,000 developers, `standard-4`, which journey 9 is sized on.
const SHARD_VCPU: f32 = 4.0;

use graftyard_types::{DbId, NetworkId, SiteId, Timestamp};

const MIN: u64 = 60_000;

fn net(n: i64) -> NetworkId {
    NetworkId(DbId(n))
}

fn load(cpu: f32) -> NetworkLoad {
    NetworkLoad { cpu_p99: cpu, queue_depth: 0 }
}

#[test]
fn fanout_steps_up_after_three_minutes_above_and_down_after_an_hour_below() {
    let mut f = Fanout::default();
    let (hi, lo) = (load(0.9), load(0.1));
    let mut k = 1;
    for m in 0..=3 {
        k = fanout([&hi, &lo], k, Timestamp(m * MIN), &mut f);
    }
    assert_eq!(k, 2, "up once the larger load held above 0.75 for 3 min");
    // The other site's load decides when it is the larger one.
    let mut f2 = Fanout::default();
    let mut k2 = 1;
    for m in 0..=3 {
        k2 = fanout([&lo, &hi], k2, Timestamp(m * MIN), &mut f2);
    }
    assert_eq!(k2, 2, "the larger of the two warm sites' loads");
    // Two minutes is not enough.
    let mut f3 = Fanout::default();
    assert_eq!(fanout([&hi, &hi], 1, Timestamp(0), &mut f3), 1);
    assert_eq!(fanout([&hi, &hi], 1, Timestamp(2 * MIN), &mut f3), 1);
    // Down: an hour below 0.4.
    let mut f = Fanout::default();
    let mut k = 4;
    let t0 = 10 * MIN;
    for m in 0..60 {
        k = fanout([&lo, &lo], k, Timestamp(t0 + m * MIN), &mut f);
    }
    assert_eq!(k, 4, "under an hour holds");
    k = fanout([&lo, &lo], k, Timestamp(t0 + FANOUT_DOWN_FOR.as_millis() as u64), &mut f);
    assert_eq!(k, 3);
    // Between the thresholds nothing moves.
    let mid = load(0.6);
    let mut f = Fanout::default();
    for m in 0..200 {
        assert_eq!(fanout([&mid, &mid], 3, Timestamp(m * MIN), &mut f), 3);
    }
    let _ = FANOUT_UP_FOR;
}

#[test]
fn fanout_never_passes_fanout_max() {
    let mut f = Fanout::default();
    let sat = load(1.0);
    let mut k = 1;
    for m in 0..1_000 {
        k = fanout([&sat, &sat], k, Timestamp(m * MIN), &mut f);
        assert!(k <= FANOUT_MAX);
    }
    assert_eq!(k, FANOUT_MAX);
}

#[test]
fn an_owner_is_held_to_its_share_and_cannot_reduce_another_owners_network() {
    // Plan shares as M0's B0-34 run set them: the monorepo's owner 6, the others 2.
    let share = |o: u64| if o == 1 { 6 } else { 2 };
    // The monorepo holds its share of 6; the hostile owner (3) asks 7 on each of three networks, listed first.
    let asks = [(net(30), 3, 0, 7), (net(31), 3, 0, 7), (net(32), 3, 0, 7), (net(1), 1, 6, 6), (net(2), 2, 0, 1)];
    let g = pool_grants(&asks, share);
    // The monorepo keeps its 6 first, though listed after the hostile networks; the 1 left goes to the hostile owner.
    assert_eq!(g[3], 6, "another owner reduced the monorepo: {g:?}");
    assert_eq!(g[..3].iter().map(|x| u16::from(*x)).sum::<u16>(), 1, "{g:?}");
    assert!(g.iter().map(|x| u16::from(*x)).sum::<u16>() <= HOT_POOL);
    // With room, the hostile owner gets its share and no more.
    let g = pool_grants(&[(net(30), 3, 0, 7), (net(31), 3, 0, 7), (net(1), 1, 0, 1)], share);
    assert_eq!(u16::from(g[0]) + u16::from(g[1]), 2, "{g:?}");
    assert_eq!(g[2], 1);
    // An owner whose plan allows the whole pool may take it, up to FANOUT_MAX - 1 per network.
    assert_eq!(pool_grants(&[(net(1), 1, 0, 7)], |_| HOT_POOL), [7]);
}

#[test]
fn a_hot_networks_clone_cache_entries_go_to_both_warm_sites() {
    let sites = [SiteId(0), SiteId(1), SiteId(2)];
    for n in 0..50 {
        let both = clone_cache_buckets(net(n), SiteId(0), &sites, true);
        assert_eq!(both.len(), 2);
        assert_eq!(both[0], SiteId(0));
        assert_ne!(both[0], both[1]);
        assert_eq!(clone_cache_buckets(net(n), SiteId(0), &sites, false), [SiteId(0)]);
    }
}

/// B0-34 in memory, through the leader's hosted path: the monorepo under journey 9 (2.68 vCPU on one network
/// [measured, B0-34]) and a tip moving 10 times a minute, then one warm site killed (the other site's load then carries
/// both), then a second owner's network turning hot, then a hostile owner making three of its networks hot. Each minute
/// each network's leader folds both warm sites' `NetworkLoad` into `LoadReports`, asks both sites' `PoolLedger`s for
/// `pool_ask`'s shards, and commits `granted_k`, as `workers/site/src/fanout.rs` does. The run checks k at most
/// FANOUT_MAX, the monorepo's k never reduced by another owner, every owner within its share on each site, no pool
/// shard held by two networks, and no call but these pure ones.
#[test]
fn b0_34_k_is_set_in_band_and_the_hostile_owner_is_held_to_its_pool_share() {
    use graftyard_git::{LoadReports, PoolLedger, granted_k, pool_ask, shards_for};
    struct N {
        id: NetworkId,
        owner: u64,
        vcpu: f32,
        k: u8,
        r: LoadReports,
    }
    let (s0, s1) = (SiteId(0), SiteId(1));
    // The monorepo at 20 vCPU, past journey 9's 2.68, so k climbs to its owner's share.
    let mut nets = vec![N { id: net(1), owner: 1, vcpu: 20.0, k: 1, r: LoadReports::default() }];
    let share = |o: u64| if o == 1 { 6 } else { 2 };
    let mut ledgers = [PoolLedger::default(), PoolLedger::default()];
    let mut mono_k = Vec::new();
    let mut max_k = 0;
    let mut denied = 0;
    let mut site_dead = false;
    for m in 0..=180u64 {
        if m == 40 {
            site_dead = true; // one warm site's containers killed: the other already holds k (the larger load sizes both)
        }
        if m == 60 {
            nets.push(N { id: net(2), owner: 2, vcpu: 5.0, k: 1, r: LoadReports::default() });
        }
        if m == 100 {
            for i in 0..3 {
                nets.push(N { id: net(30 + i), owner: 3, vcpu: 30.0, k: 1, r: LoadReports::default() });
            }
        }
        let now = Timestamp(m * MIN);
        for n in nets.iter_mut() {
            let cpu = (n.vcpu / (f32::from(n.k) * SHARD_VCPU)).min(1.0);
            n.r.report([s0, s1], s1, if site_dead { load(0.0) } else { load(cpu * 0.9) }, n.k, now);
            let step = n.r.report([s0, s1], s0, load(cpu), n.k, Timestamp(now.0 + 1));
            let Some(want) = pool_ask(n.k, step) else { continue };
            let g = ledgers.each_mut().map(|l| Some(l.ask(n.id, n.owner, want, now, share).len() as u8));
            let next = granted_k(n.k, want + 1, g);
            if next < want + 1 {
                denied += 1;
            }
            n.k = next;
            max_k = max_k.max(n.k);
        }
        for (site, l) in [s0, s1].into_iter().zip(&ledgers) {
            let mut by_owner = BTreeMap::<u64, u16>::new();
            let mut slots = Vec::new();
            for n in &nets {
                let held: Vec<u16> =
                    l.leases.iter().filter(|x| x.network == n.id).flat_map(|x| x.slots.clone()).collect();
                // Each network serves from its first shard and the slots it holds, at most k - 1 of them.
                assert!(shards_for(n.id, site, 25, &held).len() <= usize::from(n.k).max(1 + held.len()));
                *by_owner.entry(n.owner).or_default() += held.len() as u16;
                slots.extend(held);
            }
            assert!(by_owner.iter().all(|(o, v)| *v <= share(*o)), "an owner past its share at {m} min: {by_owner:?}");
            let n = slots.len();
            slots.sort();
            slots.dedup();
            assert_eq!(slots.len(), n, "a pool shard held by two networks at {m} min on {site:?}");
            assert!(n <= usize::from(HOT_POOL));
        }
        mono_k.push(nets[0].k);
    }
    // Any step down of the monorepo while its load stays above FANOUT_DOWN would be another owner's doing.
    let reduced = mono_k.windows(2).filter(|w| w[1] < w[0]).count();
    println!(
        "B0-34 in memory: monorepo k by 10 min {:?}; max k {max_k}; {denied} asks granted below the wanted k by the pool or an owner's share; monorepo k \\
         reduced after the hostile owner {reduced} times",
        mono_k.iter().step_by(10).collect::<Vec<_>>()
    );
    assert!(max_k <= FANOUT_MAX);
    assert!(mono_k[30] > 1, "the monorepo stepped up in band");
    assert_eq!(*mono_k.last().unwrap(), 7, "the monorepo reached 1 + its owner's share of 6");
    assert_eq!(reduced, 0, "another owner never reduced the monorepo's k");
    assert!(denied > 0, "the hostile owner was held below what it asked");
    let hostile: u16 = nets[3..].iter().map(|n| u16::from(n.k - 1)).sum();
    assert!(hostile <= 2, "the hostile owner's three networks hold {hostile} pool shards, past its share of 2");
}

/// A site's pool ledger, the code behind `POST /pool/grant`: a hostile owner with three hot networks gets its share
/// and no more, whichever order the asks come in, and the shards it holds are distinct from every other network's.
#[test]
fn a_hostile_owner_with_three_hot_networks_is_held_to_its_share() {
    use graftyard_git::PoolLedger;
    let share = |o: u64| if o == 3 { 2 } else { 6 };
    let t = Timestamp(0);
    let mut l = PoolLedger::default();
    let mut hostile = Vec::new();
    for i in 0..3 {
        hostile.extend(l.ask(net(30 + i), 3, 7, t, share));
    }
    assert_eq!(hostile.len(), 2, "three asks of 7 from one owner: {hostile:?}");
    // Another owner then gets what is left, 5 of 7, none of the hostile owner's.
    let other = l.ask(net(1), 1, 7, t, share);
    assert_eq!(other.len(), 5);
    assert!(other.iter().all(|s| !hostile.contains(s)), "{other:?} {hostile:?}");
    // Asking again, at every minute, never grows the hostile owner past its share.
    for m in 1..5 {
        let held: usize = (0..3).map(|i| l.ask(net(30 + i), 3, 7, Timestamp(m * MIN), share).len()).sum();
        assert_eq!(held, 2);
        assert_eq!(l.ask(net(1), 1, 7, Timestamp(m * MIN), share).len(), 5);
    }
    // With the default share, HOT_POOL - 1, no owner holds the whole pool.
    let mut l = PoolLedger::default();
    let total: usize = (0..3).map(|i| l.ask(net(40 + i), 9, 7, t, graftyard_git::owner_share).len()).sum();
    assert_eq!(total, usize::from(HOT_POOL) - 1);
}

/// Another owner's step up, however large and however often, never takes a pool shard from a network that holds it,
/// and the network keeps the same shards while it renews.
#[test]
fn another_owners_step_up_never_reduces_a_networks_held_shards() {
    use graftyard_git::PoolLedger;
    let share = |_| 6;
    let mut l = PoolLedger::default();
    let mono = l.ask(net(1), 1, 4, Timestamp(0), share);
    assert_eq!(mono.len(), 4);
    for m in 1..=10u64 {
        let other = l.ask(net(2), 2, 7, Timestamp(m * MIN), share);
        assert_eq!(other.len(), 3, "only the free slots");
        assert!(other.iter().all(|s| !mono.contains(s)));
        assert_eq!(l.ask(net(1), 1, 4, Timestamp(m * MIN + 1), share), mono, "the same slots, renewed");
    }
    // A step down releases slots, which the other owner may then take; a step up afterwards gets only free ones.
    let two = l.ask(net(1), 1, 2, Timestamp(11 * MIN), share);
    assert_eq!(two, mono[..2]);
    assert_eq!(l.ask(net(2), 2, 7, Timestamp(11 * MIN), share).len(), 5);
    assert_eq!(l.ask(net(1), 1, 4, Timestamp(11 * MIN), share), two, "nothing free: it keeps its 2");
}

/// A lease expires unless renewed within LOAD_STALE, so a network that cools frees its shards with no release call,
/// and a first shard's renewal keeps a lease alive without growing it. A ledger that answered no call for a while
/// (down, or unreachable) does not count that time against any lease.
#[test]
fn an_unrenewed_lease_expires_and_frees_its_shards() {
    use graftyard_git::{LEDGER_GAP, LOAD_STALE, PoolLedger};
    let share = |_| 6;
    let stale = LOAD_STALE.as_millis() as u64;
    let mut l = PoolLedger::default();
    assert_eq!(l.ask(net(1), 1, 6, Timestamp(0), share).len(), 6);
    // Another network renews each minute, so the ledger is in use; network 1 stops asking at minute 0.
    let mut t = 0;
    while t < stale {
        t += MIN;
        let got = l.ask(net(2), 2, 6, Timestamp(t), share).len();
        assert_eq!(got, if t < stale { 1 } else { 6 }, "at {t} ms");
    }
    assert_eq!(l.renew(net(1), Timestamp(t)), Vec::<u16>::new(), "expired: nothing to renew");
    // A first shard's renewals keep a lease alive with no leader ask, and never grow it.
    let mut l = PoolLedger::default();
    let held = l.ask(net(1), 1, 3, Timestamp(0), share);
    for m in 1..=10 {
        assert_eq!(l.renew(net(1), Timestamp(m * MIN)), held);
        assert_eq!(l.ask(net(2), 2, 6, Timestamp(m * MIN), share).len(), 4);
    }
    // The ledger then answers nothing for an hour (LEDGER_GAP passed): no lease expires on the gap alone.
    let back = 10 * MIN + 3_600_000;
    assert!(3_600_000 > LEDGER_GAP.as_millis() as u64);
    assert_eq!(l.renew(net(1), Timestamp(back)), held);
    assert_eq!(l.ask(net(2), 2, 6, Timestamp(back), share).len(), 4);
}

/// A lease nobody renews lapses LOAD_STALE after its last renewal even when the ledger is called rarely: the ledger
/// object's heartbeat (`beat` every `LEDGER_BEAT` while a lease is held) is what tells a quiet ledger from a down one.
/// LM-1's m21 tore down two networks at k 4 without releasing, no later call came within LEDGER_GAP of another, and
/// their 6 slots, their owner's whole share, stayed leased for 95 minutes (`lm1-memory` round
/// 10). The other side: a ledger that misses its beats (down) for an hour expires nothing on its return.
#[test]
fn a_lease_nobody_renews_lapses_however_rarely_the_ledger_is_called() {
    use graftyard_git::{LEDGER_BEAT, LEDGER_GAP, LOAD_STALE, PoolLedger};
    let share = |_| 6;
    let stale = LOAD_STALE.as_millis() as u64;
    let beat = LEDGER_BEAT.as_millis() as u64;
    assert!(2 * beat <= LEDGER_GAP.as_millis() as u64, "two beats fit in a gap");
    // m21: two networks of one owner at k 4 (3 slots each), then never renewed. The ledger's object runs only its
    // alarm, which beats while a lease is held, and one ask comes every 20 minutes from a third network of that owner.
    let mut l = PoolLedger::default();
    assert_eq!(l.ask(net(1), 1, 3, Timestamp(0), share).len(), 3);
    assert_eq!(l.ask(net(2), 1, 3, Timestamp(1), share).len(), 3);
    let mut t = 1;
    let mut at = |l: &mut PoolLedger, now: u64| {
        while let Some(next) = l.beat_due(Timestamp(t)).filter(|n| n.0 <= now) {
            t = next.0;
            l.beat(next);
        }
        t = now;
    };
    let mut freed = None;
    for m in 1..=6u64 {
        at(&mut l, m * 20 * MIN);
        let got = l.ask(net(3), 1, 6, Timestamp(m * 20 * MIN), share).len();
        if got == 6 && freed.is_none() {
            freed = Some(m * 20 * MIN);
        }
    }
    // LOAD_STALE (3 min) after the last renewal is before the first 20-minute ask, so that ask gets the whole share.
    assert_eq!(freed, Some(20 * MIN), "the torn-down networks' slots were freed by the first ask after LOAD_STALE");
    assert_eq!(l.renew(net(1), Timestamp(20 * MIN)), Vec::<u16>::new());
    // With no calls at all, the beats alone drop the lease LOAD_STALE after its last renewal, and then stop.
    let mut l = PoolLedger::default();
    l.ask(net(1), 1, 3, Timestamp(0), share);
    let mut now = Timestamp(0);
    let mut beats = 0;
    while let Some(next) = l.beat_due(now) {
        now = next;
        l.beat(now);
        beats += 1;
        assert!(beats < 100);
    }
    assert!(l.leases.is_empty());
    assert!(now.0 >= stale && now.0 < stale + beat, "lapsed at {} ms, LOAD_STALE {stale}", now.0);
    // A ledger that beats on time and then misses every beat for an hour (its object down) expires nothing on return.
    let mut l = PoolLedger::default();
    let held = l.ask(net(1), 1, 3, Timestamp(0), share);
    l.beat(Timestamp(beat));
    assert_eq!(l.renew(net(1), Timestamp(beat + 3_600_000)), held);
}

/// The leader's step from two warm sites' grants: up to 1 plus the fewer either granted, never past what it asked;
/// down only as `fanout` asked; and with either ledger unreachable, k stays where it was, up or down.
#[test]
fn an_unreachable_ledger_keeps_k() {
    use graftyard_git::{granted_k, pool_ask};
    assert_eq!(granted_k(3, 6, [Some(5), Some(2)]), 3, "the fewer grant bounds the step up");
    assert_eq!(granted_k(3, 6, [Some(5), Some(4)]), 5);
    assert_eq!(granted_k(3, 5, [Some(7), Some(7)]), 5, "never past the ask");
    assert_eq!(granted_k(5, 6, [Some(1), Some(7)]), 5, "a short grant never steps down");
    assert_eq!(granted_k(5, 4, [Some(3), Some(3)]), 4, "fanout's step down");
    for (k, want) in [(1, 2), (4, 8), (4, 3), (6, 1)] {
        for g in [[None, Some(7)], [Some(7), None], [None, None]] {
            assert_eq!(granted_k(k, want, g), k, "k {k} want {want} {g:?}");
        }
    }
    // Two ledgers that disagree: the leader asks the richer one again for the k both granted, and it shrinks to it.
    let (mut rich, mut poor) = (graftyard_git::PoolLedger::default(), graftyard_git::PoolLedger::default());
    let t = Timestamp(0);
    poor.ask(net(9), 9, 6, t, |_| 6);
    let want = 7u8;
    let g = [rich.ask(net(1), 1, want - 1, t, |_| 6).len() as u8, poor.ask(net(1), 1, want - 1, t, |_| 6).len() as u8];
    let k = granted_k(1, want, [Some(g[0]), Some(g[1])]);
    assert_eq!((g, k), ([6, 1], 2));
    assert_eq!(rich.ask(net(1), 1, k - 1, t, |_| 6).len(), 1, "the trim gives back what the k does not use");
    assert_eq!(rich.ask(net(2), 2, 6, t, |_| 6).len(), 6, "and another network gets it at once");
    // A cool network never asks a ledger; a held k renews, and a step asks for its pool shards.
    assert_eq!(pool_ask(1, None), None);
    assert_eq!(pool_ask(1, Some(2)), Some(1));
    assert_eq!(pool_ask(4, None), Some(3));
    assert_eq!(pool_ask(4, Some(3)), Some(2));
    assert_eq!(pool_ask(2, Some(1)), Some(0), "a step to 1 releases the leases");
}

/// B0-35 in memory: 500 open PRs, the base moving 10 times a minute, head pushes every 2 min per PR (4.17/s), each
/// push dispatching a `synchronize` run, 500 clients GETting their PR every 5 s (100/s), and 10 list requests a
/// second. Merge cost is M0's measured torvalds/linux spread. The budget is TEST_MERGE_BUDGET of k = 1's 4 vCPU.
#[test]
fn b0_35_lazy_test_merges_stay_within_the_budget_serve_dispatch_first_and_never_compute_for_lists() {
    let secs = 600u64;
    let mut b = MergeBudget::new(SHARD_VCPU, 0);
    let mut rng = 0x1234_5678_u64;
    let mut next = || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        rng
    };
    // ms of vCPU: p50 about 25, p99 about 300 (B0-34 on torvalds/linux).
    let cost = |r: u64| if r.is_multiple_of(100) { 300.0 } else { 5.0 + (r % 41) as f32 };
    let (mut push_ok, mut push_shed, mut get_ok, mut get_null, mut list_computes) = (0, 0, 0, 0, 0);
    let mut dispatch_wait: Vec<u64> = Vec::new();
    let mut per_sec_ms = vec![0f64; secs as usize];
    for ms in (0..secs * 1000).step_by(10) {
        let s = (ms / 1000) as usize;
        // 4.17 head pushes a second: one per 240 ms. Each push and its dispatch coalesce into one compute.
        if ms % 240 == 0 {
            let n = IssueNumber((next() % 500) as u32);
            if b.admit(MergeCause::HeadPush, n, MERGE_EST_MS, ms) {
                let c = cost(next());
                b.charge(MergeCause::HeadPush, MERGE_EST_MS, c, c);
                per_sec_ms[s] += f64::from(c);
                push_ok += 1;
                dispatch_wait.push(c as u64);
            } else {
                push_shed += 1;
            }
        }
        // 100 GETs a second.
        if ms % 10 == 0 {
            let n = IssueNumber((next() % 500) as u32);
            if b.admit(MergeCause::Get, n, MERGE_EST_MS, ms) {
                let c = cost(next());
                b.charge(MergeCause::Get, MERGE_EST_MS, c, c);
                per_sec_ms[s] += f64::from(c);
                get_ok += 1;
            } else {
                get_null += 1;
            }
        }
        if ms % 100 == 0 && b.admit(MergeCause::List, IssueNumber(0), MERGE_EST_MS, ms) {
            list_computes += 1;
        }
    }
    let used = b.stats().spent_ms / (secs as f64 * 1000.0);
    let budget = f64::from(TEST_MERGE_BUDGET * SHARD_VCPU);
    let mut per_sec: Vec<f64> = per_sec_ms.iter().map(|x| x / 1000.0).collect();
    per_sec.sort_by(f64::total_cmp);
    let p99_sec = per_sec[per_sec.len() * 99 / 100];
    dispatch_wait.sort_unstable();
    let dispatch_p99 = dispatch_wait[dispatch_wait.len() * 99 / 100];
    println!(
        "B0-35 in memory {secs} s: test-merge vCPU mean {used:.3} (p99 second {p99_sec:.3}) against a budget of \
         {budget:.2}; {push_ok} push merges, {push_shed} shed; {get_ok} GET computes, {get_null} GETs answered null; \
         {list_computes} list computes; dispatch merge p99 {dispatch_p99} ms"
    );
    assert!(used <= budget, "{used} vCPU over {budget}");
    assert_eq!(list_computes, 0);
    // The instruments B0-35 reads count what the run did.
    let st = b.stats();
    assert_eq!((st.computed, st.get_computes, st.list_computes), (push_ok + get_ok, get_ok, 0));
    assert_eq!(st.get_asked, get_ok + get_null, "every GET is counted, coalesced or not");
    // A GET inside GET_COALESCE_MS of its PR's last is refused before the budget is asked, so it is not shed.
    assert!(st.shed > push_shed && st.shed < push_shed + get_null, "shed {} of {get_null} null GETs", st.shed);
    assert_eq!(st.list_asked, (secs * 10) as u32, "every list ask is counted, none computed");
    assert_eq!(push_shed, 0, "journey 9's head pushes fit the budget, so dispatch is never shed");
    assert!(dispatch_p99 <= 10_000);
    assert!(get_null > 0 && get_ok > 0, "GETs coalesce and yield to pushes");
}

/// Over budget (M0's `b0-35-over2` shape): pushes need 0.42 vCPU of a 1 vCPU budget and GETs 10 vCPU. Every push is
/// served, GETs take what is left and are otherwise null, and use stays within the budget.
#[test]
fn over_budget_pushes_go_first_and_gets_are_null() {
    let mut b = MergeBudget::new(SHARD_VCPU, 0);
    let (mut push_ok, mut push_shed, mut get_ok, mut get_null) = (0, 0, 0, 0);
    for ms in (0..120_000u64).step_by(10) {
        if ms % 240 == 0 {
            if b.admit(MergeCause::HeadPush, IssueNumber((ms / 240 % 500) as u32), MERGE_EST_MS, ms) {
                b.charge(MergeCause::HeadPush, MERGE_EST_MS, 100.0, 100.0);
                push_ok += 1;
            } else {
                push_shed += 1;
            }
        }
        if b.admit(MergeCause::Get, IssueNumber((ms / 10 % 500) as u32), MERGE_EST_MS, ms) {
            b.charge(MergeCause::Get, MERGE_EST_MS, 100.0, 100.0);
            get_ok += 1;
        } else {
            get_null += 1;
        }
    }
    let used = b.stats().spent_ms / 120_000.0;
    println!(
        "over budget: {push_ok} push merges, {push_shed} shed, {get_ok} GET computes, {get_null} GETs null, use \
         {used:.2} of {:.2} vCPU",
        b.vcpu()
    );
    assert_eq!(push_shed, 0, "pushes are served first");
    assert!(get_null > get_ok * 10, "GETs past the budget answer null");
    assert!(used <= f64::from(b.vcpu()) * 1.01, "{used}");
}

/// The shard's `budgeted` on one `standard-2` shard object (0.25 vCPU of budget) that two networks' merges reach, from
/// two consumer isolates each: a hot-pool object serves several networks, and a leader's first attempt and the queue
/// consumer run in different isolates. Each network has its own budget, shared by every caller of it: network 1's
/// callers, 0.6 vCPU of 150 ms merges between them, are held to its 0.25, while network 2's one merge a second runs.
/// The counts B0-35 reads add up, only the merge's own time is charged, a shed merge runs no catch-up, and a list
/// field's ask is counted and never computes.
#[test]
fn the_shard_budgets_each_network_once_for_all_its_callers() {
    use std::cell::{Cell, RefCell};
    let budgets = RefCell::new(Budgets::new(instance_vcpu("standard-2").unwrap()));
    let clock = Cell::new(0u64);
    let catch_ups = Cell::new(0u32);
    let (n1, n2) = (net(1), net(2));
    let ask = |n: NetworkId, cause: MergeCause, cost: u64| {
        futures_executor::block_on(budgeted(
            &budgets,
            n,
            cause,
            IssueNumber(clock.get() as u32),
            || clock.get(),
            // The catch-up before a merge takes 20 ms and is not charged.
            || async {
                clock.set(clock.get() + 20);
                catch_ups.set(catch_ups.get() + 1);
                Ok::<(), ()>(())
            },
            || async {
                clock.set(clock.get() + cost);
                Ok((MergeAnswer::Conflict { conflict: vec![] }, None))
            },
        ))
        .unwrap()
        .is_some()
    };
    let (mut ran1, mut ran2) = (0u32, 0u32);
    for ms in (0..60_000u64).step_by(250) {
        // Network 1: two consumer isolates, each a head push every 500 ms, interleaved, at 150 ms of git each.
        clock.set(ms);
        ran1 += u32::from(ask(n1, MergeCause::HeadPush, 150));
        // Network 2: one push a second at 50 ms.
        if ms % 1000 == 0 {
            clock.set(ms + 200);
            ran2 += u32::from(ask(n2, MergeCause::HeadPush, 50));
        }
        if ms % 5000 == 0 {
            assert!(!ask(n1, MergeCause::List, 50), "a list field never computes");
        }
    }
    let (s1, s2) = (budgets.borrow().stats(n1).unwrap(), budgets.borrow().stats(n2).unwrap());
    println!("network 1 {s1:?}; network 2 {s2:?}");
    let used1 = s1.spent_ms / 60_000.0;
    assert!(used1 <= f64::from(s1.vcpu) * 1.1, "network 1's callers share one budget: {used1} of {}", s1.vcpu);
    assert!(s1.shed > 0 && ran1 < 240, "network 1 is shed past its budget");
    assert_eq!(ran2, 60, "network 2's merges never wait on network 1's budget");
    assert_eq!((s1.computed, s1.shed), (ran1, 240 - ran1));
    assert_eq!((s1.list_asked, s1.list_computes, s2.list_asked), (12, 0, 0));
    assert_eq!(s1.spent_ms, f64::from(s1.computed) * 150.0, "the merge's time is charged, not the catch-up's");
    assert_eq!(catch_ups.get(), s1.computed + s2.computed, "a shed merge wakes no catch-up");
    assert_eq!(s2.spent_ms, 60.0 * 50.0, "each merge is charged its measured time");
}

/// A compute for a list field, which nothing should run, shows in `list_computes`, and B0-35's "zero computes from
/// list fields" reads it.
#[test]
fn a_compute_charged_to_a_list_field_is_counted() {
    let mut b = MergeBudget::new(SHARD_VCPU, 0);
    b.charge(MergeCause::Get, MERGE_EST_MS, 20.0, 20.0);
    b.charge(MergeCause::List, MERGE_EST_MS, 30.0, 30.0);
    let st = b.stats();
    assert_eq!((st.computed, st.list_computes, st.spent_ms), (2, 1, 50.0));
}

// ---- the hosted wiring's pure halves: the leader's report fold and the edge's validator choice ----

const S0: SiteId = SiteId(0);
const S1: SiteId = SiteId(1);

/// The leader's `LoadReports`: k steps up only after FANOUT_UP_FOR of reports above the line from either warm site,
/// answers a k only when it differs from the committed one, ignores a site that is not warm, and counts a site whose
/// report went stale as idle.
#[test]
fn the_leader_folds_minute_reports_into_one_k_for_both_warm_sites() {
    let warm = [S0, S1];
    let mut r = graftyard_git::LoadReports::default();
    let mut k = 1u8;
    let mut commits = Vec::new();
    for m in 0..=4u64 {
        // Only site 1 is hot; site 0 reports idle.
        assert_eq!(r.report(warm, S0, load(0.1), k, Timestamp(m * MIN)), None, "the idle site never steps k");
        if let Some(next) = r.report(warm, S1, load(0.95), k, Timestamp(m * MIN + 1)) {
            commits.push((m, next));
            k = next;
        }
    }
    assert_eq!(commits, vec![(3, 2)], "one step, after 3 min above FANOUT_UP, from the hotter site");
    // A third site's report is not counted.
    assert_eq!(r.report(warm, SiteId(2), load(1.0), k, Timestamp(5 * MIN)), None);
    // Site 1 goes quiet after minute 4. Its last report counts until LOAD_STALE has passed, then as idle, so an hour
    // of idle reports from site 0 after that steps k down once.
    let quiet = 4 * MIN + 1 + graftyard_git::LOAD_STALE.as_millis() as u64;
    let mut down = None;
    for m in quiet.div_ceil(MIN)..=(quiet.div_ceil(MIN) + 61) {
        if let Some(next) = r.report(warm, S0, load(0.1), k, Timestamp(m * MIN)) {
            down = Some((m, next));
            break;
        }
    }
    let (m, next) = down.expect("k steps down after FANOUT_DOWN_FOR idle");
    assert_eq!(next, 1);
    assert!(m * MIN >= quiet + FANOUT_DOWN_FOR.as_millis() as u64, "not before an hour: {m}");
}

/// The first shard's validator: with no pool shards every push goes to the network's first shard under its M1 name;
/// with three granted slots pushes spread over all four by push id, each push id always to the same one, the pool ones
/// named `pool:<n>` for the granted slots, and no network's first shard changes with k.
#[test]
fn pushes_spread_over_the_k_shards_by_push_id() {
    use graftyard_git::{object_name, push_validator, shards_for};
    let (n, count) = (net(131171), 2u16);
    for p in 0..200u32 {
        let id = p.to_be_bytes();
        let v = push_validator(n, S0, count, &[], &id);
        assert_eq!(object_name(n, v, count), "git#0#net:131171");
    }
    let pool = [4u16, 0, 6];
    let ks = shards_for(n, S0, count, &pool);
    let mut hits = BTreeMap::<u16, u32>::new();
    for p in 0..4000u32 {
        let id = p.to_be_bytes();
        let v = push_validator(n, S0, count, &pool, &id);
        assert_eq!(v, push_validator(n, S0, count, &pool, &id), "a push id always picks one shard");
        assert!(ks.contains(&v));
        *hits.entry(v.shard).or_default() += 1;
    }
    assert_eq!(hits.len(), 4, "all four shards validate");
    assert!(hits.values().all(|h| (700..1300).contains(h)), "about even: {hits:?}");
    assert_eq!(shards_for(n, S0, count, &pool)[0], shards_for(n, S0, count, &[])[0]);
    let names: Vec<String> = ks[1..].iter().map(|s| object_name(n, *s, count)).collect();
    assert_eq!(names, ["git#0#pool:6", "git#0#pool:2", "git#0#pool:8"], "slot p is shard count + p");
}

/// A site's report of its k shards: the most loaded one's CPU, not their sum or mean, and their queues summed.
#[test]
fn a_site_reports_its_most_loaded_shard_and_the_summed_queue() {
    let l = |cpu, q| NetworkLoad { cpu_p99: cpu, queue_depth: q };
    let c = graftyard_git::combine(&[l(0.2, 3), l(0.9, 1), l(0.5, 0)]);
    assert_eq!(c, l(0.9, 4));
    assert_eq!(graftyard_git::combine(&[]), NetworkLoad::default());
}

/// A28: LM-1 round 5's shard spent 0.274 vCPU of 0.25 because each merge was admitted at its 50 ms estimate while the
/// ones before it still ran uncharged, up to five batches' merges at once. Five callers ask at once here, each merge
/// 300 ms of CPU, so admitting at the estimate would let all five run and leave the bucket 1.25 s short. The shard
/// runs one at a time, admitting each only after the one before was charged, so the bucket never goes lower than one
/// merge's cost below zero, and the merges admitted are charged their CPU, not the longer wall time they took.
#[test]
fn concurrent_merges_overrun_the_budget_by_at_most_one_merge_and_are_charged_their_cpu() {
    use futures_util::task::LocalSpawnExt;
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;
    struct Sleep(Rc<Clock>, u64);
    #[derive(Default)]
    struct Clock {
        now: Cell<u64>,
        waiting: RefCell<Vec<(u64, std::task::Waker)>>,
    }
    impl std::future::Future for Sleep {
        type Output = ();
        fn poll(self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<()> {
            if self.0.now.get() >= self.1 {
                return std::task::Poll::Ready(());
            }
            self.0.waiting.borrow_mut().push((self.1, cx.waker().clone()));
            std::task::Poll::Pending
        }
    }
    let budgets = Rc::new(RefCell::new(Budgets::new(instance_vcpu("standard-2").unwrap())));
    let clock = Rc::new(Clock::default());
    let ran = Rc::new(Cell::new(0u32));
    let mut pool = futures_executor::LocalPool::new();
    // Five batches at once, then five more two seconds later, each merge 300 ms of CPU over 900 ms of wall time.
    for (i, at) in (0..10u32).map(|i| (i, if i < 5 { 0 } else { 2_000 })) {
        let (budgets, clock, ran) = (budgets.clone(), clock.clone(), ran.clone());
        pool.spawner()
            .spawn_local(async move {
                Sleep(clock.clone(), at).await;
                let c = clock.clone();
                let r = budgeted(
                    &budgets,
                    net(1),
                    MergeCause::HeadPush,
                    IssueNumber(i),
                    || c.now.get(),
                    || async { Ok::<(), ()>(()) },
                    || async {
                        ran.set(ran.get() + 1);
                        Sleep(clock.clone(), clock.now.get() + 900).await;
                        Ok((MergeAnswer::Conflict { conflict: vec![] }, Some(300.0)))
                    },
                )
                .await;
                assert!(r.is_ok());
            })
            .unwrap();
    }
    loop {
        pool.run_until_stalled();
        let next = clock.waiting.borrow().iter().map(|(t, _)| *t).min();
        let Some(next) = next else { break };
        clock.now.set(next);
        let due: Vec<_> = clock.waiting.borrow_mut().drain(..).collect();
        due.into_iter().for_each(|(_, w)| w.wake());
    }
    let s = budgets.borrow().stats(net(1)).unwrap();
    println!("A28: {s:?}");
    // The five that arrived together were all in line at once; the next five arrived while some still were.
    assert!((5..=10).contains(&s.in_flight_max), "in line at once: {}", s.in_flight_max);
    assert_eq!(s.computed, ran.get());
    assert_eq!(s.computed + s.shed, 10, "every ask is computed or shed");
    assert!(s.computed >= 2, "a refilled bucket admits again");
    assert!(s.low_ms >= -300.0, "overrun by more than one merge: the bucket reached {} ms", s.low_ms);
    assert_eq!((s.spent_ms, s.wall_ms), (300.0 * f64::from(s.computed), 900.0 * f64::from(s.computed)), "CPU charged");
    assert_eq!(s.max_ms, 300.0);
    // merge-budget-4: each admitted merge's wait from its ask to its start. The first of a burst waits 0; the others
    // wait their turn behind merges of 900 ms each, so the longest is a whole number of 900 ms turns, and none is near
    // GET_COALESCE_MS.
    let waits: u32 = s.wait_hist.iter().sum();
    assert_eq!(waits, s.computed, "one wait per merge that ran");
    assert_eq!(s.wait_ms(0.0), Some(10.0), "the first merge of a burst does not wait");
    assert!(s.wait_max_ms >= 900.0 && s.wait_max_ms % 900.0 == 0.0, "waited whole turns: {}", s.wait_max_ms);
    assert!(s.wait_ms(0.99).unwrap() >= 900.0);
    assert_eq!(s.waits_over(graftyard_git::prsync::GET_COALESCE_MS), 0);
    // The bound needs a push refused on a balance below zero: a bucket 10 ms short admits nothing until it refills, so
    // the next merge cannot start 50 ms down and end a whole merge past it.
    let mut b = MergeBudget::new(instance_vcpu("standard-2").unwrap(), 0);
    assert!(b.admit(MergeCause::HeadPush, IssueNumber(1), MERGE_EST_MS, 0));
    b.charge(MergeCause::HeadPush, MERGE_EST_MS, 260.0, 260.0);
    assert_eq!(b.stats().low_ms, -10.0);
    assert!(!b.admit(MergeCause::HeadPush, IssueNumber(2), MERGE_EST_MS, 0), "a push past zero waits for the refill");
    assert!(b.admit(MergeCause::HeadPush, IssueNumber(2), MERGE_EST_MS, 100), "and runs once the bucket is positive");
}

/// A30 and A29's asks: a single-PR GET computes once per PR per GET_COALESCE_MS and is counted either way, a GET the
/// budget shed gives its slot back so the next GET may compute, and a list field's ask is counted and never computes.
#[test]
fn gets_coalesce_per_pr_a_shed_get_frees_its_slot_and_list_asks_are_counted() {
    use graftyard_git::prsync::GET_COALESCE_MS;
    let mut b = MergeBudget::new(instance_vcpu("standard-2").unwrap(), 0);
    let get = |b: &mut MergeBudget, n: u32, at: u64| {
        let ok = b.admit(MergeCause::Get, IssueNumber(n), MERGE_EST_MS, at);
        if ok {
            b.charge(MergeCause::Get, MERGE_EST_MS, 20.0, 20.0);
        }
        ok
    };
    assert!(get(&mut b, 1, 0), "a full bucket computes a GET");
    assert!(!get(&mut b, 1, 1_000), "the same PR inside GET_COALESCE_MS does not compute");
    assert!(get(&mut b, 2, 1_000), "another PR does");
    assert!(get(&mut b, 1, GET_COALESCE_MS), "the same PR after GET_COALESCE_MS computes again");
    let s = b.stats();
    assert_eq!((s.get_asked, s.get_computes, s.shed), (4, 3, 0), "a coalesced GET is asked, not shed");
    // A push takes the bucket to zero: a GET is shed, and its PR's next GET, once the bucket refilled, computes.
    let t = GET_COALESCE_MS + 1;
    assert!(b.admit(MergeCause::HeadPush, IssueNumber(9), MERGE_EST_MS, t));
    b.charge(MergeCause::HeadPush, MERGE_EST_MS, 400.0, 400.0);
    assert!(!get(&mut b, 3, t), "a GET leaves the reserve to pushes");
    assert_eq!(b.stats().shed, 1);
    assert!(get(&mut b, 3, t + 3_000), "a shed GET held no coalescing slot");
    // List asks: each counted, none computed, none shed.
    for i in 0..7 {
        assert!(!b.admit(MergeCause::List, IssueNumber(i), MERGE_EST_MS, t + 4_000));
    }
    let s = b.stats();
    assert_eq!((s.list_asked, s.list_computes, s.shed, s.get_computes), (7, 0, 1, 4));
}

/// Item 4 of merge-budget-3: a shard object resets about 90 s after its last request, so its counts are stored and a
/// restarted object goes on from them. The restored budget keeps every count and `since`, refills full at the new
/// time, and takes the shard's vCPU now; a network the object already budgets keeps its own, newer counts; and
/// `in_flight_max` counts merges in line at once, 1 when they arrive one after another.
#[test]
fn a_restarted_shard_goes_on_from_its_stored_counts_and_counts_merges_in_line() {
    use std::cell::RefCell;
    let ok = || async { Ok::<_, ()>((MergeAnswer::Conflict { conflict: vec![] }, Some(30.0))) };
    let run = |b: &RefCell<Budgets>, n: u32, at: u64, cause: MergeCause| {
        futures_executor::block_on(budgeted(b, net(7), cause, IssueNumber(n), || at, || async { Ok(()) }, ok))
    };
    let old = RefCell::new(Budgets::new(instance_vcpu("standard-2").unwrap()));
    for (i, at) in [(1, 1_000), (2, 1_200), (3, 1_400)] {
        assert!(run(&old, i, at, MergeCause::HeadPush).unwrap().is_some());
    }
    run(&old, 9, 1_500, MergeCause::List).unwrap();
    let stored = old.borrow().stats(net(7)).unwrap();
    assert_eq!((stored.computed, stored.list_asked, stored.since, stored.in_flight_max), (3, 1, 1_000, 1));
    // The object resets; the next one, on a `standard-4`, restores what was stored.
    let new = RefCell::new(Budgets::new(instance_vcpu("standard-4").unwrap()));
    new.borrow_mut().restore(net(7), stored.clone(), 200_000);
    let back = new.borrow().stats(net(7)).unwrap();
    assert_eq!((back.computed, back.spent_ms, back.list_asked, back.since), (3, 90.0, 1, 1_000), "counts go on");
    assert_eq!(back.vcpu, 1.0, "the budget is the shard's now");
    assert!(run(&new, 4, 200_000, MergeCause::HeadPush).unwrap().is_some(), "full at restart");
    assert_eq!(new.borrow().stats(net(7)).unwrap().computed, 4);
    // A second restore of older counts does not undo what this object counted since.
    new.borrow_mut().restore(net(7), stored, 200_100);
    assert_eq!(new.borrow().stats(net(7)).unwrap().computed, 4);
    // Another network's stored counts stay its own.
    assert_eq!(new.borrow().stats(net(8)), None);
    // Waits are counts too: four merges, none waiting, all in the first bucket, and they survive the restore.
    let s = new.borrow().stats(net(7)).unwrap();
    assert_eq!((s.wait_hist.first().copied(), s.wait_hist.iter().sum::<u32>()), (Some(4), 4));
}

/// The wait histogram's quantiles and the count past GET_COALESCE_MS, from known waits: an edge value lands in its
/// own bucket, one past the last edge in the overflow bucket, and a quantile answers its bucket's upper edge (or the
/// largest wait, if that is lower).
#[test]
fn merge_waits_are_counted_in_buckets_and_read_against_the_coalescing_window() {
    use graftyard_git::prsync::{BudgetStats, GET_COALESCE_MS};
    let mut b = MergeBudget::new(instance_vcpu("standard-2").unwrap(), 0);
    let empty = b.stats();
    assert_eq!((empty.wait_ms(0.5), empty.waits_over(GET_COALESCE_MS)), (None, 0));
    let s: BudgetStats = serde_json::from_value(serde_json::json!({ "waitHist": [], "waitMaxMs": 0.0 })).unwrap();
    assert_eq!(s.wait_ms(0.99), None, "an older shard's stats read as no waits");
    // 97 merges at 3 ms, one at exactly GET_COALESCE_MS, two past it.
    let waits = [3u64; 97].into_iter().chain([GET_COALESCE_MS, GET_COALESCE_MS + 1, 200_000]);
    waits.for_each(|w| b.record_wait(w));
    let s = b.stats();
    assert_eq!(s.wait_hist.iter().sum::<u32>(), 100);
    assert_eq!(s.waits_over(GET_COALESCE_MS), 2, "exactly GET_COALESCE_MS is not past it");
    assert_eq!(s.wait_ms(0.5), Some(10.0));
    assert_eq!(s.wait_ms(0.98), Some(GET_COALESCE_MS as f32));
    assert_eq!(s.wait_ms(0.99), Some(60_000.0));
    assert_eq!(s.wait_ms(1.0), Some(200_000.0));
    assert_eq!(s.wait_max_ms, 200_000.0);
    // Through `budgeted`: a cold shard's 3 s catch-up is part of the wait, which runs from the ask to the merge's start.
    let budgets = std::cell::RefCell::new(Budgets::new(instance_vcpu("standard-2").unwrap()));
    let now = std::cell::Cell::new(1_000u64);
    let r = futures_executor::block_on(budgeted(
        &budgets,
        net(3),
        MergeCause::HeadPush,
        IssueNumber(1),
        || now.get(),
        || async {
            now.set(now.get() + 3_000);
            Ok::<(), ()>(())
        },
        || async { Ok((MergeAnswer::Conflict { conflict: vec![] }, Some(20.0))) },
    ));
    assert!(r.unwrap().is_some());
    let s = budgets.borrow().stats(net(3)).unwrap();
    assert_eq!((s.wait_max_ms, s.wait_ms(1.0)), (3_000.0, Some(3_000.0)), "the catch-up is waited");
}
