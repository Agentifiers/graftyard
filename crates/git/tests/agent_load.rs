//! AM1-WP1's in-memory rerun of B0-34 (`fanout` from `NetworkLoad` with the per-owner hot pool) and B0-35 (lazy test
//! merges under `TEST_MERGE_BUDGET`), on the pure halves the hosts call (`build/BUILD-PLAN.md`, M0-WP9). The git CPU
//! is a model: a shard's CPU is the network's offered vCPU over its k shards' 4 vCPU, and merge costs follow M0's
//! torvalds/linux measurements [measured, `tests/m0/results/m0-wp9.md`, B0-34: p50 5 to 45 ms, p99 at most 338 ms].

use graftyard_git::issues::IssueNumber;
use graftyard_git::prsync::{MERGE_EST_MS, MergeBudget, MergeCause, SHARD_VCPU, TEST_MERGE_BUDGET};
use graftyard_git::{
    FANOUT_DOWN_FOR, FANOUT_MAX, FANOUT_UP_FOR, Fanout, HOT_POOL, NetworkLoad, clone_cache_buckets, fanout, pool_grants,
};
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

/// B0-34 in memory: the monorepo under journey 9 (2.68 vCPU on one network [measured, B0-34]) and a tip moving 10
/// times a minute, then one warm site killed (the other site's load then carries both), then a second owner's network
/// turning hot, then a hostile owner making three of its networks hot. `fanout` runs each minute on the two warm
/// sites' `NetworkLoad`, and pool grants are held to each owner's share. The run checks k at most FANOUT_MAX, the
/// monorepo's k never reduced by another owner, every owner within its share, and no call but these pure ones.
#[test]
fn b0_34_k_is_set_in_band_and_the_hostile_owner_is_held_to_its_pool_share() {
    struct N {
        id: NetworkId,
        owner: u64,
        vcpu: f32,
        k: u8,
        f: Fanout,
    }
    // The monorepo at 20 vCPU, past journey 9's 2.68, so k climbs to its owner's share.
    let mut nets = vec![N { id: net(1), owner: 1, vcpu: 20.0, k: 1, f: Fanout::default() }];
    let share = |o: u64| if o == 1 { 6 } else { 2 };
    let mut mono_k = Vec::new();
    let mut max_k = 0;
    let mut denied = 0;
    let mut site_dead = false;
    for m in 0..=180u64 {
        if m == 40 {
            site_dead = true; // one warm site killed: the other already holds k (the larger load sizes both)
        }
        if m == 60 {
            nets.push(N { id: net(2), owner: 2, vcpu: 5.0, k: 1, f: Fanout::default() });
        }
        if m == 100 {
            for i in 0..3 {
                nets.push(N { id: net(30 + i), owner: 3, vcpu: 30.0, k: 1, f: Fanout::default() });
            }
        }
        let now = Timestamp(m * MIN);
        let mut asks = Vec::new();
        for n in nets.iter_mut() {
            let cpu = (n.vcpu / (f32::from(n.k) * SHARD_VCPU)).min(1.0);
            let a = load(cpu);
            let b = if site_dead { load(0.0) } else { load(cpu * 0.9) };
            let want = fanout([&a, &b], n.k, now, &mut n.f);
            asks.push((n.id, n.owner, n.k - 1, want - 1));
        }
        let g = pool_grants(&asks, share);
        for ((n, got), ask) in nets.iter_mut().zip(&g).zip(&asks) {
            if *got < ask.3 {
                denied += 1;
            }
            n.k = 1 + got;
            max_k = max_k.max(n.k);
        }
        let mut by_owner = std::collections::BTreeMap::<u64, u16>::new();
        for n in &nets {
            *by_owner.entry(n.owner).or_default() += u16::from(n.k - 1);
        }
        assert!(by_owner.iter().all(|(o, v)| *v <= share(*o)), "an owner past its share at {m} min: {by_owner:?}");
        assert!(by_owner.values().sum::<u16>() <= HOT_POOL);
        mono_k.push(nets[0].k);
    }
    // Any step down of the monorepo while its load stays above FANOUT_DOWN would be another owner's doing.
    let reduced = mono_k.windows(2).filter(|w| w[1] < w[0]).count();
    println!(
        "B0-34 in memory: monorepo k by 10 min {:?}; max k {max_k}; {denied} network-minutes held below the asked k by the pool or an owner's share; monorepo k \
         reduced after the hostile owner {reduced} times",
        mono_k.iter().step_by(10).collect::<Vec<_>>()
    );
    assert!(max_k <= FANOUT_MAX);
    assert!(mono_k[30] > 1, "the monorepo stepped up in band");
    assert_eq!(*mono_k.last().unwrap(), 7, "the monorepo reached 1 + its owner's share of 6");
    assert_eq!(reduced, 0, "another owner never reduced the monorepo's k");
}

/// B0-35 in memory: 500 open PRs, the base moving 10 times a minute, head pushes every 2 min per PR (4.17/s), each
/// push dispatching a `synchronize` run, 500 clients GETting their PR every 5 s (100/s), and 10 list requests a
/// second. Merge cost is M0's measured torvalds/linux spread. The budget is TEST_MERGE_BUDGET of k = 1's 4 vCPU.
#[test]
fn b0_35_lazy_test_merges_stay_within_the_budget_serve_dispatch_first_and_never_compute_for_lists() {
    let secs = 600u64;
    let mut b = MergeBudget::new(1, 0);
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
                b.charge(MERGE_EST_MS, c);
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
                b.charge(MERGE_EST_MS, c);
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
    let used = b.spent_ms / (secs as f64 * 1000.0);
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
    assert_eq!(push_shed, 0, "journey 9's head pushes fit the budget, so dispatch is never shed");
    assert!(dispatch_p99 <= 10_000);
    assert!(get_null > 0 && get_ok > 0, "GETs coalesce and yield to pushes");
}

/// Over budget (M0's `b0-35-over2` shape): pushes need 0.42 vCPU of a 1 vCPU budget and GETs 10 vCPU. Every push is
/// served, GETs take what is left and are otherwise null, and use stays within the budget.
#[test]
fn over_budget_pushes_go_first_and_gets_are_null() {
    let mut b = MergeBudget::new(1, 0);
    let (mut push_ok, mut push_shed, mut get_ok, mut get_null) = (0, 0, 0, 0);
    for ms in (0..120_000u64).step_by(10) {
        if ms % 240 == 0 {
            if b.admit(MergeCause::HeadPush, IssueNumber((ms / 240 % 500) as u32), MERGE_EST_MS, ms) {
                b.charge(MERGE_EST_MS, 100.0);
                push_ok += 1;
            } else {
                push_shed += 1;
            }
        }
        if b.admit(MergeCause::Get, IssueNumber((ms / 10 % 500) as u32), MERGE_EST_MS, ms) {
            b.charge(MERGE_EST_MS, 100.0);
            get_ok += 1;
        } else {
            get_null += 1;
        }
    }
    let used = b.spent_ms / 120_000.0;
    println!(
        "over budget: {push_ok} push merges, {push_shed} shed, {get_ok} GET computes, {get_null} GETs null, use \
         {used:.2} of {:.2} vCPU",
        b.vcpu()
    );
    assert_eq!(push_shed, 0, "pushes are served first");
    assert!(get_null > get_ok * 10, "GETs past the budget answer null");
    assert!(used <= f64::from(b.vcpu()) * 1.01, "{used}");
}
