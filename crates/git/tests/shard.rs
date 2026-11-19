//! `shards_for`, the start cap, the region check, the inactivity renewal, and the image map.

use std::collections::BTreeMap;

use graftyard_git::*;
use graftyard_types::{DbId, NetworkId, Region, SiteId, Timestamp};

const H: u64 = 3_600_000;

fn net(n: i64) -> NetworkId {
    NetworkId(DbId(n))
}

#[test]
fn shards_counts_are_the_b0_14_values() {
    assert_eq!(SHARDS, [2, 5, 25]);
    assert_eq!(HOT_POOL, 7);
}

#[test]
fn first_shard_is_in_count_and_the_rest_are_the_granted_pool_slots() {
    for count in SHARDS {
        for n in 0..500 {
            for k in 1..=FANOUT_MAX {
                let slots: Vec<u16> = (0..u16::from(k) - 1).map(|i| (i * 3 + n as u16) % HOT_POOL).collect();
                let s = shards_for(net(n), SiteId(1), count, &slots);
                assert_eq!(s.len(), k as usize);
                assert!(s[0].shard < count);
                // Slot p is shard count + p, in the order the ledger granted them.
                assert_eq!(s[1..].iter().map(|x| x.shard - count).collect::<Vec<_>>(), slots);
                assert!(s.iter().all(|x| x.site == SiteId(1) && may_start(x.shard, count)));
                // The first shard is the network's rendezvous one whatever the pool holds.
                assert_eq!(s[0], shards_for(net(n), SiteId(1), count, &[])[0]);
            }
        }
    }
    // A slot outside the pool or named twice is skipped, and at most FANOUT_MAX shards serve.
    assert_eq!(shards_for(net(1), SiteId(0), 5, &[HOT_POOL, 2, 2]).len(), 2);
    assert_eq!(shards_for(net(1), SiteId(0), 5, &[0, 1, 2, 3, 4, 5, 6, 0]).len(), FANOUT_MAX as usize);
}

#[test]
fn raising_k_moves_no_network_and_keeps_the_earlier_shards() {
    for n in 0..300 {
        let base = shards_for(net(n), SiteId(0), 25, &[])[0];
        let pool: Vec<u16> = (0..HOT_POOL).collect();
        for k in 2..=FANOUT_MAX {
            let lo = shards_for(net(n), SiteId(0), 25, &pool[..usize::from(k) - 2]);
            let hi = shards_for(net(n), SiteId(0), 25, &pool[..usize::from(k) - 1]);
            assert_eq!(hi[0], base);
            assert_eq!(&hi[..lo.len()], &lo[..], "k-1 shards are a prefix of k");
        }
    }
}

#[test]
fn rendezvous_spreads_networks_and_a_new_count_moves_only_about_one_in_n() {
    let mut load: BTreeMap<u16, u32> = BTreeMap::new();
    for n in 0..25_000 {
        *load.entry(shards_for(net(n), SiteId(2), 25, &[])[0].shard).or_default() += 1;
    }
    assert_eq!(load.len(), 25);
    let (min, max) = (load.values().min().unwrap(), load.values().max().unwrap());
    assert!(*min > 850 && *max < 1150, "{min}..{max}");
    // From 25 to 26 shards, about 1/26 of networks move, and only onto the new shard.
    let mut moved = 0;
    for n in 0..25_000 {
        let a = shards_for(net(n), SiteId(2), 25, &[])[0];
        let b = shards_for(net(n), SiteId(2), 26, &[])[0];
        if a != b {
            assert_eq!(b.shard, 25);
            moved += 1;
        }
    }
    assert!((800..1200).contains(&moved), "{moved}");
}

#[test]
fn warm_sites_start_at_home_and_pick_another_site() {
    let sites = [SiteId(0), SiteId(1), SiteId(2)];
    let mut seconds: BTreeMap<SiteId, u32> = BTreeMap::new();
    for n in 0..3000 {
        let [home, next] = warm_sites(net(n), SiteId(0), &sites);
        assert_eq!(home, SiteId(0));
        assert_ne!(next, home);
        *seconds.entry(next).or_default() += 1;
    }
    assert_eq!(seconds.len(), 2, "both other sites are used");
    assert!(seconds.values().all(|v| *v > 1300));
}

#[test]
fn validator_is_stable_for_a_push_and_ignores_shards_not_caught_up() {
    let k = shards_for(net(9), SiteId(0), 5, &[0, 1, 2]);
    let v = validator(b"push-1", &k).unwrap();
    assert_eq!(validator(b"push-1", &k), Some(v));
    let without: Vec<GitCacheShard> = k.iter().copied().filter(|s| *s != v).collect();
    assert_ne!(validator(b"push-1", &without), Some(v));
    assert_eq!(validator(b"push-1", &[]), None);
}

#[test]
fn start_cap_is_count_plus_hot_pool() {
    assert!(may_start(0, 2));
    assert!(may_start(8, 2));
    assert!(!may_start(9, 2));
    assert!(may_start(31, 25));
    assert!(!may_start(32, 25));
}

#[test]
fn region_check_replaces_three_times_then_alarms() {
    assert_eq!(region_check(Region::Weur, Some("WEUR"), 0), RegionCheck::Keep);
    assert_eq!(region_check(Region::Weur, Some("ENAM"), 0), RegionCheck::Replace);
    assert_eq!(region_check(Region::Weur, Some("ENAM"), 2), RegionCheck::Replace);
    assert_eq!(region_check(Region::Weur, Some("ENAM"), 3), RegionCheck::Alarm);
    assert_eq!(region_check(Region::Weur, None, 0), RegionCheck::Replace, "no variable is out of region");
    assert_eq!(region_check(Region::Weur, Some("weur"), 0), RegionCheck::Replace, "the variable is upper case");
    // An Oc site accepts OC and APAC, an Apac site only APAC.
    assert_eq!(region_check(Region::Oc, Some("APAC"), 0), RegionCheck::Keep);
    assert_eq!(region_check(Region::Oc, Some("OC"), 0), RegionCheck::Keep);
    assert_eq!(region_check(Region::Apac, Some("OC"), 0), RegionCheck::Replace);
    // Every region keeps a container that reports its own region.
    for r in [Region::Enam, Region::Wnam, Region::Weur, Region::Eeur, Region::Apac, Region::Oc] {
        assert_eq!(region_check(r, Some(&r.hint().to_uppercase()), 0), RegionCheck::Keep, "{r:?}");
    }
}

#[test]
fn alarm_renews_hourly_until_a_day_after_the_last_request() {
    assert_eq!(INACTIVITY.as_secs(), 6 * 3600);
    let last = Timestamp(1_000 * H);
    // Walk the alarm from the last request: every fire renews, the gap never passes the 6 h timeout, and it stops 24 h
    // after the last request.
    let mut now = last;
    let mut fires = 0;
    while let Renewal::Renew { next } = renewal(now, last) {
        assert!(next.0 > now.0 && next.0 - now.0 <= INACTIVITY.as_millis() as u64);
        assert!(next.0 - now.0 <= RENEW_EVERY.as_millis() as u64);
        now = next;
        fires += 1;
        assert!(fires < 100);
    }
    assert_eq!(fires, 24);
    assert_eq!(now, Timestamp(last.0 + WARM_FOR.as_millis() as u64));
    // The container then stops at most 6 h later, 30 h after the last request at worst; a request in between restarts
    // the walk.
    assert_eq!(
        renewal(Timestamp(last.0 + 23 * H), Timestamp(last.0 + 20 * H)),
        Renewal::Renew { next: Timestamp(last.0 + 24 * H) }
    );
    assert_eq!(renewal(Timestamp(last.0 + 25 * H), last), Renewal::Lapse);
    // Alarms may fire late. A late one never renews past 24 h after the last request.
    assert_eq!(renewal(Timestamp(last.0 + 23 * H + H / 2), last), Renewal::Renew { next: Timestamp(last.0 + 24 * H) });
}

/// A client's request writes `last_request` and the alarm only when none is stored or the stored one is TOUCH_EVERY
/// old, so a burst of requests writes once a minute, not once each (`touch_due`; B0-32 m3fr8's network shard took 434
/// in about 1 s). The day of warmth then runs from at most a minute before the last request.
#[test]
fn a_shard_records_its_last_request_at_most_once_a_minute() {
    const M: u64 = 60_000;
    assert_eq!(TOUCH_EVERY.as_millis() as u64, M);
    assert!(touch_due(Timestamp(5 * H), None), "a shard with no request stored records the first");
    // A burst: 434 requests across one second after a write record nothing more.
    let mut last = Timestamp(5 * H);
    let mut writes = 0;
    for k in 0..434u64 {
        let now = Timestamp(5 * H + k * 1_000 / 434);
        if touch_due(now, Some(last)) {
            last = now;
            writes += 1;
        }
    }
    assert_eq!(writes, 0, "every request of the burst came within a minute of the stored one");
    assert!(!touch_due(Timestamp(5 * H + M - 1), Some(Timestamp(5 * H))), "1 ms short of a minute");
    assert!(touch_due(Timestamp(5 * H + M), Some(Timestamp(5 * H))), "a minute on, the next request records");
    // A clock behind the stored stamp records nothing and does not wrap.
    assert!(!touch_due(Timestamp(5 * H - 1), Some(Timestamp(5 * H))));
    // Requests every 30 s for an hour then none: the stored stamp is within a minute of the last request, so the
    // shard lapses at most a minute before a day after it.
    let mut stored = None;
    for k in 0..=120u64 {
        let now = Timestamp(k * 30_000);
        if touch_due(now, stored) {
            stored = Some(now);
        }
    }
    let (end, stored) = (Timestamp(H), stored.unwrap());
    assert!(end.0 - stored.0 < M);
    assert_eq!(renewal(Timestamp(stored.0 + 24 * H), stored), Renewal::Lapse);
    assert!(matches!(renewal(Timestamp(end.0 + 23 * H), stored), Renewal::Renew { .. }));
}

/// A new leadership's warm catch-up starts no container and keeps none warm: on `graft-m2b`, groups with no client
/// re-elected every hour or so, and each `warm` started its network's shard, which `WARM_FOR` then held for a day
/// (`tests/am1/results/idle-shards.md`). A client's call still starts a stopped shard and keeps it warm.
#[test]
fn only_a_clients_call_starts_a_shard_or_keeps_it_warm() {
    assert_eq!(serve(Caller::Warm, false), Serve::Skip);
    assert_eq!(serve(Caller::Warm, true), Serve::Run { touch: false });
    assert_eq!(serve(Caller::Client, false), Serve::Run { touch: true });
    assert_eq!(serve(Caller::Client, true), Serve::Run { touch: true });
    // The leader's warm of a pool slot its network holds starts a stopped one (a pool shard nobody started never
    // becomes warm) but is not a request: the ledger tears the slot down when the lease goes.
    assert_eq!(serve(Caller::PoolWarm, false), Serve::Run { touch: false });
    assert_eq!(serve(Caller::PoolWarm, true), Serve::Run { touch: false });
    // Hourly warms on a running shard, with no client after hour 0: it lapses a day after the client, as it would
    // with no warms at all.
    let client = Timestamp(0);
    let mut last = client;
    for h in 1..=30 {
        if serve(Caller::Warm, true) == (Serve::Run { touch: true }) {
            last = Timestamp(h * H);
        }
    }
    assert_eq!(renewal(Timestamp(25 * H), last), Renewal::Lapse);
}

/// The caller a shard call names with `warm`: promote-ahead's `/promote`, `/validate` and `/idx` carry `warm=1`, as a
/// new leadership's `/catchup` does, so none starts a stopped shard; only `/catchup?warm=pool` is the pool warm, and a
/// call without `warm` is a client's (`tests/am1/results/lm1-memory.md`, round 28).
#[test]
fn a_warm_query_on_any_route_is_a_warm_call_and_pool_only_on_catchup() {
    for path in ["/catchup", "/promote", "/validate", "/idx"] {
        assert_eq!(caller(path, Some("1")), Caller::Warm, "{path}");
        assert_eq!(caller(path, None), Caller::Client, "{path}");
        assert_eq!(serve(caller(path, Some("1")), false), Serve::Skip, "{path}");
    }
    assert_eq!(caller("/catchup", Some("pool")), Caller::PoolWarm);
    assert_eq!(caller("/promote", Some("pool")), Caller::Warm);
}

/// A due range's promotion (`warm=start`) starts a stopped shard but records no request, and on a shard no client used
/// in `WARM_FOR` it sets `PROMOTE_LAPSE`, not the 6 h a client's start sets: as a client it held each of 131330's five
/// starts on idle `graft-m2b` a day (`tests/am1/results/lm1-memory.md`, round 34). A client within `WARM_FOR` keeps it.
#[test]
fn a_due_range_promotion_starts_a_shard_and_lets_a_quiet_one_lapse() {
    for path in ["/promote", "/validate", "/idx"] {
        assert_eq!(caller(path, Some("start")), Caller::Start, "{path}");
        assert_eq!(serve(Caller::Start, false), Serve::Run { touch: false }, "it starts a stopped shard");
        assert_eq!(serve(Caller::Start, true), Serve::Run { touch: false });
    }
    let now = Timestamp(100 * H);
    assert_eq!(promote_lapse(Caller::Start, now, None), Some(PROMOTE_LAPSE), "never a client");
    assert_eq!(promote_lapse(Caller::Start, now, Some(Timestamp(now.0 - 24 * H))), Some(PROMOTE_LAPSE));
    assert_eq!(promote_lapse(Caller::Start, now, Some(Timestamp(now.0 - 23 * H))), None, "a client 23 h ago");
    for c in [Caller::Client, Caller::Warm, Caller::PoolWarm] {
        assert_eq!(promote_lapse(c, now, None), None, "{c:?}");
    }
    assert!(PROMOTE_LAPSE < RENEW_EVERY && PROMOTE_LAPSE < INACTIVITY);
}

#[test]
fn container_day_covers_10_50_100_percent_in_rendezvous_order_and_rollback_wins() {
    let count = 25;
    let all: Vec<GitCacheShard> = (0..count + HOT_POOL).map(|s| GitCacheShard { site: SiteId(1), shard: s }).collect();
    let on = |step: Option<u8>, back: bool| {
        all.iter().filter(|s| image_for(**s, count, step, back) == ImageSlot::Git).count()
    };
    assert_eq!(on(None, false), 0);
    assert_eq!(on(Some(0), false), 4, "ceil(10% of 32)");
    assert_eq!(on(Some(1), false), 16);
    assert_eq!(on(Some(2), false), 32);
    assert_eq!(on(Some(2), true), 0);
    // Each step covers every shard of the step before.
    for s in &all {
        if image_for(*s, count, Some(0), false) == ImageSlot::Git {
            assert_eq!(image_for(*s, count, Some(1), false), ImageSlot::Git);
        }
    }
    assert_eq!(ImageSlot::GitPrev.name(), "git_prev");
}

fn repo(n: i64, mb: u64, used: u64) -> DiskRepo {
    DiskRepo {
        network: net(n),
        bytes: mb << 20,
        used: Timestamp(used),
        pack_bytes: 0,
        fetched: None,
        pushed: None,
        caught_up: None,
    }
}

#[test]
fn evict_plan_frees_least_recently_used_until_under_budget() {
    let disk = 1000 << 20; // budget is 800 MiB
    let repos = [repo(1, 300, 10), repo(2, 300, 30), repo(3, 300, 20)];
    // 900 MiB on disk: the oldest (1) goes, and 600 MiB fits.
    assert_eq!(evict_plan(&repos, disk, 0, net(9)), [net(1)]);
    // 300 MiB more coming: 1 and then 3, the next oldest.
    assert_eq!(evict_plan(&repos, disk, 300 << 20, net(9)), [net(1), net(3)]);
    // Under budget: nothing.
    assert_eq!(evict_plan(&repos[..2], disk, 0, net(9)), Vec::<NetworkId>::new());
    // Exactly at budget is under it.
    assert_eq!(evict_plan(&[repo(1, 500, 1), repo(2, 300, 2)], disk, 0, net(9)), Vec::<NetworkId>::new());
}

#[test]
fn evict_plan_never_evicts_the_network_being_served() {
    let disk = 1000 << 20;
    let repos = [repo(1, 300, 10), repo(2, 300, 30), repo(3, 300, 20)];
    // 1 is the oldest but is the one this request is for: 3 goes instead.
    assert_eq!(evict_plan(&repos, disk, 0, net(1)), [net(3)]);
    // A repository larger than the whole budget evicts every other and stays.
    let big = [repo(1, 2000, 99), repo(2, 10, 1), repo(3, 10, 2)];
    assert_eq!(evict_plan(&big, disk, 0, net(1)), [net(2), net(3)]);
    // An empty disk listing, or only the kept one, evicts nothing.
    assert!(evict_plan(&[], disk, 5000 << 20, net(1)).is_empty());
    assert!(evict_plan(&big[..1], disk, 0, net(1)).is_empty());
}

#[test]
fn evict_plan_breaks_ties_by_network_and_counts_the_incoming_rehydrate() {
    let disk = 1000 << 20;
    let tie = [repo(5, 400, 7), repo(4, 400, 7), repo(6, 100, 8)];
    assert_eq!(evict_plan(&tie, disk, 0, net(9)), [net(4)]);
    // A rehydrate of 750 MiB into a disk holding 900 MiB: everything not kept must go.
    assert_eq!(evict_plan(&tie, disk, 750 << 20, net(6)), [net(4), net(5)]);
}

/// The archive's promotion tries the leader's own site's shard first, even when the record lists it last or it is not
/// the home, then the other Full sites in record order, and never a Witness, which has no git cache.
#[test]
fn a_promotion_tries_its_own_site_then_the_other_full_sites() {
    use graftyard_types::{PlacementName, PlacementRecord};
    let rec = |name, sites: &[u8], home: u8| PlacementRecord {
        name,
        sites: sites.iter().map(|&s| SiteId(s)).collect(),
        home: SiteId(home),
        generation: 1,
        moving_from: None,
        at: Timestamp(1),
    };
    let ids = |v: Vec<SiteId>| v.into_iter().map(|s| s.0).collect::<Vec<_>>();
    assert_eq!(ids(promote_sites(&rec(PlacementName::Tri, &[0, 1, 2], 0), SiteId(2))), [2, 0, 1]);
    assert_eq!(ids(promote_sites(&rec(PlacementName::Tri, &[0, 1, 2], 0), SiteId(0))), [0, 1, 2]);
    // Us2w: the third slot is the Witness, site 1 here; a leader on site 4 tries 4 then 3.
    assert_eq!(ids(promote_sites(&rec(PlacementName::Us2w, &[3, 4, 1], 3), SiteId(4))), [4, 3]);
    assert_eq!(ids(promote_sites(&rec(PlacementName::Solo, &[5], 5), SiteId(5))), [5]);
    // A shard's warming, a pack it never ingested, or a failed call moves on; the packs' own 422 does not.
    assert!(promote_moves_on(503) && promote_moves_on(409) && promote_moves_on(0) && promote_moves_on(500));
    assert!(!promote_moves_on(422));
}

/// A shard answers a failed validate 422 only for the container's own 4xx, the pack's or the request's answer, the
/// same on every shard. A container 5xx or no answer at all (its catch-up failed, a kill broke the call) is 503, which
/// the edge and the archive move past to the next site's shard rather than refusing the push.
#[test]
fn a_shard_answers_422_only_for_the_containers_own_refusal() {
    assert_eq!(validate_status(Some(422)), 422);
    assert_eq!(validate_status(Some(409)), 422);
    assert_eq!(validate_status(Some(400)), 422);
    assert_eq!(validate_status(Some(500)), 503);
    assert_eq!(validate_status(Some(503)), 503);
    assert_eq!(validate_status(None), 503);
    assert!(promote_moves_on(validate_status(None)) && !promote_moves_on(validate_status(Some(422))));
}

/// A shard answers `warming` at once only when the caller has another shard to try and a rehydrate runs here, or when
/// it is a cold pool shard asked to validate a forwarded push. A caller's last try waits even on a rehydrating shard,
/// and a shard that is only catching up (no rehydrate) lets every caller wait.
#[test]
fn a_shard_answers_warming_at_once_only_to_a_caller_with_somewhere_else_to_go() {
    assert!(warming_now(true, true, false));
    assert!(!warming_now(false, true, false), "the last try waits");
    assert!(!warming_now(true, false, false), "a catch-up, not a rehydrate, is short");
    assert!(warming_now(false, false, true) && warming_now(true, false, true));
    assert!(!warming_now(false, false, false));
    // A promotion waits briefly on a shard with another after it, so one behind costs one short wait: a new leader's
    // first promotion waited the full 60 s on a shard still catching up (B0-30's rerun). The last try waits longest.
    assert!(promote_wait_s(true) <= 10 && promote_wait_s(false) == PROMOTE_WAIT_S);
    assert!(promote_wait_s(true) < promote_wait_s(false));
}

/// A pool shard that answered `warming` gets no push for `POOL_COLD_SKIP_MS`, then is tried again, so a carried-over
/// k does not make every push wait on a cold pool shard and a pool shard that warmed takes pushes again.
#[test]
fn a_pool_shard_that_answered_warming_gets_no_push_until_the_skip_passes() {
    assert!(pool_forward(None, 0));
    assert!(!pool_forward(Some(1_000), 1_000));
    assert!(!pool_forward(Some(1_000), 1_000 + POOL_COLD_SKIP_MS - 1));
    assert!(pool_forward(Some(1_000), 1_000 + POOL_COLD_SKIP_MS));
}

/// A leadership catches its network's shards up at once, then again only after `KEEP_WARM_MS` and only when it applied
/// entries since, so the shards a push falls back to stay a few seconds behind, and an idle group wakes nothing.
#[test]
fn a_leader_keeps_the_shards_warm_only_while_it_applies_entries() {
    use graftyard_types::Index;
    assert!(keep_warm_due(None, 0, Index(0)), "a new leadership catches up at once");
    let last = Some((1_000, Index(50)));
    assert!(!keep_warm_due(last, 1_000 + KEEP_WARM_MS - 1, Index(60)), "not before KEEP_WARM_MS");
    assert!(keep_warm_due(last, 1_000 + KEEP_WARM_MS, Index(51)), "due once it passed, with an entry applied");
    assert!(!keep_warm_due(last, 1_000 + 10 * KEEP_WARM_MS, Index(50)), "an idle group wakes no shard");
}

/// A pool warm is answered at once from the slot's own state, and only a finished rehydrate is warm: running, a cursor
/// for its disk, and no rehydrate under way. A rehydrate saves its cursor after its first ref page, before its tail is
/// caught up, so a cursor alone is not warm. Any other caller still waits for a catch-up. Under main's rule a pool warm
/// waited for a catch-up too, which under B0-34's load (catch-ups of 1 to 14 s against the warm's 2 s) answered a
/// rehydrated slot cold 34 times of 59 (LM-1 round 36).
#[test]
fn a_pool_warm_is_answered_at_once_and_only_a_finished_rehydrate_is_warm() {
    use CatchupAnswer::*;
    assert_eq!(catchup_answer(Caller::PoolWarm, true, true, false), Warm);
    assert_eq!(catchup_answer(Caller::PoolWarm, true, true, true), Cold, "a cursor mid-rehydrate is not warm");
    assert_eq!(catchup_answer(Caller::PoolWarm, true, false, false), Cold, "no cursor for this disk");
    assert_eq!(catchup_answer(Caller::PoolWarm, false, true, false), Cold, "a stopped container is not warm");
    for c in [Caller::Client, Caller::Warm, Caller::Start] {
        assert_eq!(catchup_answer(c, true, true, false), WaitForCatchUp, "{c:?} waits for its catch-up");
    }
}

/// A composable miss on a tip the shard has not caught up to (`NOT_YET`) is composed again once the catch-up already
/// running ends, and goes to the slow path (a catch-up that starts after now, then stock or composed) only when that
/// one did not bring the want, when none runs, or when the reason is one a later disk answers the same. On main every
/// such miss took the slow path: m41's 143 waited p50 3.1 s, p99 13.1 s after the 409, all composed in the end.
/// Over the diff cap is retried too, after the chain's next step (which builds a new base on a far tip): m1b34's 128
/// over-the-cap lines came from a chain on the wrong tip and all went stock or 503 (`tests/am1/results/b034-attr.md`).
#[test]
fn a_miss_on_a_just_moved_tip_or_a_far_chain_is_composed_after_the_catch_up_or_the_step() {
    use graftyard_git::compose::{AfterCompose, NOT_YET, OVER_CAP, compose_then_catch_up};
    use std::cell::{Cell, RefCell};
    // The disk's tip and the chain's tip; the running catch-up brings the disk to `brings`, the step moves the chain
    // to the disk's tip. A compose answers when both reach the want.
    let run = |why: &str, running: bool, brings: u32, want: u32| {
        let (disk, chain) = (Cell::new(1u32), Cell::new(0u32));
        let calls = RefCell::new(Vec::new());
        let r = futures_executor::block_on(compose_then_catch_up(
            std::future::pending(),
            Some(why),
            running,
            || async {
                calls.borrow_mut().push("in_flight");
                disk.set(brings);
            },
            || async {
                calls.borrow_mut().push("step");
                chain.set(disk.get());
            },
            || async {
                calls.borrow_mut().push("compose");
                let on_chain = why != OVER_CAP || chain.get() >= want;
                (disk.get() >= want && on_chain).then_some(disk.get())
            },
        ));
        (r, calls.into_inner())
    };
    // m41's case: the tip moved to 2, the running catch-up brings 2, so the miss composes after it, in that order.
    assert_eq!(run(NOT_YET, true, 2, 2), (AfterCompose::Composed(2), vec!["in_flight", "compose"]));
    // The running catch-up read the head before the move: the miss waits for a later one, as on main.
    assert_eq!(run(NOT_YET, true, 1, 2), (AfterCompose::CatchUp, vec!["in_flight", "compose"]));
    // No catch-up runs: nothing to wait for first, so the slow path at once, with no second compose.
    assert_eq!(run(NOT_YET, false, 2, 2), (AfterCompose::CatchUp, vec![]));
    // Over the cap: the step comes after the running catch-up (so it builds at the caught-up tip), then the compose.
    assert_eq!(run(OVER_CAP, true, 2, 2), (AfterCompose::Composed(2), vec!["in_flight", "step", "compose"]));
    // Over the cap with no catch-up running still takes the step, which a later disk would not change.
    assert_eq!(run(OVER_CAP, false, 1, 1), (AfterCompose::Composed(1), vec!["step", "compose"]));
    // Reasons a later disk or step answers the same go the slow path at once.
    for why in ["stock: no chain", "stock: an annotated tag points at it", "stock: not a commit"] {
        assert_eq!(run(why, true, 2, 2), (AfterCompose::CatchUp, vec![]), "{why}");
    }
}

/// Every wait of a composable miss draws on its upload-pack's one deadline: a running catch-up or a chain step that
/// never ends gives `Late` (the shard answers 503 `warming` and the edge asks the next site) once the deadline passes,
/// and nothing after it runs. On main `Jobs::idle` had no bound: 25 of m1b34's 50 shard 503s waited 29 to 99 s.
#[test]
fn a_stuck_catch_up_or_step_ends_at_the_deadline() {
    use futures_util::task::LocalSpawnExt;
    use graftyard_git::compose::{AfterCompose, NOT_YET, OVER_CAP, compose_then_catch_up};
    use std::cell::Cell;
    use std::rc::Rc;
    for (why, stuck_step) in [(NOT_YET, false), (OVER_CAP, false), (OVER_CAP, true)] {
        let composed = Rc::new(Cell::new(false));
        let out = Rc::new(Cell::new(None));
        let mut pool = futures_executor::LocalPool::new();
        let (tx, rx) = futures_channel::oneshot::channel::<()>();
        let (c, o) = (composed.clone(), out.clone());
        pool.spawner()
            .spawn_local(async move {
                let r = compose_then_catch_up(
                    async {
                        let _ = rx.await;
                    },
                    Some(why),
                    true,
                    || async move {
                        if !stuck_step {
                            std::future::pending::<()>().await;
                        }
                    },
                    || async move {
                        if stuck_step {
                            std::future::pending::<()>().await;
                        }
                    },
                    || async move {
                        c.set(true);
                        Some(())
                    },
                )
                .await;
                o.set(Some(r));
            })
            .unwrap();
        pool.run_until_stalled();
        assert_eq!(out.take(), None, "{why}: answered before the deadline while stuck");
        // The deadline fires while the miss is stuck; with nothing bounding the wait, as `idle` was, it stalls on.
        let _ = tx.send(());
        pool.run_until_stalled();
        assert_eq!(out.take(), Some(AfterCompose::Late), "{why} stuck_step={stuck_step}");
        assert!(!composed.get(), "no compose after the deadline");
    }
}

/// A composable miss's wait for its chain (`chain_ready`), on a virtual clock: the chain is ready `ready_at` after the
/// miss arrives (`None`: not within any wait), and the shard answers `warming` when its wait ends first. With another
/// site to try (`next=1`) the wait is `CHAIN_WAIT_FORWARDED`, not the deadline; the last site waits all that is left.
/// On main every site waited the whole deadline: B0-34 mt6's builder held 38 misses 15 s each while site b had a synced
/// chain (`tests/am1/results/b034-mt6.md`).
#[test]
fn a_forwarded_miss_waits_briefly_for_its_chain_and_the_last_site_waits_the_deadline() {
    use graftyard_git::compose::{CHAIN_WAIT_FORWARDED, chain_ready};
    use std::cell::Cell;
    use std::task::Poll;
    use std::time::Duration;
    let s = Duration::from_secs;
    // (answered composed, the wait the shard slept for)
    let run = |follows: bool, left: Duration, ready_at: Option<Duration>| {
        let slept = Cell::new(None::<Duration>);
        let ready = std::future::poll_fn(|_| match (slept.get(), ready_at) {
            (Some(w), Some(r)) if r <= w => Poll::Ready(()),
            _ => Poll::Pending,
        });
        let composed = futures_executor::block_on(chain_ready(follows, left, ready, |w| {
            slept.set(Some(w));
            std::future::poll_fn(
                move |_| if ready_at.is_some_and(|r| r <= w) { Poll::Pending } else { Poll::Ready(()) },
            )
        }));
        (composed, slept.get().expect("the shard sleeps for its wait"))
    };
    assert_eq!(CHAIN_WAIT_FORWARDED, s(2));
    // mt6's case: the home (wait=15, next=1), its chain not stepped. Warming at the bound, not at 15 s.
    assert_eq!(run(true, s(15), None), (false, s(2)));
    // The same on a middle site (wait=60, next=1), and a chain 5 s away still sends it on: the next site composes.
    assert_eq!(run(true, s(60), Some(s(5))), (false, s(2)));
    // A deadline already under the bound keeps what is left.
    assert_eq!(run(true, Duration::from_millis(800), None), (false, Duration::from_millis(800)));
    // The last site (no next) waits all that is left: kill's case ends here, at the last site's full wait.
    assert_eq!(run(false, s(60), None), (false, s(60)));
    assert_eq!(run(false, s(60), Some(s(14))), (true, s(60)), "a chain 14 s away composes on the last site");
    // A ready chain composes as before, on any site, and one ready inside the bound composes on a forwarded miss.
    assert_eq!(run(true, s(15), Some(Duration::ZERO)), (true, s(2)));
    assert_eq!(run(false, s(15), Some(Duration::ZERO)), (true, s(15)));
    assert_eq!(run(true, s(15), Some(Duration::from_millis(1_500))), (true, s(2)));
}

/// Stock upload-packs past the container's `STOCK_MAX` wait in the shard object in arrival order: the head of the
/// line asks the container, later ones never jump it, one past its deadline leaves without holding the next one up,
/// and with nobody waiting a request asks at once. A wait, not a 429 to the client: git retries neither by default.
#[test]
fn stock_waits_in_arrival_order_within_its_deadline() {
    use graftyard_git::compose::{STOCK_MAX, StockLine, stock_in_line};
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;
    assert_eq!(STOCK_MAX, 2, "a named guess the next B0-34 row's stock-process count measures");
    let line = Rc::new(RefCell::new(StockLine::default()));
    let slots = Rc::new(Cell::new(0u32)); // free container slots
    let order = Rc::new(RefCell::new(Vec::new()));
    let mut pool = futures_executor::LocalPool::new();
    let spawner = futures_util::task::LocalSpawnExt::spawn_local;
    // A tick the test drives: each `pause` waits for the next one.
    let ticks = Rc::new(RefCell::new(Vec::<futures_channel::oneshot::Sender<()>>::new()));
    let tick = {
        let ticks = ticks.clone();
        move || {
            let (tx, rx) = futures_channel::oneshot::channel();
            ticks.borrow_mut().push(tx);
            async move {
                let _ = rx.await;
            }
        }
    };
    let deadlines: Rc<RefCell<Vec<Option<futures_channel::oneshot::Sender<()>>>>> = Rc::default();
    let start = |name: &'static str| {
        let (line, slots, order, tick) = (line.clone(), slots.clone(), order.clone(), tick.clone());
        let (tx, rx) = futures_channel::oneshot::channel::<()>();
        deadlines.borrow_mut().push(Some(tx));
        async move {
            let r = stock_in_line(
                &line,
                async {
                    let _ = rx.await;
                },
                || {
                    let slots = slots.clone();
                    async move {
                        (slots.get() > 0).then(|| {
                            slots.set(slots.get() - 1);
                            name
                        })
                    }
                },
                tick.clone(),
            )
            .await;
            order.borrow_mut().push(r.unwrap_or("late"));
        }
    };
    let s = pool.spawner();
    for n in ["a", "b", "c", "d"] {
        spawner(&s, start(n)).unwrap();
    }
    pool.run_until_stalled();
    assert_eq!(line.borrow().len(), 4, "all four refused at once and queued");
    // Every waiter wakes, latest first, so a place that asks out of turn would take the slot.
    let round = |pool: &mut futures_executor::LocalPool| {
        let woken: Vec<_> = ticks.borrow_mut().drain(..).collect();
        for t in woken.into_iter().rev() {
            let _ = t.send(());
        }
        pool.run_until_stalled();
    };
    // One slot frees: only the head, a, takes it, though every waiter wakes.
    slots.set(1);
    round(&mut pool);
    assert_eq!(*order.borrow(), vec!["a"]);
    // b's deadline passes while it heads the line: it leaves, and c becomes the head without holding d's place.
    let _ = deadlines.borrow_mut()[1].take().unwrap().send(());
    pool.run_until_stalled();
    assert_eq!(*order.borrow(), vec!["a", "late"]);
    // A newcomer while c and d wait does not ask, even with a slot free: it joins behind them.
    slots.set(1);
    spawner(&s, start("f")).unwrap();
    pool.run_until_stalled();
    assert_eq!(*order.borrow(), vec!["a", "late"], "f asked ahead of c and d");
    assert_eq!(line.borrow().len(), 3);
    slots.set(2);
    round(&mut pool);
    round(&mut pool);
    assert_eq!(*order.borrow(), vec!["a", "late", "c", "d"], "c before d, one place at a time");
    slots.set(1);
    round(&mut pool);
    assert_eq!(*order.borrow(), vec!["a", "late", "c", "d", "f"]);
    assert!(line.borrow().is_empty());
    // Nobody waiting and a slot free: a new request asks at once, with no pause.
    slots.set(1);
    spawner(&s, start("e")).unwrap();
    pool.run_until_stalled();
    assert_eq!(order.borrow().last(), Some(&"e"));
    assert!(ticks.borrow().is_empty(), "e never paused");
}

/// The daily `shard stats` line counts a network in the day's fetched, pushed, or caught-up total only when that kind
/// of request came within the 24 h before the line, 1 ms short of a day in and a full day out, and sums the
/// repository's whole bytes, not its packs'. It is due once a day.
#[test]
fn shard_stats_count_each_kind_within_the_day() {
    const H: u64 = 3_600_000;
    let now = Timestamp(100 * H);
    let at = |t: u64| Some(Timestamp(t));
    let r = |n: i64, mb: u64, used: u64, fetched, pushed, caught_up| DiskRepo {
        network: net(n),
        bytes: mb << 20,
        used: Timestamp(used),
        pack_bytes: (mb << 20) / 2,
        fetched,
        pushed,
        caught_up,
    };
    let repos = [
        // Fetched 1 ms inside the day, pushed long ago.
        r(1, 300, now.0 - 1, at(now.0 - 24 * H + 1), at(10 * H), at(now.0 - 1)),
        // Fetched exactly a day ago: out. Caught up within the day only.
        r(2, 500, now.0 - 2 * H, at(now.0 - 24 * H), None, at(now.0 - 2 * H)),
        // Pushed within the day, never fetched.
        r(3, 70, now.0 - H, None, at(now.0 - H), None),
        // Idle two days.
        r(4, 900, now.0 - 48 * H, at(now.0 - 48 * H), None, None),
    ];
    let v = stats_line(1 << 40, &repos, now);
    let day = |k: &str| (v["day"][k]["networks"].as_u64().unwrap(), v["day"][k]["bytes"].as_u64().unwrap() >> 20);
    assert_eq!((v["held"]["networks"].as_u64(), v["held"]["bytes"].as_u64().map(|b| b >> 20)), (Some(4), Some(1770)));
    assert_eq!(day("fetched"), (1, 300));
    assert_eq!(day("pushed"), (1, 70));
    assert_eq!(day("caught_up"), (2, 800));
    assert_eq!(day("used"), (3, 870));
    assert_eq!(v["repos"][2]["fetched"], serde_json::Value::Null);
    assert_eq!(v["repos"][0]["pack_bytes"].as_u64(), Some(150 << 20));
    assert!(stats_due(now, None));
    assert!(!stats_due(now, Some(Timestamp(now.0 - 24 * H + 1))));
    assert!(stats_due(now, Some(Timestamp(now.0 - 24 * H))));
}
